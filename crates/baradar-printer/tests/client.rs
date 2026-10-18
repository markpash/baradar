//! The printer client against ptouch's virtual printer: connect, status, keepalive, print
//! progress, cancel, printer errors, link loss, the waking retry, settings, probe, packet log.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use baradar_printer::{
    ClientConfig, ClientError, ClientState, Command, ConnectTarget, Event, PacketDir, PrintPhase,
    PrintProgress, ProblemId, Severity, Stage,
};
use common::{Sim, connect, fast_config, job, states, wait_for};
use ptouch::{Behaviour, Model, Notification, SessionConfig, SettingId, SettingValue};
use ptouch_transport::Endpoint;

const T: Duration = Duration::from_secs(15);

fn virtual_endpoint(width: u8) -> ConnectTarget {
    ConnectTarget::Endpoint(Endpoint::Virtual { width_mm: width })
}

#[test]
fn connects_to_the_virtual_endpoint_and_reports_status() {
    let ch = common::client(fast_config());
    let seen = connect(&ch, virtual_endpoint(12));
    assert_eq!(
        states(&seen),
        [
            ClientState::Opening,
            ClientState::Handshaking,
            ClientState::Connected
        ]
    );
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::OpenProgress { attempt: 1, of: 3 }))
    );
    let opened = seen
        .iter()
        .find_map(|e| match e {
            Event::Opened(o) => Some(o.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(opened.link, baradar_printer::LinkKind::Virtual);
    assert_eq!(
        opened.endpoint,
        Some(Endpoint::Virtual { width_mm: 12 }),
        "the endpoint to remember"
    );

    // The handshake status: model, tape, colours.
    let info = seen
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Status(i) => Some(i.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(info.model_name(), Some("PT-E720BT"));
    assert_eq!(info.tape_width_mm(), Some(12));
    assert_eq!(info.tape_width_text().as_deref(), Some("12 mm"));
    assert_eq!(info.tape.map(|t| t.width_mm_x10), Some(120));
    assert_eq!(info.media_type(), ptouch::MediaType::Laminated);
    assert_eq!(info.tape_color(), ptouch::TapeColor::White);
    assert_eq!(info.text_color(), ptouch::TextColor::Black);
    assert!(info.errors().is_empty());
    assert!(info.is_ready());
    assert!(info.raw_hex().starts_with("80 20 42"));

    // A refresh round-trips.
    ch.client.send(Command::RefreshStatus).unwrap();
    let (refreshed, _) = wait_for(&ch.events, T, |e| match e {
        Event::StatusRefreshed(r) => Some(r.clone()),
        _ => None,
    });
    assert_eq!(refreshed.unwrap().tape_width_mm(), Some(12));

    // Disconnect.
    ch.client.send(Command::Disconnect).unwrap();
    let (state, _) = wait_for(&ch.events, T, |e| match e {
        Event::State(s) => Some(*s),
        _ => None,
    });
    assert_eq!(state, ClientState::Idle);
}

#[test]
fn packet_log_streams_hex() {
    let ch = common::client(fast_config());
    connect(&ch, virtual_endpoint(12));
    let mut entries = Vec::new();
    while let Ok(e) = ch.packets.try_recv() {
        entries.push(e);
    }
    let sent = entries
        .iter()
        .find(|e| e.dir == PacketDir::Sent && e.bytes.ends_with(&[0x1B, 0x69, 0x53]))
        .expect("a status request in the log");
    assert!(sent.to_string().contains(">> "), "{sent}");
    let received = entries
        .iter()
        .find(|e| e.dir == PacketDir::Received)
        .expect("a status frame in the log");
    assert!(received.to_string().contains("<< 80 20 42"), "{received}");
    assert!(entries.iter().any(|e| e.dir == PacketDir::Note));
}

#[test]
fn print_reports_per_page_progress() {
    let sim = Sim::e720(12, Behaviour::Normal);
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 2)))
        .unwrap();
    let (result, seen) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    result.unwrap();
    let progress: Vec<PrintProgress> = seen
        .iter()
        .filter_map(|e| match e {
            Event::Progress(p) => Some(*p),
            _ => None,
        })
        .collect();
    let p = |page, phase| PrintProgress { page, of: 2, phase };
    assert_eq!(
        progress,
        [
            p(1, PrintPhase::Sending),
            p(1, PrintPhase::Printing),
            p(2, PrintPhase::Sending),
            p(2, PrintPhase::Printing),
            p(2, PrintPhase::Done),
        ]
    );
    assert_eq!(
        states(&seen),
        [ClientState::Printing, ClientState::Connected]
    );

    // A second job right after (during the post-job drain) waits for the drain instead of
    // failing with BUSY.
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 1)))
        .unwrap();
    let (result, _) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    result.unwrap();
    ch.client.send(Command::Disconnect).unwrap();
    wait_for(&ch.events, T, |e| {
        matches!(e, Event::State(ClientState::Idle)).then_some(())
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sim.printed_pages(), 3);
}

#[test]
fn cancel_stops_the_job_and_returns_to_connected() {
    let mut sim = Sim::e720(12, Behaviour::Normal);
    sim.timing.base_print_ms = 400;
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 4)))
        .unwrap();
    wait_for(&ch.events, T, |e| {
        matches!(
            e,
            Event::Progress(PrintProgress {
                phase: PrintPhase::Printing,
                ..
            })
        )
        .then_some(())
    });
    ch.client.send(Command::Cancel).unwrap();
    let (result, seen) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    assert!(states(&seen).contains(&ClientState::Cancelling));
    let failure = result.unwrap_err();
    assert!(failure.error.is_cancel(), "{:?}", failure.error);
    assert_eq!(failure.problem.id, ProblemId::Cancelled);
    assert_eq!(failure.problem.title, "Printing cancelled");
    assert_eq!(
        failure.problem.detail,
        "The printer was reset. Check the tape before printing again."
    );
    // No banner for a deliberate cancel; back to connected once the printer is ready.
    let (state, seen) = wait_for(&ch.events, T, |e| match e {
        Event::State(s) => Some(*s),
        _ => None,
    });
    assert_eq!(state, ClientState::Connected);
    assert!(!seen.iter().any(|e| matches!(e, Event::Problem(Some(_)))));
}

#[test]
fn no_tape_shows_a_problem_and_fails_preflight() {
    let sim = Sim::e720(12, Behaviour::NoMedia);
    let ch = common::client(fast_config());
    let seen = connect(&ch, sim.target());
    let problem = seen
        .iter()
        .find_map(|e| match e {
            Event::Problem(Some(p)) => Some(p.clone()),
            _ => None,
        })
        .expect("a status problem");
    assert_eq!(problem.id, ProblemId::NoMedia);
    assert_eq!(problem.title, "No tape cassette");
    assert_eq!(
        problem.detail,
        "Insert a TZe tape cassette and close the cover, then try again."
    );

    ch.client
        .send(Command::Print(job(sim.profile(), 12, 1)))
        .unwrap();
    let (result, _) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    let failure = result.unwrap_err();
    assert_eq!(
        failure.error.code(),
        Some("NO_MEDIA"),
        "{:?}",
        failure.error
    );
    assert_eq!(failure.problem.id, ProblemId::NoMedia);
}

#[test]
fn wrong_tape_width_names_both_widths() {
    let sim = Sim::e720(12, Behaviour::Normal);
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 24, 1)))
        .unwrap();
    let (result, _) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    let failure = result.unwrap_err();
    assert_eq!(failure.error.code(), Some("MEDIA_MISMATCH"));
    assert_eq!(failure.problem.title, "Wrong tape width");
    assert_eq!(
        failure.problem.detail,
        "The printer has 12 mm tape loaded, but this label is designed for 24 mm. Switch the \
         design to 12 mm, or load 24 mm tape."
    );
    assert_eq!(
        failure.problem.actions,
        [
            baradar_printer::ProblemAction::SwitchTape,
            baradar_printer::ProblemAction::Dismiss
        ]
    );
}

#[test]
fn cover_open_during_a_job_fails_it_and_recovers() {
    let sim = Sim::e720(12, Behaviour::CoverOpenOnPage(1));
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 2)))
        .unwrap();
    let (result, _) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    let failure = result.unwrap_err();
    match &failure.error {
        ClientError::Session {
            error: ptouch::Error::Printer(_),
            stage: Stage::Print,
            resume_from_page: Some(1),
        } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(failure.problem.id, ProblemId::CoverOpen);
    assert_eq!(failure.problem.title, "The printer cover is open");
    assert_eq!(
        failure.problem.detail,
        "Close the tape compartment cover until it clicks, then try again. The label was not \
         finished."
    );
    // The session sends its cancel sequence and polls until ready again.
    wait_for(&ch.events, T, |e| {
        matches!(e, Event::State(ClientState::Connected)).then_some(())
    });
}

#[test]
fn cooling_notification_sets_and_clears_a_problem() {
    let sim = Sim::e720(12, Behaviour::CoolingOnPage(1));
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 1)))
        .unwrap();
    let (result, seen) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    result.unwrap();
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Notification(Notification::CoolingStarted)))
    );
    let cooling = seen
        .iter()
        .find_map(|e| match e {
            Event::Problem(Some(p)) => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(cooling.title, "Cooling down");
    assert_eq!(cooling.severity, Severity::Info);
    assert!(seen.iter().any(|e| matches!(e, Event::Problem(None))));
}

#[test]
fn silent_printer_ends_in_no_reply() {
    let sim = Sim::e720(12, Behaviour::Silent);
    let ch = common::client(ClientConfig {
        session: Some(SessionConfig {
            status_timeout_ms: 150,
            status_attempts: 2,
            ..SessionConfig::default()
        }),
        ..fast_config()
    });
    ch.client.send(Command::Connect(sim.target())).unwrap();
    let (failure, seen) = wait_for(&ch.events, T, |e| match e {
        Event::ConnectFailed(f) => Some(f.clone()),
        _ => None,
    });
    assert_eq!(states(&seen).last(), Some(&ClientState::NoReply));
    assert_eq!(failure.error.code(), Some("TIMEOUT"));
    assert_eq!(failure.problem.title, "The printer didn’t answer");
    assert_eq!(
        failure.problem.detail,
        "Make sure the printer is turned on and nearby. If it is, turn it off and on again, then \
         reconnect."
    );
}

#[test]
fn transient_open_failures_are_retried_while_waking() {
    let sim = Sim::e720(12, Behaviour::Normal);
    sim.failing_opens.store(2, Ordering::SeqCst);
    let ch = common::client(fast_config());
    let seen = connect(&ch, sim.target());
    let attempts: Vec<(u8, u8)> = seen
        .iter()
        .filter_map(|e| match e {
            Event::OpenProgress { attempt, of } => Some((*attempt, *of)),
            _ => None,
        })
        .collect();
    assert_eq!(attempts, [(1, 3), (2, 3), (3, 3)]);
    assert_eq!(
        states(&seen),
        [
            ClientState::Opening,
            ClientState::Waking,
            ClientState::Handshaking,
            ClientState::Connected
        ]
    );
    assert_eq!(sim.opens.load(Ordering::SeqCst), 3);
}

#[test]
fn open_gives_up_after_three_attempts() {
    let sim = Sim::e720(12, Behaviour::Normal);
    sim.failing_opens.store(10, Ordering::SeqCst);
    let ch = common::client(fast_config());
    ch.client.send(Command::Connect(sim.target())).unwrap();
    let (failure, seen) = wait_for(&ch.events, T, |e| match e {
        Event::ConnectFailed(f) => Some(f.clone()),
        _ => None,
    });
    assert!(matches!(
        failure.error,
        ClientError::Open { attempts: 3, .. }
    ));
    assert_eq!(states(&seen).last(), Some(&ClientState::Error));
    assert_eq!(failure.problem.title, "Couldn’t connect to the printer");

    // Non-transient errors are not retried.
    let mut sim = Sim::e720(12, Behaviour::Normal);
    sim.open_not_found = true;
    ch.client.send(Command::Connect(sim.target())).unwrap();
    let (failure, _) = wait_for(&ch.events, T, |e| match e {
        Event::ConnectFailed(f) => Some(f.clone()),
        _ => None,
    });
    assert!(matches!(
        failure.error,
        ClientError::Open { attempts: 1, .. }
    ));
}

#[test]
fn cancel_abandons_a_hanging_open_at_once() {
    let mut sim = Sim::e720(12, Behaviour::Normal);
    sim.open_delay = Duration::from_millis(800);
    let ch = common::client(fast_config());
    ch.client.send(Command::Connect(sim.target())).unwrap();
    wait_for(&ch.events, T, |e| {
        matches!(e, Event::OpenProgress { .. }).then_some(())
    });
    let t = std::time::Instant::now();
    ch.client.send(Command::Cancel).unwrap();
    wait_for(&ch.events, T, |e| {
        matches!(e, Event::State(ClientState::Idle)).then_some(())
    });
    assert!(
        t.elapsed() < Duration::from_millis(400),
        "{:?}",
        t.elapsed()
    );

    // The next attempt waits for the abandoned one, then connects; the old attempt's events
    // never show up.
    let seen = connect(&ch, Sim::e720(12, Behaviour::Normal).target());
    assert!(!seen.iter().any(|e| matches!(e, Event::ConnectFailed(_))));
    assert_eq!(states(&seen).last(), Some(&ClientState::Connected));
}

#[test]
fn link_loss_while_printing() {
    let mut sim = Sim::e720(12, Behaviour::Normal);
    sim.lose_after_bytes = Some(400); // the handshake fits, the raster does not
    let ch = common::client(fast_config());
    connect(&ch, sim.target());
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 1)))
        .unwrap();
    let (result, seen) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    let failure = result.unwrap_err();
    assert!(matches!(failure.error, ClientError::LinkLost(_)));
    assert_eq!(states(&seen).last(), Some(&ClientState::Lost));
    assert_eq!(
        failure.problem.title,
        "The printer disconnected while printing"
    );

    // Reconnect re-opens the same target.
    ch.client.send(Command::Reconnect).unwrap();
    wait_for(&ch.events, T, |e| {
        matches!(e, Event::Connected(_)).then_some(())
    });
    assert_eq!(sim.opens.load(Ordering::SeqCst), 2);
}

#[test]
fn keepalive_polls_and_reports_a_sleeping_printer() {
    let sim = Sim::e720(12, Behaviour::Normal);
    let ch = common::client(ClientConfig {
        keepalive: Some(Duration::from_millis(200)),
        session: Some(SessionConfig {
            status_timeout_ms: 300,
            ..SessionConfig::default()
        }),
        ..fast_config()
    });
    connect(&ch, sim.target());
    // An unsolicited status from the keepalive (no StatusRefreshed: nobody asked).
    let (_, seen) = wait_for(&ch.events, T, |e| {
        matches!(e, Event::Status(_)).then_some(())
    });
    assert!(!seen.iter().any(|e| matches!(e, Event::StatusRefreshed(_))));

    // The printer falls asleep: the next keepalive times out.
    sim.mute.store(true, Ordering::SeqCst);
    let (failure, seen) = wait_for(&ch.events, T, |e| match e {
        Event::Error(f) => Some(f.clone()),
        _ => None,
    });
    assert_eq!(failure.error.code(), Some("TIMEOUT"));
    assert_eq!(failure.problem.id, ProblemId::PrinterOff);
    assert_eq!(failure.problem.title, "The printer went to sleep");
    assert_eq!(
        failure.problem.detail,
        "It turns itself off after a while without use. Turn it on, then reconnect."
    );
    assert!(states(&seen).contains(&ClientState::NoReply));
}

#[test]
fn settings_read_and_write_on_a_p710bt() {
    let ch = common::client(ClientConfig {
        open_options: ptouch_transport::OpenOptions {
            model_hint: ptouch::profile(Model::PtP710bt),
            ..ptouch_transport::OpenOptions::default()
        },
        ..fast_config()
    });
    connect(&ch, virtual_endpoint(24));
    ch.client.send(Command::ReadSettings(None)).unwrap();
    let (read, seen) = wait_for(&ch.events, T, |e| match e {
        Event::SettingsRead(r) => Some(r.clone()),
        _ => None,
    });
    let report = read.unwrap();
    assert!(!report.is_empty());
    assert!(states(&seen).contains(&ClientState::Settings));

    ch.client
        .send(Command::WriteSettings(vec![(
            SettingId::AutoPowerOn,
            SettingValue::Enabled(true),
        )]))
        .unwrap();
    let (written, _) = wait_for(&ch.events, T, |e| match e {
        Event::SettingsWritten(r) => Some(r.clone()),
        _ => None,
    });
    assert!(written.unwrap().verified);
}

#[test]
fn settings_on_a_model_without_them_are_unsupported() {
    let ch = common::client(fast_config());
    connect(&ch, virtual_endpoint(12));
    ch.client.send(Command::ReadSettings(None)).unwrap();
    let (read, _) = wait_for(&ch.events, T, |e| match e {
        Event::SettingsRead(r) => Some(r.clone()),
        _ => None,
    });
    let failure = read.unwrap_err();
    assert!(
        matches!(failure.error.code(), Some("UNSUPPORTED" | "EMPTY")),
        "{:?}",
        failure.error
    );
}

#[test]
fn commands_without_a_connection() {
    let ch = common::client(fast_config());
    ch.client.send(Command::Reconnect).unwrap();
    let (problem, _) = wait_for(&ch.events, T, |e| match e {
        Event::Problem(Some(p)) => Some(p.clone()),
        _ => None,
    });
    assert_eq!(problem.title, "Choose your printer");

    let sim = Sim::e720(12, Behaviour::Normal);
    ch.client
        .send(Command::Print(job(sim.profile(), 12, 1)))
        .unwrap();
    let (result, _) = wait_for(&ch.events, T, |e| match e {
        Event::PrintFinished(r) => Some(r.clone()),
        _ => None,
    });
    let failure = result.unwrap_err();
    assert_eq!(failure.error, ClientError::NotConnected);
    assert_eq!(failure.problem.title, "No printer connected");
    assert_eq!(failure.problem.detail, "Connect a printer first.");

    ch.client.send(Command::Shutdown).unwrap();
    wait_for(&ch.events, T, |e| matches!(e, Event::Stopped).then_some(()));
}

#[test]
fn probe_answers_while_idle_and_refuses_while_connected() {
    let ch = common::client(fast_config());
    ch.client
        .send(Command::Probe {
            target: Some(virtual_endpoint(12)),
            cycles: 2,
        })
        .unwrap();
    let (report, _) = wait_for(&ch.events, T, |e| match e {
        Event::Probe(r) => Some(r.clone()),
        _ => None,
    });
    assert_eq!(report.answered, 2, "{report:#?}");
    assert_eq!(report.message, "Done: 2 of 2 status requests answered.");
    assert_eq!(report.steps.len(), 8);
    assert!(report.steps.iter().all(|s| s.ok));
    assert_eq!(report.status.unwrap().model_name(), Some("PT-E720BT"));

    connect(&ch, virtual_endpoint(12));
    ch.client
        .send(Command::Probe {
            target: None,
            cycles: 1,
        })
        .unwrap();
    let (report, _) = wait_for(&ch.events, T, |e| match e {
        Event::Probe(r) => Some(r.clone()),
        _ => None,
    });
    assert!(report.message.starts_with("Disconnect the printer first"));
}

#[test]
fn dropping_every_handle_stops_the_client() {
    let ch = common::client(fast_config());
    connect(&ch, virtual_endpoint(12));
    let events = ch.events.clone();
    drop(ch);
    wait_for(&events, T, |e| matches!(e, Event::Stopped).then_some(()));
}
