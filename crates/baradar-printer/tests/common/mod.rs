//! Test helpers: a transport over ptouch's `VirtualPrinter` with knobs the stock `virtual:`
//! endpoint lacks (timing, failing opens, link loss, going silent), and event helpers.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_channel::Receiver;
use baradar_printer::{
    ClientChannels, ClientConfig, Command, ConnectTarget, Event, PrinterClient, TransportFactory,
};
use ptouch::{
    Behaviour, Bitmap, EncodedJob, JobOptions, Model, ModelProfile, TapeKind, VirtualPrinter,
    VirtualTiming,
};
use ptouch_transport::{Transport, TransportError, TransportInfo, TransportKind};

/// Knobs shared between a test and the transports its factory opens.
#[derive(Clone)]
pub struct Sim {
    pub model: Model,
    pub width_mm: u8,
    pub behaviour: Behaviour,
    pub timing: VirtualTiming,
    /// Opens that fail with a Bluetooth-like timeout before one succeeds.
    pub failing_opens: Arc<AtomicUsize>,
    /// Every open fails with this (non-transient) error kind.
    pub open_not_found: bool,
    /// Each open sleeps this long first (a hanging Bluetooth open).
    pub open_delay: Duration,
    /// Writes fail (link lost) once this many bytes were written in total.
    pub lose_after_bytes: Option<usize>,
    /// While set, the printer receives nothing and answers nothing.
    pub mute: Arc<AtomicBool>,
    /// Opens performed.
    pub opens: Arc<AtomicUsize>,
    /// The printer of the last open, for assertions.
    pub printer: Arc<Mutex<Option<VirtualPrinter>>>,
}

impl Sim {
    pub fn new(model: Model, width_mm: u8, behaviour: Behaviour) -> Self {
        Self {
            model,
            width_mm,
            behaviour,
            timing: VirtualTiming {
                base_print_ms: 150,
                per_line_ms: 1,
                cooling_ms: 300,
                ..VirtualTiming::default()
            },
            failing_opens: Arc::new(AtomicUsize::new(0)),
            open_not_found: false,
            open_delay: Duration::ZERO,
            lose_after_bytes: None,
            mute: Arc::new(AtomicBool::new(false)),
            opens: Arc::new(AtomicUsize::new(0)),
            printer: Arc::new(Mutex::new(None)),
        }
    }

    pub fn e720(width_mm: u8, behaviour: Behaviour) -> Self {
        Self::new(Model::PtE720bt, width_mm, behaviour)
    }

    pub fn profile(&self) -> &'static ModelProfile {
        ptouch::profile(self.model).unwrap()
    }

    pub fn target(&self) -> ConnectTarget {
        let sim = self.clone();
        let open: TransportFactory = Arc::new(move || sim.open());
        ConnectTarget::Custom {
            label: format!("sim {:?}", self.behaviour),
            open,
        }
    }

    fn open(&self) -> Result<Box<dyn Transport>, TransportError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.open_delay);
        if self.open_not_found {
            return Err(TransportError::NotFound(String::from("sim printer")));
        }
        if self
            .failing_opens
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(TransportError::Timeout {
                op: "rfcommChannelOpenComplete",
            });
        }
        let profile = self.profile();
        let tape = profile.tape(self.width_mm, TapeKind::Tze).unwrap();
        let printer = VirtualPrinter::new(profile, tape, self.behaviour).with_timing(self.timing);
        Ok(Box::new(SimTransport {
            sim: self.clone(),
            printer: Some(printer),
            start: Instant::now(),
            pending: VecDeque::new(),
            written: 0,
            closed: false,
            info: TransportInfo {
                kind: TransportKind::Virtual,
                label: String::from("sim"),
                max_write: None,
            },
        }))
    }

    pub fn printed_pages(&self) -> usize {
        self.printer
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |p| p.printed().len())
    }
}

struct SimTransport {
    sim: Sim,
    printer: Option<VirtualPrinter>,
    start: Instant,
    pending: VecDeque<Vec<u8>>,
    written: usize,
    closed: bool,
    info: TransportInfo,
}

impl SimTransport {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap()
    }
    fn printer(&mut self) -> &mut VirtualPrinter {
        self.printer.as_mut().unwrap()
    }
}

impl Transport for SimTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        self.written += bytes.len();
        if self.sim.lose_after_bytes.is_some_and(|n| self.written > n) {
            self.closed = true;
            return Err(TransportError::Closed);
        }
        if self.sim.mute.load(Ordering::SeqCst) {
            return Ok(());
        }
        let now = self.now();
        self.printer().handle_input(bytes, now);
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        let deadline = Instant::now() + timeout.min(Duration::from_secs(5));
        loop {
            let now = self.now();
            while let Some(frame) = self.printer().poll_output(now) {
                if !self.sim.mute.load(Ordering::SeqCst) {
                    self.pending.push_back(frame);
                }
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
            if Instant::now() >= deadline {
                return Ok(0);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.closed = true;
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for SimTransport {
    fn drop(&mut self) {
        *self.sim.printer.lock().unwrap() = self.printer.take();
    }
}

/// A client with fast test timing.
pub fn client(config: ClientConfig) -> ClientChannels {
    PrinterClient::spawn(config)
}

pub fn fast_config() -> ClientConfig {
    ClientConfig {
        open_backoff: vec![Duration::from_millis(10)],
        slow_open: Duration::from_millis(100),
        keepalive: None,
        ..ClientConfig::default()
    }
}

/// Receives events until `pick` returns `Some`, failing after `timeout`. Returns the picked
/// value and every event seen before it.
pub fn wait_for<T>(
    events: &Receiver<Event>,
    timeout: Duration,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> (T, Vec<Event>) {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    loop {
        match events.try_recv() {
            Ok(ev) => {
                if let Some(t) = pick(&ev) {
                    return (t, seen);
                }
                seen.push(ev);
            }
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "timed out after {timeout:?}; events so far: {seen:#?}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Connects and waits for `Connected`.
pub fn connect(ch: &ClientChannels, target: ConnectTarget) -> Vec<Event> {
    ch.client.send(Command::Connect(target)).unwrap();
    let (_, seen) = wait_for(&ch.events, Duration::from_secs(10), |e| {
        matches!(e, Event::Connected(_)).then_some(())
    });
    seen
}

/// Every state change in `events`.
pub fn states(events: &[Event]) -> Vec<baradar_printer::ClientState> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::State(s) => Some(*s),
            _ => None,
        })
        .collect()
}

/// An encoded job of `pages` labels for `profile` on `width_mm` laminated tape.
pub fn job(profile: &'static ModelProfile, width_mm: u8, pages: usize) -> EncodedJob {
    let tape = ptouch::tape_for_status(profile, width_mm, 0x01).unwrap();
    let labels: Vec<Bitmap> = (0..pages)
        .map(|i| {
            Bitmap::from_fn(120, tape.print_pins, move |x, y| {
                (x / 8 + u32::from(y) / 8 + i as u32).is_multiple_of(2)
            })
        })
        .collect();
    let (pages, opts) =
        ptouch::prepare_pages(profile, tape, &labels, &JobOptions::default()).unwrap();
    let refs: Vec<&Bitmap> = pages.iter().collect();
    ptouch::encode_job(profile, tape, &refs, &opts).unwrap()
}
