//! User-facing problems, ported from the web studio's `printer/problems.ts`. The UI only renders
//! [`Problem`]s; all copy lives here so the connect dialog, print bar and diagnostics agree.
//!
//! The text is the studio's, verbatim, except where it talks about the browser. Changes:
//! - Firefox / Web Serial / Safari / iOS / Brave entries (`no-reply-firefox`, `unsupported-browser`,
//!   `describeSupport`) and the canvas-noise problem are dropped: a desktop app always talks to
//!   the printer itself.
//! - `wrong-port` (Chrome listing the Apple iAP service next to SPP) and `InvalidStateError`
//!   ("This page still holds the printer connection…") have no native counterpart and are
//!   dropped.
//! - "No printer chosen" (browser chooser dismissed) becomes "No printer found" for automatic
//!   discovery that found nothing.
//! - Permission: "Allow your browser to use Bluetooth…" says "Allow Baradar…"; the non-macOS
//!   text "…that this site may use serial devices (site settings)…" says "…that your user may
//!   open serial and Bluetooth devices…".
//! - USB: the Windows text says "so this app can’t use it" instead of "so the browser can’t use
//!   it"; "or another tab" is dropped from the busy text; the Linux usblp text no longer tells
//!   the user to relaunch Chrome with `--enable-features=AutomaticUsbDetach` and keeps only the
//!   "blacklist usblp" route, with `sudo rmmod usblp` as the command.
//! - "isn’t a model this studio knows how to drive" and "this version of the studio" say
//!   "this app".
//! - New: a USB printer in Editor Lite mode gets its own text (WebUSB never lists those).
//! - USB open errors are classified by link first, so a busy USB device is `usb-busy` rather
//!   than the serial `port-in-use` text.

use std::fmt;

use ptouch::{PrinterError, Status};

use crate::error::{ClientError, LinkKind, OpenErrorKind, Stage};
use crate::info::mm_text;

/// Which problem it is (the studio's `ProblemId`; [`ProblemId::as_str`] gives its id string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemId {
    /// Opening the link failed.
    OpenFailed,
    /// The serial port is held by another app.
    PortInUse,
    /// The printer did not answer.
    NoReply,
    /// The link dropped, or nothing is connected.
    LinkLost,
    /// The OS refused access (Bluetooth privacy setting, device permissions).
    PermissionDenied,
    /// The USB interface is held by another driver or app.
    UsbBusy,
    /// The USB device could not be opened.
    UsbAccess,
    /// No tape cassette, or the tape ran out.
    NoMedia,
    /// The loaded tape does not match the label, or is not supported.
    WrongMedia,
    /// The cover is open.
    CoverOpen,
    /// The cutter is jammed.
    CutterJam,
    /// The print head is too hot, or cooling down.
    Overheating,
    /// The battery is weak.
    WeakBattery,
    /// Any other printer error.
    PrinterError,
    /// The printer turned off or went to sleep.
    PrinterOff,
    /// The printer is busy.
    Busy,
    /// A deliberate cancel.
    Cancelled,
    /// Anything else.
    Unknown,
}

impl ProblemId {
    /// The studio's id string, e.g. `cover-open`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenFailed => "open-failed",
            Self::PortInUse => "port-in-use",
            Self::NoReply => "no-reply",
            Self::LinkLost => "link-lost",
            Self::PermissionDenied => "permission-denied",
            Self::UsbBusy => "usb-busy",
            Self::UsbAccess => "usb-access",
            Self::NoMedia => "no-media",
            Self::WrongMedia => "wrong-media",
            Self::CoverOpen => "cover-open",
            Self::CutterJam => "cutter-jam",
            Self::Overheating => "overheating",
            Self::WeakBattery => "weak-battery",
            Self::PrinterError => "printer-error",
            Self::PrinterOff => "printer-off",
            Self::Busy => "busy",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    /// Problems derived from status frames; they clear themselves when the status is clean.
    #[must_use]
    pub fn is_status_problem(self) -> bool {
        matches!(
            self,
            Self::CoverOpen
                | Self::NoMedia
                | Self::CutterJam
                | Self::Overheating
                | Self::WeakBattery
                | Self::PrinterError
                | Self::WrongMedia
        )
    }
}

/// How serious a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// A note.
    Info,
    /// Needs attention, but nothing failed for good.
    Warning,
    /// Something failed.
    Error,
}

/// A button offered with a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemAction {
    /// Try the failed operation again.
    Retry,
    /// Reconnect to the same printer.
    Reconnect,
    /// Pick a port from the full list.
    ChoosePort,
    /// Connect over Bluetooth.
    ConnectBluetooth,
    /// Connect over USB.
    ConnectUsb,
    /// Switch the design to the loaded tape width.
    SwitchTape,
    /// Open the diagnostics page.
    OpenDiagnostics,
    /// Close the banner (always the last action).
    Dismiss,
}

impl ProblemAction {
    /// The button label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Retry => "Try again",
            Self::Reconnect => "Reconnect",
            Self::ChoosePort => "Choose port…",
            Self::ConnectBluetooth => "Bluetooth",
            Self::ConnectUsb => "USB cable",
            Self::SwitchTape => "Use loaded tape",
            Self::OpenDiagnostics => "Diagnostics",
            Self::Dismiss => "Dismiss",
        }
    }

    /// Drawn as a primary button (retry, reconnect, switch-tape).
    #[must_use]
    pub fn is_primary(self) -> bool {
        matches!(self, Self::Retry | Self::Reconnect | Self::SwitchTape)
    }
}

/// An actionable, user-facing problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Which problem.
    pub id: ProblemId,
    /// How serious.
    pub severity: Severity,
    /// Headline.
    pub title: String,
    /// One or two sentences: what happened and what to do.
    pub detail: String,
    /// Buttons, `Dismiss` last.
    pub actions: Vec<ProblemAction>,
    /// A command or config line the detail refers to (shown as selectable code).
    pub command: Option<String>,
    /// Raw error text for "Show details" and "Copy diagnostics".
    pub technical: Option<String>,
    /// Printer errors behind the problem, if any.
    pub printer_errors: Vec<PrinterError>,
}

impl Problem {
    fn new(
        id: ProblemId,
        severity: Severity,
        title: &str,
        detail: impl Into<String>,
        actions: &[ProblemAction],
    ) -> Self {
        Self {
            id,
            severity,
            title: title.to_owned(),
            detail: detail.into(),
            actions: actions.to_vec(),
            command: None,
            technical: None,
            printer_errors: Vec::new(),
        }
    }

    fn with_command(mut self, command: &str) -> Self {
        self.command = Some(command.to_owned());
        self
    }

    /// Every banner can be closed: `Dismiss` is always the last action.
    fn dismissable(mut self) -> Self {
        if !self.actions.contains(&ProblemAction::Dismiss) {
            self.actions.push(ProblemAction::Dismiss);
        }
        self
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}. {}", self.title, self.detail)
    }
}

/// The operating system, for platform-specific wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// macOS.
    Mac,
    /// Linux.
    Linux,
    /// Windows.
    Windows,
    /// Anything else.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }

    /// Bluetooth RFCOMM is available natively (the "connect-bluetooth" fallback).
    fn has_bluetooth(self) -> bool {
        matches!(self, Self::Mac | Self::Linux)
    }
}

/// What the problem mapping needs to know besides the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemContext {
    /// Where the error happened.
    pub stage: Stage,
    /// The link in use, if any.
    pub link: Option<LinkKind>,
    /// Last known status (the wrong-media text names the loaded tape).
    pub status: Option<Status>,
    /// Tape width of the label being printed (media width byte), for the wrong-media text.
    pub design_width_mm: Option<u8>,
    /// The operating system.
    pub platform: Platform,
}

impl ProblemContext {
    /// A context for `stage` on this platform, without link, status or design.
    #[must_use]
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            link: None,
            status: None,
            design_width_mm: None,
            platform: Platform::current(),
        }
    }

    fn is_mac_os_port(&self) -> bool {
        self.link == Some(LinkKind::Serial) && self.platform == Platform::Mac
    }

    /// USB failures: retry, or fall back to Bluetooth where available.
    fn usb_fallback(&self) -> Vec<ProblemAction> {
        if self.platform.has_bluetooth() {
            vec![
                ProblemAction::Retry,
                ProblemAction::ConnectBluetooth,
                ProblemAction::Dismiss,
            ]
        } else {
            vec![ProblemAction::Retry, ProblemAction::Dismiss]
        }
    }
}

use ProblemAction::{
    ChoosePort, ConnectBluetooth, Dismiss, OpenDiagnostics, Reconnect, Retry, SwitchTape,
};

/// The studio's ids for the printer errors it words specially.
fn printer_error_id(e: PrinterError) -> &'static str {
    match e {
        PrinterError::CoverOpen => "cover-open",
        PrinterError::NoMedia => "no-media",
        PrinterError::EndOfMedia => "end-of-media",
        PrinterError::WrongMedia => "wrong-media",
        PrinterError::CutterJam => "cutter-jam",
        PrinterError::Overheating => "overheating",
        PrinterError::WeakBatteries => "weak-batteries",
        PrinterError::PowerTurnedOff => "power-turned-off",
        PrinterError::PrinterInUse => "printer-in-use",
        _ => "other",
    }
}

/// Problem for printer errors (from a status frame or a failed job).
fn printer_problem(errors: &[PrinterError], ctx: &ProblemContext) -> Problem {
    let has = |id: &str| errors.iter().any(|e| printer_error_id(*e) == id);
    let during = if ctx.stage == Stage::Print {
        " The label was not finished."
    } else {
        ""
    };
    let mut p = if has("cover-open") {
        Problem::new(
            ProblemId::CoverOpen,
            Severity::Error,
            "The printer cover is open",
            format!("Close the tape compartment cover until it clicks, then try again.{during}"),
            &[Retry, Dismiss],
        )
    } else if has("no-media") || has("end-of-media") {
        if has("end-of-media") {
            Problem::new(
                ProblemId::NoMedia,
                Severity::Error,
                "The tape has run out",
                format!("Insert a new TZe cassette, close the cover, then try again.{during}"),
                &[Retry, Dismiss],
            )
        } else {
            Problem::new(
                ProblemId::NoMedia,
                Severity::Error,
                "No tape cassette",
                "Insert a TZe tape cassette and close the cover, then try again.",
                &[Retry, Dismiss],
            )
        }
    } else if has("wrong-media") {
        wrong_media(ctx)
    } else if has("cutter-jam") {
        Problem::new(
            ProblemId::CutterJam,
            Severity::Error,
            "The cutter is jammed",
            "Turn the printer off, open the cover and gently remove any stuck tape around the \
             cutter, then turn it back on and reconnect.",
            &[Reconnect, Dismiss],
        )
    } else if has("overheating") {
        Problem::new(
            ProblemId::Overheating,
            Severity::Warning,
            "The print head is too hot",
            "The printer is cooling down. Wait a minute or two without printing, then try again.",
            &[Retry, Dismiss],
        )
    } else if has("weak-batteries") {
        Problem::new(
            ProblemId::WeakBattery,
            Severity::Warning,
            "The battery is weak",
            "Charge the printer or connect the AC adapter, then try again. Printing on a weak \
             battery can stop mid-label.",
            &[Retry, Dismiss],
        )
    } else if has("power-turned-off") {
        Problem::new(
            ProblemId::PrinterOff,
            Severity::Error,
            "The printer turned off",
            "Turn the printer on, then reconnect.",
            &[Reconnect, Dismiss],
        )
    } else if has("printer-in-use") {
        Problem::new(
            ProblemId::Busy,
            Severity::Warning,
            "The printer is busy",
            "Another device or app is using the printer. Wait for it to finish, then try again.",
            &[Retry, Dismiss],
        )
    } else {
        let list: Vec<&str> = errors.iter().map(PrinterError::message).collect();
        let list = list.join(", ");
        let lead = if list.is_empty() {
            String::new()
        } else {
            format!("{}. ", capitalize(&list))
        };
        Problem::new(
            ProblemId::PrinterError,
            Severity::Error,
            "The printer reported an error",
            format!("{lead}Turn the printer off and on again, then reconnect."),
            &[Reconnect, OpenDiagnostics, Dismiss],
        )
    };
    p.printer_errors = errors.to_vec();
    p
}

fn wrong_media(ctx: &ProblemContext) -> Problem {
    let loaded = ctx
        .status
        .as_ref()
        .filter(|s| s.media_width_mm > 0)
        .map(|s| mm_text(s.media_width_mm));
    let design = ctx.design_width_mm.map(mm_text);
    let detail = match (&loaded, &design) {
        (Some(loaded), Some(design)) => format!(
            "The printer has {loaded} tape loaded, but this label is designed for {design}. \
             Switch the design to {loaded}, or load {design} tape."
        ),
        (Some(loaded), None) => format!(
            "The printer has {loaded} tape loaded, which doesn't match this label. Switch the \
             design to {loaded}, or change the cassette."
        ),
        _ => String::from(
            "The loaded tape doesn’t match this label. Check the cassette, or switch the design \
             to the loaded tape.",
        ),
    };
    let actions: &[ProblemAction] = if loaded.is_some() {
        &[SwitchTape, Dismiss]
    } else {
        &[Retry, Dismiss]
    };
    Problem::new(
        ProblemId::WrongMedia,
        Severity::Warning,
        "Wrong tape width",
        detail,
        actions,
    )
}

fn no_reply(ctx: &ProblemContext) -> Problem {
    match ctx.stage {
        Stage::Settings => Problem::new(
            ProblemId::NoReply,
            Severity::Warning,
            "The printer didn’t answer the settings request",
            "It may be asleep, or it may not support this over this connection. Wake it and try \
             again.",
            &[Dismiss],
        ),
        Stage::Print => Problem::new(
            ProblemId::NoReply,
            Severity::Error,
            "The printer stopped responding",
            "Printing may not have finished. Check the printer, turn it off and on again if \
             needed, then reconnect.",
            &[Reconnect, Dismiss],
        ),
        Stage::Idle | Stage::Status => Problem::new(
            ProblemId::PrinterOff,
            Severity::Warning,
            "The printer went to sleep",
            "It turns itself off after a while without use. Turn it on, then reconnect.",
            &[Reconnect, Dismiss],
        ),
        Stage::Open | Stage::Handshake if ctx.is_mac_os_port() => Problem::new(
            ProblemId::NoReply,
            Severity::Error,
            "The printer didn’t answer",
            "The serial port opened, but the printer sent nothing back. Turn the printer off and \
             on again and reconnect, or use Bluetooth instead of the system port.",
            &[Reconnect, ConnectBluetooth, OpenDiagnostics],
        ),
        Stage::Open | Stage::Handshake => Problem::new(
            ProblemId::NoReply,
            Severity::Error,
            "The printer didn’t answer",
            "Make sure the printer is turned on and nearby. If it is, turn it off and on again, \
             then reconnect.",
            &[Reconnect, OpenDiagnostics, Dismiss],
        ),
    }
}

fn link_lost(ctx: &ProblemContext) -> Problem {
    if ctx.stage == Stage::Print {
        return Problem::new(
            ProblemId::LinkLost,
            Severity::Error,
            "The printer disconnected while printing",
            "The label may be incomplete. Make sure the printer is on and nearby, reconnect, and \
             print again.",
            &[Reconnect, Dismiss],
        );
    }
    let detail = if ctx.link == Some(LinkKind::Usb) {
        "Check the USB cable and that the printer is on, then reconnect."
    } else {
        "It may have turned itself off or gone out of range. Turn it on, then reconnect."
    };
    Problem::new(
        ProblemId::LinkLost,
        Severity::Warning,
        "The printer disconnected",
        detail,
        &[Reconnect, Dismiss],
    )
}

const WINDOWS_USB: &str = "Windows’ printer driver holds the device, so this app can’t use it. Connect via Bluetooth \
     instead.";

fn open_failed(ctx: &ProblemContext, kind: OpenErrorKind) -> Problem {
    let link = ctx.link;
    if kind == OpenErrorKind::PermissionDenied {
        return permission_denied(ctx);
    }
    if link == Some(LinkKind::Usb) {
        return match kind {
            OpenErrorKind::UsbClaim | OpenErrorKind::Busy => usb_busy(ctx),
            OpenErrorKind::UsbAccess => usb_access(ctx),
            // Not in the studio (its WebUSB filter never lists Editor Lite devices).
            OpenErrorKind::EditorLite => Problem::new(
                ProblemId::OpenFailed,
                Severity::Error,
                "Couldn’t open the USB printer",
                "The printer is in Editor Lite mode, which doesn’t accept labels over USB. Switch \
                 Editor Lite off on the printer, then try again.",
                &[Retry, Dismiss],
            ),
            _ => Problem {
                actions: ctx.usb_fallback(),
                ..Problem::new(
                    ProblemId::OpenFailed,
                    Severity::Error,
                    "Couldn’t open the USB printer",
                    if ctx.platform == Platform::Windows {
                        WINDOWS_USB
                    } else {
                        "Another app (such as P-touch Editor or the system print queue) may be \
                         using it. Close it, unplug and replug the cable, then try again."
                    },
                    &[],
                )
            },
        };
    }
    if link == Some(LinkKind::Serial) || kind == OpenErrorKind::Busy {
        let bt = ctx.platform.has_bluetooth();
        let detail = format!(
            "Another app may be using it — close other label or terminal apps (and the ptouch \
             command-line tool), make sure the printer is on, then try again.{}",
            if bt {
                " Connecting via Bluetooth avoids the system port."
            } else {
                ""
            }
        );
        let actions: &[ProblemAction] = if bt {
            &[Retry, ConnectBluetooth, OpenDiagnostics]
        } else {
            &[Retry, OpenDiagnostics, Dismiss]
        };
        return Problem::new(
            ProblemId::PortInUse,
            Severity::Error,
            "The serial port is busy or unavailable",
            detail,
            actions,
        );
    }
    Problem::new(
        ProblemId::OpenFailed,
        Severity::Error,
        "Couldn’t connect to the printer",
        "Make sure the printer is on (the Bluetooth light is lit) and close to this computer, \
         then try again. If it keeps failing, use “Choose port…” and pick the printer from the \
         full list.",
        &[Retry, ChoosePort, OpenDiagnostics],
    )
}

/// The printer interface is held by someone else (on Linux: the kernel's usblp driver).
fn usb_busy(ctx: &ProblemContext) -> Problem {
    if ctx.platform == Platform::Linux {
        return Problem {
            actions: ctx.usb_fallback(),
            ..Problem::new(
                ProblemId::UsbBusy,
                Severity::Error,
                "Linux’s printer driver is holding the printer",
                "The kernel’s usblp driver claims USB printers. Block the driver with “blacklist \
                 usblp” in /etc/modprobe.d/ and run the command below, then try again.",
                &[],
            )
            .with_command("sudo rmmod usblp")
        };
    }
    Problem {
        actions: ctx.usb_fallback(),
        ..Problem::new(
            ProblemId::UsbBusy,
            Severity::Error,
            "The USB printer is in use",
            if ctx.platform == Platform::Windows {
                WINDOWS_USB
            } else {
                "Another app (such as P-touch Editor, the system print queue or the ptouch \
                 command-line tool) is using it. Close it, unplug and replug the cable, then try \
                 again."
            },
            &[],
        )
    }
}

/// The OS refused to open the USB device (on Linux: no udev rule).
fn usb_access(ctx: &ProblemContext) -> Problem {
    if ctx.platform == Platform::Linux {
        return Problem {
            actions: ctx.usb_fallback(),
            ..Problem::new(
                ProblemId::UsbAccess,
                Severity::Error,
                "No permission to open the USB printer",
                "Your user can’t open the printer’s USB device. Add the udev rule below to \
                 /etc/udev/rules.d/60-ptouch.rules, run “sudo udevadm control --reload” and \
                 replug the printer, then try again.",
                &[],
            )
            .with_command(LINUX_USB_UDEV_RULE)
        };
    }
    Problem {
        actions: ctx.usb_fallback(),
        ..Problem::new(
            ProblemId::UsbAccess,
            Severity::Error,
            "Access to the USB printer was denied",
            if ctx.platform == Platform::Windows {
                WINDOWS_USB
            } else {
                "The system refused access to the printer. Close other apps using it, unplug and \
                 replug the cable, then try again."
            },
            &[],
        )
    }
}

/// udev rule giving the logged-in user access to Brother USB devices (Linux).
pub const LINUX_USB_UDEV_RULE: &str = r#"SUBSYSTEM=="usb", ATTR{idVendor}=="04f9", TAG+="uaccess""#;

fn permission_denied(ctx: &ProblemContext) -> Problem {
    if ctx.link == Some(LinkKind::Usb) {
        return usb_access(ctx);
    }
    let detail = if ctx.platform == Platform::Mac {
        "Allow Baradar to use Bluetooth in System Settings › Privacy & Security › Bluetooth, make \
         sure Bluetooth is on, then try again."
    } else {
        "Make sure Bluetooth is on and that your user may open serial and Bluetooth devices, then \
         try again."
    };
    Problem::new(
        ProblemId::PermissionDenied,
        Severity::Error,
        "Bluetooth permission needed",
        detail,
        &[Retry, Dismiss],
    )
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn technical(error: &ClientError, ctx: &ProblemContext) -> String {
    let mut parts = vec![format!("{:?}", ctx.stage).to_ascii_lowercase()];
    if let Some(link) = ctx.link {
        parts.push(format!("{link:?}").to_ascii_lowercase());
    }
    if let Some(code) = error.code() {
        parts.push(code.to_owned());
    }
    format!("{}: {error}", parts.join(" · "))
}

/// Maps any client error to a problem (the studio's `describeProblem`).
#[must_use]
pub fn describe_error(error: &ClientError, ctx: &ProblemContext) -> Problem {
    let mut p = classify(error, ctx);
    if p.technical.is_none() {
        p.technical = Some(technical(error, ctx));
    }
    p.dismissable()
}

fn classify(error: &ClientError, ctx: &ProblemContext) -> Problem {
    if error.is_cancel() {
        return if ctx.stage == Stage::Print {
            Problem::new(
                ProblemId::Cancelled,
                Severity::Info,
                "Printing cancelled",
                "The printer was reset. Check the tape before printing again.",
                &[Dismiss],
            )
        } else {
            Problem::new(
                ProblemId::Cancelled,
                Severity::Info,
                "Cancelled",
                "Nothing was changed.",
                &[Dismiss],
            )
        };
    }
    match error {
        ClientError::Open { cause, .. } => {
            let ctx = ProblemContext {
                link: Some(cause.link),
                ..ctx.clone()
            };
            open_failed(&ctx, cause.kind)
        }
        ClientError::LinkLost(_) => link_lost(ctx),
        ClientError::NotConnected | ClientError::Disconnected => Problem::new(
            ProblemId::LinkLost,
            Severity::Warning,
            "No printer connected",
            "Connect a printer first.",
            if ctx.platform.has_bluetooth() {
                &[ConnectBluetooth, Dismiss]
            } else {
                &[Dismiss]
            },
        ),
        ClientError::NothingToReconnect => no_remembered_printer(ctx.platform),
        ClientError::NoPrinterFound => Problem::new(
            ProblemId::OpenFailed,
            Severity::Warning,
            "No printer found",
            "Pair the printer in your computer’s Bluetooth settings first and make sure it is on \
             — or use “Choose port…” to see every serial port.",
            &[Retry, ChoosePort, Dismiss],
        ),
        ClientError::Session { error, .. } => session_problem(error, ctx),
        ClientError::Cancelled | ClientError::Bluetooth(_) => fallback(),
    }
}

fn session_problem(error: &ptouch::Error, ctx: &ProblemContext) -> Problem {
    use ptouch::Error as E;
    match error {
        E::Printer(errors) if !errors.is_empty() => {
            let list: Vec<PrinterError> = errors.iter().collect();
            printer_problem(&list, ctx)
        }
        E::Printer(_) => Problem::new(
            ProblemId::PrinterError,
            Severity::Error,
            "The printer reported an error",
            "Turn the printer off and on again, then reconnect.",
            &[Reconnect, Dismiss],
        ),
        E::NoMedia => printer_problem(&[PrinterError::NoMedia], ctx),
        E::MediaMismatch { .. } => wrong_media(ctx),
        E::Timeout(_) => no_reply(ctx),
        E::PrinterOff => Problem::new(
            ProblemId::PrinterOff,
            Severity::Warning,
            "The printer turned off",
            if ctx.stage == Stage::Print {
                "The label was not finished. Turn the printer on, reconnect and print again."
            } else {
                "Turn it on, then reconnect."
            },
            &[Reconnect, Dismiss],
        ),
        E::Busy | E::NotReady => Problem::new(
            ProblemId::Busy,
            Severity::Warning,
            "The printer is busy",
            "Wait until the current label is finished, then try again.",
            &[Dismiss],
        ),
        E::UnknownModel { .. } => Problem::new(
            ProblemId::Unknown,
            Severity::Error,
            "This printer model isn’t supported",
            "The printer answered, but it isn’t a model this app knows how to drive. Open \
             Diagnostics and copy the report if you want to request support.",
            &[OpenDiagnostics, Dismiss],
        ),
        E::UnsupportedMedia { .. } => Problem::new(
            ProblemId::WrongMedia,
            Severity::Warning,
            "This tape isn’t supported",
            "The loaded tape type or width can’t be used with this printer. Load a TZe cassette.",
            &[Dismiss],
        ),
        E::StatusLength { .. } | E::StatusHeader { .. } | E::Protocol(_) | E::Corrupt { .. } => {
            Problem::new(
                ProblemId::Unknown,
                Severity::Error,
                "Unexpected reply from the printer",
                "Turn the printer off and on again, then reconnect. If it keeps happening, open \
                 Diagnostics.",
                &[Reconnect, OpenDiagnostics],
            )
        }
        E::TooShort { .. }
        | E::TooLong { .. }
        | E::Empty
        | E::BitmapSize { .. }
        | E::DataLength { .. }
        | E::InvalidInput(_) => {
            if ctx.stage == Stage::Settings {
                Problem::new(
                    ProblemId::Unknown,
                    Severity::Warning,
                    "This printer doesn’t offer that value",
                    "Pick one of the listed values.",
                    &[Dismiss],
                )
            } else {
                Problem::new(
                    ProblemId::Unknown,
                    Severity::Warning,
                    "This label can’t be printed as is",
                    format!(
                        "{}. Adjust the label and try again.",
                        capitalize(&error.to_string())
                    ),
                    &[Dismiss],
                )
            }
        }
        E::Unsupported(_) => Problem::new(
            ProblemId::Unknown,
            Severity::Warning,
            "Not available yet",
            "This feature isn’t available in this version of the app.",
            &[Dismiss],
        ),
        E::Cancelled => Problem::new(
            ProblemId::Cancelled,
            Severity::Info,
            "Cancelled",
            "Nothing was changed.",
            &[Dismiss],
        ),
        _ => fallback(),
    }
}

fn fallback() -> Problem {
    Problem::new(
        ProblemId::Unknown,
        Severity::Error,
        "Something went wrong",
        "Try again. If it keeps happening, open Diagnostics and copy the report.",
        &[Retry, OpenDiagnostics],
    )
}

/// Problem for errors shown in a status frame (cover open, no tape, weak battery, printer
/// errors), or `None` when the printer is fine (the studio's `describeStatus`). Use after
/// connecting and for status updates while idle.
#[must_use]
pub fn describe_status(status: &Status, ctx: &ProblemContext) -> Option<Problem> {
    let info = crate::info::PrinterInfo::new(None, status.clone());
    let errors = info.errors();
    let stage = if matches!(ctx.stage, Stage::Print) {
        Stage::Print
    } else {
        Stage::Status
    };
    let ctx = ProblemContext {
        stage,
        status: Some(status.clone()),
        ..ctx.clone()
    };
    let mut p = if errors.is_empty() {
        if !status.has_media() || status.media_width_mm == 0 {
            let mut p = printer_problem(&[PrinterError::NoMedia], &ctx);
            p.technical = Some(String::from("status: no media"));
            p
        } else if status.weak_battery() {
            printer_problem(&[PrinterError::WeakBatteries], &ctx)
        } else {
            return None;
        }
    } else {
        let mut p = printer_problem(&errors, &ctx);
        let ids: Vec<&str> = errors.iter().map(|e| printer_error_id(*e)).collect();
        p.technical = Some(format!("status errors: {}", ids.join(", ")));
        p
    };
    p = p.dismissable();
    Some(p)
}

/// The info problem shown while the print head cools down (notification "cooling started").
#[must_use]
pub fn cooling_problem() -> Problem {
    Problem::new(
        ProblemId::Overheating,
        Severity::Info,
        "Cooling down",
        "The print head is hot. Printing continues automatically in a moment.",
        &[Dismiss],
    )
}

/// The problem for a "cover open" notification.
#[must_use]
pub fn cover_open_problem(ctx: &ProblemContext) -> Problem {
    let ctx = ProblemContext {
        stage: Stage::Status,
        ..ctx.clone()
    };
    printer_problem(&[PrinterError::CoverOpen], &ctx).dismissable()
}

/// Reconnect without a remembered printer.
#[must_use]
pub fn no_remembered_printer(platform: Platform) -> Problem {
    Problem::new(
        ProblemId::LinkLost,
        Severity::Info,
        "Choose your printer",
        "There is no remembered printer to reconnect to. Connect one to continue.",
        if platform.has_bluetooth() {
            &[ConnectBluetooth, Dismiss]
        } else {
            &[Dismiss]
        },
    )
    .dismissable()
}
