//! Diagnostics link probe (the studio's `probe.ts`): open the transport without a session, send
//! the generic handshake and a status request (`00×200 1B 40 1B 69 61 01 1B 69 53`), wait up
//! to 5 s for a status frame, close; repeated with a 400 ms pause.

use std::thread;
use std::time::{Duration, Instant};

use ptouch::{STATUS_REQUEST, StatusFramer};
use ptouch_transport::{OpenOptions, Transport};

use crate::client::ConnectTarget;
use crate::conn::PacketSink;
use crate::info::PrinterInfo;
use crate::packet_log::{PacketDir, hex};

/// Wait for the status reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Pause between cycles.
const CYCLE_PAUSE: Duration = Duration::from_millis(400);
/// Most cycles per probe.
const MAX_CYCLES: u8 = 5;

/// One step of a probe cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeStep {
    /// 1-based cycle.
    pub cycle: u8,
    /// `open`, `write`, `status reply` or `close`.
    pub name: &'static str,
    /// The step succeeded.
    pub ok: bool,
    /// Duration of the step.
    pub ms: u64,
    /// Details (error text, bytes, frame hex).
    pub detail: String,
}

/// Result of a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Every step, in order.
    pub steps: Vec<ProbeStep>,
    /// The last status frame received, decoded.
    pub status: Option<PrinterInfo>,
    /// Cycles whose status request was answered.
    pub answered: u8,
    /// Cycles run.
    pub cycles: u8,
    /// Summary, e.g. "Done: 3 of 3 status requests answered.", or why the probe did not run.
    pub message: String,
}

impl ProbeReport {
    /// A probe that did not run.
    pub(crate) fn refused(message: &str) -> Self {
        Self {
            steps: Vec::new(),
            status: None,
            answered: 0,
            cycles: 0,
            message: message.to_owned(),
        }
    }
}

fn ms_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Runs `cycles` (1–5) probe cycles against `target`.
pub(crate) fn run(
    target: &ConnectTarget,
    options: &OpenOptions,
    cycles: u8,
    packets: &PacketSink,
) -> ProbeReport {
    let cycles = cycles.clamp(1, MAX_CYCLES);
    let mut report = ProbeReport {
        steps: Vec::new(),
        status: None,
        answered: 0,
        cycles,
        message: String::new(),
    };
    let opener: Box<dyn Fn() -> Result<Box<dyn Transport>, String>> = match target {
        ConnectTarget::Endpoint(endpoint) => {
            let endpoint = endpoint.clone();
            let options = options.clone();
            Box::new(move || ptouch_transport::open(&endpoint, &options).map_err(|e| e.to_string()))
        }
        ConnectTarget::Auto => match crate::printer::find_printer() {
            Ok(Some(device)) => {
                let options = options.clone();
                Box::new(move || {
                    ptouch_transport::open(&device.endpoint, &options).map_err(|e| e.to_string())
                })
            }
            Ok(None) => return ProbeReport::refused("No device chosen."),
            Err(e) => return ProbeReport::refused(&format!("Discovery failed: {e:#}")),
        },
        ConnectTarget::Custom { open, .. } => {
            let open = open.clone();
            Box::new(move || open().map_err(|e| e.to_string()))
        }
    };
    packets.note(format!("probe {target:?}, {cycles}×"));
    for cycle in 1..=cycles {
        if cycle > 1 {
            thread::sleep(CYCLE_PAUSE);
        }
        let t = Instant::now();
        let mut transport = match opener() {
            Ok(t) => t,
            Err(e) => {
                report.steps.push(ProbeStep {
                    cycle,
                    name: "open",
                    ok: false,
                    ms: ms_since(t),
                    detail: e,
                });
                continue;
            }
        };
        report.steps.push(ProbeStep {
            cycle,
            name: "open",
            ok: true,
            ms: ms_since(t),
            detail: transport.info().label.clone(),
        });

        let mut request = ptouch::encode::generic_handshake_sequence();
        request.extend_from_slice(&STATUS_REQUEST);
        let t = Instant::now();
        packets.data(PacketDir::Sent, &request);
        let written = transport.write_all(&request);
        report.steps.push(ProbeStep {
            cycle,
            name: "write",
            ok: written.is_ok(),
            ms: ms_since(t),
            detail: match &written {
                Ok(()) => format!("{} bytes", request.len()),
                Err(e) => e.to_string(),
            },
        });

        if written.is_ok() {
            let t = Instant::now();
            let (ok, detail) = match read_status(&mut *transport, packets) {
                Ok(status) => {
                    let detail = hex(&status.raw);
                    // The model comes from the frame's own series/model codes.
                    let model = ptouch::profile_by_codes(status.series_code, status.model_code);
                    report.status = Some(PrinterInfo::new(model, status));
                    report.answered = report.answered.saturating_add(1);
                    (true, detail)
                }
                Err(e) => (false, e),
            };
            report.steps.push(ProbeStep {
                cycle,
                name: "status reply",
                ok,
                ms: ms_since(t),
                detail,
            });
        }

        let t = Instant::now();
        let closed = transport.close();
        report.steps.push(ProbeStep {
            cycle,
            name: "close",
            ok: closed.is_ok(),
            ms: ms_since(t),
            detail: closed.err().map(|e| e.to_string()).unwrap_or_default(),
        });
    }
    report.message = format!(
        "Done: {} of {cycles} status requests answered.",
        report.answered
    );
    packets.note(report.message.clone());
    report
}

/// Reads until a complete status frame arrives or [`REPLY_TIMEOUT`] passes.
fn read_status(
    transport: &mut dyn Transport,
    packets: &PacketSink,
) -> Result<ptouch::Status, String> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut framer = StatusFramer::new();
    let mut buf = [0u8; 256];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(String::from("no status frame within 5 s"));
        }
        let n = transport
            .read(&mut buf, left.min(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        if n == 0 {
            continue;
        }
        let input = buf.get(..n).unwrap_or_default();
        packets.data(PacketDir::Received, input);
        framer.push(input);
        while let Some(frame) = framer.next_frame() {
            if let Ok(status) = frame {
                return Ok(status);
            }
        }
    }
}
