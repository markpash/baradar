//! Linux raw RFCOMM socket transport. See the parent module for the contract.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use crate::endpoint::rfcomm::{PROBE_ATTEMPTS, PROBE_TIMEOUT, channel_candidates, probe_channel};
use crate::{BtAddr, OpenOptions, Transport, TransportError, TransportInfo, TransportKind};

/// `AF_BLUETOOTH` (`<sys/socket.h>`).
const AF_BLUETOOTH: libc::c_int = 31;
/// `BTPROTO_RFCOMM` (`<bluetooth/bluetooth.h>`).
const BTPROTO_RFCOMM: libc::c_int = 3;
/// How long to retry `EBUSY` while a previous link is still closing (PROTOCOL.md §2.3).
const EBUSY_RETRY: Duration = Duration::from_secs(5);

/// `struct sockaddr_rc` from `<bluetooth/rfcomm.h>`.
#[repr(C)]
struct SockaddrRc {
    rc_family: libc::sa_family_t,
    /// `bdaddr_t`, least-significant byte first (reversed printed order).
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

/// RFCOMM socket to a printer.
#[derive(Debug)]
pub struct LinuxRfcommTransport {
    fd: Option<OwnedFd>,
    info: TransportInfo,
    /// Wait after close (PROTOCOL.md §2.1 "Timing").
    close_wait: Duration,
}

fn last_os_error() -> io::Error {
    io::Error::last_os_error()
}

fn bt_err(op: &'static str, e: &io::Error) -> TransportError {
    TransportError::Bluetooth {
        op,
        code: i64::from(e.raw_os_error().unwrap_or(0)),
    }
}

/// Milliseconds for `poll`, clamped to `i32`.
fn poll_ms(d: Duration) -> libc::c_int {
    libc::c_int::try_from(d.as_millis()).unwrap_or(libc::c_int::MAX)
}

/// Waits for `events` on `fd`; `Ok(false)` on timeout.
fn poll_fd(fd: RawFd, events: libc::c_short, timeout: Duration) -> io::Result<bool> {
    let deadline = crate::deadline_after(timeout);
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let left = crate::time_left(deadline);
        // SAFETY: `pfd` is a valid, initialised pollfd and we pass a count of 1.
        let r = unsafe { libc::poll(&mut pfd, 1, poll_ms(left)) };
        if r < 0 {
            let e = last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        return Ok(r > 0);
    }
}

fn set_nonblocking(fd: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: F_GETFL/F_SETFL on a descriptor we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(last_os_error());
    }
    let flags = if on {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } < 0 {
        return Err(last_os_error());
    }
    Ok(())
}

fn set_send_timeout(fd: RawFd, timeout: Duration) -> io::Result<()> {
    // `suseconds_t` is i32 on some 32-bit targets, hence the fallible conversion.
    #[allow(clippy::unnecessary_fallible_conversions)]
    let tv = libc::timeval {
        tv_sec: libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_usec: libc::suseconds_t::try_from(timeout.subsec_micros()).unwrap_or(0),
    };
    // SAFETY: `tv` is a valid timeval and the length matches its size.
    let r = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDTIMEO,
            (&raw const tv).cast(),
            libc::socklen_t::try_from(size_of::<libc::timeval>()).unwrap_or(0),
        )
    };
    if r < 0 {
        return Err(last_os_error());
    }
    Ok(())
}

/// One connect attempt to `addr` on `channel`, bounded by `timeout`.
fn connect_once(addr: BtAddr, channel: u8, timeout: Duration) -> io::Result<OwnedFd> {
    // SAFETY: plain socket(2) call; the result is checked before use.
    let raw = unsafe {
        libc::socket(
            AF_BLUETOOTH,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            BTPROTO_RFCOMM,
        )
    };
    if raw < 0 {
        return Err(last_os_error());
    }
    // SAFETY: `raw` is a freshly created descriptor that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    set_nonblocking(fd.as_raw_fd(), true)?;

    let mut bdaddr = addr.0;
    bdaddr.reverse();
    let sa = SockaddrRc {
        rc_family: libc::sa_family_t::try_from(AF_BLUETOOTH).unwrap_or(31),
        rc_bdaddr: bdaddr,
        rc_channel: channel,
    };
    // SAFETY: `sa` is a valid sockaddr_rc and the length matches its size.
    let r = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const sa).cast(),
            libc::socklen_t::try_from(size_of::<SockaddrRc>()).unwrap_or(0),
        )
    };
    if r < 0 {
        let e = last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        if !poll_fd(fd.as_raw_fd(), libc::POLLOUT, timeout)? {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        let mut err: libc::c_int = 0;
        let mut len = libc::socklen_t::try_from(size_of::<libc::c_int>()).unwrap_or(0);
        // SAFETY: `err`/`len` are valid out-parameters for SO_ERROR.
        let r = unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut err).cast(),
                &mut len,
            )
        };
        if r < 0 {
            return Err(last_os_error());
        }
        if err != 0 {
            return Err(io::Error::from_raw_os_error(err));
        }
    }
    set_nonblocking(fd.as_raw_fd(), false)?;
    Ok(fd)
}

/// Connects, retrying `EBUSY` for up to [`EBUSY_RETRY`].
fn connect_channel(addr: BtAddr, channel: u8, timeout: Duration) -> io::Result<OwnedFd> {
    let start = Instant::now();
    loop {
        match connect_once(addr, channel, timeout) {
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) && start.elapsed() < EBUSY_RETRY => {
                std::thread::sleep(Duration::from_millis(250));
            }
            r => return r,
        }
    }
}

/// `true` for errors that mean "the device is not reachable at all" (no point trying other
/// channels): printer off, asleep or out of range.
fn is_unreachable(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ETIMEDOUT | libc::EHOSTDOWN | libc::EHOSTUNREACH)
    )
}

impl LinuxRfcommTransport {
    /// Connects to `addr` on `channel` (or the fallback order).
    ///
    /// With an explicit `channel` the link is returned as soon as `connect` succeeds. Without
    /// one, the candidates of [`channel_candidates`] (`rfcomm_channel_observed` of
    /// `opts.model_hint`, then 1, then 2) are tried in turn and a channel is accepted only
    /// after a valid status reply ([`probe_channel`], up to 3 probes).
    ///
    /// # Errors
    /// [`TransportError::Bluetooth`] with the `errno`, or [`TransportError::NoWorkingChannel`].
    pub fn connect(
        addr: BtAddr,
        channel: Option<u8>,
        opts: &OpenOptions,
    ) -> Result<Self, TransportError> {
        let (candidates, probe) = match channel {
            Some(ch) => (vec![ch], false),
            None => {
                // Model hint: explicit, else from the bonded device name (PROTOCOL.md §2.1).
                let model = opts.model_hint.or_else(|| {
                    super::paired_devices()
                        .into_iter()
                        .find(|d| d.addr == addr)
                        .and_then(|d| ptouch::profile_by_bt_name(&d.name))
                });
                let observed = model.and_then(|m| m.rfcomm_channel_observed);
                (channel_candidates(None, observed), true)
            }
        };
        let mut last_err = None;
        let mut opened_any = false;
        for ch in candidates {
            let fd = match connect_channel(addr, ch, opts.connect_timeout) {
                Ok(fd) => fd,
                Err(e) if is_unreachable(&e) => return Err(bt_err("connect", &e)),
                Err(e) => {
                    last_err = Some(bt_err("connect", &e));
                    continue;
                }
            };
            set_send_timeout(fd.as_raw_fd(), opts.write_timeout)?;
            let mut t = Self {
                fd: Some(fd),
                info: TransportInfo {
                    kind: TransportKind::Rfcomm,
                    label: format!("RFCOMM ch {ch}"),
                    max_write: None,
                },
                close_wait: opts.close_wait,
            };
            std::thread::sleep(opts.settle);
            if !probe {
                return Ok(t);
            }
            opened_any = true;
            match probe_channel(&mut t, PROBE_ATTEMPTS, PROBE_TIMEOUT) {
                Ok(true) => return Ok(t),
                Ok(false) => {}
                Err(e) => last_err = Some(e),
            }
            // Let the link go down before the next connect (avoids EBUSY): at least 500 ms,
            // `close` itself waits `close_wait`.
            let _ = t.close();
            std::thread::sleep(Duration::from_millis(500).saturating_sub(opts.close_wait));
        }
        match last_err {
            Some(e) if !opened_any => Err(e),
            _ => Err(TransportError::NoWorkingChannel),
        }
    }

    fn raw_fd(&self) -> Result<RawFd, TransportError> {
        self.fd
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .ok_or(TransportError::Closed)
    }
}

fn map_io(op: &'static str, e: &io::Error) -> TransportError {
    match e.raw_os_error() {
        Some(
            libc::ECONNRESET | libc::ENOTCONN | libc::EPIPE | libc::ECONNABORTED | libc::EHOSTDOWN,
        ) => TransportError::Closed,
        _ => bt_err(op, e),
    }
}

impl Transport for LinuxRfcommTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let fd = self.raw_fd()?;
        let mut rest = bytes;
        while !rest.is_empty() {
            // SAFETY: `rest` is a valid readable buffer of `rest.len()` bytes.
            let n = unsafe { libc::send(fd, rest.as_ptr().cast(), rest.len(), libc::MSG_NOSIGNAL) };
            if n < 0 {
                let e = last_os_error();
                match e.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                        return Err(TransportError::WriteTimeout);
                    }
                    _ => return Err(map_io("send", &e)),
                }
            }
            let n = usize::try_from(n).unwrap_or(0);
            if n == 0 {
                return Err(TransportError::Closed);
            }
            rest = rest.get(n..).unwrap_or_default();
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let fd = self.raw_fd()?;
        loop {
            if !poll_fd(fd, libc::POLLIN, timeout).map_err(|e| map_io("poll", &e))? {
                return Ok(0);
            }
            // SAFETY: `buf` is a valid writable buffer of `buf.len()` bytes.
            let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                let e = last_os_error();
                match e.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => return Ok(0),
                    _ => return Err(map_io("recv", &e)),
                }
            }
            return match usize::try_from(n).unwrap_or(0) {
                0 => Err(TransportError::Closed),
                n => Ok(n),
            };
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        if let Some(fd) = self.fd.take() {
            // SAFETY: shutdown(2) on a descriptor we own; errors are irrelevant here.
            unsafe {
                libc::shutdown(fd.as_raw_fd(), libc::SHUT_RDWR);
            }
            drop(fd);
            // Let the link go down before anything reopens it (PROTOCOL.md §2.1 "Timing").
            std::thread::sleep(self.close_wait);
        }
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for LinuxRfcommTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
