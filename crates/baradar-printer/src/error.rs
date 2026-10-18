//! Errors of the printer client, modelled on the web studio's `printer/errors.ts`: each one says
//! what failed and in which stage, so [`crate::problems`] can phrase it for the user.

use std::fmt;

use ptouch_transport::{Endpoint, TransportError};

/// Where in the connection's life an error happened (`ErrorStage` in the studio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Opening the transport.
    Open,
    /// Waiting for the first status after opening.
    Handshake,
    /// An explicit status request.
    Status,
    /// A print job.
    Print,
    /// A printer-settings read or write.
    Settings,
    /// Nothing in flight (keepalive, link lost or printer turned off while idle).
    Idle,
}

/// The kind of link, known before it is opened (picks problem wording and fallbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// Bluetooth Classic RFCOMM opened by this app (`bt:`).
    Bluetooth,
    /// An OS serial node (`serial:/dev/cu.…`, `COM5`, `/dev/rfcomm0`).
    Serial,
    /// USB printer class (`usb:`).
    Usb,
    /// Raw TCP 9100 (`tcp:`).
    Tcp,
    /// The in-process virtual printer (`virtual:`).
    Virtual,
    /// A transport supplied by the caller ([`crate::ConnectTarget::Custom`]).
    Custom,
}

impl LinkKind {
    /// The kind of `endpoint`.
    #[must_use]
    pub fn of(endpoint: &Endpoint) -> Self {
        match endpoint {
            Endpoint::Bluetooth { .. } => Self::Bluetooth,
            Endpoint::Serial { .. } => Self::Serial,
            Endpoint::Usb { .. } => Self::Usb,
            Endpoint::Tcp { .. } => Self::Tcp,
            Endpoint::Virtual { .. } => Self::Virtual,
            _ => Self::Custom,
        }
    }
}

/// Why opening a transport failed, classified for the user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenErrorKind {
    /// The port or device is held by another app (serial `EBUSY`, "in use", "already open").
    Busy,
    /// The OS refused access (file permissions, Bluetooth privacy setting).
    PermissionDenied,
    /// The USB interface could not be claimed (another driver or app holds it).
    UsbClaim,
    /// The USB device could not be opened (permissions).
    UsbAccess,
    /// The USB device is in Editor Lite (mass-storage) mode.
    EditorLite,
    /// No such device or port.
    NotFound,
    /// The backend is not available in this build or on this OS.
    Unsupported,
    /// No RFCOMM channel answered with a valid status.
    NoChannel,
    /// The printer did not answer the link setup in time (off, asleep, out of range).
    Timeout,
    /// The macOS Bluetooth event loop is not available (programming error).
    EventLoop,
    /// Anything else.
    Other,
}

/// A failed open: the classification plus the transport's own message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenError {
    /// Classification.
    pub kind: OpenErrorKind,
    /// The link that was being opened.
    pub link: LinkKind,
    /// The transport error's text.
    pub message: String,
}

impl OpenError {
    /// Classifies a transport error from opening a `link`.
    #[must_use]
    pub fn new(link: LinkKind, error: &TransportError) -> Self {
        let message = error.to_string();
        let lower = message.to_ascii_lowercase();
        let busy_text = [
            "in use",
            "busy",
            "already open",
            "access denied",
            "exclusive",
        ]
        .iter()
        .any(|w| lower.contains(w));
        let kind = match error {
            TransportError::Io(e) => match e.kind() {
                std::io::ErrorKind::PermissionDenied => OpenErrorKind::PermissionDenied,
                std::io::ErrorKind::NotFound => OpenErrorKind::NotFound,
                std::io::ErrorKind::ResourceBusy => OpenErrorKind::Busy,
                std::io::ErrorKind::TimedOut => OpenErrorKind::Timeout,
                _ if busy_text => OpenErrorKind::Busy,
                _ => OpenErrorKind::Other,
            },
            // serialport errors carry the OS description ("Resource busy", "Permission denied").
            TransportError::Serial(_) if busy_text => OpenErrorKind::Busy,
            TransportError::Serial(_) if lower.contains("permission denied") => {
                OpenErrorKind::PermissionDenied
            }
            TransportError::Serial(_)
                if lower.contains("no such file") || lower.contains("not found") =>
            {
                OpenErrorKind::NotFound
            }
            TransportError::Usb { op, .. } if op.contains("claim") => OpenErrorKind::UsbClaim,
            TransportError::Usb { .. }
                if lower.contains("access") || lower.contains("permission") =>
            {
                OpenErrorKind::UsbAccess
            }
            TransportError::Usb { .. } if busy_text => OpenErrorKind::UsbClaim,
            TransportError::EditorLiteMode { .. } => OpenErrorKind::EditorLite,
            TransportError::NotFound(_) | TransportError::InvalidEndpoint { .. } => {
                OpenErrorKind::NotFound
            }
            TransportError::Unsupported(_) => OpenErrorKind::Unsupported,
            TransportError::NoWorkingChannel => OpenErrorKind::NoChannel,
            TransportError::Timeout { .. } | TransportError::WriteTimeout => OpenErrorKind::Timeout,
            TransportError::EventLoopNotRunning => OpenErrorKind::EventLoop,
            _ if busy_text => OpenErrorKind::Busy,
            _ => OpenErrorKind::Other,
        };
        Self {
            kind,
            link,
            message,
        }
    }

    /// `true` for failures worth retrying ("Waking printer…"): the first Bluetooth connection to
    /// a sleeping printer often fails or times out, and the next one succeeds.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self.link, LinkKind::Bluetooth | LinkKind::Custom)
            && matches!(
                self.kind,
                OpenErrorKind::Timeout | OpenErrorKind::NoChannel | OpenErrorKind::Other
            )
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Every error the client reports. Cloneable, so it can travel in events.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    /// Opening the transport failed after `attempts` attempts.
    #[error(
        "could not open the printer after {attempts} attempt{}: {cause}",
        if *attempts == 1 { "" } else { "s" }
    )]
    Open {
        /// Attempts made.
        attempts: u8,
        /// The last attempt's error.
        cause: OpenError,
    },
    /// The protocol session failed or refused: handshake timeout, printer error during a job,
    /// cancel, printer turned off, a preflight error, a busy printer…
    #[error("{error}")]
    Session {
        /// The core's error (its `code()` is the studio's error code).
        error: ptouch::Error,
        /// Where it happened.
        stage: Stage,
        /// For job failures: the 1-based page to resume from (never used, as in the studio).
        resume_from_page: Option<u16>,
    },
    /// The link dropped (read or write failed, device gone).
    #[error("the connection to the printer was lost: {0}")]
    LinkLost(String),
    /// The operation needs a connected printer.
    #[error("no printer is connected")]
    NotConnected,
    /// A connect attempt was cancelled.
    #[error("connecting was cancelled")]
    Cancelled,
    /// The operation was abandoned because the printer was disconnected.
    #[error("disconnected")]
    Disconnected,
    /// Auto-discovery found no P-touch printer.
    #[error("no paired P-touch printer found")]
    NoPrinterFound,
    /// Reconnect without a previous connection.
    #[error("there is no printer to reconnect to")]
    NothingToReconnect,
    /// Bluetooth scanning or pairing failed.
    #[error("{0}")]
    Bluetooth(String),
}

impl ClientError {
    /// A session error in `stage`.
    #[must_use]
    pub fn session(error: ptouch::Error, stage: Stage) -> Self {
        Self::Session {
            error,
            stage,
            resume_from_page: None,
        }
    }

    /// The core error code (`"TIMEOUT"`, `"PRINTER"`…), for session errors.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Session { error, .. } => Some(error.code()),
            _ => None,
        }
    }

    /// A deliberate cancel (a job or a connect attempt): show nothing, or a quiet note.
    #[must_use]
    pub fn is_cancel(&self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Session {
                    error: ptouch::Error::Cancelled,
                    ..
                }
        )
    }

    /// BUSY (or NOT_READY) from the client or the session.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Session {
                error: ptouch::Error::Busy,
                ..
            }
        )
    }
}
