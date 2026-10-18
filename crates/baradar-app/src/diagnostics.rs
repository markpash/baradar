//! Diagnostics (port of `web/src/ui/diagnostics/Diagnostics.svelte` with `probe.ts`,
//! `report.ts`, `virtual.ts` and `testlabels.ts`), shown as a large dialog.
//!
//! The browser-only parts are adapted to the desktop: the environment card lists the
//! transports this build has instead of Web Serial / WebUSB support, "paired printers" lists
//! what discovery finds, and the link probe opens an endpoint typed by the user. The packet log,
//! status dump, test labels, virtual printer and "Copy diagnostics" (addresses masked) work as
//! in the studio.
//!
//! The printer client's packet channel has a single consumer: [`packet_log`] drains it into one
//! [`DiagLog`] per studio, created on first use and kept for the life of the app.

#[path = "diagnostics_report.rs"]
mod report;

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use baradar_core::doc::LabelDoc;
use baradar_core::render::renderer::{RenderOptions, RenderTarget, Renderer};
use baradar_core::vars::variables::{VariableContext, resolve_doc};
use baradar_printer::{
    ClientState, Command, ConnectTarget, Event, OpenedLink, PacketDir, PacketEntry, PacketLog, PrinterInfo,
    ProbeReport,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch::encode::{CutMode, EncodedJob, JobOptions};
use ptouch::{Bitmap, TapeKind};
use ptouch_transport::{DiscoverOptions, Endpoint};

use self::report::{TestLabel, kv, mask_text, orientation_doc, ruler_doc, run_file_base};
use crate::batch::bitmap_image;
use crate::studio::{Studio, StudioEvent, is_connected, local_today, width_byte};
use crate::view_model::format_mm;

/// Probes kept for the report (`the last 5 probes`).
const PROBES_KEPT: usize = 5;
/// Packet log lines shown (`newest 400`).
const LOG_SHOWN: usize = 400;
/// Packet log lines in the report.
const LOG_REPORT: usize = 200;
/// Decoded commands listed (`first 300`).
const COMMANDS_SHOWN: usize = 300;
/// Height of the packet log and command lists.
const LIST_H: f32 = 260.0;
const ROW_H: f32 = 18.0;
const MONO: &str = "Menlo";

// ------------------------------------------------------------------------------------------
// The packet log (one per studio)
// ------------------------------------------------------------------------------------------

/// One finished probe.
#[derive(Clone)]
pub struct ProbeRecord {
    /// The endpoint probed (as typed).
    pub endpoint: String,
    pub report: ProbeReport,
}

/// What the diagnostics view keeps across openings: the packet log (fed from the client's
/// packet channel), recent probes and the link the client opened.
pub struct DiagLog {
    pub log: PacketLog,
    pub probes: VecDeque<ProbeRecord>,
    /// The endpoint of the probe in flight.
    pub probing: Option<String>,
    pub link: Option<OpenedLink>,
    _drain: Task<()>,
    _sub: Subscription,
}

#[derive(Default)]
struct DiagLogs(HashMap<EntityId, Entity<DiagLog>>);

impl Global for DiagLogs {}

impl DiagLog {
    fn new(studio: &Entity<Studio>, cx: &mut Context<Self>) -> Self {
        let rx = studio.read(cx).packets.clone();
        let drain = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch: Vec<PacketEntry> = vec![first];
                while batch.len() < 500
                    && let Ok(e) = rx.try_recv()
                {
                    batch.push(e);
                }
                let pushed = this.update(cx, |d, cx| {
                    for e in batch {
                        d.log.push(e);
                    }
                    cx.notify();
                });
                if pushed.is_err() {
                    break;
                }
            }
        });
        let sub = cx.subscribe(studio, |d: &mut DiagLog, _, ev: &StudioEvent, cx| {
            let StudioEvent::Printer(e) = ev else { return };
            match e {
                Event::Probe(report) => {
                    let endpoint = d.probing.take().unwrap_or_default();
                    d.probes.push_back(ProbeRecord {
                        endpoint,
                        report: report.clone(),
                    });
                    while d.probes.len() > PROBES_KEPT {
                        d.probes.pop_front();
                    }
                    cx.notify();
                }
                Event::Opened(link) => {
                    d.link = Some(link.clone());
                    cx.notify();
                }
                Event::State(ClientState::Idle) => {
                    d.link = None;
                    cx.notify();
                }
                _ => {}
            }
        });
        Self {
            log: PacketLog::default(),
            probes: VecDeque::new(),
            probing: None,
            link: None,
            _drain: drain,
            _sub: sub,
        }
    }
}

/// The studio's packet log, created (and its channel drained from then on) on first use. Call
/// it at startup to capture packets from the start; the channel itself keeps the newest 2000.
pub fn packet_log(studio: &Entity<Studio>, cx: &mut App) -> Entity<DiagLog> {
    let id = studio.entity_id();
    if let Some(log) = cx.try_global::<DiagLogs>().and_then(|g| g.0.get(&id)).cloned() {
        return log;
    }
    let log = cx.new(|cx| DiagLog::new(studio, cx));
    cx.default_global::<DiagLogs>().0.insert(id, log.clone());
    log
}

/// Opens the Diagnostics view.
pub fn open_diagnostics(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let log = packet_log(&studio, cx);
    let view = cx.new(|cx| DiagnosticsView::new(studio, log, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Diagnostics")
            .w(px(1000.))
            .child(view.clone())
    });
}

// ------------------------------------------------------------------------------------------
// Virtual printer (virtual.ts)
// ------------------------------------------------------------------------------------------

/// `VirtualRun`: a label rendered, encoded, and decoded again by the virtual printer.
struct VirtualRun {
    doc: LabelDoc,
    target: RenderTarget,
    length_dots: i64,
    height_dots: i64,
    length_mm: f64,
    warnings: Vec<String>,
    bytes: Vec<u8>,
    job: EncodedJob,
    decoded: Vec<Bitmap>,
    violations: Vec<String>,
    commands: Arc<Vec<String>>,
    rendered_img: Option<Arc<RenderImage>>,
    decoded_img: Option<Arc<RenderImage>>,
}

/// One decoded command as a short line.
fn command_line(c: &ptouch::virtual_printer::parser::Command) -> String {
    use ptouch::virtual_printer::parser::Command as C;
    match c {
        C::RasterLine(data) => format!("RasterLine ({} bytes)", data.len()),
        C::Invalidate(n) => format!("Invalidate ({n} × 00)"),
        other => format!("{other:?}"),
    }
}

/// `runVirtual`: render → encode (as the studio prints) → bytes → decode.
fn run_virtual(renderer: &Renderer, doc: LabelDoc, target: RenderTarget) -> Result<VirtualRun, String> {
    let render = renderer
        .render(&doc, &target, &RenderOptions::default())
        .map_err(|e| e.to_string())?;
    if render.blocking {
        let msg = render
            .warnings
            .iter()
            .find(|w| w.blocking)
            .map_or_else(|| "The label cannot be printed as designed.".to_owned(), |w| w.message.clone());
        return Err(msg);
    }
    let copies = doc.print.copies.clamp(1, 99) as usize;
    let pages = vec![render.bitmap.clone(); copies];
    let opts = JobOptions {
        cut: if doc.print.auto_cut { CutMode::EveryLabel } else { CutMode::None },
        chain: doc.print.chain,
        mirror: doc.print.mirror,
        ..JobOptions::default()
    };
    let job = ptouch::prepare_pages(target.profile, target.tape, &pages, &opts)
        .and_then(|(pre, o)| {
            let refs: Vec<&Bitmap> = pre.iter().collect();
            ptouch::encode_job(target.profile, target.tape, &refs, &o)
        })
        .map_err(|e| e.to_string())?;
    let bytes = job.to_bytes();
    let decoded = ptouch::decode_job(target.profile, &bytes).map_err(|e| e.to_string())?;
    let colors = (0xFF_FF_FF, 0x00_00_00);
    Ok(VirtualRun {
        rendered_img: bitmap_image(&render.bitmap, colors),
        decoded_img: decoded.pages.first().and_then(|p| bitmap_image(p, colors)),
        length_dots: render.length_dots,
        height_dots: render.height_dots,
        length_mm: render.length_mm,
        warnings: render.warnings.iter().map(|w| w.message.clone()).collect(),
        commands: Arc::new(decoded.commands.iter().take(COMMANDS_SHOWN).map(command_line).collect()),
        violations: decoded.violations.iter().map(|v| format!("{v:?}")).collect(),
        decoded: decoded.pages,
        bytes,
        job,
        doc,
        target,
    })
}

fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

// ------------------------------------------------------------------------------------------
// The view
// ------------------------------------------------------------------------------------------

/// Paired / attached printers as found by discovery (labels masked when shown in a report).
type DeviceList = Result<Vec<String>, String>;

struct DiagnosticsView {
    studio: Entity<Studio>,
    log: Entity<DiagLog>,
    endpoint: Entity<InputState>,
    cycles: u8,
    probe_error: Option<String>,
    log_filter: Entity<InputState>,
    log_dir: Option<PacketDir>,
    devices: Option<DeviceList>,
    listing: bool,
    show_status: bool,
    test: TestLabel,
    width_mm: f64,
    run: Option<VirtualRun>,
    run_error: Option<String>,
    running: bool,
    copied: bool,
    _subs: Vec<Subscription>,
}

/// A text pill for the connection state.
fn state_text(s: ClientState) -> &'static str {
    match s {
        ClientState::Idle => "not connected",
        ClientState::Opening => "opening",
        ClientState::Waking => "waking",
        ClientState::Handshaking => "handshaking",
        ClientState::Connected => "connected",
        ClientState::Printing => "printing",
        ClientState::Cancelling => "cancelling",
        ClientState::Settings => "settings",
        ClientState::NoReply => "no reply",
        ClientState::Lost => "lost",
        ClientState::Error => "error",
    }
}

/// `tapeText`-like media summary: "24 mm white / black".
fn media_text(info: &PrinterInfo) -> String {
    let w = info
        .tape
        .map(|t| format_mm(f64::from(t.width_mm_x10) / 10.0))
        .or_else(|| info.tape_width_text())
        .unwrap_or_else(|| "no tape".into());
    format!("{w} {} / {}", info.tape_color().name(), info.text_color().name())
}

impl DiagnosticsView {
    fn new(studio: Entity<Studio>, log: Entity<DiagLog>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = studio.read(cx);
        let endpoint_default = s.prefs.last_endpoint.clone().unwrap_or_else(|| "virtual:12".into());
        let widths = s.tape_widths();
        let width_mm = if widths.contains(&s.doc.tape.width_mm) {
            s.doc.tape.width_mm
        } else {
            widths.first().copied().unwrap_or(12.0)
        };
        let endpoint = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("bt:…, serial:/dev/cu.…, usb:, tcp:host:9100, virtual:12")
                .default_value(endpoint_default)
        });
        let log_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter the log"));
        let subs = vec![
            cx.observe(&studio, |_, _, cx| cx.notify()),
            cx.observe(&log, |_, _, cx| cx.notify()),
            cx.observe(&log_filter, |_, _, cx| cx.notify()),
        ];
        Self {
            studio,
            log,
            endpoint,
            cycles: 1,
            probe_error: None,
            log_filter,
            log_dir: None,
            devices: None,
            listing: false,
            show_status: false,
            test: TestLabel::Orientation,
            width_mm,
            run: None,
            run_error: None,
            running: false,
            copied: false,
            _subs: subs,
        }
    }

    // ----- actions -----

    /// The link probe (`probe.ts`), refused while the studio is connected.
    fn run_probe(&mut self, cx: &mut Context<Self>) {
        let text = self.endpoint.read(cx).value().trim().to_owned();
        self.probe_error = None;
        if text.is_empty() {
            self.probe_error = Some("No device chosen.".into());
            cx.notify();
            return;
        }
        let s = self.studio.read(cx);
        if is_connected(s.conn) || s.conn.is_connecting() {
            self.probe_error =
                Some("Disconnect the studio first: a port can only be opened by one connection at a time.".into());
            cx.notify();
            return;
        }
        let endpoint: Endpoint = match text.parse() {
            Ok(e) => e,
            Err(e) => {
                self.probe_error = Some(format!("{e}"));
                cx.notify();
                return;
            }
        };
        let sent = s.printer.send(Command::Probe {
            target: Some(ConnectTarget::Endpoint(endpoint)),
            cycles: self.cycles,
        });
        if sent.is_err() {
            self.probe_error = Some("The printer client has stopped.".into());
        } else {
            self.log.update(cx, |d, cx| {
                d.probing = Some(text);
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Lists paired and attached printers (discovery; reads macOS's paired Bluetooth devices).
    fn list_devices(&mut self, cx: &mut Context<Self>) {
        if self.listing {
            return;
        }
        self.listing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: DeviceList = cx
                .background_executor()
                .spawn(async move {
                    ptouch_transport::discover(&DiscoverOptions::default())
                        .map(|list| {
                            list.into_iter()
                                .map(|d| {
                                    format!(
                                        "{} · {} · {:?}{}",
                                        d.label,
                                        d.endpoint,
                                        d.source,
                                        d.model.map(|m| format!(" · {}", m.name)).unwrap_or_default()
                                    )
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.listing = false;
                this.devices = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The render target for the virtual printer: the loaded tape when it matches the width.
    fn virtual_target(&self, cx: &App) -> Option<RenderTarget> {
        let s = self.studio.read(cx);
        let profile = s.model();
        let byte = width_byte(self.width_mm);
        let loaded = s
            .printer_model()
            .and(s.printer_info.as_ref())
            .and_then(|i| i.tape)
            .filter(|t| t.media_width_byte == byte);
        let tape = loaded.or_else(|| profile.tape(byte, TapeKind::Tze))?;
        Some(RenderTarget { profile, tape })
    }

    /// [Render & encode].
    fn render_encode(&mut self, cx: &mut Context<Self>) {
        let s = self.studio.read(cx);
        let (doc, target) = match self.test {
            TestLabel::Current => {
                let ctx = VariableContext::new(local_today());
                let doc = resolve_doc(&s.doc, s.preview_row, &ctx).doc.into_owned();
                match s.target() {
                    Ok(t) => (doc, t),
                    Err(e) => {
                        self.run_error = Some(e);
                        cx.notify();
                        return;
                    }
                }
            }
            kind => {
                let Some(target) = self.virtual_target(cx) else {
                    self.run_error = Some(format!(
                        "{} doesn’t take {} tape.",
                        self.studio.read(cx).model().name,
                        format_mm(self.width_mm)
                    ));
                    cx.notify();
                    return;
                };
                let area = target.area();
                let band_mm = area.height_dots as f64 / f64::from(area.dpi) * 25.4;
                let doc = if kind == TestLabel::Ruler {
                    ruler_doc(self.width_mm, band_mm, 60)
                } else {
                    orientation_doc(self.width_mm)
                };
                (doc, target)
            }
        };
        let renderer = self.studio.read(cx).renderer.clone();
        self.running = true;
        self.run_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { run_virtual(&renderer, doc, target) })
                .await;
            this.update(cx, |this, cx| {
                this.running = false;
                match result {
                    Ok(run) => this.run = Some(run),
                    Err(e) => {
                        this.run = None;
                        this.run_error = Some(e);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks where to save `bytes`, then writes them.
    fn save_file(&mut self, name: String, bytes: Vec<u8>, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_new_path(&downloads_dir(), Some(&name));
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let result = std::fs::write(&path, &bytes);
                cx.update(|window, cx| {
                    let msg = match result {
                        Ok(()) => format!("Saved {}", path.display()),
                        Err(e) => format!("Could not save the file: {e}"),
                    };
                    window.push_notification(Notification::new().message(msg), cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn print_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = &self.run else { return };
        let sent = self.studio.read(cx).printer.send(Command::Print(run.job.clone()));
        let msg = if sent.is_ok() { "Sent to the printer." } else { "The printer client has stopped." };
        window.push_notification(Notification::new().message(msg), cx);
    }

    /// `buildReport`: the "Copy diagnostics" text, masked.
    fn report(&self, cx: &App) -> String {
        let s = self.studio.read(cx);
        let d = self.log.read(cx);
        let mut lines: Vec<String> = Vec::new();
        lines.push("# baradar diagnostics".into());
        kv(&mut lines, "generated", baradar_core::doc::now_iso());
        kv(&mut lines, "app", format!("baradar {}", env!("CARGO_PKG_VERSION")));
        kv(&mut lines, "platform", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
        lines.push(String::new());
        lines.push("## Transports".into());
        for (k, v) in transports() {
            kv(&mut lines, k, v);
        }

        let model = s.model();
        let source = if s.printer_model().is_some() { "connected printer" } else { "design for" };
        let widths = s.tape_widths();
        lines.push(String::new());
        lines.push("## Printer model".into());
        kv(&mut lines, "model", format!("{} ({source})", model.name));
        if s.printer_model().is_some() && s.prefs.design_model != model.name {
            kv(&mut lines, "design for", s.prefs.design_model.clone());
        }
        let list: Vec<String> = widths.iter().map(|w| format_mm(*w).trim_end_matches(" mm").to_owned()).collect();
        kv(&mut lines, "tape widths", if list.is_empty() { String::new() } else { format!("{} mm", list.join(", ")) });
        let width_ok = widths.contains(&s.doc.tape.width_mm);
        kv(
            &mut lines,
            "label tape",
            format!("{}{}", format_mm(s.doc.tape.width_mm), if width_ok { "" } else { " (not taken by this model)" }),
        );

        lines.push(String::new());
        lines.push("## Connection".into());
        kv(&mut lines, "endpoint", s.prefs.last_endpoint.clone().unwrap_or_default());
        kv(&mut lines, "state", state_text(s.conn));
        kv(
            &mut lines,
            "transport",
            d.link
                .as_ref()
                .map(|l| format!("{:?} · {} · {}", l.link, l.label, l.transport.label))
                .unwrap_or_default(),
        );
        if let Some(info) = &s.printer_info {
            kv(&mut lines, "model", info.model_name().unwrap_or("unknown"));
            kv(&mut lines, "media", info.tape.map(|t| t.id.to_owned()).unwrap_or_default());
            kv(
                &mut lines,
                "status",
                format!("{} · {}", if info.is_ready() { "ready" } else { "not ready" }, media_text(info)),
            );
            kv(&mut lines, "status hex", info.raw_hex());
            let errors = info.errors();
            if !errors.is_empty() {
                let list: Vec<String> = errors.iter().map(ToString::to_string).collect();
                kv(&mut lines, "printer errors", list.join(", "));
            }
        } else {
            kv(&mut lines, "status hex", String::new());
        }
        if let Some((attempt, of)) = s.open_progress {
            kv(&mut lines, "open progress", format!("attempt {attempt} of {of}"));
        }
        if let Some(p) = &s.problem {
            kv(&mut lines, "problem", format!("{:?} ({:?}): {}", p.id, p.severity, p.title));
            if let Some(t) = &p.technical {
                kv(&mut lines, "technical", t.clone());
            }
        }

        if let Some(Ok(devices)) = &self.devices {
            lines.push(String::new());
            lines.push("## Printers found".into());
            if devices.is_empty() {
                lines.push("none".into());
            }
            lines.extend(devices.iter().cloned());
        }

        for p in &d.probes {
            lines.push(String::new());
            lines.push(format!("## Probe: {}", p.endpoint));
            for st in &p.report.steps {
                lines.push(format!(
                    "{} {:<22} {:>9}{}",
                    if st.ok { "ok  " } else { "FAIL" },
                    format!("{} #{}", st.name, st.cycle),
                    format!("{} ms", st.ms),
                    if st.detail.is_empty() { String::new() } else { format!("  {}", st.detail) }
                ));
            }
            lines.push(p.report.message.clone());
            if let Some(info) = &p.report.status {
                lines.push(format!("reply: {}", info.raw_hex()));
            }
        }

        let n = d.log.entries().len();
        lines.push(String::new());
        lines.push(format!("## Packet log (last {} of {n})", LOG_REPORT.min(n)));
        lines.push(d.log.to_text(LOG_REPORT));
        mask_text(&lines.join("\n"))
    }

    fn copy_report(&mut self, cx: &mut Context<Self>) {
        let text = self.report(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ----- rendering -----

    fn card(title: &str, theme: &gpui_kit::component::Theme) -> Div {
        v_flex()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title.to_owned()))
    }

    fn row(key: &str, value: impl Into<SharedString>, theme: &gpui_kit::component::Theme) -> Div {
        h_flex()
            .gap_3()
            .items_start()
            .text_sm()
            .child(div().w(px(140.)).flex_shrink_0().text_color(theme.muted_foreground).child(key.to_owned()))
            .child(div().flex_1().min_w_0().child(value.into()))
    }

    fn render_environment(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut card = Self::card("Environment", &theme)
            .child(Self::row("App", format!("baradar {}", env!("CARGO_PKG_VERSION")), &theme))
            .child(Self::row(
                "Platform",
                format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                &theme,
            ));
        for (k, v) in transports() {
            card = card.child(Self::row(&crate::view_model::capitalize(k), v, &theme));
        }
        card = card.child(
            v_flex()
                .gap_1()
                .pt_2()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Printers found"))
                        .child(
                            Button::new("list-devices")
                                .small()
                                .outline()
                                .icon(IconName::Search)
                                .label(if self.listing { "Looking…" } else { "List printers" })
                                .disabled(self.listing)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.list_devices(cx))),
                        ),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    "Paired Bluetooth printers, USB printers and serial device nodes. Nothing is opened.",
                ))
                .map(|d| match &self.devices {
                    None => d,
                    Some(Err(e)) => d.child(div().text_xs().text_color(theme.danger).child(e.clone())),
                    Some(Ok(list)) if list.is_empty() => {
                        d.child(div().text_sm().text_color(theme.muted_foreground).child("No printers found."))
                    }
                    Some(Ok(list)) => d.children(
                        list.iter().map(|l| div().text_sm().font_family(MONO).child(mask_text(l))),
                    ),
                }),
        );
        card.into_any_element()
    }

    fn render_connection(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let d = self.log.read(cx);
        let connected = is_connected(s.conn);
        let model = s.model();
        let mut card = Self::card("Studio connection", &theme)
            .child(Self::row("State", state_text(s.conn), &theme))
            .child(Self::row(
                "Endpoint",
                s.prefs.last_endpoint.as_deref().map(mask_text).unwrap_or_else(|| "—".into()),
                &theme,
            ))
            .child(Self::row(
                "Transport",
                d.link
                    .as_ref()
                    .map(|l| mask_text(&format!("{:?} · {}", l.link, l.transport.label)))
                    .unwrap_or_else(|| "—".into()),
                &theme,
            ))
            .child(Self::row(
                "Designing for",
                format!(
                    "{} ({})",
                    model.name,
                    if s.printer_model().is_some() { "connected printer" } else { "design for" }
                ),
                &theme,
            ));
        if let Some((attempt, of)) = s.open_progress {
            card = card.child(Self::row("Opening", format!("attempt {attempt} of {of}"), &theme));
        }
        if let Some(info) = &s.printer_info {
            card = card
                .child(Self::row("Model", info.model_name().unwrap_or("unknown model"), &theme))
                .child(Self::row("Media", media_text(info), &theme))
                .child(Self::row("Battery", format!("{:?}", info.battery), &theme))
                .child(Self::row(
                    "Ready",
                    if info.is_ready() { "yes".to_owned() } else { "no".to_owned() },
                    &theme,
                ));
            let errors = info.errors();
            if !errors.is_empty() {
                let list: Vec<String> = errors.iter().map(ToString::to_string).collect();
                card = card.child(Self::row("Printer errors", list.join(", "), &theme));
            }
            card = card.child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .text_sm()
                    .child(div().w(px(140.)).flex_shrink_0().text_color(theme.muted_foreground).child("Last status"))
                    .child(div().flex_1().min_w_0().font_family(MONO).text_xs().child(info.raw_hex())),
            );
            card = card.child(
                div()
                    .id("toggle-status")
                    .text_xs()
                    .text_color(theme.primary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_status = !this.show_status;
                        cx.notify();
                    }))
                    .child(if self.show_status { "Hide decoded status" } else { "Show decoded status" }),
            );
            if self.show_status {
                card = card.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(theme.secondary)
                        .font_family(MONO)
                        .text_xs()
                        .child(format!("{:#?}", info.status)),
                );
            }
        }
        if let Some(p) = &s.problem {
            card = card.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.danger)
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(p.title.clone()))
                    .child(div().text_sm().child(p.detail.clone()))
                    .when_some(p.technical.clone(), |d, t| {
                        d.child(div().text_xs().font_family(MONO).text_color(theme.muted_foreground).child(mask_text(&t)))
                    }),
            );
        }
        card = card.child(
            h_flex()
                .gap_2()
                .when(connected, |d| {
                    d.child(
                        Button::new("refresh-status")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label("Request status")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                let _ = this.studio.read(cx).printer.send(Command::RefreshStatus);
                            })),
                    )
                    .child(
                        Button::new("disconnect")
                            .small()
                            .outline()
                            .icon(crate::icons::icon("eject")).label("Disconnect")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.studio.update(cx, |s, cx| s.disconnect(cx));
                            })),
                    )
                })
                .child(
                    Button::new("printer-settings")
                        .small()
                        .outline()
                        .icon(IconName::Settings)
                        .label("Printer settings…")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            crate::connect::open_printer_settings(this.studio.clone(), window, cx);
                        })),
                )
                .when(!connected, |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Connect a printer from the top bar, or probe one below."),
                    )
                }),
        );
        card.into_any_element()
    }

    fn render_probe(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let d = self.log.read(cx);
        let probing = d.probing.is_some();
        let cycles = [1u8, 3, 5].map(|n| {
            Button::new(SharedString::from(format!("cycles-{n}")))
                .small()
                .label(format!("{n}×"))
                .map(|b| if self.cycles == n { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.cycles = n;
                    cx.notify();
                }))
        });
        let mut card = Self::card("Link probe", &theme)
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Opens the endpoint without a session, sends the handshake and a status request, waits up to 5 s for a status reply and closes; repeated with a 400 ms pause.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.endpoint).small()))
                    .children(cycles)
                    .child(
                        Button::new("run-probe")
                            .small()
                            .primary()
                            .icon(crate::icons::icon("activity")).label(if probing { "Probing…" } else { "Run probe" })
                            .loading(probing)
                            .disabled(probing)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.run_probe(cx))),
                    ),
            )
            .when_some(self.probe_error.clone(), |c, e| {
                c.child(div().text_sm().text_color(theme.danger).child(e))
            });
        if let Some(p) = d.probes.back() {
            let steps = p.report.steps.iter().map(|st| {
                h_flex()
                    .gap_3()
                    .text_xs()
                    .font_family(MONO)
                    .child(
                        div()
                            .w(px(36.))
                            .text_color(if st.ok { theme.success } else { theme.danger })
                            .child(if st.ok { "ok" } else { "FAIL" }),
                    )
                    .child(div().w(px(28.)).child(format!("#{}", st.cycle)))
                    .child(div().w(px(100.)).child(st.name))
                    .child(div().w(px(70.)).child(format!("{} ms", st.ms)))
                    .child(div().flex_1().min_w_0().child(mask_text(&st.detail)))
            });
            card = card
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(mask_text(&p.endpoint)))
                .child(v_flex().gap_0p5().children(steps))
                .child(div().text_sm().child(p.report.message.clone()));
            if let Some(info) = &p.report.status {
                card = card.child(Self::row("Reply", media_text(info), &theme)).child(
                    div().text_xs().font_family(MONO).child(info.raw_hex()),
                );
            }
        }
        card.into_any_element()
    }

    fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let d = self.log.read(cx);
        let filter = self.log_filter.read(cx).value().to_lowercase();
        let total = d.log.entries().len();
        let lines: Vec<String> = d
            .log
            .entries()
            .rev()
            .filter(|e| self.log_dir.is_none_or(|dir| e.dir == dir))
            .map(ToString::to_string)
            .filter(|l| filter.is_empty() || l.to_lowercase().contains(&filter))
            .take(LOG_SHOWN)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let lines = Arc::new(lines);
        let dirs: [(Option<PacketDir>, &str); 4] = [
            (None, "All"),
            (Some(PacketDir::Sent), ">> sent"),
            (Some(PacketDir::Received), "<< received"),
            (Some(PacketDir::Note), "-- notes"),
        ];
        let dir_buttons = dirs.into_iter().enumerate().map(|(i, (dir, label))| {
            Button::new(SharedString::from(format!("log-dir-{i}")))
                .xsmall()
                .label(label)
                .map(|b| if self.log_dir == dir { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.log_dir = dir;
                    cx.notify();
                }))
        });
        let count = lines.len();
        let rows = lines.clone();
        Self::card("Packet log", &theme)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(220.)).child(Input::new(&self.log_filter).small().cleanable(true)))
                    .children(dir_buttons)
                    .child(div().flex_1())
                    .child(
                        Button::new("log-copy")
                            .xsmall()
                            .outline()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(mask_text(&lines.join("\n"))));
                                window.push_notification(Notification::new().message("Packet log copied"), cx);
                            })),
                    )
                    .child(
                        Button::new("log-clear")
                            .xsmall()
                            .ghost()
                            .icon(crate::icons::icon("trash")).label("Clear")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.log.update(cx, |d, cx| {
                                    d.log.clear();
                                    cx.notify();
                                });
                            })),
                    ),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                "{count} of {total} entries shown (newest {LOG_SHOWN}). >> sent, << received, -- note; writes are cut to 96 bytes."
            )))
            .child(
                div()
                    .h(px(LIST_H))
                    .p_1()
                    .rounded_md()
                    .bg(theme.secondary)
                    .child(
                        uniform_list("packet-log", count, move |range: Range<usize>, _, _| {
                            range
                                .filter_map(|i| {
                                    let line = rows.get(i)?.clone();
                                    Some(
                                        div()
                                            .id(i)
                                            .h(px(ROW_H))
                                            .whitespace_nowrap()
                                            .font_family(MONO)
                                            .text_xs()
                                            .child(line),
                                    )
                                })
                                .collect()
                        })
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    fn render_virtual(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let widths = s.tape_widths();
        let connected = is_connected(s.conn);
        let loaded_byte = s.printer_info.as_ref().and_then(|i| i.tape).map(|t| t.media_width_byte);
        let sources = TestLabel::ALL.map(|t| {
            Button::new(SharedString::from(format!("src-{}", t.label())))
                .small()
                .label(t.label())
                .map(|b| if self.test == t { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.test = t;
                    cx.notify();
                }))
        });
        let width_buttons = widths.iter().enumerate().map(|(i, w)| {
            let w = *w;
            Button::new(SharedString::from(format!("vw-{i}")))
                .xsmall()
                .label(format_mm(w))
                .disabled(self.test == TestLabel::Current)
                .map(|b| if (self.width_mm - w).abs() < 0.01 { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.width_mm = w;
                    cx.notify();
                }))
        });
        let mut card = Self::card("Virtual printer & test labels", &theme)
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Renders a label, encodes it as a print job for the model the studio designs for, and decodes the job again as the printer would. Nothing is sent unless you print.",
            ))
            .child(h_flex().gap_2().flex_wrap().child(div().text_sm().w(px(80.)).child("Label")).children(sources))
            .child(h_flex().gap_1().flex_wrap().child(div().text_sm().w(px(80.)).child("Tape")).children(width_buttons))
            .child(
                h_flex().child(
                    Button::new("render-encode")
                        .small()
                        .primary()
                        .icon(crate::icons::icon("printer")).label("Render & encode")
                        .loading(self.running)
                        .disabled(self.running)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.render_encode(cx))),
                ),
            )
            .when_some(self.run_error.clone(), |c, e| c.child(div().text_sm().text_color(theme.danger).child(e)));

        if let Some(run) = &self.run {
            let dpi = run.target.profile.dpi;
            let base = run_file_base(&run.doc.name, f64::from(run.target.tape.width_mm_x10) / 10.0);
            let can_print = connected && loaded_byte == Some(run.target.tape.media_width_byte);
            card = card
                .child(Self::row(
                    "Target",
                    format!("{} · {} ({dpi} dpi)", run.target.profile.name, run.target.tape.id),
                    &theme,
                ))
                .child(Self::row(
                    "Label",
                    format!("{} × {} dots ({:.1} mm)", run.length_dots, run.height_dots, run.length_mm),
                    &theme,
                ))
                .child(Self::row(
                    "Job",
                    format!(
                        "{} bytes · {} pages · {} lines",
                        run.bytes.len(),
                        run.job.page_count(),
                        run.job.total_lines()
                    ),
                    &theme,
                ))
                .child(Self::row(
                    "Protocol violations",
                    if run.violations.is_empty() { "none".to_owned() } else { run.violations.join("; ") },
                    &theme,
                ))
                .when(!run.warnings.is_empty(), |c| {
                    c.child(Self::row("Render warnings", run.warnings.join(" "), &theme))
                });
            let picture = |title: &str, image: Option<Arc<RenderImage>>| {
                v_flex()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(title.to_owned()))
                    .child(
                        div()
                            .h(px(90.))
                            .p_1()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .map(|d| match image {
                                Some(i) => d.child(img(i).max_h(px(80.)).max_w_full().object_fit(ObjectFit::Contain)),
                                None => d.child(div().text_xs().child("Empty")),
                            }),
                    )
            };
            card = card.child(
                h_flex()
                    .gap_3()
                    .child(picture("Rendered", run.rendered_img.clone()))
                    .child(picture("Decoded (what the printer prints)", run.decoded_img.clone())),
            );
            let bin = run.bytes.clone();
            let bin_name = format!("{base}.bin");
            let page = run.decoded.first().cloned();
            let png_page = page.clone();
            let png_name = format!("{base}-decoded.png");
            let pbm_name = format!("{base}-decoded.pbm");
            let open_doc = run.doc.clone();
            card = card.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("save-bin")
                            .small()
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Save job .bin…")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.save_file(bin_name.clone(), bin.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("save-png")
                            .small()
                            .outline()
                            .icon(crate::icons::icon("download")).label("Decoded page PNG…")
                            .disabled(page.is_none())
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let Some(p) = &png_page else { return };
                                match baradar_core::export::png::export_png(p, dpi) {
                                    Ok(bytes) => this.save_file(png_name.clone(), bytes, window, cx),
                                    Err(e) => window.push_notification(
                                        Notification::new().message(format!("PNG encoding failed: {e}")),
                                        cx,
                                    ),
                                }
                            })),
                    )
                    .child(
                        Button::new("save-pbm")
                            .small()
                            .outline()
                            .icon(crate::icons::icon("download")).label("Decoded page PBM…")
                            .disabled(page.is_none())
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                if let Some(p) = &page {
                                    this.save_file(pbm_name.clone(), p.to_pbm(), window, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("open-in-studio")
                            .small()
                            .outline()
                            .icon(crate::icons::icon("file-plus")).label("Open in studio")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let doc = open_doc.clone();
                                this.studio.update(cx, |s, cx| s.open_new(doc, cx));
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("print-run")
                            .small()
                            .outline()
                            .icon(crate::icons::icon("printer")).label("Print on printer")
                            .disabled(!can_print)
                            .tooltip(if can_print {
                                "Send this job to the connected printer"
                            } else {
                                "Connect a printer with this tape loaded"
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.print_run(window, cx))),
                    ),
            );
            let commands = run.commands.clone();
            let n = commands.len();
            card = card
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(format!(
                    "Decoded commands (first {n})"
                )))
                .child(
                    div().h(px(LIST_H)).p_1().rounded_md().bg(theme.secondary).child(
                        uniform_list("decoded-commands", n, move |range: Range<usize>, _, _| {
                            range
                                .filter_map(|i| {
                                    let line = commands.get(i)?.clone();
                                    Some(
                                        div()
                                            .id(i)
                                            .h(px(ROW_H))
                                            .whitespace_nowrap()
                                            .font_family(MONO)
                                            .text_xs()
                                            .child(format!("{:>4}  {line}", i + 1)),
                                    )
                                })
                                .collect()
                        })
                        .size_full(),
                    ),
                );
        }
        card.into_any_element()
    }
}

/// Transports this build can open (the desktop stand-in for the browser support card).
fn transports() -> Vec<(&'static str, String)> {
    let bluetooth = if cfg!(target_os = "macos") {
        "Bluetooth Classic RFCOMM (IOBluetooth), bt:<address>"
    } else if cfg!(target_os = "linux") {
        "Bluetooth Classic RFCOMM (BlueZ), bt:<address>"
    } else {
        "not on this platform; use the printer’s serial port"
    };
    vec![
        ("bluetooth", bluetooth.to_owned()),
        ("usb", "USB printer class, usb:".to_owned()),
        ("serial", "device nodes, serial:/dev/cu.… or COMn".to_owned()),
        ("network", "raw TCP, tcp:<host>:9100".to_owned()),
        ("virtual", "virtual printer, virtual:<tape mm>".to_owned()),
    ]
}

impl Render for DiagnosticsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let environment = self.render_environment(cx);
        let connection = self.render_connection(cx);
        let probe = self.render_probe(cx);
        let log = self.render_log(cx);
        let virt = self.render_virtual(cx);
        v_flex()
            .gap_3()
            .pb_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().text_color(cx.theme().muted_foreground).child(
                        "Connection details, raw printer traffic and test prints. “Copy diagnostics” masks printer names, Bluetooth addresses and serial numbers.",
                    ))
                    .child(
                        Button::new("copy-diagnostics")
                            .small()
                            .primary()
                            .icon(if self.copied { IconName::Check } else { IconName::Copy })
                            .label(if self.copied { "Copied ✓" } else { "Copy diagnostics" })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_report(cx))),
                    ),
            )
            .child(h_flex().gap_3().items_start().child(div().flex_1().min_w_0().child(environment)).child(div().flex_1().min_w_0().child(connection)))
            .child(probe)
            .child(log)
            .child(virt)
    }
}

/// Headless UI tests against the in-process virtual printer (never Bluetooth).
#[cfg(test)]
mod ui_tests {
    // NOT `use super::*` (see `batch::ui_tests`).
    use super::{DiagnosticsView, open_diagnostics, packet_log};
    use crate::batch::ui_tests::{new_studio, open_test_window, wait_until};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext};

    #[gpui_kit::test]
    async fn probe_virtual_printer_encode_and_copy_report(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "diagnostics");
        let log = cx.update(|cx| packet_log(&studio, cx));
        let (s2, l2) = (studio.clone(), log.clone());
        let (handle, view) = open_test_window(cx, move |window, cx| DiagnosticsView::new(s2, l2, window, cx));

        // Probe the virtual printer (the default endpoint without a remembered printer).
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("run-probe", cx);
        })
        .expect("window");
        wait_until(cx, "the probe", |cx| cx.update(|cx| !log.read(cx).probes.is_empty()));
        cx.update(|cx| {
            let d = log.read(cx);
            let p = d.probes.back().expect("a probe");
            assert_eq!(p.endpoint, "virtual:12");
            assert_eq!(p.report.answered, 1, "{}", p.report.message);
            assert!(d.log.entries().len() > 0, "the packet log is fed");
        });

        // Orientation test through the virtual printer, then the ruler.
        for source in ["src-Orientation test", "src-Ruler test (60 mm)"] {
            cx.update_window(handle, move |_, window, cx| {
                window.render_frame(cx);
                window.click(source, cx);
                window.click("render-encode", cx);
            })
            .expect("window");
            wait_until(cx, "the virtual run", |cx| cx.update(|cx| !view.read(cx).running));
            cx.update(|cx| {
                let v = view.read(cx);
                assert_eq!(v.run_error, None);
                let run = v.run.as_ref().expect("a run");
                assert!(!run.bytes.is_empty());
                assert_eq!(run.decoded.len(), 1);
                assert!(run.violations.is_empty(), "{:?}", run.violations);
                assert!(!run.commands.is_empty());
            });
        }
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("save-bin").is_some());
            window.click("copy-diagnostics", cx);
        })
        .expect("window");
        let text = cx.read_from_clipboard().and_then(|c| c.text()).expect("report on the clipboard");
        assert!(text.starts_with("# baradar diagnostics"), "{text}");
        assert!(text.contains("## Probe: virtual:12"), "{text}");
        assert!(text.contains("## Packet log"), "{text}");

        // The dialog opens over the window.
        let s3 = studio.clone();
        cx.update_window(handle, move |_, window, cx| {
            open_diagnostics(s3, window, cx);
            window.render_frame(cx);
        })
        .expect("window");
    }
}
