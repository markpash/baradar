//! Raw TCP port 9100 transport.
//!
//! # Contract (PROTOCOL.md §2.9)
//! - Connects with `connect_timeout`, sets `TCP_NODELAY`, writes the unchanged byte stream.
//! - Reading the 32-byte status back over the socket is **UNKNOWN** for PT network models;
//!   `read` simply returns whatever arrives (`Ok(0)` on timeout). SNMP status is out of scope
//!   for this work package (noted as a follow-up).
//! - An orderly shutdown by the peer is reported as [`TransportError::Closed`] (never as
//!   `Ok(0)`, which means "timeout").

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{Transport, TransportError, TransportInfo, TransportKind};

/// Default write timeout (the printer may stop reading while it prints).
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// TCP transport.
#[derive(Debug)]
pub struct TcpTransport {
    stream: Option<TcpStream>,
    info: TransportInfo,
}

impl TcpTransport {
    /// Connects to `host:port`, trying every resolved address in turn.
    ///
    /// # Errors
    /// [`TransportError::Io`] on resolution or connection failure;
    /// [`TransportError::NotFound`] if the host resolves to no address.
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self, TransportError> {
        let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
        if addrs.is_empty() {
            return Err(TransportError::NotFound(format!("{host}:{port}")));
        }
        let timeout = if timeout.is_zero() {
            Duration::from_secs(10)
        } else {
            timeout
        };
        let mut last_err = None;
        for addr in &addrs {
            match TcpStream::connect_timeout(addr, timeout) {
                Ok(stream) => return Self::from_stream(stream, format!("tcp {host}:{port}")),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .map(TransportError::Io)
            .unwrap_or_else(|| TransportError::NotFound(format!("{host}:{port}"))))
    }

    /// Wraps an already-connected stream.
    ///
    /// # Errors
    /// [`TransportError::Io`] if socket options cannot be set.
    pub fn from_stream(stream: TcpStream, label: String) -> Result<Self, TransportError> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(DEFAULT_WRITE_TIMEOUT))?;
        Ok(Self {
            stream: Some(stream),
            info: TransportInfo {
                kind: TransportKind::Tcp,
                label,
                max_write: None,
            },
        })
    }

    /// Sets the write timeout (default 60 s). A zero duration means "no timeout".
    ///
    /// # Errors
    /// [`TransportError::Closed`] after `close`; [`TransportError::Io`] from the socket.
    pub fn set_write_timeout(&mut self, timeout: Duration) -> Result<(), TransportError> {
        let s = self.stream.as_ref().ok_or(TransportError::Closed)?;
        s.set_write_timeout((!timeout.is_zero()).then_some(timeout))?;
        Ok(())
    }
}

impl Transport for TcpTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let s = self.stream.as_mut().ok_or(TransportError::Closed)?;
        match s.write_all(bytes) {
            Ok(()) => Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                Err(TransportError::WriteTimeout)
            }
            Err(e) if is_disconnect(e.kind()) => Err(TransportError::Closed),
            Err(e) => Err(e.into()),
        }
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let s = self.stream.as_mut().ok_or(TransportError::Closed)?;
        // `set_read_timeout(Some(0))` is an error; a zero timeout is a non-blocking poll.
        let zero = timeout.is_zero();
        if zero {
            s.set_nonblocking(true)?;
        } else {
            s.set_read_timeout(Some(timeout))?;
        }
        let result = loop {
            match s.read(buf) {
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                r => break r,
            }
        };
        if zero {
            s.set_nonblocking(false)?;
        }
        match result {
            Ok(0) => Err(TransportError::Closed),
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(0),
            Err(e) if is_disconnect(e.kind()) => Err(TransportError::Closed),
            Err(e) => Err(e.into()),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        if let Some(s) = self.stream.take() {
            // The peer may already be gone; the socket is released on drop either way.
            let _ = s.shutdown(Shutdown::Both);
        }
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for TcpTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn is_disconnect(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected
            | ErrorKind::UnexpectedEof
    )
}
