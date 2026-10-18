//! Transport error type.
//!
//! Every backend reports failures through [`TransportError`]. Variants carry the platform
//! status code where one exists (`IOReturn` on macOS, `errno` on Linux) so that a UI can map
//! them to actionable hints (ARCHITECTURE.md §4.4, `CliError::hint`).

/// Every error a transport can report.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// Operating-system I/O error (serial, TCP, Linux RFCOMM socket).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Error from the `serialport` crate.
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),
    /// USB error (message from `nusb`).
    #[error("USB error during {op}: {message}")]
    Usb {
        /// Operation that failed, e.g. `"claim_interface"`.
        op: &'static str,
        /// Backend message.
        message: String,
    },
    /// Bluetooth error with the platform code (`IOReturn` on macOS, `errno` on Linux).
    #[error("Bluetooth error during {op} (code {code:#x})")]
    Bluetooth {
        /// Operation that failed, e.g. `"openRFCOMMChannelSync"`.
        op: &'static str,
        /// Platform status code.
        code: i64,
    },
    /// The endpoint string could not be parsed.
    #[error("invalid endpoint {input:?}: {reason}")]
    InvalidEndpoint {
        /// The input.
        input: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// No such device / port.
    #[error("device not found: {0}")]
    NotFound(String),
    /// Backend not compiled in or not available on this OS.
    #[error("{0} is not supported in this build or on this platform")]
    Unsupported(&'static str),
    /// No RFCOMM channel candidate answered a status request (PROTOCOL.md §2.1).
    #[error("no RFCOMM channel answered with a valid status")]
    NoWorkingChannel,
    /// The link was closed or lost.
    #[error("transport closed")]
    Closed,
    /// A write did not complete in time.
    #[error("write timed out")]
    WriteTimeout,
    /// A connect / open step did not complete in time (printer off, asleep or out of range).
    #[error("timed out during {op}")]
    Timeout {
        /// The step that timed out, e.g. `"rfcommChannelOpenComplete"`.
        op: &'static str,
    },
    /// The USB device is in Editor Lite (mass-storage) mode and does not accept raster data
    /// (PROTOCOL.md §2.8). The user must switch Editor Lite off on the printer.
    #[error(
        "USB device 04f9:{product_id:04x} is in Editor Lite mode; switch Editor Lite off on the printer"
    )]
    EditorLiteMode {
        /// The Editor Lite product ID (0x2064 PT-P700, 0x2065 PT-P750W).
        product_id: u16,
    },
    /// A macOS Bluetooth operation was attempted without the main-thread event loop
    /// ([`crate::run_with_event_loop`]) running or host run loop mode
    /// (`macos::use_host_run_loop`) on, or from the main thread itself.
    #[error(
        "the Bluetooth event loop is not running (call ptouch_transport::run_with_event_loop from main, or use macos::use_host_run_loop and call from a worker thread)"
    )]
    EventLoopNotRunning,
    /// Protocol-core error (virtual transport).
    #[error(transparent)]
    Protocol(#[from] ptouch::Error),
}
