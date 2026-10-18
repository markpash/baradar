//! # ptouch-transport — native byte pipes for Brother P-touch printers
//!
//! Transports are **dumb byte pipes** (ARCHITECTURE.md §4.3–§4.4): they move bytes and know
//! nothing about the protocol. Every protocol decision lives in [`ptouch::Session`]; the CLI's
//! driver loop is the only place where the two meet.
//!
//! | Backend | Module | Feature / target | Spec |
//! |---|---|---|---|
//! | OS serial node (`/dev/cu.*`, `COMn`, `/dev/rfcommN`) | [`serial`] | always | PROTOCOL.md §2.2 step 12, §2.3, §2.4 |
//! | macOS IOBluetooth RFCOMM | `macos` | `macos-rfcomm`, macOS | PROTOCOL.md §2.2 |
//! | Linux BlueZ raw RFCOMM socket | `linux` | `linux-rfcomm`, Linux | PROTOCOL.md §2.3 |
//! | USB printer class | `usb` | `usb` | PROTOCOL.md §2.8 |
//! | TCP 9100 | [`tcp`] | always | PROTOCOL.md §2.9 |
//! | In-process virtual printer | `virtual_transport` | `virtual` | ARCHITECTURE.md §8.1 |
//!
//! Endpoints are addressed with a URI-like string ([`Endpoint`]), e.g.
//! `serial:/dev/cu.PT-P710BTxxxx`, `bt:XX:XX:XX:XX:XX:XX`, `usb:`, `tcp:192.0.2.10`,
//! `virtual:24`. [`open`] turns one into a boxed [`Transport`]; [`discover`] lists candidates.
//!
//! macOS IOBluetooth delivers every callback on the main queue, so the main thread must pump
//! its run loop (PROTOCOL.md §2.2 step 6). Binaries call [`run_with_event_loop`] from `main`
//! and do all transport work inside the closure; on other platforms it simply calls it. GUI
//! applications, whose main thread already runs a run loop, call
//! `macos::use_host_run_loop()` instead and do transport work on worker threads.

use std::time::{Duration, Instant};

pub mod discovery;
pub mod endpoint;
pub mod error;
pub mod runloop;
pub mod serial;
pub mod tcp;

#[cfg(all(target_os = "linux", feature = "linux-rfcomm"))]
pub mod linux;
#[cfg(all(target_os = "macos", feature = "macos-rfcomm"))]
pub mod macos;
#[cfg(feature = "usb")]
pub mod usb;
#[cfg(feature = "virtual")]
pub mod virtual_transport;

pub use discovery::{DiscoverOptions, DiscoveredDevice, discover};
pub use endpoint::{BtAddr, Endpoint, OpenOptions, open};
pub use error::TransportError;
pub use runloop::run_with_event_loop;

/// Kind of link, for logs and UI hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportKind {
    /// OS serial device node.
    Serial,
    /// Bluetooth Classic RFCOMM socket/channel opened by this crate.
    Rfcomm,
    /// USB bulk endpoints.
    Usb,
    /// TCP port 9100.
    Tcp,
    /// In-process virtual printer.
    Virtual,
}

/// Static description of an open transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportInfo {
    /// Link kind.
    pub kind: TransportKind,
    /// Human-readable label, e.g. `"cu.PT-P710BTxxxx"`, `"RFCOMM ch 1"`, `"USB 04f9:20af"`.
    pub label: String,
    /// Largest single write the link accepts (e.g. the RFCOMM MTU); the transport chunks
    /// internally, this is informational.
    pub max_write: Option<usize>,
}

/// A blocking, bidirectional byte pipe to one printer.
///
/// Contract for implementors:
/// - [`Transport::write_all`] returns once the platform has **accepted** all bytes
///   (backpressure), not when the printer received them. It chunks to the link MTU itself,
///   never issues a zero-length write, and never retries a partially written buffer.
/// - [`Transport::read`] waits at most `timeout` and returns `Ok(0)` on timeout (not an
///   error). It returns whatever bytes are available (no framing). Every `Duration` is valid:
///   one too large to add to the clock (e.g. `Duration::MAX`) means "wait until data arrives";
///   it never panics.
/// - [`Transport::close`] is idempotent and also runs on drop (implementors add `Drop`).
/// - Errors are typed ([`TransportError`]); a lost link is [`TransportError::Closed`] or an
///   I/O error, never a panic.
pub trait Transport {
    /// Writes all bytes (see trait docs).
    ///
    /// # Errors
    /// Link failure or a closed transport.
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError>;

    /// Reads available bytes into `buf`, waiting up to `timeout` (any value, see the trait
    /// docs); `Ok(0)` on timeout.
    ///
    /// # Errors
    /// Link failure or a closed transport.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError>;

    /// Closes the link (idempotent).
    ///
    /// # Errors
    /// Failure while tearing down; the transport is unusable afterwards either way.
    fn close(&mut self) -> Result<(), TransportError>;

    /// Static description.
    fn info(&self) -> &TransportInfo;
}

/// `now + timeout`, or `None` (no deadline) when that instant is not representable.
pub(crate) fn deadline_after(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

/// Time left until `deadline`; `Duration::MAX` without a deadline.
pub(crate) fn time_left(deadline: Option<Instant>) -> Duration {
    deadline.map_or(Duration::MAX, |d| {
        d.saturating_duration_since(Instant::now())
    })
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        (**self).write_all(bytes)
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        (**self).read(buf, timeout)
    }
    fn close(&mut self) -> Result<(), TransportError> {
        (**self).close()
    }
    fn info(&self) -> &TransportInfo {
        (**self).info()
    }
}
