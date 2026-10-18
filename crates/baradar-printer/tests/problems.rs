//! Error → problem mapping (the studio's `problems.ts` copy, adapted for the desktop).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io;

use baradar_printer::problems::{
    Platform, ProblemContext, cooling_problem, describe_error, describe_status,
};
use baradar_printer::{
    ClientError, LinkKind, OpenError, ProblemAction, ProblemId, Severity, Stage,
};
use ptouch::{Notification, PrinterErrors, Status, TimeoutKind};
use ptouch_transport::TransportError;

use ProblemAction::{
    ChoosePort, ConnectBluetooth, Dismiss, OpenDiagnostics, Reconnect, Retry, SwitchTape,
};

/// A real PT-P710BT idle reply: 24 mm laminated, black on white, no errors.
const IDLE: [u8; 32] = [
    0x80, 0x20, 0x42, 0x30, 0x76, 0x30, 0x00, 0x00, 0x00, 0x00, 0x18, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn idle() -> Status {
    ptouch::parse_status(&IDLE).unwrap()
}

fn ctx(stage: Stage, link: Option<LinkKind>, platform: Platform) -> ProblemContext {
    ProblemContext {
        stage,
        link,
        status: None,
        design_width_mm: None,
        platform,
    }
}

fn session(error: ptouch::Error, stage: Stage) -> ClientError {
    ClientError::session(error, stage)
}

fn open(link: LinkKind, error: TransportError) -> ClientError {
    ClientError::Open {
        attempts: 3,
        cause: OpenError::new(link, &error),
    }
}

fn printer(info1: u8, info2: u8) -> ptouch::Error {
    ptouch::Error::Printer(PrinterErrors::new(info1, info2, 0))
}

#[test]
fn printer_errors_use_the_studio_copy() {
    let print = ctx(Stage::Print, Some(LinkKind::Bluetooth), Platform::Mac);
    let status = ctx(Stage::Status, Some(LinkKind::Bluetooth), Platform::Mac);

    // Cover open (st[9] 0x10) during a print.
    let p = describe_error(&session(printer(0, 0x10), Stage::Print), &print);
    assert_eq!(p.id, ProblemId::CoverOpen);
    assert_eq!(
        p.detail,
        "Close the tape compartment cover until it clicks, then try again. The label was not \
         finished."
    );
    assert_eq!(p.actions, [Retry, Dismiss]);
    assert_eq!(p.severity, Severity::Error);

    // End of tape (st[8] 0x02).
    let p = describe_error(&session(printer(0x02, 0), Stage::Print), &print);
    assert_eq!(p.title, "The tape has run out");
    assert_eq!(
        p.detail,
        "Insert a new TZe cassette, close the cover, then try again. The label was not finished."
    );

    // Cutter jam (st[8] 0x04).
    let p = describe_error(&session(printer(0x04, 0), Stage::Print), &status);
    assert_eq!(p.title, "The cutter is jammed");
    assert_eq!(p.actions, [Reconnect, Dismiss]);

    // Overheating (st[9] 0x20) is a warning.
    let p = describe_error(&session(printer(0, 0x20), Stage::Print), &status);
    assert_eq!(p.title, "The print head is too hot");
    assert_eq!(p.severity, Severity::Warning);

    // Weak battery (st[8] 0x08).
    let p = describe_error(&session(printer(0x08, 0), Stage::Print), &status);
    assert_eq!(p.id, ProblemId::WeakBattery);
    assert_eq!(
        p.detail,
        "Charge the printer or connect the AC adapter, then try again. Printing on a weak battery \
         can stop mid-label."
    );

    // Anything else lists the messages.
    let p = describe_error(&session(printer(0, 0x04), Stage::Idle), &status);
    assert_eq!(p.title, "The printer reported an error");
    assert_eq!(
        p.detail,
        "Communication error. Turn the printer off and on again, then reconnect."
    );
    assert_eq!(p.actions, [Reconnect, OpenDiagnostics, Dismiss]);
    assert!(!p.printer_errors.is_empty());
}

#[test]
fn timeouts_depend_on_the_stage() {
    let t = |stage, link, platform| {
        describe_error(
            &session(ptouch::Error::Timeout(TimeoutKind::Handshake), stage),
            &ctx(stage, link, platform),
        )
    };
    let p = t(Stage::Handshake, Some(LinkKind::Serial), Platform::Mac);
    assert_eq!(p.title, "The printer didn’t answer");
    assert_eq!(
        p.detail,
        "The serial port opened, but the printer sent nothing back. Turn the printer off and on \
         again and reconnect, or use Bluetooth instead of the system port."
    );
    assert_eq!(
        p.actions,
        [Reconnect, ConnectBluetooth, OpenDiagnostics, Dismiss]
    );

    let p = t(Stage::Handshake, Some(LinkKind::Bluetooth), Platform::Mac);
    assert_eq!(p.actions, [Reconnect, OpenDiagnostics, Dismiss]);

    let p = t(Stage::Settings, Some(LinkKind::Usb), Platform::Mac);
    assert_eq!(p.title, "The printer didn’t answer the settings request");

    let p = t(Stage::Print, Some(LinkKind::Bluetooth), Platform::Mac);
    assert_eq!(p.title, "The printer stopped responding");

    let p = t(Stage::Status, Some(LinkKind::Bluetooth), Platform::Mac);
    assert_eq!(p.title, "The printer went to sleep");
    assert_eq!(p.id, ProblemId::PrinterOff);
}

#[test]
fn open_failures() {
    let mac = Platform::Mac;
    // A busy serial node on macOS.
    let busy = io::Error::from(io::ErrorKind::ResourceBusy);
    let p = describe_error(
        &open(LinkKind::Serial, TransportError::Io(busy)),
        &ctx(Stage::Open, None, mac),
    );
    assert_eq!(p.id, ProblemId::PortInUse);
    assert_eq!(
        p.detail,
        "Another app may be using it — close other label or terminal apps (and the ptouch \
         command-line tool), make sure the printer is on, then try again. Connecting via \
         Bluetooth avoids the system port."
    );
    assert_eq!(
        p.actions,
        [Retry, ConnectBluetooth, OpenDiagnostics, Dismiss]
    );

    // A Bluetooth open that timed out.
    let p = describe_error(
        &open(
            LinkKind::Bluetooth,
            TransportError::Timeout {
                op: "rfcommChannelOpenComplete",
            },
        ),
        &ctx(Stage::Open, None, mac),
    );
    assert_eq!(p.title, "Couldn’t connect to the printer");
    assert_eq!(p.actions, [Retry, ChoosePort, OpenDiagnostics, Dismiss]);
    assert!(p.technical.as_deref().unwrap().contains("timed out"));

    // USB claimed by usblp on Linux.
    let claim = TransportError::Usb {
        op: "claim_interface",
        message: String::from("Device or resource busy"),
    };
    let p = describe_error(
        &open(LinkKind::Usb, claim),
        &ctx(Stage::Open, None, Platform::Linux),
    );
    assert_eq!(p.id, ProblemId::UsbBusy);
    assert_eq!(p.command.as_deref(), Some("sudo rmmod usblp"));
    assert_eq!(p.actions, [Retry, ConnectBluetooth, Dismiss]);

    // USB permissions on Linux.
    let access = TransportError::Usb {
        op: "open",
        message: String::from("Permission denied (access)"),
    };
    let p = describe_error(
        &open(LinkKind::Usb, access),
        &ctx(Stage::Open, None, Platform::Linux),
    );
    assert_eq!(p.title, "No permission to open the USB printer");
    assert_eq!(
        p.command.as_deref(),
        Some(r#"SUBSYSTEM=="usb", ATTR{idVendor}=="04f9", TAG+="uaccess""#)
    );

    // USB on Windows.
    let p = describe_error(
        &open(
            LinkKind::Usb,
            TransportError::Usb {
                op: "open IN endpoint 0x81",
                message: String::from("failed"),
            },
        ),
        &ctx(Stage::Open, None, Platform::Windows),
    );
    assert_eq!(p.title, "Couldn’t open the USB printer");
    assert!(p.detail.contains("so this app can’t use it"));
    assert_eq!(p.actions, [Retry, Dismiss]);

    // Permission denied on macOS.
    let denied = io::Error::from(io::ErrorKind::PermissionDenied);
    let p = describe_error(
        &open(LinkKind::Bluetooth, TransportError::Io(denied)),
        &ctx(Stage::Open, None, mac),
    );
    assert_eq!(p.title, "Bluetooth permission needed");
    assert_eq!(
        p.detail,
        "Allow Baradar to use Bluetooth in System Settings › Privacy & Security › Bluetooth, make \
         sure Bluetooth is on, then try again."
    );
}

#[test]
fn transient_open_errors() {
    let t = |link, e: TransportError| OpenError::new(link, &e).is_transient();
    assert!(t(
        LinkKind::Bluetooth,
        TransportError::Timeout {
            op: "rfcommChannelOpenComplete"
        }
    ));
    assert!(t(LinkKind::Bluetooth, TransportError::NoWorkingChannel));
    assert!(t(
        LinkKind::Bluetooth,
        TransportError::Bluetooth {
            op: "openRFCOMMChannelSync",
            code: 0xe00002d6
        }
    ));
    assert!(!t(
        LinkKind::Bluetooth,
        TransportError::NotFound(String::from("x"))
    ));
    assert!(!t(LinkKind::Bluetooth, TransportError::EventLoopNotRunning));
    assert!(!t(LinkKind::Usb, TransportError::Closed));
}

#[test]
fn link_lost_and_session_codes() {
    let mac = Platform::Mac;
    let lost = ClientError::LinkLost(String::from("transport closed"));
    let p = describe_error(&lost, &ctx(Stage::Idle, Some(LinkKind::Usb), mac));
    assert_eq!(p.title, "The printer disconnected");
    assert_eq!(
        p.detail,
        "Check the USB cable and that the printer is on, then reconnect."
    );
    let p = describe_error(&lost, &ctx(Stage::Idle, Some(LinkKind::Bluetooth), mac));
    assert_eq!(
        p.detail,
        "It may have turned itself off or gone out of range. Turn it on, then reconnect."
    );

    let off = |stage| {
        describe_error(
            &session(ptouch::Error::PrinterOff, stage),
            &ctx(stage, None, mac),
        )
    };
    assert_eq!(
        off(Stage::Print).detail,
        "The label was not finished. Turn the printer on, reconnect and print again."
    );
    assert_eq!(off(Stage::Idle).detail, "Turn it on, then reconnect.");

    let c = ctx(Stage::Print, None, mac);
    let p = describe_error(&session(ptouch::Error::Busy, Stage::Print), &c);
    assert_eq!(p.title, "The printer is busy");
    let p = describe_error(
        &session(
            ptouch::Error::UnknownModel {
                series: 0x30,
                model: 0x99,
            },
            Stage::Handshake,
        ),
        &c,
    );
    assert_eq!(p.title, "This printer model isn’t supported");
    assert!(p.detail.contains("this app knows how to drive"));
    let p = describe_error(&session(ptouch::Error::Protocol("x"), Stage::Print), &c);
    assert_eq!(p.title, "Unexpected reply from the printer");
    assert_eq!(p.actions, [Reconnect, OpenDiagnostics, Dismiss]);
    let p = describe_error(
        &session(
            ptouch::Error::TooLong {
                dots: 9000,
                max: 8000,
            },
            Stage::Print,
        ),
        &c,
    );
    assert_eq!(p.title, "This label can’t be printed as is");
    assert_eq!(
        p.detail,
        "Label is 9000 dots long, maximum is 8000. Adjust the label and try again."
    );
    let p = describe_error(
        &session(
            ptouch::Error::InvalidInput("value not offered"),
            Stage::Settings,
        ),
        &ctx(Stage::Settings, None, mac),
    );
    assert_eq!(p.title, "This printer doesn’t offer that value");
    let p = describe_error(
        &session(ptouch::Error::Unsupported("x"), Stage::Settings),
        &c,
    );
    assert_eq!(
        p.detail,
        "This feature isn’t available in this version of the app."
    );

    // Wrong tape without a known loaded width.
    let p = describe_error(
        &session(
            ptouch::Error::MediaMismatch {
                loaded_mm: 12,
                job_mm: 24,
            },
            Stage::Print,
        ),
        &c,
    );
    assert_eq!(
        p.detail,
        "The loaded tape doesn’t match this label. Check the cassette, or switch the design to \
         the loaded tape."
    );
    assert_eq!(p.actions, [Retry, Dismiss]);

    // With the status known but no design width.
    let mut with_status = c.clone();
    with_status.status = Some(idle());
    let p = describe_error(
        &session(
            ptouch::Error::MediaMismatch {
                loaded_mm: 24,
                job_mm: 12,
            },
            Stage::Print,
        ),
        &with_status,
    );
    assert_eq!(
        p.detail,
        "The printer has 24 mm tape loaded, which doesn't match this label. Switch the design to \
         24 mm, or change the cassette."
    );
    assert_eq!(p.actions, [SwitchTape, Dismiss]);

    // Discovery found nothing.
    let p = describe_error(&ClientError::NoPrinterFound, &ctx(Stage::Open, None, mac));
    assert_eq!(p.title, "No printer found");
}

#[test]
fn every_problem_ends_with_dismiss_and_has_technical_text() {
    let mac = Platform::Mac;
    let errors = [
        ClientError::NotConnected,
        ClientError::Cancelled,
        ClientError::LinkLost(String::new()),
        session(ptouch::Error::Cancelled, Stage::Print),
        session(ptouch::Error::NotReady, Stage::Print),
        session(
            ptouch::Error::Corrupt {
                what: "x",
                offset: 1,
            },
            Stage::Print,
        ),
        ClientError::Bluetooth(String::from("x")),
    ];
    for e in errors {
        let p = describe_error(&e, &ctx(Stage::Print, Some(LinkKind::Bluetooth), mac));
        assert_eq!(p.actions.last(), Some(&Dismiss), "{e:?}");
        assert!(p.technical.is_some(), "{e:?}");
    }
    let p = describe_error(&ClientError::Cancelled, &ctx(Stage::Open, None, mac));
    assert_eq!(
        (p.title.as_str(), p.detail.as_str()),
        ("Cancelled", "Nothing was changed.")
    );
}

#[test]
fn status_problems() {
    let c = ctx(Stage::Status, Some(LinkKind::Bluetooth), Platform::Mac);
    assert_eq!(describe_status(&idle(), &c), None);

    let mut st = idle();
    st.errors = PrinterErrors::new(0x08, 0, 0);
    let p = describe_status(&st, &c).unwrap();
    assert_eq!(p.title, "The battery is weak");
    assert_eq!(
        p.technical.as_deref(),
        Some("status errors: weak-batteries")
    );

    let mut st = idle();
    st.notification = Notification::CoverOpen;
    assert_eq!(describe_status(&st, &c).unwrap().id, ProblemId::CoverOpen);

    let mut st = idle();
    st.media_width_mm = 0;
    st.media_type = ptouch::MediaType::None;
    let p = describe_status(&st, &c).unwrap();
    assert_eq!(p.title, "No tape cassette");
    assert_eq!(p.technical.as_deref(), Some("status: no media"));

    let p = cooling_problem();
    assert_eq!(
        (p.title.as_str(), p.detail.as_str()),
        (
            "Cooling down",
            "The print head is hot. Printing continues automatically in a moment."
        )
    );
    assert_eq!(ProblemAction::Retry.label(), "Try again");
    assert_eq!(ProblemAction::ChoosePort.label(), "Choose port…");
    assert_eq!(ProblemAction::SwitchTape.label(), "Use loaded tape");
}
