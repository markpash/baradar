//! In-process transport backed by `ptouch::VirtualPrinter` (feature `virtual`).
//!
//! # Contract
//! - `write_all` feeds the virtual printer with `now_ms` from a monotonic clock started at
//!   construction; `read` returns due frames, sleeping until the next one is due (bounded by
//!   the timeout) — so the CLI driver loop runs unchanged against it.
//! - Optional fragmentation (`fragment`) splits every frame into chunks of the given sizes,
//!   like RFCOMM does (e.g. `[7, 25]`). One `read` returns at most one chunk.
//! - [`VirtualTransport::printer`] exposes the device for assertions (`printed()`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ptouch::{Behaviour, TapeKind, VirtualPrinter};

use crate::{Transport, TransportError, TransportInfo, TransportKind};

/// Status media-type byte for laminated TZe tape (PROTOCOL.md §4.6.1).
const MEDIA_TYPE_LAMINATED: u8 = 0x01;

/// Virtual-printer transport.
#[derive(Debug)]
pub struct VirtualTransport {
    printer: VirtualPrinter,
    start: Instant,
    /// Chunks ready to be returned by `read`, in order.
    pending: VecDeque<Vec<u8>>,
    fragment: Vec<usize>,
    closed: bool,
    info: TransportInfo,
}

impl VirtualTransport {
    /// A PT-P710BT (or `profile`) with `media_width_mm` tape loaded.
    ///
    /// # Errors
    /// [`TransportError::Protocol`] if the model has no such media.
    pub fn new(
        profile: &'static ptouch::ModelProfile,
        media_width_mm: u8,
        behaviour: Behaviour,
    ) -> Result<Self, TransportError> {
        let tape =
            profile
                .tape(media_width_mm, TapeKind::Tze)
                .ok_or(ptouch::Error::UnsupportedMedia {
                    width_mm: media_width_mm,
                    media_type: MEDIA_TYPE_LAMINATED,
                })?;
        Ok(Self {
            printer: VirtualPrinter::new(profile, tape, behaviour),
            start: Instant::now(),
            pending: VecDeque::new(),
            fragment: Vec::new(),
            closed: false,
            info: TransportInfo {
                kind: TransportKind::Virtual,
                label: format!("virtual {} {media_width_mm} mm", profile.name),
                max_write: None,
            },
        })
    }

    /// Split every outgoing frame into chunks of these sizes (cycled; zero sizes ignored).
    #[must_use]
    pub fn with_fragmentation(mut self, sizes: Vec<usize>) -> Self {
        self.fragment = sizes.into_iter().filter(|&s| s > 0).collect();
        self
    }

    /// The device, for assertions.
    #[must_use]
    pub fn printer(&self) -> &VirtualPrinter {
        &self.printer
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Moves every reply (status frame or 3-byte settings reply) due at `now_ms` into `pending`,
    /// fragmented.
    fn collect_due(&mut self, now_ms: u64) {
        while let Some(frame) = self.printer.poll_output(now_ms) {
            if self.fragment.is_empty() {
                self.pending.push_back(frame);
                continue;
            }
            let mut rest: &[u8] = &frame;
            let mut sizes = self.fragment.iter().cycle();
            while !rest.is_empty() {
                let size = sizes.next().copied().unwrap_or(rest.len()).min(rest.len());
                let (head, tail) = rest.split_at(size);
                self.pending.push_back(head.to_vec());
                rest = tail;
            }
        }
    }
}

impl Transport for VirtualTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        let now = self.now_ms();
        self.printer.handle_input(bytes, now);
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = crate::deadline_after(timeout);
        loop {
            if self.pending.is_empty() {
                let now = self.now_ms();
                self.collect_due(now);
            }
            if let Some(chunk) = self.pending.front_mut() {
                let n = buf.len().min(chunk.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                chunk.drain(..n);
                if chunk.is_empty() {
                    self.pending.pop_front();
                }
                return Ok(n);
            }
            let left = crate::time_left(deadline);
            if left.is_zero() {
                return Ok(0);
            }
            let wait = match self.printer.next_output_at() {
                Some(due) => Duration::from_millis(due.saturating_sub(self.now_ms())).min(left),
                None => left,
            };
            // Always make progress in time, even when a frame is already due.
            std::thread::sleep(wait.max(Duration::from_millis(1)).min(left));
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.closed = true;
        self.pending.clear();
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}
