//! The connect dialog: connection paths, Bluetooth scan and pair, progress, the current problem
//! with its actions, the connected printer card, and the printer settings view (port of
//! `connect/ConnectDialog.svelte`, `connect/copy.ts`, `common/ProblemBanner.svelte` and the
//! connect helpers of `state/view-model.ts`; the settings panel is in `connect_settings.rs`).
//!
//! Desktop adaptation: the browser choosers (Web Serial, WebUSB, Web Bluetooth) become
//! - the remembered printer (`prefs.last_endpoint`),
//! - "Paired printer" ([`ConnectTarget::Auto`]: the paired PT printer, a serial node or USB),
//! - Bluetooth scan → list → pair → connect (macOS),
//! - USB (`usb:`),
//! - a serial device path (or any endpoint string, e.g. `tcp:host`), with the candidate ports
//!   listed,
//! - "No printer": a virtual printer of the design model.

#[path = "connect_settings.rs"]
mod settings;

use std::rc::Rc;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Duration;

use baradar_printer::problems::{Problem, ProblemAction, Severity};
use baradar_printer::{ClientState, Command, ConnectTarget, Event, OpenedLink, PrinterInfo};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch::status::{Battery, PowerSource};
use ptouch_transport::{Endpoint, Transport, TransportError};

use crate::media::{
    EXAMPLE_DEVICE, EXAMPLE_MODEL, ModelPicker, is_verified_model, link_is_virtual, transport_kind_label,
};
use crate::studio::{Studio, StudioEvent, is_connected, width_byte};
use crate::view_model::format_mm;

// ------------------------------------------------------------------------------------------
// The open link (kept app-wide: Studio does not store it)
// ------------------------------------------------------------------------------------------

/// The link the current (or last) connection opened: transport kind and label. Studio keeps
/// only the endpoint, so this is tracked from [`Event::Opened`] by [`track_printer_link`].
#[derive(Default)]
pub struct PrinterLink {
    pub link: Option<OpenedLink>,
}

impl Global for PrinterLink {}

/// Keeps [`PrinterLink`] current. The media bar (which lives as long as the window) holds the
/// subscription.
pub fn track_printer_link(studio: &Entity<Studio>, cx: &mut App) -> Subscription {
    cx.subscribe(studio, |_, ev: &StudioEvent, cx| {
        let StudioEvent::Printer(e) = ev else { return };
        match e {
            Event::Opened(link) => cx.set_global(PrinterLink { link: Some(link.clone()) }),
            Event::State(ClientState::Idle) => cx.set_global(PrinterLink { link: None }),
            _ => {}
        }
    })
}

fn current_link(cx: &App) -> Option<OpenedLink> {
    cx.try_global::<PrinterLink>().and_then(|l| l.link.clone())
}

// ------------------------------------------------------------------------------------------
// Pure text helpers (state/view-model.ts, connect/copy.ts)
// ------------------------------------------------------------------------------------------

/// `openProgressText`: "Waking printer… this can take up to 10 s", "Opening the connection…",
/// "Waking printer… (attempt 2 of 3)".
pub fn open_progress_text(progress: Option<(u8, u8)>, state: ClientState) -> String {
    match progress {
        Some((attempt, of)) if attempt > 1 => format!("Waking printer… (attempt {attempt} of {of})"),
        _ if state == ClientState::Waking => "Waking printer… this can take up to 10 s".into(),
        _ => "Opening the connection…".into(),
    }
}

/// `batteryText`: "Battery low", "On AC power", "Battery 80 %".
pub fn battery_text(info: &PrinterInfo) -> Option<String> {
    if info.weak_battery() {
        return Some("Battery low".into());
    }
    match info.battery {
        Battery::Level { source: PowerSource::Ac, .. } | Battery::NoBattery { source: PowerSource::Ac } => {
            Some("On AC power".into())
        }
        Battery::Level { percent: Some(p), .. } => Some(format!("Battery {p} %")),
        _ => None,
    }
}

/// `tapeText`: "24 mm white / black" (3.5 mm tape reports 4).
pub fn tape_text(info: &PrinterInfo) -> Option<String> {
    let w = info
        .tape
        .map(|t| format_mm(f64::from(t.width_mm_x10) / 10.0))
        .or_else(|| info.tape_width_text())?;
    let (tape, ink) = (info.tape_color().name(), info.text_color().name());
    Some(if tape != "unknown" && !tape.is_empty() && !ink.is_empty() { format!("{w} {tape} / {ink}") } else { w })
}

/// A Bluetooth address as `AA:BB:CC:DD:EE:FF`.
fn addr_text(addr: &ptouch_transport::BtAddr) -> String {
    addr.0.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// How the remembered endpoint is shown: "Bluetooth printer AA:BB:…", the device path…
pub fn endpoint_text(e: &Endpoint) -> String {
    match e {
        Endpoint::Bluetooth { addr, .. } => format!("Bluetooth printer {}", addr_text(addr)),
        Endpoint::Serial { path } => path.clone(),
        Endpoint::Usb { .. } => "USB printer".into(),
        Endpoint::Tcp { host, port } => format!("Network printer {host}:{port}"),
        Endpoint::Virtual { width_mm } => format!("Virtual printer, {width_mm} mm tape"),
        other => other.to_string(),
    }
}

/// Parses the serial field: a device path or any endpoint string (`serial:`, `tcp:`, `bt:`…).
/// A bare name like `cu.PT-P710BTxxxx` is taken as a node under `/dev`.
pub fn parse_port_input(input: &str) -> Result<Endpoint, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Type the printer’s device path, e.g. /dev/cu.PT-P710BTxxxx.".into());
    }
    match t.parse::<Endpoint>() {
        Ok(e) => Ok(e),
        Err(_) if !t.contains(':') && (t.starts_with("cu.") || t.starts_with("tty.") || t.starts_with("rfcomm")) => {
            Ok(Endpoint::Serial { path: format!("/dev/{t}") })
        }
        Err(TransportError::InvalidEndpoint { reason, .. }) => Err(format!("That isn’t a device path or endpoint: {reason}.")),
        Err(e) => Err(e.to_string()),
    }
}

/// A connection path of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// `prefs.last_endpoint`.
    Remembered,
    /// The paired PT printer, found automatically.
    Auto,
    /// Scan, pair, connect (macOS).
    Bluetooth,
    Usb,
    Serial,
    Virtual,
}

/// The hint under the progress line (`progressHint`).
fn progress_hint(state: ClientState, progress: Option<(u8, u8)>, choice: Option<Choice>) -> &'static str {
    match state {
        ClientState::Handshaking => "Reading the loaded tape and printer status.",
        ClientState::Waking if progress.is_some_and(|(a, _)| a > 1) => {
            "The first Bluetooth connection after the printer was idle often fails; trying again automatically."
        }
        ClientState::Waking => {
            "The printer may be asleep. The first Bluetooth connection can take up to 10 seconds, then it is retried automatically."
        }
        _ => match choice {
            Some(Choice::Virtual) => "Starting the virtual printer.",
            Some(Choice::Bluetooth) => "Opening the Bluetooth connection.",
            Some(Choice::Usb) => "Opening the USB connection.",
            Some(Choice::Serial) => "Opening the serial port.",
            _ => "Opening the printer connection.",
        },
    }
}

fn option_hint(choice: Choice, remembered: Option<&str>, design_model: &str) -> String {
    let mac = cfg!(target_os = "macos");
    match choice {
        Choice::Remembered => format!("Reconnect to {}.", remembered.unwrap_or("the last printer")),
        Choice::Auto if mac => {
            "Finds the paired P-touch printer (Bluetooth, serial port or USB) and connects. Pair it in System Settings › Bluetooth first.".into()
        }
        Choice::Auto => {
            "Finds the paired or attached P-touch printer and connects. Pair it in your computer’s Bluetooth settings first.".into()
        }
        Choice::Bluetooth => format!(
            "Scan for nearby printers, pair a new one here, then connect (e.g. “{EXAMPLE_DEVICE}”)."
        ),
        Choice::Usb if cfg!(target_os = "linux") => {
            "Most reliable: answers instantly, no pairing or waking. Linux’ usblp driver must not hold the printer.".into()
        }
        Choice::Usb => "Most reliable: answers instantly, no pairing or waking. Close P-touch Editor first.".into(),
        Choice::Serial if mac => format!(
            "Pick the printer’s “cu.” port (e.g. “cu.{EXAMPLE_DEVICE}”) or type a device path."
        ),
        Choice::Serial => "Pick the printer’s serial port (e.g. /dev/rfcomm0 or COM5) or type its path.".into(),
        Choice::Virtual => format!("Design and test with a virtual {design_model}."),
    }
}

/// The icon beside a connection path (`ConnectDialog.svelte`).
fn option_icon(choice: Choice) -> &'static str {
    match choice {
        Choice::Remembered => "history",
        Choice::Auto => "printer",
        Choice::Bluetooth => "bluetooth",
        Choice::Usb => "usb",
        Choice::Serial => "cable",
        Choice::Virtual => "monitor",
    }
}

fn option_title(choice: Choice) -> &'static str {
    match choice {
        Choice::Remembered => "Last printer",
        Choice::Auto => "Paired printer",
        Choice::Bluetooth => "Bluetooth: find and pair",
        Choice::Usb => "USB cable",
        Choice::Serial => "Serial port…",
        Choice::Virtual => "No printer",
    }
}

// ------------------------------------------------------------------------------------------
// Studio actions used by the dialog
// ------------------------------------------------------------------------------------------

/// What a problem action needs from the view (the rest is done by Studio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAction {
    /// Show the serial port list.
    ChoosePort,
    /// Show the Bluetooth scan.
    Bluetooth,
    /// Open Diagnostics.
    Diagnostics,
}

impl Studio {
    /// Sends a command to the printer client (a stopped client is ignored, like Studio does).
    pub fn send_printer(&self, command: Command) {
        let _ = self.printer.send(command);
    }

    /// "No printer": a virtual printer of the design model with the label's tape loaded (the
    /// nearest width the model takes otherwise). Not remembered for auto-reconnect.
    pub fn connect_virtual_printer(&mut self) {
        let profile = ptouch::profile_by_name(&self.prefs.design_model).unwrap_or(self.model());
        let widths = crate::studio::tape_widths_of(profile);
        let w = crate::media::nearest_tape_width(self.doc.tape.width_mm, &widths);
        let width = width_byte(w);
        let open: baradar_printer::TransportFactory = Arc::new(move || {
            ptouch_transport::virtual_transport::VirtualTransport::new(profile, width, ptouch::Behaviour::Normal)
                .map(|t| Box::new(t) as Box<dyn Transport>)
        });
        self.connect_to(ConnectTarget::Custom {
            label: format!("virtual {}", profile.name),
            open,
        });
    }

    /// "Forget printer": disconnects and drops the remembered endpoint.
    pub fn forget_printer(&mut self, cx: &mut Context<Self>) {
        self.prefs.last_endpoint = None;
        self.disconnect(cx);
    }

    /// `handleProblemAction`. Returns what the view has to do itself, if anything.
    pub fn handle_problem_action(&mut self, action: ProblemAction, has_link: bool, cx: &mut Context<Self>) -> Option<ViewAction> {
        match action {
            ProblemAction::Retry => {
                if has_link {
                    self.reconnect();
                } else {
                    self.connect();
                }
            }
            ProblemAction::Reconnect => self.reconnect(),
            ProblemAction::ChoosePort => return Some(ViewAction::ChoosePort),
            ProblemAction::ConnectBluetooth => return Some(ViewAction::Bluetooth),
            ProblemAction::ConnectUsb => {
                if let Ok(e) = "usb:".parse::<Endpoint>() {
                    self.connect_to(ConnectTarget::Endpoint(e));
                }
            }
            ProblemAction::SwitchTape => {
                self.use_loaded_tape(cx);
                self.dismiss_problem(cx);
            }
            ProblemAction::OpenDiagnostics => return Some(ViewAction::Diagnostics),
            ProblemAction::Dismiss => self.dismiss_problem(cx),
        }
        None
    }
}

/// Runs a problem action from a banner outside the connect dialog (the centre column).
#[allow(dead_code)] // For app.rs's centre-column problem banner.
pub fn run_problem_action(studio: Entity<Studio>, action: ProblemAction, window: &mut Window, cx: &mut App) {
    let has_link = current_link(cx).is_some();
    let view = studio.update(cx, |s, cx| s.handle_problem_action(action, has_link, cx));
    match view {
        Some(ViewAction::ChoosePort) => open_with(studio, false, Some(Choice::Serial), window, cx),
        Some(ViewAction::Bluetooth) => open_with(studio, false, Some(bluetooth_choice()), window, cx),
        Some(ViewAction::Diagnostics) => crate::diagnostics::open_diagnostics(studio, window, cx),
        None => {}
    }
}

/// The Bluetooth path: scan and pair on macOS, the automatic paired-printer path elsewhere.
fn bluetooth_choice() -> Choice {
    if cfg!(target_os = "macos") { Choice::Bluetooth } else { Choice::Auto }
}

// ------------------------------------------------------------------------------------------
// Problem banner (common/ProblemBanner.svelte)
// ------------------------------------------------------------------------------------------

type ActionHandler = Rc<dyn Fn(ProblemAction, &mut Window, &mut App)>;

/// A problem with its action buttons (primary: retry, reconnect, switch-tape), the command
/// line it refers to, and a "Show/Hide details" toggle for the technical text.
/// Toggles the technical details of a problem banner.
type ToggleHandler = Rc<dyn Fn(&mut App)>;

fn problem_banner_el(
    problem: &Problem,
    id: &str,
    show_technical: bool,
    on_toggle: Option<ToggleHandler>,
    on_action: ActionHandler,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let color = match problem.severity {
        Severity::Error => theme.danger,
        Severity::Warning => theme.warning,
        Severity::Info => theme.info,
    };
    let actions = problem.actions.iter().copied().enumerate().map(|(i, a)| {
        let on_action = on_action.clone();
        Button::new(SharedString::from(format!("{id}-action-{i}")))
            .label(a.label())
            .small()
            .map(|b| if a.is_primary() { b.primary() } else if a == ProblemAction::Dismiss { b.ghost() } else { b.outline() })
            .on_click(move |_, window, cx| on_action(a, window, cx))
    });
    let technical = problem.technical.clone();
    v_flex()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(color)
        .bg(color.opacity(0.06))
        .child(
            h_flex()
                .gap_2()
                .child(div().text_color(color).child(crate::icons::icon(if problem.severity == Severity::Info { "info" } else { "alert" })))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(problem.title.clone())),
        )
        .child(div().text_sm().child(problem.detail.clone()))
        .when_some(problem.command.clone(), |d, c| {
            d.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(theme.muted)
                    .font_family("Menlo")
                    .text_xs()
                    .child(c),
            )
        })
        .child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .pt_1()
                .children(actions)
                .when_some(technical.as_ref().zip(on_toggle), |d, (_, toggle)| {
                    d.child(
                        Button::new(SharedString::from(format!("{id}-details")))
                            .label(if show_technical { "Hide details" } else { "Show details" })
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| toggle(cx)),
                    )
                }),
        )
        .when(show_technical, |d| {
            d.when_some(technical, |d, t| {
                d.child(div().text_xs().font_family("Menlo").text_color(theme.muted_foreground).child(t))
            })
        })
        .into_any_element()
}

/// The problem banner for the centre column, with its actions wired up (Try again,
/// Reconnect, Choose port…, Bluetooth, USB cable, Use loaded tape, Diagnostics, Dismiss).
#[allow(dead_code)] // For app.rs's centre-column problem banner.
pub fn problem_banner(studio: &Entity<Studio>, problem: &Problem, cx: &App) -> AnyElement {
    let studio = studio.clone();
    let on_action: ActionHandler = Rc::new(move |a, window, cx| run_problem_action(studio.clone(), a, window, cx));
    problem_banner_el(problem, "problem", false, None, on_action, cx)
}

// ------------------------------------------------------------------------------------------
// The dialog
// ------------------------------------------------------------------------------------------

/// A Bluetooth device found by a scan.
#[derive(Debug, Clone)]
struct Nearby {
    addr: ptouch_transport::BtAddr,
    name: String,
    paired: bool,
    is_printer: bool,
}

#[derive(Debug, Clone)]
enum Scan {
    Idle,
    Scanning,
    Done(Vec<Nearby>),
    Failed(String),
}

#[derive(Debug, Clone)]
struct Pairing {
    addr: ptouch_transport::BtAddr,
    name: String,
    message: Option<String>,
}

/// How long a scan runs.
#[cfg(target_os = "macos")]
const SCAN_SECONDS: u64 = 8;

pub struct ConnectDialog {
    studio: Entity<Studio>,
    settings_view: bool,
    settings: Option<(&'static str, Entity<settings::SettingsPanel>)>,
    expanded: Option<Choice>,
    last_choice: Option<Choice>,
    /// A connect started from this dialog: close it shortly after it succeeds.
    attempted: bool,
    serial_input: Entity<InputState>,
    serial_ports: Option<Vec<String>>,
    serial_error: Option<String>,
    scan: Scan,
    show_all_devices: bool,
    pairing: Option<Pairing>,
    pair_error: Option<String>,
    show_help: bool,
    show_technical: bool,
    picker: Entity<ModelPicker>,
    ports_task: Option<Task<()>>,
    close_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

/// Opens the connect dialog.
pub fn open_connect(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    open_with(studio, false, None, window, cx);
}

/// Opens the connect dialog on its "Printer settings" view (the connect view when the
/// printer has no settings or is not connected).
pub fn open_printer_settings(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    open_with(studio, true, None, window, cx);
}

fn open_with(studio: Entity<Studio>, settings: bool, expand: Option<Choice>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ConnectDialog::new(studio, settings, expand, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let v = view.read(cx);
        let (title, subtitle) = v.titles(cx);
        let back = v.settings_view.then(|| {
            let view = view.clone();
            Button::new("settings-back")
                .icon(IconName::ChevronLeft)
                .ghost()
                .small()
                .tooltip("Back")
                .on_click(move |_, _, cx| {
                    view.update(cx, |v, cx| {
                        v.settings_view = false;
                        cx.notify();
                    })
                })
        });
        let muted = cx.theme().muted_foreground;
        dialog
            .w(px(600.))
            .title(
                h_flex()
                    .gap_2()
                    .items_start()
                    .children(back)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(title)
                            .child(div().text_sm().font_weight(FontWeight::NORMAL).text_color(muted).child(subtitle)),
                    ),
            )
            .child(view.clone())
    });
}

impl ConnectDialog {
    fn new(studio: Entity<Studio>, settings: bool, expand: Option<Choice>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let serial_input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("/dev/cu.{EXAMPLE_DEVICE}")));
        let picker = cx.new(|cx| ModelPicker::new(studio.clone(), window, cx));
        let subs = vec![
            cx.observe(&studio, |_, _, cx| cx.notify()),
            cx.subscribe_in(&studio, window, |this, _, ev: &StudioEvent, window, cx| {
                if let StudioEvent::Printer(e) = ev {
                    this.on_printer_event(e, window, cx);
                }
            }),
            cx.subscribe_in(&serial_input, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } => this.connect_serial_input(cx),
                InputEvent::Change
                    if this.serial_error.take().is_some() => {
                        cx.notify();
                    }
                _ => {}
            }),
        ];
        let mut this = Self {
            studio,
            settings_view: false,
            settings: None,
            expanded: None,
            last_choice: None,
            attempted: false,
            serial_input,
            serial_ports: None,
            serial_error: None,
            scan: Scan::Idle,
            show_all_devices: false,
            pairing: None,
            pair_error: None,
            show_help: false,
            show_technical: false,
            picker,
            ports_task: None,
            close_task: None,
            _subs: subs,
        };
        if settings && this.has_settings(cx) {
            this.settings_view = true;
        }
        if let Some(choice) = expand {
            this.expand(choice, cx);
        }
        this
    }

    /// Title and subtitle for the current view.
    fn titles(&self, cx: &App) -> (String, String) {
        let s = self.studio.read(cx);
        let connected = is_connected(s.conn);
        if self.settings_view {
            let model = s.printer_model().map_or("Printer", |m| m.name);
            return ("Printer settings".into(), format!("{model} · stored in the printer"));
        }
        if connected {
            let sub = current_link(cx).map_or_else(String::new, |l| {
                let kind = transport_kind_label(l.transport.kind);
                if l.transport.label.is_empty() { kind.to_owned() } else { format!("{kind} · {}", l.transport.label) }
            });
            return ("Printer connected".into(), sub);
        }
        (
            "Connect your printer".into(),
            format!(
                "Your Brother P-touch label printer (e.g. {EXAMPLE_MODEL}) over Bluetooth, USB or a serial port. Nothing leaves this computer."
            ),
        )
    }

    /// The connected printer has settings this app can change.
    fn has_settings(&self, cx: &App) -> bool {
        let s = self.studio.read(cx);
        is_connected(s.conn) && s.printer_model().is_some_and(|m| !ptouch::settings_for(m).is_empty())
    }

    fn on_printer_event(&mut self, ev: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            Event::Connected(_) if self.attempted => {
                self.attempted = false;
                // Close shortly after a connection attempt started here succeeds.
                self.close_task = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(900)).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if is_connected(this.studio.read(cx).conn) && !this.settings_view {
                            window.close_dialog(cx);
                        }
                    });
                }));
            }
            Event::ConnectFailed(_) => self.attempted = false,
            #[cfg(target_os = "macos")]
            Event::ScanFinished(result) => {
                self.scan = match result {
                    Ok(devices) => Scan::Done(
                        devices
                            .iter()
                            .map(|d| Nearby {
                                addr: d.addr,
                                name: d.name.clone(),
                                paired: d.paired,
                                is_printer: d.is_printer,
                            })
                            .collect(),
                    ),
                    Err(e) => Scan::Failed(e.to_string()),
                };
            }
            #[cfg(target_os = "macos")]
            Event::Pairing(pe) => {
                use baradar_printer::bluetooth::PairEvent;
                if let Some(p) = &mut self.pairing {
                    p.message = match pe {
                        PairEvent::PinRequested => Some("The printer asked for a PIN; 0000 was sent.".into()),
                        PairEvent::ConfirmNumber(n) => Some(format!("Check that the printer shows {n:06}.")),
                        PairEvent::Passkey(n) => Some(format!("Type {n:06} on the printer.")),
                        PairEvent::SimplePairingComplete(_) => Some("Finishing pairing…".into()),
                        PairEvent::Finished(_) => p.message.take(),
                    };
                }
            }
            #[cfg(target_os = "macos")]
            Event::PairFinished(result) => {
                if let Some(p) = self.pairing.take() {
                    match result {
                        Ok(()) => {
                            if let Scan::Done(list) = &mut self.scan
                                && let Some(d) = list.iter_mut().find(|d| d.addr == p.addr)
                            {
                                d.paired = true;
                            }
                            self.connect_endpoint(Endpoint::Bluetooth { addr: p.addr, channel: None }, Choice::Bluetooth, cx);
                        }
                        Err(e) => self.pair_error = Some(format!("Couldn’t pair with {}: {e}", p.name)),
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn expand(&mut self, choice: Choice, cx: &mut Context<Self>) {
        self.expanded = if self.expanded == Some(choice) { None } else { Some(choice) };
        if self.expanded == Some(Choice::Serial) && self.serial_ports.is_none() && self.ports_task.is_none() {
            self.list_ports(cx);
        }
        cx.notify();
    }

    /// Lists candidate serial ports off the UI thread (IOKit / `/dev`; no Bluetooth).
    fn list_ports(&mut self, cx: &mut Context<Self>) {
        self.ports_task = Some(cx.spawn(async move |this, cx| {
            let ports = cx
                .background_executor()
                .spawn(async move { ptouch_transport::serial::SerialTransport::list_candidates().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.serial_ports = Some(ports);
                this.ports_task = None;
                cx.notify();
            });
        }));
    }

    fn choose(&mut self, choice: Choice, cx: &mut Context<Self>) {
        match choice {
            Choice::Bluetooth | Choice::Serial => self.expand(choice, cx),
            Choice::Remembered => {
                self.begin(choice);
                self.studio.update(cx, |s, _| s.connect());
            }
            Choice::Auto => {
                self.begin(choice);
                self.studio.update(cx, |s, _| s.connect_to(ConnectTarget::Auto));
            }
            Choice::Usb => match "usb:".parse::<Endpoint>() {
                Ok(e) => self.connect_endpoint(e, choice, cx),
                Err(e) => self.serial_error = Some(e.to_string()),
            },
            Choice::Virtual => {
                self.begin(choice);
                self.studio.update(cx, |s, _| s.connect_virtual_printer());
            }
        }
        cx.notify();
    }

    fn begin(&mut self, choice: Choice) {
        self.attempted = true;
        self.last_choice = Some(choice);
        self.pair_error = None;
    }

    fn connect_endpoint(&mut self, endpoint: Endpoint, choice: Choice, cx: &mut Context<Self>) {
        self.begin(choice);
        self.studio.update(cx, |s, _| s.connect_to(ConnectTarget::Endpoint(endpoint)));
        cx.notify();
    }

    fn connect_serial_input(&mut self, cx: &mut Context<Self>) {
        let text = self.serial_input.read(cx).value().to_string();
        match parse_port_input(&text) {
            Ok(e) => {
                self.serial_error = None;
                self.connect_endpoint(e, Choice::Serial, cx);
            }
            Err(msg) => {
                self.serial_error = Some(msg);
                cx.notify();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn start_scan(&mut self, cx: &mut Context<Self>) {
        self.scan = Scan::Scanning;
        self.pair_error = None;
        self.studio.read(cx).send_printer(Command::Scan(Duration::from_secs(SCAN_SECONDS)));
        cx.notify();
    }

    #[cfg(not(target_os = "macos"))]
    fn start_scan(&mut self, cx: &mut Context<Self>) {
        self.scan = Scan::Failed("Scanning is only available on macOS.".into());
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn pair(&mut self, device: &Nearby, cx: &mut Context<Self>) {
        self.pair_error = None;
        self.pairing = Some(Pairing { addr: device.addr, name: device.name.clone(), message: None });
        self.studio.read(cx).send_printer(Command::Pair(device.addr));
        cx.notify();
    }

    #[cfg(not(target_os = "macos"))]
    fn pair(&mut self, _device: &Nearby, cx: &mut Context<Self>) {
        self.pair_error = Some("Pair the printer in your computer’s Bluetooth settings.".into());
        cx.notify();
    }

    fn problem_action(&mut self, action: ProblemAction, window: &mut Window, cx: &mut Context<Self>) {
        let has_link = current_link(cx).is_some();
        let view = self.studio.update(cx, |s, cx| s.handle_problem_action(action, has_link, cx));
        match view {
            Some(ViewAction::ChoosePort) => {
                self.expanded = None;
                self.expand(Choice::Serial, cx);
            }
            Some(ViewAction::Bluetooth) => {
                self.expanded = None;
                self.expand(bluetooth_choice(), cx);
            }
            Some(ViewAction::Diagnostics) => {
                window.close_dialog(cx);
                crate::diagnostics::open_diagnostics(self.studio.clone(), window, cx);
            }
            None => {
                if matches!(action, ProblemAction::Retry | ProblemAction::Reconnect | ProblemAction::ConnectUsb) {
                    self.attempted = true;
                }
            }
        }
        cx.notify();
    }

    // --------------------------------------------------------------------------------------
    // Rendering
    // --------------------------------------------------------------------------------------

    fn render_problem(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let problem = self.studio.read(cx).problem.clone()?;
        let weak = cx.entity().downgrade();
        let on_action: ActionHandler = Rc::new({
            let weak = weak.clone();
            move |a, window, cx| {
                let _ = weak.update(cx, |this, cx| this.problem_action(a, window, cx));
            }
        });
        let toggle: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
            let _ = weak.update(cx, |this, cx| {
                this.show_technical = !this.show_technical;
                cx.notify();
            });
        });
        Some(problem_banner_el(&problem, "connect-problem", self.show_technical, Some(toggle), on_action, cx))
    }

    fn render_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let text = if s.conn == ClientState::Handshaking {
            "Talking to the printer…".to_owned()
        } else {
            open_progress_text(s.open_progress, s.conn)
        };
        let hint = progress_hint(s.conn, s.open_progress, self.last_choice);
        h_flex()
            .gap_3()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(Spinner::new())
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(text))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(hint)),
            )
            .child(
                Button::new("cancel-connect")
                    .label("Cancel")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.attempted = false;
                        this.studio.update(cx, |s, _| s.cancel());
                    })),
            )
            .into_any_element()
    }

    fn render_printer_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let is_virtual = link_is_virtual(cx);
        let has_settings = self.has_settings(cx);
        let s = self.studio.read(cx);
        let ready = s.conn == ClientState::Connected;
        let printing = s.conn == ClientState::Printing;
        let model = s.printer_model().map(|m| m.name);
        let info = s.printer_info.clone();
        let tape = info.as_ref().and_then(tape_text).unwrap_or_else(|| "No tape detected".into());
        let power = info.as_ref().and_then(battery_text);
        let needs_attention = info.as_ref().is_some_and(|i| !i.errors().is_empty());
        let (status, status_color) = if printing {
            ("Printing", theme.foreground)
        } else if needs_attention {
            ("Needs attention", theme.warning)
        } else {
            ("Ready", theme.success)
        };
        let busy_printing = printing || s.printing;
        let row = |k: &'static str, v: String, color: Option<Hsla>| {
            h_flex()
                .gap_2()
                .child(div().w(px(64.)).text_sm().text_color(theme.muted_foreground).child(k))
                .child(div().text_sm().when_some(color, |d, c| d.text_color(c)).child(v))
        };
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(model.unwrap_or("Printer")))
                            .when(model.is_some_and(|m| !is_virtual && !is_verified_model(m)), |d| {
                                d.child(Tag::warning().small().child("Untested model"))
                            }),
                    )
                    .child(row("Tape", tape, None))
                    .when_some(power, |d, p| d.child(row("Power", p, None)))
                    .child(row("Status", status.into(), Some(status_color))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("refresh-status")
                            .icon(IconName::RefreshCw)
                            .label("Refresh status")
                            .small()
                            .outline()
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, _, cx| this.studio.read(cx).send_printer(Command::RefreshStatus))),
                    )
                    .child(
                        Button::new("disconnect")
                            .label("Disconnect")
                            .small()
                            .outline()
                            .disabled(printing)
                            .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.disconnect(cx)))),
                    )
                    .when(has_settings, |d| {
                        d.child(
                            Button::new("printer-settings")
                                .icon(IconName::Settings)
                                .label("Printer settings…")
                                .small()
                                .outline()
                                .disabled(!ready || busy_printing)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings_view = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!is_virtual, |d| {
                        d.child(
                            Button::new("forget-printer")
                                .label("Forget printer")
                                .small()
                                .ghost()
                                .disabled(printing)
                                .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.forget_printer(cx)))),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The connection paths, in recommended order.
    fn choices(&self, cx: &App) -> Vec<(Choice, bool)> {
        let remembered = self.studio.read(cx).prefs.last_endpoint.as_deref().and_then(|e| e.parse::<Endpoint>().ok());
        let mut v = Vec::new();
        if remembered.is_some() {
            v.push((Choice::Remembered, true));
        }
        v.push((Choice::Auto, remembered.is_none()));
        if cfg!(target_os = "macos") {
            v.push((Choice::Bluetooth, false));
        }
        v.extend([(Choice::Usb, false), (Choice::Serial, false), (Choice::Virtual, false)]);
        v
    }

    fn render_options(&self, connecting: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (remembered, design_model) = {
            let s = self.studio.read(cx);
            let r = s.prefs.last_endpoint.as_deref().and_then(|e| e.parse::<Endpoint>().ok()).map(|e| endpoint_text(&e));
            (r, s.prefs.design_model.clone())
        };
        let mut col = v_flex().gap_2();
        for (choice, recommended) in self.choices(cx) {
            let expanded = self.expanded == Some(choice);
            let expandable = matches!(choice, Choice::Bluetooth | Choice::Serial);
            let row = h_flex()
                .id(SharedString::from(format!("connect-option-{choice:?}")))
                .role(Role::Button)
                .aria_label(option_title(choice))
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(if recommended { theme.primary } else { theme.border })
                .when(!connecting, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(theme.accent))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(choice, cx)))
                })
                .when(connecting, |d| d.opacity(0.5))
                .child(div().text_color(theme.muted_foreground).child(crate::icons::icon(option_icon(choice))))
                .child(
                    v_flex()
                        .flex_1()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(option_title(choice)))
                                .when(recommended, |d| d.child(Tag::primary().small().child("Recommended"))),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(option_hint(choice, remembered.as_deref(), &design_model)),
                        ),
                )
                .child(
                    gpui_kit::component::Icon::new(if expandable && expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                        .small()
                        .text_color(theme.muted_foreground),
                );
            col = col.child(row.test_support());
            if expanded {
                let panel = match choice {
                    Choice::Bluetooth => self.render_bluetooth(connecting, cx),
                    Choice::Serial => self.render_serial(connecting, cx),
                    _ => continue,
                };
                col = col.child(div().pl_4().child(panel));
            }
        }
        col.into_any_element()
    }

    fn render_bluetooth(&self, connecting: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let scanning = matches!(self.scan, Scan::Scanning);
        let pairing = self.pairing.clone();
        let mut col = v_flex().gap_2().child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("bt-scan")
                        .icon(IconName::Search)
                        .label(if matches!(self.scan, Scan::Done(_) | Scan::Failed(_)) { "Scan again" } else { "Scan for printers" })
                        .small()
                        .primary()
                        .loading(scanning)
                        .disabled(scanning || connecting || pairing.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))),
                )
                .when(scanning, |d| {
                    d.child(div().text_sm().text_color(theme.muted_foreground).child(
                        "Searching… Make sure the printer is on and its Bluetooth light is lit.",
                    ))
                }),
        );
        match &self.scan {
            Scan::Idle | Scan::Scanning => {}
            Scan::Failed(e) => {
                col = col.child(div().text_sm().text_color(theme.danger).child(format!(
                    "The scan failed: {e}. Make sure Bluetooth is on and that Baradar may use it (System Settings › Privacy & Security › Bluetooth)."
                )));
            }
            Scan::Done(devices) => {
                let mut list: Vec<Nearby> =
                    devices.iter().filter(|d| d.is_printer || self.show_all_devices).cloned().collect();
                list.sort_by_key(|d| (!d.is_printer, !d.paired, d.name.to_lowercase()));
                let hidden = devices.len() - devices.iter().filter(|d| d.is_printer).count();
                if list.is_empty() {
                    col = col.child(div().text_sm().text_color(theme.muted_foreground).child(
                        "No printers found. Turn the printer on and press its Bluetooth button, then scan again. If nothing shows up at all, allow Baradar to use Bluetooth in System Settings › Privacy & Security › Bluetooth.",
                    ));
                }
                for (i, d) in list.into_iter().enumerate() {
                    let is_pairing = pairing.as_ref().is_some_and(|p| p.addr == d.addr);
                    let device = d.clone();
                    let action = if d.paired {
                        Button::new(SharedString::from(format!("bt-connect-{i}")))
                            .label("Connect")
                            .small()
                            .primary()
                            .disabled(connecting || pairing.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.connect_endpoint(Endpoint::Bluetooth { addr: device.addr, channel: None }, Choice::Bluetooth, cx)
                            }))
                    } else {
                        Button::new(SharedString::from(format!("bt-pair-{i}")))
                            .label("Pair")
                            .small()
                            .outline()
                            .loading(is_pairing)
                            .disabled(connecting || pairing.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.pair(&device, cx)))
                    };
                    col = col.child(
                        h_flex()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(d.name.clone()))
                                    .child(div().text_xs().text_color(theme.muted_foreground).child(addr_text(&d.addr))),
                            )
                            .when(d.paired, |r| r.child(Tag::secondary().small().child("Paired")))
                            .child(action),
                    );
                }
                if hidden > 0 {
                    col = col.child(
                        Button::new("bt-show-all")
                            .label(if self.show_all_devices {
                                "Show printers only".to_owned()
                            } else {
                                format!("Show all devices ({hidden} more)")
                            })
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_all_devices = !this.show_all_devices;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        if let Some(p) = &pairing {
            col = col.child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().child(format!("Pairing with {}… Keep the printer on and close by.", p.name)))
                    .when_some(p.message.clone(), |d, m| {
                        d.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(m))
                    }),
            );
        }
        if let Some(e) = &self.pair_error {
            col = col.child(div().text_sm().text_color(theme.danger).child(e.clone()));
        }
        col.into_any_element()
    }

    fn render_serial(&self, connecting: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut col = v_flex().gap_2();
        match &self.serial_ports {
            None => col = col.child(div().text_sm().text_color(theme.muted_foreground).child("Looking for printer ports…")),
            Some(ports) if ports.is_empty() => {
                col = col.child(div().text_sm().text_color(theme.muted_foreground).child(
                    "No printer ports found. Pair the printer first, or type the port’s path below.",
                ));
            }
            Some(ports) => {
                for (i, port) in ports.iter().enumerate() {
                    let path = port.clone();
                    col = col.child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().text_sm().font_family("Menlo").child(port.clone()))
                            .child(
                                Button::new(SharedString::from(format!("serial-port-{i}")))
                                    .label("Connect")
                                    .small()
                                    .outline()
                                    .disabled(connecting)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.connect_endpoint(Endpoint::Serial { path: path.clone() }, Choice::Serial, cx)
                                    })),
                            ),
                    );
                }
            }
        }
        col = col
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.serial_input).small()))
                    .child(
                        Button::new("serial-connect")
                            .label("Connect")
                            .small()
                            .primary()
                            .disabled(connecting)
                            .on_click(cx.listener(|this, _, _, cx| this.connect_serial_input(cx))),
                    )
                    .child(
                        Button::new("serial-refresh")
                            .icon(IconName::RefreshCw)
                            .small()
                            .ghost()
                            .tooltip("List ports again")
                            .loading(self.ports_task.is_some())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.serial_ports = None;
                                this.list_ports(cx);
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.serial_error.clone(), |d, e| d.child(div().text_sm().text_color(theme.danger).child(e)));
        col.into_any_element()
    }

    fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mac = cfg!(target_os = "macos");
        let steps: Vec<String> = vec![
            "Turn the printer on and press its Bluetooth button until the Bluetooth light is on.".into(),
            if mac {
                format!("Pair your printer (e.g. {EXAMPLE_DEVICE}) in System Settings › Bluetooth (no PIN, or 0000), or use “Bluetooth: find and pair” here.")
            } else {
                format!("Pair your printer (e.g. {EXAMPLE_DEVICE}) in your computer’s Bluetooth settings (no PIN, or 0000).")
            },
            "Come back here and choose “Paired printer”. The first attempt can take a few seconds while the printer wakes up.".into(),
            if mac {
                format!("Still nothing? Use “Serial port…” and pick its “cu.” port (e.g. cu.{EXAMPLE_DEVICE}).")
            } else {
                "Still nothing? Use “Serial port…” and pick the printer’s serial port.".into()
            },
        ];
        v_flex()
            .gap_1()
            .child(
                Button::new("pair-help")
                    .label("First time? How to pair the printer")
                    .icon(if self.show_help { IconName::ChevronDown } else { IconName::ChevronRight })
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_help = !this.show_help;
                        cx.notify();
                    })),
            )
            .when(self.show_help, |d| {
                d.children(steps.into_iter().enumerate().map(|(i, t)| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .pl_3()
                        .child(div().text_sm().text_color(theme.muted_foreground).child(format!("{}.", i + 1)))
                        .child(div().text_sm().child(t))
                }))
            })
            .into_any_element()
    }

    fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.studio.read(cx).printer_model();
        let Some(model) = model else {
            return div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("The printer isn’t ready.")
                .into_any_element();
        };
        let panel = match &self.settings {
            Some((name, panel)) if *name == model.name => panel.clone(),
            _ => {
                let studio = self.studio.clone();
                let panel = cx.new(|cx| settings::SettingsPanel::new(studio, model, window, cx));
                self.settings = Some((model.name, panel.clone()));
                panel
            }
        };
        panel.into_any_element()
    }
}

impl Render for ConnectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings_view && !self.has_settings(cx) {
            self.settings_view = false;
        }
        let body = if self.settings_view {
            self.render_settings(window, cx)
        } else {
            let (connecting, connected) = {
                let s = self.studio.read(cx);
                (s.conn.is_connecting(), is_connected(s.conn))
            };
            let problem = self.render_problem(cx);
            v_flex()
                .gap_3()
                .children(problem)
                .when(connecting, |d| d.child(self.render_progress(cx)))
                .when(connected && !connecting, |d| d.child(self.render_printer_card(cx)))
                .when(!connected, |d| {
                    d.child(self.render_options(connecting, cx))
                        .child(self.picker.clone())
                        .child(self.render_help(cx))
                })
                .into_any_element()
        };
        div()
            .id("connect-dialog-body")
            .max_h(px(560.))
            .overflow_y_scroll()
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{endpoint_text, open_progress_text, parse_port_input};
    use baradar_printer::ClientState;
    use ptouch_transport::Endpoint;

    #[test]
    fn progress_text() {
        assert_eq!(open_progress_text(None, ClientState::Waking), "Waking printer… this can take up to 10 s");
        assert_eq!(open_progress_text(Some((1, 3)), ClientState::Opening), "Opening the connection…");
        assert_eq!(open_progress_text(Some((2, 3)), ClientState::Waking), "Waking printer… (attempt 2 of 3)");
    }

    #[test]
    fn port_input() {
        assert_eq!(
            parse_port_input("/dev/cu.PT-P710BTxxxx"),
            Ok(Endpoint::Serial { path: "/dev/cu.PT-P710BTxxxx".into() })
        );
        assert_eq!(
            parse_port_input(" cu.PT-P710BTxxxx "),
            Ok(Endpoint::Serial { path: "/dev/cu.PT-P710BTxxxx".into() })
        );
        assert!(matches!(parse_port_input("tcp:192.168.1.20"), Ok(Endpoint::Tcp { .. })));
        assert!(parse_port_input("").is_err());
        assert!(parse_port_input("nonsense:thing").is_err());
        assert_eq!(endpoint_text(&Endpoint::Virtual { width_mm: 12 }), "Virtual printer, 12 mm tape");
    }
}

/// Headless UI tests against the in-process virtual printer (never Bluetooth).
#[cfg(test)]
pub(crate) mod ui_tests {
    // NOT `use super::*`: with test-support, `gpui_kit::*` exports GPUI's `test` macro, which
    // shadows Rust's #[test] and makes #[gpui_kit::test] recurse forever.
    use super::{PrinterLink, open_connect};
    use crate::media::{MediaBar, link_is_virtual};
    use crate::studio::{Studio, is_connected};
    use baradar_core::doc::LengthSettings;
    use baradar_core::store::Store;
    use baradar_printer::ClientState;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Context, Entity, Render, TestAppContext, Window, WindowBounds,
        WindowOptions, point, px, size,
    };
    use std::time::Duration;

    pub(crate) fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("baradar-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(dir).expect("temp store")
    }

    /// Runs the app until `f` holds (the printer client works on real threads).
    pub(crate) fn wait_until(cx: &mut TestAppContext, what: &str, mut f: impl FnMut(&mut TestAppContext) -> bool) {
        for _ in 0..600 {
            cx.run_until_parked();
            if f(cx) {
                return;
            }
            cx.executor().advance_clock(Duration::from_millis(20));
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    pub(crate) fn open_test_window<V: Render + 'static>(
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    ) -> (AnyWindowHandle, Entity<V>) {
        cx.update(|cx| {
            let (w, v) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(1400.))))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| build(window, cx)),
            )
            .expect("open test window");
            (w, v)
        })
    }

    pub(crate) fn new_studio(cx: &mut TestAppContext, name: &str) -> Entity<Studio> {
        cx.update(gpui_kit::init);
        // The printer client runs on its own threads and wakes UI tasks from there.
        cx.executor().allow_parking();
        let store = temp_store(name);
        cx.update(|cx| cx.new(|cx| Studio::new(store, cx)))
    }

    #[gpui_kit::test]
    async fn media_bar_and_connect_dialog_with_virtual_printer(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "connect");
        let s2 = studio.clone();
        let (handle, _bar) = open_test_window(cx, move |window, cx| MediaBar::new(s2, window, cx));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("length-fixed", cx);
        })
        .expect("window");
        cx.run_until_parked();
        cx.update(|cx| assert!(matches!(studio.read(cx).doc.length, LengthSettings::Fixed { .. })));

        // Open the dialog and pick "No printer".
        let s3 = studio.clone();
        cx.update_window(handle, move |_, window, cx| open_connect(s3, window, cx)).expect("window");
        cx.run_until_parked();
        cx.update_window(handle, move |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("connect-option-Virtual").is_some(), "options are listed");
            window.click("connect-option-Virtual", cx);
        })
        .expect("window");
        wait_until(cx, "the virtual printer to connect", |cx| cx.update(|cx| is_connected(studio.read(cx).conn)));
        cx.update(|cx| {
            assert!(link_is_virtual(cx), "the opened link is the virtual printer");
            assert!(studio.read(cx).printer_model().is_some());
            // Not remembered: a virtual printer is never auto-reconnected.
            assert_eq!(studio.read(cx).prefs.last_endpoint, None);
        });
        // The dialog shows the printer card, then closes itself shortly after.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .expect("window");
        wait_until(cx, "the dialog to close", |cx| {
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("refresh-status").is_none()
            })
            .unwrap_or(false)
        });
        // The media bar renders the connected state.
        cx.update_window(handle, |_, window, cx| window.render_frame(cx)).expect("window");

        studio.update(cx, |s, cx| s.disconnect(cx));
        wait_until(cx, "the disconnect", |cx| cx.update(|cx| studio.read(cx).conn == ClientState::Idle));
        cx.update(|cx| assert!(cx.try_global::<PrinterLink>().is_none_or(|l| l.link.is_none())));
    }
}
