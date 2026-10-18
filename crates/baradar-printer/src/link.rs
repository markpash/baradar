//! One open link: a transport plus the sans-IO [`ptouch::Session`] driving it. The pump is the
//! studio's `#pump` and ptouch-cli's `driver.rs` loop: write queued bytes, read until the
//! session's deadline (bounded), feed input and timeouts back, collect events.

use std::time::{Duration, Instant};

use ptouch::{Event, Session};
use ptouch_transport::{Transport, TransportError};

use crate::packet_log::PacketDir;

/// Receives every buffer written (`Sent`) and read (`Received`).
pub(crate) type PacketHook = Box<dyn FnMut(PacketDir, &[u8])>;

/// A transport and its session.
pub(crate) struct Link {
    transport: Box<dyn Transport>,
    pub(crate) session: Session,
    start: Instant,
    hook: Option<PacketHook>,
    closed: bool,
}

impl Link {
    /// Wraps an open transport; the session clock starts now.
    pub(crate) fn new(
        transport: Box<dyn Transport>,
        session: Session,
        hook: Option<PacketHook>,
    ) -> Self {
        Self {
            transport,
            session,
            start: Instant::now(),
            hook,
            closed: false,
        }
    }

    /// Milliseconds on the session's clock.
    pub(crate) fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Writes every buffer the session queued, in order.
    pub(crate) fn flush(&mut self) -> Result<(), TransportError> {
        while let Some(bytes) = self.session.poll_transmit() {
            if let Some(hook) = &mut self.hook {
                hook(PacketDir::Sent, &bytes);
            }
            self.transport.write_all(&bytes)?;
        }
        Ok(())
    }

    /// Events the session has queued.
    pub(crate) fn events(&mut self) -> Vec<Event> {
        std::iter::from_fn(|| self.session.poll_event()).collect()
    }

    /// One step: flush, then (if no event is pending) read for at most `max_wait` or until the
    /// session's deadline, feed input and timeouts back, flush again. Returns the events.
    pub(crate) fn pump(&mut self, max_wait: Duration) -> Result<Vec<Event>, TransportError> {
        self.flush()?;
        let mut events = self.events();
        if !events.is_empty() {
            return Ok(events);
        }
        let now = self.now_ms();
        let max_ms = u64::try_from(max_wait.as_millis()).unwrap_or(u64::MAX);
        let wait = self
            .session
            .poll_timeout()
            .map_or(max_ms, |d| d.saturating_sub(now))
            .min(max_ms);
        let mut buf = [0u8; 512];
        let n = self.transport.read(&mut buf, Duration::from_millis(wait))?;
        let now = self.now_ms();
        if n > 0 {
            let input = buf.get(..n).unwrap_or_default();
            if let Some(hook) = &mut self.hook {
                hook(PacketDir::Received, input);
            }
            self.session.handle_input(input, now);
        }
        if self.session.poll_timeout().is_some_and(|d| now >= d) {
            self.session.handle_timeout(now);
        }
        self.flush()?;
        events.extend(self.events());
        Ok(events)
    }

    /// Closes the transport (idempotent; errors ignored, the link is unusable either way).
    pub(crate) fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.transport.close();
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}
