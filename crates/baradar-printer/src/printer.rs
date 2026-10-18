//! Blocking connection to the printer for the command line: a driver loop around the sans-IO
//! [`ptouch::Session`] ([`crate::link::Link`]), modelled on `ptouch-cli`'s `driver.rs` (minus
//! Ctrl-C handling and progress reporting). The app uses [`crate::PrinterClient`] instead.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use ptouch::{
    Bitmap, Event, JobOptions, ModelProfile, Session, SessionConfig, SessionState, Status, TapeSpec,
};
use ptouch_transport::{DiscoverOptions, DiscoveredDevice, Endpoint, OpenOptions};

use crate::link::Link;

/// The model this crate targets.
pub const MODEL: ptouch::Model = ptouch::Model::PtE720bt;

/// Longest single read wait, so the loop re-checks session timeouts regularly.
const MAX_READ_WAIT: Duration = Duration::from_millis(200);
/// Safety net for waits the session should end by itself (handshake, status, drain).
const STALL_LIMIT_MS: u64 = 120_000;

/// An open, handshaken connection to a P-touch printer.
pub struct Printer {
    link: Link,
}

impl Printer {
    /// Finds the paired PT-E720BT (Bluetooth, serial node or USB; another P-touch printer if
    /// there is no PT-E720BT) and connects to it.
    pub fn find() -> Result<Self> {
        let device = find_printer()?.ok_or_else(|| {
            anyhow!(
                "no {} found; pair it in System Settings → Bluetooth (and allow this \
                 terminal Bluetooth access under Privacy & Security), or pass an endpoint",
                model_hint().name
            )
        })?;
        Self::connect(&device.endpoint)
    }

    /// Connects to an endpoint string such as `bt:XX-XX-XX-XX-XX-XX`, `serial:/dev/cu.…`,
    /// `usb:` or `virtual:12` (an in-process simulated PT-E720BT with 12 mm tape).
    pub fn open(endpoint: &str) -> Result<Self> {
        let endpoint: Endpoint = endpoint.parse().context("parsing the printer endpoint")?;
        Self::connect(&endpoint)
    }

    fn connect(endpoint: &Endpoint) -> Result<Self> {
        let transport =
            ptouch_transport::open(endpoint, &open_options()).context("opening the printer")?;
        let session = Session::new(Some(model_hint()), SessionConfig::default());
        let mut printer = Self {
            link: Link::new(transport, session, None),
        };
        let now = printer.link.now_ms();
        printer.link.session.connect(now);
        printer.wait_for("the connection handshake", |ev| match ev {
            Event::Ready(_) => Some(()),
            _ => None,
        })?;
        Ok(printer)
    }

    /// The last status the printer sent.
    pub fn status(&self) -> Result<&Status> {
        self.link
            .session
            .last_status()
            .context("no status received yet")
    }

    /// The model the printer reported.
    pub fn profile(&self) -> Result<&'static ModelProfile> {
        let status = self.status()?;
        self.link.session.profile().ok_or_else(|| {
            anyhow!(
                "unknown printer (series {:#04x}, model {:#04x})",
                status.series_code,
                status.model_code
            )
        })
    }

    /// The loaded tape, from the last status.
    pub fn tape(&self) -> Result<&'static TapeSpec> {
        let status = self.status()?;
        if !status.has_media() {
            bail!("no tape loaded");
        }
        Ok(ptouch::tape_for_status(
            self.profile()?,
            status.media_width_mm,
            status.media_type.to_byte(),
        )?)
    }

    /// Asks the printer for a fresh status.
    pub fn refresh_status(&mut self) -> Result<Status> {
        let now = self.link.now_ms();
        self.link.session.request_status(now)?;
        self.wait_for("the status request", |ev| match ev {
            Event::Status(st) => Some(st),
            _ => None,
        })
    }

    /// Prints one label. `label.height()` must be the tape's `print_pins`.
    pub fn print_bitmap(&mut self, label: &Bitmap) -> Result<()> {
        let status = self.status()?;
        if status.is_error() {
            bail!("printer reports errors: {}", status.errors);
        }
        let profile = self.profile()?;
        let tape = self.tape()?;
        let (pages, opts) = ptouch::prepare_pages(
            profile,
            tape,
            std::slice::from_ref(label),
            &JobOptions::default(),
        )?;
        let refs: Vec<&Bitmap> = pages.iter().collect();
        let job = ptouch::encode_job(profile, tape, &refs, &opts)?;

        let now = self.link.now_ms();
        self.link.session.submit(job, now)?;
        // No stall limit while printing: the session owns the (cooling-aware) page timeouts.
        loop {
            for ev in self.link.pump(MAX_READ_WAIT)? {
                match ev {
                    Event::JobCompleted => return self.drain(),
                    Event::Failed { error, .. } => {
                        // Let the session send its cancel sequence before giving up.
                        let _ = self.drain();
                        return Err(anyhow!(error).context("print job failed"));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Pumps until `pick` accepts an event, or the session reports a failure.
    fn wait_for<R>(&mut self, what: &str, mut pick: impl FnMut(Event) -> Option<R>) -> Result<R> {
        let since = self.link.now_ms();
        loop {
            for ev in self.link.pump(MAX_READ_WAIT)? {
                if let Event::Failed { error, .. } = ev {
                    return Err(anyhow!(error).context(format!("{what} failed")));
                }
                if let Some(r) = pick(ev) {
                    return Ok(r);
                }
            }
            if self.link.now_ms().saturating_sub(since) > STALL_LIMIT_MS {
                bail!("{what} stalled");
            }
        }
    }

    /// After a job: keep pumping while the session drains trailing status frames, so the
    /// epilogue reaches the printer and late errors (e.g. a cutter jam) are reported.
    fn drain(&mut self) -> Result<()> {
        let since = self.link.now_ms();
        while matches!(
            self.link.session.state(),
            SessionState::Draining | SessionState::Recovering
        ) && self.link.now_ms().saturating_sub(since) < STALL_LIMIT_MS
        {
            for ev in self.link.pump(MAX_READ_WAIT)? {
                if let Event::Failed { error, .. } = ev {
                    return Err(anyhow!(error).context("printer reported an error after the job"));
                }
            }
        }
        Ok(self.link.flush()?)
    }
}

/// The PT-E720BT profile: the model hint for opening links (RFCOMM channel order, the model a
/// `virtual:` endpoint simulates).
pub(crate) fn model_hint() -> &'static ModelProfile {
    ptouch::profile(MODEL).expect("PT-E720BT is in the ptouch model table")
}

/// Open options for every link: the PT-E720BT model hint, otherwise ptouch's defaults.
pub(crate) fn open_options() -> OpenOptions {
    OpenOptions {
        model_hint: Some(model_hint()),
        ..OpenOptions::default()
    }
}

/// Finds a paired or attached P-touch printer, preferring the PT-E720BT: paired Bluetooth
/// devices, USB printers and serial nodes, in that order of discovery. `Ok(None)` when there is
/// none. On macOS, call it from a worker thread inside `run_with_event_loop` or in host run
/// loop mode.
pub fn find_printer() -> Result<Option<DiscoveredDevice>> {
    let devices =
        ptouch_transport::discover(&DiscoverOptions::default()).context("discovering printers")?;
    Ok(pick_printer(devices))
}

/// The best candidate: a PT-E720BT first, then any other P-touch printer.
fn pick_printer(devices: Vec<DiscoveredDevice>) -> Option<DiscoveredDevice> {
    let is_e720 = |d: &DiscoveredDevice| {
        d.model.is_some_and(|m| m.model == MODEL) || d.label.to_ascii_uppercase().contains("E720")
    };
    let is_ptouch = |d: &DiscoveredDevice| {
        d.model.is_some() || d.label.trim_start().to_ascii_uppercase().starts_with("PT-")
    };
    let mut rest = Vec::new();
    for d in devices {
        if is_e720(&d) {
            return Some(d);
        }
        rest.push(d);
    }
    rest.into_iter().find(is_ptouch)
}
