//! Printer connection for baradar.
//!
//! - [`PrinterClient`]: the app's printer client, a thread that owns the connection and talks
//!   to the UI through async channels (commands in, [`Event`]s and the [`PacketEntry`] log
//!   out). A port of the web studio's printer layer: states, "Waking printer…" retry,
//!   handshake, keepalive, print with progress, cancel, settings, diagnostics.
//! - [`problems`]: every failure as a user-facing [`Problem`] (the studio's `problems.ts`).
//! - [`Printer`]: a blocking connection for the command line.
//! - `bluetooth` (macOS): scanning and pairing.
//!
//! On macOS, a GUI app calls `ptouch_transport::macos::use_host_run_loop()` once at startup
//! (its main thread runs `NSApplication`); a command-line tool wraps its work in
//! `ptouch_transport::run_with_event_loop`.

#[cfg(target_os = "macos")]
pub mod bluetooth;
mod client;
mod conn;
pub mod error;
mod info;
mod link;
pub mod packet_log;
mod printer;
mod probe;
pub mod problems;

pub use client::{
    ClientChannels, ClientConfig, ClientState, ClientStopped, Command, ConnectTarget, Event,
    Failure, OpenedLink, PrintPhase, PrintProgress, PrinterClient, SettingsWriteResult,
    TransportFactory,
};
pub use error::{ClientError, LinkKind, OpenError, OpenErrorKind, Stage};
pub use info::{PrinterInfo, mm_text};
pub use packet_log::{PacketDir, PacketEntry, PacketLog};
pub use printer::{MODEL, Printer, find_printer};
pub use probe::{ProbeReport, ProbeStep};
pub use problems::{Problem, ProblemAction, ProblemId, Severity};
