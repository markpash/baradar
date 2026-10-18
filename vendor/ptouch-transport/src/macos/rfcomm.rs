//! macOS RFCOMM transport handle. See the parent module for the contract.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_int;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_io_bluetooth::{
    BluetoothRFCOMMChannelID, IOBluetoothDevice, IOBluetoothRFCOMMChannel, IOBluetoothSDPUUID,
};

use super::delegates::{Event, RfcommDelegate, SdpQueryTarget};
use super::runtime::{self, BtState, OpenChannel};
use super::{K_IO_RETURN_NOT_OPEN, K_IO_RETURN_SUCCESS, K_IO_RETURN_TIMEOUT, device_for};
use crate::endpoint::rfcomm::{
    PROBE_ATTEMPTS, PROBE_TIMEOUT, SPP_UUID16, channel_candidates, chunk_len, probe_channel,
};
use crate::{BtAddr, OpenOptions, Transport, TransportError, TransportInfo, TransportKind};

/// Baseband page timeout, in 0.625 ms slots (0x2710 = 6.25 s, Brother's value).
const PAGE_TIMEOUT: u16 = 0x2710;
/// SDP polling interval and cap (PROTOCOL.md §2.2 step 4).
const SDP_POLL: Duration = Duration::from_millis(200);
const SDP_TIMEOUT: Duration = Duration::from_secs(10);
/// Async open cap (Brother: 50 × 200 ms).
const ASYNC_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for `rfcommChannelClosed:` (PROTOCOL.md §2.2 step 9).
const CLOSE_WAIT: Duration = Duration::from_secs(1);
/// Drain after `closeConnection` / channel close.
const CLOSE_DRAIN: Duration = Duration::from_millis(250);
/// Pause after multiplexer priming (PROTOCOL.md §2.2 step 10).
const PRIME_PAUSE: Duration = Duration::from_millis(300);
/// Retry window for `kIOReturnNotOpen` right after an open (step 7).
const NOT_OPEN_RETRY: Duration = Duration::from_secs(2);
/// Bytes handed to the main thread per write job; the run loop is pumped between jobs.
const WRITE_BATCH: usize = 4096;
/// How long the first `writeAsync` batch of a channel waits for its completion callbacks
/// before concluding that this IOBluetooth never delivers them (and switching to
/// `writeSync`). The first write is the handshake, which a live printer always accepts at once.
const FIRST_COMPLETION_WAIT: Duration = Duration::from_secs(3);
/// Bound on each main-thread call made while closing a channel whose writer timed out.
const STUCK_CALL_WAIT: Duration = Duration::from_secs(2);

fn bt(op: &'static str, code: c_int) -> TransportError {
    TransportError::Bluetooth {
        op,
        code: super::io_return_code(code),
    }
}

/// RFCOMM channel to a printer over IOBluetooth.
#[derive(Debug)]
pub struct MacRfcommTransport {
    /// Handle into the main-thread channel table; `None` once closed.
    id: Option<u32>,
    rx: Receiver<Event>,
    rbuf: VecDeque<u8>,
    /// Raw `getMTU()` value (may be 0, see [`chunk_len`]).
    mtu_raw: usize,
    opened_at: Instant,
    peer_closed: bool,
    info: TransportInfo,
    /// Bound on each write (`OpenOptions::write_timeout`).
    write_timeout: Duration,
    /// Wait after close (`OpenOptions::close_wait`, PROTOCOL.md §2.1 "Timing").
    close_wait: Duration,
    /// Next `writeAsync` refcon.
    next_refcon: usize,
    /// Refcons whose completion was seen (their buffers are released on the next write).
    completed: Vec<usize>,
    /// `None` until the first write: do completion callbacks arrive? `Some(false)` falls back
    /// to `writeSync` with a bounded wait.
    async_writes: Option<bool>,
    /// A `writeSync` did not return in time: the main thread may be blocked inside it, so no
    /// further unbounded main-thread call is made.
    stuck: bool,
}

/// How waiting for write completions ended badly.
enum WriteWait {
    /// No (or not every) completion within the deadline.
    Timeout,
    /// The channel closed or a write failed.
    Failed(TransportError),
}

/// What the main thread knows about a device before connecting.
struct DeviceSnapshot {
    connected: bool,
    cached_spp: Option<u8>,
    name: Option<String>,
}

/// RFCOMM channel of the SPP (0x1101) record in IOBluetooth's SDP cache.
fn spp_channel_from_cache(dev: &IOBluetoothDevice) -> Option<u8> {
    // SAFETY: IOBluetooth getters on the main thread.
    unsafe {
        let uuid = IOBluetoothSDPUUID::uuid16(SPP_UUID16)?;
        let rec = dev.getServiceRecordForUUID(Some(&uuid))?;
        let mut ch: BluetoothRFCOMMChannelID = 0;
        (rec.getRFCOMMChannelID(&mut ch) == K_IO_RETURN_SUCCESS && ch != 0).then_some(ch)
    }
}

fn last_services_update(dev: &IOBluetoothDevice) -> Option<f64> {
    // SAFETY: IOBluetooth getter on the main thread.
    unsafe { dev.getLastServicesUpdate() }.map(|d| d.timeIntervalSinceReferenceDate())
}

fn snapshot(addr: BtAddr) -> Result<DeviceSnapshot, TransportError> {
    runtime::call(move |_| {
        let dev = device_for(addr)?;
        // SAFETY: IOBluetooth getters on the main thread.
        let (connected, name) = unsafe {
            #[allow(deprecated)] // `getName` returns an Option; `name` assumes non-nil.
            let name = dev.getName().map(|s| s.to_string());
            (dev.isConnected(), name)
        };
        Some(DeviceSnapshot {
            connected,
            cached_spp: spp_channel_from_cache(&dev),
            name,
        })
    })?
    .ok_or_else(|| TransportError::NotFound(format!("Bluetooth device {addr}")))
}

/// Opens the baseband if needed (step 3). `Ok(true)` when this call opened it.
fn open_baseband(addr: BtAddr) -> Result<bool, TransportError> {
    let st = runtime::call(move |_| {
        let dev = device_for(addr)?;
        // SAFETY: IOBluetooth calls on the main thread.
        unsafe {
            if dev.isConnected() {
                return Some(None);
            }
            Some(Some(
                dev.openConnection_withPageTimeout_authenticationRequired(
                    None,
                    PAGE_TIMEOUT,
                    false,
                ),
            ))
        }
    })?;
    match st {
        None => Err(TransportError::NotFound(format!("Bluetooth device {addr}"))),
        Some(None) => Ok(false),
        Some(Some(K_IO_RETURN_SUCCESS)) => Ok(true),
        // Printer off / out of range: no point in trying RFCOMM.
        Some(Some(K_IO_RETURN_TIMEOUT)) => Err(bt("openConnection", K_IO_RETURN_TIMEOUT)),
        // Anything else: let the RFCOMM open page the device itself.
        Some(Some(_)) => Ok(false),
    }
}

/// `closeConnection` on the device (after a failed open, or when we opened the baseband).
fn close_baseband(addr: BtAddr) {
    let _ = runtime::call(move |_| {
        if let Some(dev) = device_for(addr) {
            // SAFETY: IOBluetooth call on the main thread.
            unsafe {
                dev.closeConnection();
            }
        }
    });
    std::thread::sleep(CLOSE_DRAIN);
}

/// Fresh SDP query, completion detected by polling `getLastServicesUpdate` (step 4).
fn sdp_query(addr: BtAddr, timeout: Duration) -> Result<Option<u8>, TransportError> {
    let started = runtime::call(move |state: &mut BtState| {
        let dev = device_for(addr)?;
        let before = last_services_update(&dev);
        let target = SdpQueryTarget::new();
        let target_obj: &AnyObject = &target;
        // SAFETY: IOBluetooth call on the main thread; the target is kept alive below.
        let st = unsafe { dev.performSDPQuery(Some(target_obj)) };
        state.keep_alive.push(Retained::into_super(target));
        Some((st, before))
    })?;
    let Some((st, before)) = started else {
        return Err(TransportError::NotFound(format!("Bluetooth device {addr}")));
    };
    if st != K_IO_RETURN_SUCCESS {
        return Err(bt("performSDPQuery", st));
    }
    let deadline = crate::deadline_after(timeout);
    while deadline.is_none_or(|d| Instant::now() < d) {
        std::thread::sleep(SDP_POLL);
        let now = runtime::call(move |_| device_for(addr).and_then(|d| last_services_update(&d)))?;
        if now.is_some() && now != before {
            break;
        }
    }
    // On timeout, still use whatever the cache now holds (Brother does the same).
    runtime::call(move |_| device_for(addr).and_then(|d| spp_channel_from_cache(&d)))
}

/// Result of an open job on the main thread.
enum OpenOutcome {
    /// Open (sync) or opening (async); events arrive on the receiver.
    Opened {
        id: u32,
        sync: bool,
        rx: Receiver<Event>,
    },
    /// Failed with this `IOReturn` from this call.
    Failed(&'static str, c_int),
}

/// Sync open, falling back to async on any failure except `kIOReturnTimeout` (step 5).
fn open_channel_job(addr: BtAddr, channel: u8) -> Result<OpenOutcome, TransportError> {
    runtime::call(move |state: &mut BtState| {
        let Some(dev) = device_for(addr) else {
            return OpenOutcome::Failed("deviceWithAddressString", K_IO_RETURN_NOT_OPEN);
        };
        // Sync attempt, with its own delegate/receiver so that late events of a failed attempt
        // can never be mistaken for the async attempt's completion.
        let (tx, rx) = mpsc::channel();
        let delegate = RfcommDelegate::new(tx);
        let delegate_obj: &AnyObject = &delegate;
        let mut chan: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
        // SAFETY: IOBluetooth call on the main thread; the delegate outlives the channel.
        let st = unsafe {
            dev.openRFCOMMChannelSync_withChannelID_delegate(
                Some(&mut chan),
                channel,
                Some(delegate_obj),
            )
        };
        if st == K_IO_RETURN_SUCCESS
            && let Some(channel) = chan.take()
        {
            let id = state.insert_channel(OpenChannel {
                channel,
                delegate,
                device: dev,
                close_baseband: false,
                pending_writes: HashMap::new(),
            });
            return OpenOutcome::Opened { id, sync: true, rx };
        }
        if let Some(c) = chan.take() {
            // SAFETY: as above.
            unsafe {
                c.closeChannel();
            }
        }
        state.keep_alive.push(Retained::into_super(delegate));
        if st == K_IO_RETURN_TIMEOUT {
            return OpenOutcome::Failed("openRFCOMMChannelSync", st);
        }

        let (tx, rx) = mpsc::channel();
        let delegate = RfcommDelegate::new(tx);
        let delegate_obj: &AnyObject = &delegate;
        let mut chan: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
        // SAFETY: as above.
        let st = unsafe {
            dev.openRFCOMMChannelAsync_withChannelID_delegate(
                Some(&mut chan),
                channel,
                Some(delegate_obj),
            )
        };
        match chan.take() {
            Some(channel) if st == K_IO_RETURN_SUCCESS => {
                let id = state.insert_channel(OpenChannel {
                    channel,
                    delegate,
                    device: dev,
                    close_baseband: false,
                    pending_writes: HashMap::new(),
                });
                OpenOutcome::Opened {
                    id,
                    sync: false,
                    rx,
                }
            }
            other => {
                if let Some(c) = other {
                    // SAFETY: as above.
                    unsafe {
                        c.closeChannel();
                    }
                }
                state.keep_alive.push(Retained::into_super(delegate));
                OpenOutcome::Failed(
                    "openRFCOMMChannelAsync",
                    if st == K_IO_RETURN_SUCCESS {
                        K_IO_RETURN_NOT_OPEN
                    } else {
                        st
                    },
                )
            }
        }
    })
}

/// Opens channel 1 and closes it again to prime a cold RFCOMM multiplexer (step 10).
fn prime_multiplexer(addr: BtAddr) -> Result<(), TransportError> {
    runtime::call(move |state: &mut BtState| {
        let Some(dev) = device_for(addr) else { return };
        let (tx, _rx) = mpsc::channel();
        let delegate = RfcommDelegate::new(tx);
        let delegate_obj: &AnyObject = &delegate;
        let mut chan: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
        // SAFETY: IOBluetooth calls on the main thread; the delegate is kept alive.
        unsafe {
            let _ = dev.openRFCOMMChannelSync_withChannelID_delegate(
                Some(&mut chan),
                1,
                Some(delegate_obj),
            );
            if let Some(c) = chan {
                c.closeChannel();
            }
        }
        state.keep_alive.push(Retained::into_super(delegate));
    })?;
    std::thread::sleep(PRIME_PAUSE);
    Ok(())
}

/// How one candidate channel attempt ended.
enum Attempt {
    Accepted(MacRfcommTransport),
    /// The channel opened but did not answer (wrong channel / iAP).
    Silent,
    /// The open failed.
    OpenFailed(TransportError),
}

/// Queues the channel from a fresh SDP query first, allowing exactly one retry of a channel
/// that was already tried (the retry of PROTOCOL.md §2.2 step 4).
fn requeue_after_sdp(queue: &mut VecDeque<u8>, tried: &mut Vec<u8>, channel: u8) {
    tried.retain(|&c| c != channel);
    queue.retain(|&c| c != channel);
    queue.push_front(channel);
}

impl MacRfcommTransport {
    /// Opens the SPP channel to `addr` (`channel = None` ⇒ SDP + fallback order). Must be
    /// called from inside [`super::run_main_loop`] (not on the main thread).
    ///
    /// Sequence (PROTOCOL.md §2.2): baseband open if needed (step 3); SPP channel from the SDP
    /// cache, else a polled fresh SDP query (step 4); candidates per §2.1 (SDP,
    /// `rfcomm_channel_observed` of `opts.model_hint` or of the device name, 1, 2); for each:
    /// prime the multiplexer when the channel is not 1 (step 10), sync open with fail-fast on
    /// `kIOReturnTimeout`, else async open (step 5), settle, then accept only after a valid
    /// status reply (up to 3 probes). If the cached SDP channel fails to open, a fresh SDP
    /// query is run once and its channel retried once, even when it is the same channel.
    /// With an explicit `channel`, no status probe is made.
    ///
    /// # Errors
    /// [`TransportError::Bluetooth`] with the `IOReturn` code, [`TransportError::Timeout`],
    /// [`TransportError::NoWorkingChannel`], or [`TransportError::EventLoopNotRunning`].
    pub fn connect(
        addr: BtAddr,
        channel: Option<u8>,
        opts: &OpenOptions,
    ) -> Result<Self, TransportError> {
        let snap = snapshot(addr)?;
        let opened_baseband = if snap.connected {
            false
        } else {
            open_baseband(addr)?
        };
        let result = Self::connect_candidates(addr, channel, opts, &snap);
        match result {
            Ok(t) => {
                if opened_baseband && let Some(id) = t.id {
                    let _ = runtime::call(move |st: &mut BtState| {
                        if let Some(c) = st.channels.get_mut(&id) {
                            c.close_baseband = true;
                        }
                    });
                }
                Ok(t)
            }
            Err(e) => {
                // After a failed open, release the baseband even if we did not open it
                // (stale link after a power cycle, step 9).
                close_baseband(addr);
                Err(e)
            }
        }
    }

    fn connect_candidates(
        addr: BtAddr,
        channel: Option<u8>,
        opts: &OpenOptions,
        snap: &DeviceSnapshot,
    ) -> Result<Self, TransportError> {
        if let Some(ch) = channel {
            if ch != 1 {
                prime_multiplexer(addr)?;
            }
            return match Self::attempt(addr, ch, opts, false)? {
                Attempt::Accepted(t) => Ok(t),
                Attempt::Silent => Err(TransportError::NoWorkingChannel),
                Attempt::OpenFailed(e) => Err(e),
            };
        }
        let sdp_timeout = SDP_TIMEOUT.min(opts.connect_timeout.max(SDP_POLL));
        let mut fresh_sdp_done = false;
        let sdp = match snap.cached_spp {
            Some(ch) => Some(ch),
            None => {
                fresh_sdp_done = true;
                // SDP unavailable → the §2.1 fallback order without an SDP channel.
                match sdp_query(addr, sdp_timeout) {
                    Ok(ch) => ch,
                    Err(TransportError::EventLoopNotRunning) => {
                        return Err(TransportError::EventLoopNotRunning);
                    }
                    Err(_) => None,
                }
            }
        };
        let observed = opts
            .model_hint
            .or_else(|| snap.name.as_deref().and_then(ptouch::profile_by_bt_name))
            .and_then(|m| m.rfcomm_channel_observed);
        let mut queue: VecDeque<u8> = channel_candidates(sdp, observed).into();
        let mut tried: Vec<u8> = Vec::new();
        let mut last_err = None;
        let mut opened_any = false;
        while let Some(ch) = queue.pop_front() {
            if tried.contains(&ch) {
                continue;
            }
            tried.push(ch);
            if ch != 1 {
                prime_multiplexer(addr)?;
            }
            match Self::attempt(addr, ch, opts, true)? {
                Attempt::Accepted(t) => return Ok(t),
                Attempt::Silent => opened_any = true,
                Attempt::OpenFailed(e) => {
                    if matches!(e, TransportError::Bluetooth { code, .. } if code == super::io_return_code(K_IO_RETURN_TIMEOUT))
                    {
                        // Page timeout: printer off or out of range (fail fast, step 5).
                        return Err(e);
                    }
                    last_err = Some(e);
                    // The cached channel failed to open: run a fresh SDP query and retry once
                    // (PROTOCOL.md §2.2 step 4), also when SDP still names the same channel:
                    // after a power cycle the link was stale, and the failed open has just
                    // released the baseband, so the same channel usually opens now.
                    if !fresh_sdp_done && Some(ch) == sdp {
                        fresh_sdp_done = true;
                        if let Ok(Some(new)) = sdp_query(addr, sdp_timeout) {
                            requeue_after_sdp(&mut queue, &mut tried, new);
                        }
                    }
                }
            }
        }
        match last_err {
            Some(e) if !opened_any => Err(e),
            _ => Err(TransportError::NoWorkingChannel),
        }
    }

    /// Opens one candidate channel and (optionally) probes it.
    fn attempt(
        addr: BtAddr,
        ch: u8,
        opts: &OpenOptions,
        probe: bool,
    ) -> Result<Attempt, TransportError> {
        let (id, sync, rx) = match open_channel_job(addr, ch)? {
            OpenOutcome::Opened { id, sync, rx } => (id, sync, rx),
            OpenOutcome::Failed(op, code) => return Ok(Attempt::OpenFailed(bt(op, code))),
        };
        let mut t = Self {
            id: Some(id),
            rx,
            rbuf: VecDeque::new(),
            mtu_raw: 0,
            opened_at: Instant::now(),
            peer_closed: false,
            info: TransportInfo {
                kind: TransportKind::Rfcomm,
                label: format!("RFCOMM ch {ch}"),
                max_write: None,
            },
            write_timeout: opts.write_timeout,
            close_wait: opts.close_wait,
            next_refcon: 1,
            completed: Vec::new(),
            async_writes: None,
            stuck: false,
        };
        let mut open_complete_seen = false;
        if !sync {
            // Only rfcommChannelOpenComplete:status: is trustworthy; isOpen flips early.
            let timeout = ASYNC_OPEN_TIMEOUT.min(opts.connect_timeout.max(SDP_POLL));
            match t.wait_open_complete(timeout) {
                Ok(()) => open_complete_seen = true,
                Err(e) => {
                    // Close the half-open channel and the baseband before any retry (step 5).
                    let _ = t.close_inner(true);
                    return Ok(Attempt::OpenFailed(e));
                }
            }
            t.opened_at = Instant::now();
        }
        t.mtu_raw = t.read_mtu()?;
        if t.mtu_raw == 0 && !open_complete_seen {
            // getMTU() can read 0 right after an open; wait for open-complete and re-read.
            let _ = t.wait_open_complete(Duration::from_secs(1));
            t.mtu_raw = t.read_mtu()?;
        }
        t.info.max_write = Some(chunk_len(t.mtu_raw));
        std::thread::sleep(opts.settle);
        if !probe {
            return Ok(Attempt::Accepted(t));
        }
        match probe_channel(&mut t, PROBE_ATTEMPTS, PROBE_TIMEOUT) {
            Ok(true) => Ok(Attempt::Accepted(t)),
            Ok(false) | Err(_) => {
                let _ = t.close();
                Ok(Attempt::Silent)
            }
        }
    }

    /// Waits for `rfcommChannelOpenComplete:status:`; data that arrives meanwhile is kept.
    fn wait_open_complete(&mut self, timeout: Duration) -> Result<(), TransportError> {
        let deadline = crate::deadline_after(timeout);
        loop {
            let left = crate::time_left(deadline);
            match self.rx.recv_timeout(left) {
                Ok(Event::OpenComplete(K_IO_RETURN_SUCCESS)) => return Ok(()),
                Ok(Event::OpenComplete(status)) => {
                    return Err(bt("rfcommChannelOpenComplete", status));
                }
                Ok(Event::Data(d)) => self.rbuf.extend(d),
                Ok(Event::WriteComplete { refcon, .. }) => self.completed.push(refcon),
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.peer_closed = true;
                    return Err(TransportError::Closed);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(TransportError::Timeout {
                        op: "rfcommChannelOpenComplete",
                    });
                }
            }
        }
    }

    fn read_mtu(&self) -> Result<usize, TransportError> {
        let id = self.id.ok_or(TransportError::Closed)?;
        runtime::call(move |st: &mut BtState| {
            st.channels
                .get(&id)
                // SAFETY: IOBluetooth getter on the main thread.
                .map_or(0, |c| usize::from(unsafe { c.channel.getMTU() }))
        })
    }

    /// The negotiated RFCOMM MTU (320 on PT-P710BT), i.e. the write chunk size: never 0
    /// (an MTU that still reads 0 after open-complete is replaced by 320).
    #[must_use]
    pub fn mtu(&self) -> usize {
        chunk_len(self.mtu_raw)
    }

    /// Moves queued delegate events into the read buffer without blocking.
    fn drain_events(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Event::Data(d)) => self.rbuf.extend(d),
                Ok(Event::Closed) | Err(TryRecvError::Disconnected) => {
                    self.peer_closed = true;
                    return;
                }
                Ok(Event::OpenComplete(_)) => {}
                Ok(Event::WriteComplete { refcon, .. }) => self.completed.push(refcon),
                Err(TryRecvError::Empty) => return,
            }
        }
    }

    /// Closes the channel; `release_baseband` forces `closeConnection` (failed open).
    fn close_inner(&mut self, release_baseband: bool) -> Result<(), TransportError> {
        let Some(id) = self.id.take() else {
            return Ok(());
        };
        if self.stuck {
            // The main thread may still be blocked in writeSync: bounded best effort only.
            let _ = runtime::call_timeout(
                move |st: &mut BtState| {
                    if let Some(c) = st.channels.remove(&id) {
                        // SAFETY: IOBluetooth calls on the main thread.
                        unsafe {
                            c.channel.closeChannel();
                            c.channel.setDelegate(None);
                            c.device.closeConnection();
                        }
                        st.keep_alive.push(Retained::into_super(c.delegate));
                    }
                },
                STUCK_CALL_WAIT,
            );
            self.rbuf.clear();
            std::thread::sleep(self.close_wait);
            return Ok(());
        }
        // Step 9: closeChannel with the delegate still set ...
        let closing = runtime::call(move |st: &mut BtState| {
            st.channels.get(&id).map(|c| {
                // SAFETY: IOBluetooth call on the main thread.
                unsafe { c.channel.closeChannel() }
            })
        });
        match closing {
            Ok(_) => {}
            // The loop already shut down and closed everything.
            Err(TransportError::EventLoopNotRunning) => return Ok(()),
            Err(e) => return Err(e),
        }
        // ... wait for rfcommChannelClosed: (≤ 1 s) ...
        let deadline = Instant::now() + CLOSE_WAIT;
        while !self.peer_closed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.rx.recv_timeout(left) {
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => self.peer_closed = true,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        // ... then clear the delegate and release the baseband if needed.
        let released = runtime::call(move |st: &mut BtState| {
            let Some(c) = st.channels.remove(&id) else {
                return false;
            };
            // SAFETY: IOBluetooth calls on the main thread.
            unsafe {
                c.channel.setDelegate(None);
            }
            let release = c.close_baseband || release_baseband;
            if release {
                // SAFETY: as above.
                unsafe {
                    c.device.closeConnection();
                }
            }
            st.keep_alive.push(Retained::into_super(c.delegate));
            release
        });
        // Let bluetoothd tear the link down before anything reopens it: the `closeConnection`
        // drain (step 9) and the close wait of PROTOCOL.md §2.1 "Timing" (500 ms).
        let wait = if matches!(released, Ok(true)) {
            CLOSE_DRAIN.max(self.close_wait)
        } else {
            self.close_wait
        };
        std::thread::sleep(wait);
        self.rbuf.clear();
        Ok(())
    }
}

impl MacRfcommTransport {
    /// Hands MTU chunks to `writeAsync:length:refcon:` on the main thread (refcons
    /// `first..`), releasing the buffers of completed writes. `Err((issued, status))` when a
    /// chunk was refused (the earlier `issued` chunks were accepted).
    fn issue_async(
        id: u32,
        parts: Vec<Vec<u8>>,
        first: usize,
        done: Vec<usize>,
    ) -> Result<Result<usize, (usize, c_int)>, TransportError> {
        runtime::call(move |st: &mut BtState| {
            let Some(c) = st.channels.get_mut(&id) else {
                return Err((0, K_IO_RETURN_NOT_OPEN));
            };
            for r in &done {
                c.pending_writes.remove(r);
            }
            let mut issued = 0;
            for part in parts {
                let Ok(len) = u16::try_from(part.len()) else {
                    return Err((issued, K_IO_RETURN_NOT_OPEN));
                };
                let refcon = first.wrapping_add(issued);
                // SAFETY: the buffer stays alive in `pending_writes` (moving the Vec does not
                // move its heap allocation) until the writer has seen its completion or the
                // channel is closed; writeAsync takes a non-const pointer but does not modify
                // the data. The refcon is an opaque integer that IOBluetooth never
                // dereferences.
                let s = unsafe {
                    c.channel.writeAsync_length_refcon(
                        part.as_ptr().cast_mut().cast(),
                        len,
                        std::ptr::without_provenance_mut(refcon),
                    )
                };
                if s != K_IO_RETURN_SUCCESS {
                    return Err((issued, s));
                }
                c.pending_writes.insert(refcon, part);
                issued += 1;
            }
            Ok(issued)
        })
    }

    /// Waits until the `count` writes with refcons `first..` completed, keeping data that
    /// arrives meanwhile.
    fn wait_writes(
        &mut self,
        first: usize,
        count: usize,
        deadline: Option<Instant>,
    ) -> Result<(), WriteWait> {
        let mut outstanding = count;
        let mine = |r: usize| r.wrapping_sub(first) < count;
        while outstanding > 0 {
            match self.rx.recv_timeout(crate::time_left(deadline)) {
                Ok(Event::WriteComplete { refcon, status }) => {
                    self.completed.push(refcon);
                    if mine(refcon) {
                        if status != K_IO_RETURN_SUCCESS {
                            return Err(WriteWait::Failed(bt(
                                "rfcommChannelWriteComplete",
                                status,
                            )));
                        }
                        outstanding -= 1;
                    }
                }
                Ok(Event::Data(d)) => self.rbuf.extend(d),
                Ok(Event::OpenComplete(_)) => {}
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.peer_closed = true;
                    return Err(WriteWait::Failed(TransportError::Closed));
                }
                Err(RecvTimeoutError::Timeout) => return Err(WriteWait::Timeout),
            }
        }
        Ok(())
    }

    /// Fallback when `rfcommChannelWriteComplete:` never arrives: `writeSync` per MTU chunk
    /// (PROTOCOL.md §2.2 step 7), the worker's wait bounded by `write_timeout`. If the main
    /// thread does not answer in time it may be blocked inside `writeSync`; the transport is
    /// then unusable ([`TransportError::WriteTimeout`], later calls `Closed`).
    fn write_all_sync(&mut self, id: u32, bytes: &[u8]) -> Result<(), TransportError> {
        let chunk = chunk_len(self.mtu_raw);
        // Whole chunks per main-thread job, at least one.
        let batch = (WRITE_BATCH / chunk).max(1) * chunk;
        let mut offset = 0;
        while offset < bytes.len() {
            let end = bytes.len().min(offset + batch);
            let data = bytes.get(offset..end).unwrap_or_default().to_vec();
            let job = move |st: &mut BtState| -> Result<usize, (usize, c_int)> {
                let Some(c) = st.channels.get(&id) else {
                    return Err((0, K_IO_RETURN_NOT_OPEN));
                };
                let mut written = 0;
                for part in data.chunks(chunk) {
                    let Ok(len) = u16::try_from(part.len()) else {
                        return Err((written, K_IO_RETURN_NOT_OPEN));
                    };
                    // SAFETY: `part` is a live buffer of `len` bytes; writeSync takes a
                    // non-const pointer but does not modify the data.
                    let s = unsafe {
                        c.channel
                            .writeSync_length(part.as_ptr().cast_mut().cast(), len)
                    };
                    if s != K_IO_RETURN_SUCCESS {
                        return Err((written, s));
                    }
                    written += part.len();
                }
                Ok(written)
            };
            let Some(r) = runtime::call_timeout(job, self.write_timeout)? else {
                self.stuck = true;
                return Err(TransportError::WriteTimeout);
            };
            match r {
                Ok(n) => offset += n,
                Err((n, status)) => {
                    offset += n;
                    self.drain_events();
                    if self.peer_closed {
                        return Err(TransportError::Closed);
                    }
                    if status == K_IO_RETURN_NOT_OPEN && self.opened_at.elapsed() < NOT_OPEN_RETRY {
                        // Transient right after the open: nothing of this chunk was sent.
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    return Err(bt("writeSync", status));
                }
            }
        }
        Ok(())
    }
}

impl Transport for MacRfcommTransport {
    /// Writes in MTU chunks with `writeAsync:length:refcon:` and waits for each batch's
    /// `rfcommChannelWriteComplete:` callbacks, bounded by `OpenOptions::write_timeout`: a
    /// printer that stops granting RFCOMM credits yields [`TransportError::WriteTimeout`] (and
    /// the channel is closed) instead of blocking the main thread inside `writeSync`. The
    /// main thread stays free, so status frames keep arriving meanwhile. If the first batch
    /// of a channel gets no completion callback at all, the transport falls back to the
    /// `writeSync` loop of PROTOCOL.md §2.2 step 7 (UNVERIFIED on hardware which path macOS
    /// 26/27 takes).
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let id = self.id.ok_or(TransportError::Closed)?;
        if self.stuck {
            return Err(TransportError::Closed);
        }
        self.drain_events();
        if self.peer_closed {
            return Err(TransportError::Closed);
        }
        if self.async_writes == Some(false) {
            return self.write_all_sync(id, bytes);
        }
        let chunk = chunk_len(self.mtu_raw);
        let batch = (WRITE_BATCH / chunk).max(1) * chunk;
        let mut offset = 0;
        while offset < bytes.len() {
            let end = bytes.len().min(offset + batch);
            let parts: Vec<Vec<u8>> = bytes
                .get(offset..end)
                .unwrap_or_default()
                .chunks(chunk)
                .map(<[u8]>::to_vec)
                .collect();
            let sizes: Vec<usize> = parts.iter().map(Vec::len).collect();
            let first = self.next_refcon;
            self.next_refcon = self.next_refcon.wrapping_add(parts.len());
            let done = std::mem::take(&mut self.completed);
            let (issued, refused) = match Self::issue_async(id, parts, first, done)? {
                Ok(n) => (n, None),
                Err((n, status)) => (n, Some(status)),
            };
            if issued > 0 {
                let probing = self.async_writes.is_none();
                let wait = if probing {
                    FIRST_COMPLETION_WAIT.min(self.write_timeout)
                } else {
                    self.write_timeout
                };
                match self.wait_writes(first, issued, crate::deadline_after(wait)) {
                    Ok(()) => self.async_writes = Some(true),
                    // No callback at all: writeAsync accepted ("buffered") the data, but this
                    // IOBluetooth does not report completions. Continue with writeSync.
                    Err(WriteWait::Timeout) if probing => self.async_writes = Some(false),
                    Err(WriteWait::Timeout) => {
                        let _ = self.close_inner(true);
                        return Err(TransportError::WriteTimeout);
                    }
                    Err(WriteWait::Failed(e)) => return Err(e),
                }
                offset += sizes.iter().take(issued).sum::<usize>();
            }
            if let Some(status) = refused {
                self.drain_events();
                if self.peer_closed {
                    return Err(TransportError::Closed);
                }
                if status == K_IO_RETURN_NOT_OPEN && self.opened_at.elapsed() < NOT_OPEN_RETRY {
                    // Transient right after the open: the refused chunk was not sent.
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                return Err(bt("writeAsync", status));
            }
            if self.async_writes == Some(false) {
                return self.write_all_sync(id, bytes.get(offset..).unwrap_or_default());
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if self.id.is_none() {
            return Err(TransportError::Closed);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        self.drain_events();
        let deadline = crate::deadline_after(timeout);
        while self.rbuf.is_empty() && !self.peer_closed {
            let left = crate::time_left(deadline);
            if left.is_zero() {
                break;
            }
            match self.rx.recv_timeout(left) {
                Ok(Event::Data(d)) => self.rbuf.extend(d),
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => self.peer_closed = true,
                Ok(Event::OpenComplete(_)) => {}
                Ok(Event::WriteComplete { refcon, .. }) => self.completed.push(refcon),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        self.drain_events();
        if self.rbuf.is_empty() {
            return if self.peer_closed {
                Err(TransportError::Closed)
            } else {
                Ok(0)
            };
        }
        let n = buf.len().min(self.rbuf.len());
        for (dst, src) in buf.iter_mut().zip(self.rbuf.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.close_inner(false)
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for MacRfcommTransport {
    fn drop(&mut self) {
        let _ = self.close_inner(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn same_channel_is_retried_once_after_sdp() {
        // P710BT: cached SPP channel 1 failed, SDP still says 1 → retry 1 before 2.
        let mut queue: VecDeque<u8> = VecDeque::from([2]);
        let mut tried = vec![1];
        requeue_after_sdp(&mut queue, &mut tried, 1);
        assert_eq!(queue, [1, 2]);
        assert!(tried.is_empty());
        // A new channel from SDP goes first and is not tried twice.
        let mut queue: VecDeque<u8> = VecDeque::from([2, 3]);
        let mut tried = vec![1];
        requeue_after_sdp(&mut queue, &mut tried, 3);
        assert_eq!(queue, [3, 2]);
    }

    #[test]
    fn io_return_codes_print_as_documented() {
        let e = bt("openRFCOMMChannelSync", K_IO_RETURN_TIMEOUT);
        assert!(e.to_string().contains("0xe00002d6"), "{e}");
        assert_eq!(
            super::super::io_return_code(K_IO_RETURN_NOT_OPEN),
            0xE000_02CD
        );
    }

    #[test]
    fn transport_handle_is_send() {
        assert_send::<MacRfcommTransport>();
    }

    #[test]
    fn connect_outside_event_loop_fails_cleanly() {
        // libtest threads are never the main thread and no loop runs: no IOBluetooth call is
        // made, the error is immediate.
        let addr = BtAddr([0, 0, 0, 0, 0, 1]);
        let r = MacRfcommTransport::connect(addr, None, &OpenOptions::default());
        assert!(
            matches!(r, Err(TransportError::EventLoopNotRunning)),
            "{r:?}"
        );
        let r = super::super::inquiry(Duration::from_secs(1));
        assert!(
            matches!(r, Err(TransportError::EventLoopNotRunning)),
            "{r:?}"
        );
    }

    #[test]
    fn run_main_loop_off_main_thread_just_calls() {
        assert_eq!(super::super::run_main_loop(|| 41 + 1), 42);
    }
}
