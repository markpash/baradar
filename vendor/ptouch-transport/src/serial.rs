//! OS serial-node transport via `serialport`.
//!
//! # Contract
//! - Opens `/dev/cu.*` (macOS), `COMn` (Windows) or `/dev/rfcommN` (Linux) at 9600 8N1, no
//!   flow control (the baud rate is ignored by Bluetooth pseudo-serial drivers; ≤ 38400 avoids
//!   `IOSSIOSPEED` on macOS, ARCHITECTURE.md §3.2), exclusive where supported.
//! - Asserts DTR/RTS after open, ignoring failure (P300BT stays silent otherwise,
//!   PROTOCOL.md §2.6).
//! - `read` maps `TimedOut` to `Ok(0)`; `write_all` writes in ≤ 4096-byte chunks, each bounded
//!   by a 60 s timeout. It does **not** `flush()`: that is `tcdrain(2)`, which has no timeout
//!   and blocks forever on a half-open `/dev/cu.*` node that accepts writes but has no link
//!   behind it (PROTOCOL.md §2.2 step 12). The trait only promises that the platform accepted
//!   the bytes, which `write` already guarantees.
//! - `close` waits up to [`CLOSE_DRAIN`] for the output queue to empty (polling `TIOCOUTQ`, so
//!   a last cancel or epilogue still goes out), discards what is left (`tcflush`, so the
//!   `close(2)` cannot block on undrained output), then waits the close wait (PROTOCOL.md §2.1
//!   "Timing").
//! - On macOS this is an opt-in fallback only (PROTOCOL.md §2.2 step 12: "works once").

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, StopBits};

use crate::{Transport, TransportError, TransportInfo, TransportKind};

/// Baud rate (ignored by Bluetooth pseudo-serial drivers).
const BAUD: u32 = 9600;
/// Maximum bytes per `write` call.
const WRITE_CHUNK: usize = 4096;
/// Per-chunk write timeout. Bluetooth serial drivers back-pressure while the printer prints.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest wait for queued output to drain on close.
pub const CLOSE_DRAIN: Duration = Duration::from_secs(2);

/// Serial-port transport.
pub struct SerialTransport {
    port: Option<Box<dyn SerialPort>>,
    info: TransportInfo,
    /// Wait after close (PROTOCOL.md §2.1 "Timing"); zero for [`SerialTransport::from_port`].
    close_wait: Duration,
}

impl std::fmt::Debug for SerialTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialTransport")
            .field("info", &self.info)
            .field("open", &self.port.is_some())
            .finish_non_exhaustive()
    }
}

/// Short label for a port path: the file name (`cu.PT-P710BTxxxx`, `rfcomm0`, `COM5`).
fn label_for(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

/// `true` when a serial port name looks like a P-touch printer (PROTOCOL.md §2.1 device
/// names): `PT-` in the name, or a Linux `/dev/rfcommN` node. On macOS only the `cu.`
/// call-out nodes are returned (the `tty.` twins block on carrier detect).
#[must_use]
pub fn is_candidate_port_name(name: &str) -> bool {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if file.starts_with("rfcomm") {
        return true;
    }
    if file.starts_with("tty.") {
        return false;
    }
    file.to_ascii_uppercase().contains("PT-")
}

impl SerialTransport {
    /// Opens `path` with the settings in the module docs.
    ///
    /// # Errors
    /// [`TransportError::Serial`] if the port cannot be opened.
    pub fn open(path: &str) -> Result<Self, TransportError> {
        let mut port = serialport::new(path, BAUD)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .dtr_on_open(true)
            .timeout(WRITE_TIMEOUT)
            .open()?;
        // Best effort: pseudo-terminals and some drivers reject modem-line ioctls.
        let _ = port.write_data_terminal_ready(true);
        let _ = port.write_request_to_send(true);
        Ok(Self::from_port(port, label_for(path)))
    }

    /// Wraps an already-open port (e.g. a pseudo-terminal in tests). No close wait.
    #[must_use]
    pub fn from_port(port: Box<dyn SerialPort>, label: String) -> Self {
        Self {
            port: Some(port),
            info: TransportInfo {
                kind: TransportKind::Serial,
                label,
                max_write: Some(WRITE_CHUNK),
            },
            close_wait: Duration::ZERO,
        }
    }

    /// Sets the wait after [`Transport::close`] ([`crate::OpenOptions::close_wait`]).
    pub fn set_close_wait(&mut self, wait: Duration) {
        self.close_wait = wait;
    }

    /// Serial ports whose name looks like a P-touch printer (`PT-` in the name), plus all
    /// `/dev/rfcomm*` nodes.
    ///
    /// # Errors
    /// [`TransportError::Serial`] if enumeration fails.
    pub fn list_candidates() -> Result<Vec<String>, TransportError> {
        let mut out: Vec<String> = serialport::available_ports()?
            .into_iter()
            .filter(|p| {
                is_candidate_port_name(&p.port_name)
                    // Windows names BT ports COMn; keep the ones the OS tags as Bluetooth.
                    || (cfg!(windows)
                        && matches!(p.port_type, serialport::SerialPortType::BluetoothPort))
            })
            .map(|p| p.port_name)
            .collect();
        // `available_ports` without libudev may miss rfcomm nodes and some BT nodes; scan /dev.
        #[cfg(unix)]
        if let Ok(rd) = std::fs::read_dir("/dev") {
            for entry in rd.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let is_node = name.starts_with("rfcomm")
                    || (name.starts_with("cu.") && is_candidate_port_name(name));
                if is_node {
                    let full = format!("/dev/{name}");
                    if !out.contains(&full) {
                        out.push(full);
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn port(&mut self) -> Result<&mut Box<dyn SerialPort>, TransportError> {
        self.port.as_mut().ok_or(TransportError::Closed)
    }
}

impl Transport for SerialTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let port = self.port()?;
        port.set_timeout(WRITE_TIMEOUT)?;
        for chunk in bytes.chunks(WRITE_CHUNK) {
            match port.write_all(chunk) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::TimedOut => {
                    return Err(TransportError::WriteTimeout);
                }
                Err(e) => return Err(e.into()),
            }
        }
        // No flush(): tcdrain has no timeout (see the module docs).
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let port = self.port()?;
        port.set_timeout(timeout)?;
        loop {
            return match port.read(buf) {
                // Raw mode with VMIN = 1: a zero-length read after poll readiness is a hang-up.
                Ok(0) => Err(TransportError::Closed),
                Ok(n) => Ok(n),
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(0),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => Err(e.into()),
            };
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        let Some(port) = self.port.take() else {
            return Ok(());
        };
        // Give queued output (a cancel sequence, the epilogue) a bounded chance to go out ...
        let deadline = Instant::now() + CLOSE_DRAIN;
        while port.bytes_to_write().is_ok_and(|n| n > 0) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // ... then discard the rest so that close(2) cannot block on it.
        let _ = port.clear(ClearBuffer::Output);
        // Dropping the port closes the descriptor and releases the exclusive lock.
        drop(port);
        std::thread::sleep(self.close_wait);
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for SerialTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_names() {
        assert!(is_candidate_port_name("/dev/cu.PT-P710BTxxxx"));
        assert!(!is_candidate_port_name("/dev/tty.PT-P710BTxxxx"));
        assert!(is_candidate_port_name("/dev/rfcomm0"));
        assert!(!is_candidate_port_name("/dev/cu.Bluetooth-Incoming-Port"));
        assert!(!is_candidate_port_name("COM5"));
        assert_eq!(label_for("/dev/cu.PT-P710BTxxxx"), "cu.PT-P710BTxxxx");
        assert_eq!(label_for("COM5"), "COM5");
    }
}
