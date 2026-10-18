//! The connection thread: one per connect attempt, the port of the studio's `PrinterClient`
//! (`printer/client.ts`). It opens the transport (with the "Waking printer…" retry), runs the
//! handshake, pumps the session, executes link commands (status, print, cancel, settings) and
//! keeps the idle printer awake. It owns the client state machine and reports to the manager
//! thread ([`crate::client`]), which filters stale attempts and turns failures into problems.

use std::sync::mpsc::{self, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use async_channel::Sender;
use ptouch::{
    EncodedJob, Event as SessionEvent, ModelProfile, Notification, Session, SessionConfig,
    SessionState, SettingId, SettingReport, SettingValue, Status,
};
use ptouch_transport::{Transport, TransportError, TransportInfo};

use crate::client::{
    ClientConfig, ClientState, ConnectTarget, Input, OpenedLink, PrintPhase, PrintProgress,
    SettingsWriteResult,
};
use crate::error::{ClientError, LinkKind, OpenError, Stage};
use crate::info::PrinterInfo;
use crate::link::Link;
use crate::packet_log::{PacketDir, PacketEntry};

/// How long a disconnect waits for a running job's cancel to reach the printer.
const CLOSE_CANCEL_WAIT: Duration = Duration::from_millis(1500);
/// How long a disconnect waits for a cancelled settings transaction's epilogue.
const CLOSE_SETTINGS_WAIT: Duration = Duration::from_millis(500);

/// A command for the connection thread.
pub(crate) enum ConnCmd {
    RefreshStatus,
    Print(EncodedJob),
    Cancel,
    ReadSettings(Option<Vec<SettingId>>),
    WriteSettings(Vec<(SettingId, SettingValue)>),
    Close,
}

/// What the connection thread reports.
pub(crate) enum ConnEvent {
    OpenProgress {
        attempt: u8,
        of: u8,
    },
    /// The first open is taking long (Bluetooth): show "Waking printer…".
    SlowOpen,
    Opened(OpenedLink),
    State(ClientState),
    Connected(PrinterInfo),
    ConnectFailed(ClientError),
    Status(PrinterInfo),
    Notification(Notification),
    Progress(PrintProgress),
    PrintFinished(Result<(), ClientError>),
    StatusRefreshed(Result<PrinterInfo, ClientError>),
    SettingsRead(Result<Vec<SettingReport>, ClientError>),
    SettingsWritten(Result<SettingsWriteResult, ClientError>),
    /// A failure nobody waited for (link lost or printer off while idle, keepalive timeout).
    Failure(ClientError, Stage),
    /// The thread ended (link closed).
    Closed,
}

/// Sends packet-log entries (bounded channel: the oldest entry is dropped when full).
#[derive(Clone)]
pub(crate) struct PacketSink {
    pub(crate) tx: Sender<PacketEntry>,
    pub(crate) start: Instant,
}

impl PacketSink {
    pub(crate) fn data(&self, dir: PacketDir, bytes: &[u8]) {
        let _ = self
            .tx
            .force_send(PacketEntry::data(self.start.elapsed(), dir, bytes));
    }

    pub(crate) fn note(&self, text: impl Into<String>) {
        let _ = self
            .tx
            .force_send(PacketEntry::note(self.start.elapsed(), text));
    }
}

/// Everything a connection thread needs.
pub(crate) struct ConnSpawn {
    pub(crate) attempt: u64,
    pub(crate) target: ConnectTarget,
    pub(crate) config: ClientConfig,
    /// The previous connection's thread: joined before opening, so an old close never overlaps
    /// a new open.
    pub(crate) previous: Vec<JoinHandle<()>>,
    pub(crate) cmds: mpsc::Receiver<ConnCmd>,
    pub(crate) out: Sender<Input>,
    pub(crate) packets: PacketSink,
}

/// Reports to the manager, tagged with the attempt.
#[derive(Clone)]
struct Emitter {
    attempt: u64,
    out: Sender<Input>,
}

impl Emitter {
    fn send(&self, ev: ConnEvent) {
        let _ = self.out.try_send(Input::Conn {
            attempt: self.attempt,
            ev,
        });
    }
}

/// Opens a transport.
type Opener = Box<dyn Fn() -> Result<Box<dyn Transport>, TransportError>>;

/// Thread body.
pub(crate) fn run(spawn: ConnSpawn) {
    let ConnSpawn {
        attempt,
        target,
        config,
        previous,
        cmds,
        out,
        packets,
    } = spawn;
    let emit = Emitter { attempt, out };
    for previous in previous {
        let _ = previous.join();
    }
    let mut opening = Opening {
        emit: emit.clone(),
        cmds: &cmds,
        config: &config,
        packets: &packets,
    };
    let Some(link) = opening.open(target) else {
        emit.send(ConnEvent::Closed);
        return;
    };
    let mut conn = Conn {
        link,
        emit: emit.clone(),
        cmds,
        config,
        packets,
        state: ClientState::Opening,
        op: Op::None,
        pending: None,
        keepalive_at: None,
        dead: false,
    };
    conn.run();
    conn.link.close();
    emit.send(ConnEvent::Closed);
}

/// The open phase (before a session exists).
struct Opening<'a> {
    emit: Emitter,
    cmds: &'a mpsc::Receiver<ConnCmd>,
    config: &'a ClientConfig,
    packets: &'a PacketSink,
}

impl Opening<'_> {
    /// `true` once the manager asked this attempt to close (or went away). Other commands
    /// cannot arrive before the connection is up.
    fn close_requested(&self) -> bool {
        loop {
            match self.cmds.try_recv() {
                Ok(ConnCmd::Close) | Err(TryRecvError::Disconnected) => return true,
                Ok(_) => {}
                Err(TryRecvError::Empty) => return false,
            }
        }
    }

    /// Sleeps `total`, waking early when a close is requested. `false` if one was.
    fn sleep(&self, total: Duration) -> bool {
        let deadline = Instant::now() + total;
        while Instant::now() < deadline {
            if self.close_requested() {
                return false;
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            );
        }
        !self.close_requested()
    }

    fn fail(&self, error: ClientError) -> Option<Link> {
        self.packets.note(format!("open failed: {error}"));
        self.emit.send(ConnEvent::State(ClientState::Error));
        self.emit.send(ConnEvent::ConnectFailed(error));
        None
    }

    /// Resolves the target, opens it with retries, creates the session. `None` when the open
    /// failed (reported) or a close was requested.
    fn open(&mut self, target: ConnectTarget) -> Option<Link> {
        if self.close_requested() {
            return None;
        }
        let (endpoint, link_kind, label, opener): (_, _, String, Opener) = match target {
            ConnectTarget::Endpoint(endpoint) => {
                let opts = self.config.open_options.clone();
                let ep = endpoint.clone();
                (
                    Some(endpoint.clone()),
                    LinkKind::of(&endpoint),
                    endpoint.to_string(),
                    Box::new(move || ptouch_transport::open(&ep, &opts)),
                )
            }
            ConnectTarget::Auto => {
                self.packets.note("looking for a paired printer");
                let device = match crate::printer::find_printer() {
                    Ok(Some(device)) => device,
                    Ok(None) => return self.fail(ClientError::NoPrinterFound),
                    Err(e) => {
                        self.packets.note(format!("discovery failed: {e:#}"));
                        return self.fail(ClientError::NoPrinterFound);
                    }
                };
                if self.close_requested() {
                    return None;
                }
                let opts = self.config.open_options.clone();
                let ep = device.endpoint.clone();
                (
                    Some(device.endpoint.clone()),
                    LinkKind::of(&device.endpoint),
                    device.label,
                    Box::new(move || ptouch_transport::open(&ep, &opts)),
                )
            }
            ConnectTarget::Custom { label, open } => {
                (None, LinkKind::Custom, label, Box::new(move || open()))
            }
        };

        let transport = self.open_with_retry(&*opener, link_kind, &label)?;
        let info: TransportInfo = transport.info().clone();
        self.emit.send(ConnEvent::Opened(OpenedLink {
            endpoint,
            link: link_kind,
            label,
            transport: info,
        }));
        let session_config = self.config.session.unwrap_or_else(|| {
            // macOS serial nodes often go silent after the first use: give up after ~6 s
            // instead of the core's 3 × 5 s (the studio does this for Firefox's OS ports).
            if link_kind == LinkKind::Serial && cfg!(target_os = "macos") {
                SessionConfig {
                    status_timeout_ms: 3_000,
                    status_attempts: 2,
                    ..SessionConfig::default()
                }
            } else {
                SessionConfig::default()
            }
        });
        let session = Session::new(self.config.model, session_config);
        let sink = self.packets.clone();
        Some(Link::new(
            transport,
            session,
            Some(Box::new(move |dir, bytes| sink.data(dir, bytes))),
        ))
    }

    /// The studio's `openWithRetry`: up to `open_attempts`, retrying transient failures after
    /// a backoff; "waking" from the second attempt, or when a Bluetooth open is slow.
    fn open_with_retry(
        &self,
        opener: &dyn Fn() -> Result<Box<dyn Transport>, TransportError>,
        link: LinkKind,
        label: &str,
    ) -> Option<Box<dyn Transport>> {
        let of = self.config.open_attempts.max(1);
        for n in 1..=of {
            if self.close_requested() {
                return None;
            }
            self.emit.send(ConnEvent::OpenProgress { attempt: n, of });
            if n == 1 {
                self.packets.note(format!("open {label}"));
                if link == LinkKind::Bluetooth && !self.config.slow_open.is_zero() {
                    let emit = self.emit.clone();
                    let after = self.config.slow_open;
                    thread::spawn(move || {
                        thread::sleep(after);
                        emit.send(ConnEvent::SlowOpen);
                    });
                }
            } else {
                self.emit.send(ConnEvent::State(ClientState::Waking));
            }
            match opener() {
                Ok(transport) => {
                    if self.close_requested() {
                        let mut transport = transport;
                        let _ = transport.close();
                        self.packets.note("open cancelled");
                        return None;
                    }
                    return Some(transport);
                }
                Err(e) => {
                    let cause = OpenError::new(link, &e);
                    let last = n == of || !cause.is_transient();
                    if last {
                        self.fail(ClientError::Open { attempts: n, cause });
                        return None;
                    }
                    self.packets
                        .note(format!("open failed ({e}); retry {}/{of}", n + 1));
                    let backoff = self
                        .config
                        .open_backoff
                        .get(usize::from(n - 1))
                        .or(self.config.open_backoff.last())
                        .copied()
                        .unwrap_or_default();
                    if !self.sleep(backoff) {
                        return None;
                    }
                }
            }
        }
        None
    }
}

/// The operation in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    None,
    Connecting,
    Print {
        of: u16,
    },
    Settings {
        write: bool,
    },
    /// A status request; `refresh` when someone waits for the answer (not just the keepalive).
    Status {
        refresh: bool,
    },
}

/// A command deferred while the session drains after a job.
enum Pending {
    Print(EncodedJob),
    Read(Option<Vec<SettingId>>),
    Write(Vec<(SettingId, SettingValue)>),
}

struct Conn {
    link: Link,
    emit: Emitter,
    cmds: mpsc::Receiver<ConnCmd>,
    config: ClientConfig,
    packets: PacketSink,
    state: ClientState,
    op: Op,
    pending: Option<Pending>,
    keepalive_at: Option<Instant>,
    /// The session needs a new connect (or the link is gone): leave the loop and close.
    dead: bool,
}

impl Conn {
    fn run(&mut self) {
        self.set_state(ClientState::Handshaking);
        let now = self.link.now_ms();
        self.link.session.connect(now);
        self.op = Op::Connecting;
        while !self.dead {
            loop {
                match self.cmds.try_recv() {
                    Ok(ConnCmd::Close) | Err(TryRecvError::Disconnected) => {
                        self.close_gracefully();
                        return;
                    }
                    Ok(cmd) => self.command(cmd),
                    Err(TryRecvError::Empty) => break,
                }
            }
            self.start_pending();
            if self.keepalive_due() {
                self.start_status(false);
            }
            let wait = self.config.poll_interval.min(self.until_keepalive());
            match self.link.pump(wait) {
                Ok(events) => {
                    for ev in events {
                        self.on_event(ev);
                    }
                }
                Err(e) => self.on_lost(&e),
            }
        }
    }

    fn model(&self) -> Option<&'static ModelProfile> {
        self.link.session.profile()
    }

    fn info(&self, status: Status) -> PrinterInfo {
        PrinterInfo::new(self.model(), status)
    }

    fn set_state(&mut self, state: ClientState) {
        if self.state != state {
            self.state = state;
            self.emit.send(ConnEvent::State(state));
        }
    }

    // ----- commands -----------------------------------------------------------------------

    fn command(&mut self, cmd: ConnCmd) {
        match cmd {
            ConnCmd::RefreshStatus => match self.op {
                Op::Status { .. } => self.op = Op::Status { refresh: true },
                _ => self.start_status(true),
            },
            ConnCmd::Print(job) => self.start_print(job),
            ConnCmd::Cancel => self.cancel(),
            ConnCmd::ReadSettings(ids) => self.start_read(ids),
            ConnCmd::WriteSettings(values) => self.start_write(values),
            ConnCmd::Close => {}
        }
    }

    /// Ready for a new operation (the studio's `state === 'ready'` checks).
    fn idle(&self) -> bool {
        self.op == Op::None && self.pending.is_none() && self.state == ClientState::Connected
    }

    fn draining(&self) -> bool {
        self.link.session.state() == SessionState::Draining
    }

    fn start_status(&mut self, refresh: bool) {
        let allowed = self.op == Op::None
            && matches!(
                self.state,
                ClientState::Connected | ClientState::NoReply | ClientState::Error
            );
        if !allowed {
            if refresh {
                self.emit
                    .send(ConnEvent::StatusRefreshed(Err(ClientError::session(
                        ptouch::Error::Busy,
                        Stage::Status,
                    ))));
            }
            return;
        }
        self.stop_keepalive();
        let now = self.link.now_ms();
        match self.link.session.request_status(now) {
            Ok(()) => self.op = Op::Status { refresh },
            Err(e) => {
                self.start_keepalive();
                if refresh {
                    self.emit
                        .send(ConnEvent::StatusRefreshed(Err(ClientError::session(
                            e,
                            Stage::Status,
                        ))));
                }
            }
        }
    }

    fn start_print(&mut self, job: EncodedJob) {
        if !self.idle() {
            self.emit
                .send(ConnEvent::PrintFinished(Err(ClientError::session(
                    ptouch::Error::Busy,
                    Stage::Print,
                ))));
            return;
        }
        // Right after a job the session drains for a moment and refuses submit(): wait.
        if self.draining() {
            self.pending = Some(Pending::Print(job));
            return;
        }
        self.submit(job);
    }

    fn submit(&mut self, job: EncodedJob) {
        self.stop_keepalive();
        let of = job.page_count();
        let lines = job.total_lines();
        let now = self.link.now_ms();
        if let Err(e) = self.link.session.submit(job, now) {
            // Nothing was sent: keep watching the idle printer.
            self.start_keepalive();
            self.emit
                .send(ConnEvent::PrintFinished(Err(ClientError::session(
                    e,
                    Stage::Print,
                ))));
            return;
        }
        self.packets.note(format!(
            "print {of} page{}, {lines} lines",
            if of == 1 { "" } else { "s" }
        ));
        self.op = Op::Print { of };
        self.set_state(ClientState::Printing);
        self.emit.send(ConnEvent::Progress(PrintProgress {
            page: 1,
            of,
            phase: PrintPhase::Sending,
        }));
    }

    fn settings_ids(&self, ids: Option<Vec<SettingId>>) -> Vec<SettingId> {
        ids.unwrap_or_else(|| {
            self.model()
                .map(|m| ptouch::settings_for(m).iter().map(|s| s.id).collect())
                .unwrap_or_default()
        })
    }

    fn start_read(&mut self, ids: Option<Vec<SettingId>>) {
        if !self.idle() {
            self.emit
                .send(ConnEvent::SettingsRead(Err(ClientError::session(
                    ptouch::Error::Busy,
                    Stage::Settings,
                ))));
            return;
        }
        if self.draining() {
            self.pending = Some(Pending::Read(ids));
            return;
        }
        let ids = self.settings_ids(ids);
        self.stop_keepalive();
        let now = self.link.now_ms();
        match self.link.session.read_settings(&ids, now) {
            Ok(()) => {
                self.op = Op::Settings { write: false };
                self.set_state(ClientState::Settings);
            }
            Err(e) => {
                self.start_keepalive();
                self.emit
                    .send(ConnEvent::SettingsRead(Err(ClientError::session(
                        e,
                        Stage::Settings,
                    ))));
            }
        }
    }

    fn start_write(&mut self, values: Vec<(SettingId, SettingValue)>) {
        if !self.idle() {
            self.emit
                .send(ConnEvent::SettingsWritten(Err(ClientError::session(
                    ptouch::Error::Busy,
                    Stage::Settings,
                ))));
            return;
        }
        if self.draining() {
            self.pending = Some(Pending::Write(values));
            return;
        }
        self.stop_keepalive();
        let now = self.link.now_ms();
        match self.link.session.write_settings(&values, now) {
            Ok(()) => {
                self.op = Op::Settings { write: true };
                self.set_state(ClientState::Settings);
            }
            Err(e) => {
                self.start_keepalive();
                self.emit
                    .send(ConnEvent::SettingsWritten(Err(ClientError::session(
                        e,
                        Stage::Settings,
                    ))));
            }
        }
    }

    /// Runs a command deferred by the post-job drain once the drain is over.
    fn start_pending(&mut self) {
        if self.pending.is_none() || self.draining() {
            return;
        }
        match self.pending.take() {
            Some(Pending::Print(job)) => self.start_print(job),
            Some(Pending::Read(ids)) => self.start_read(ids),
            Some(Pending::Write(values)) => self.start_write(values),
            None => {}
        }
    }

    /// The studio's `cancel()`: a running job is cancelled through the session (cancel
    /// sequence, then recovery back to ready); a settings transaction is abandoned.
    fn cancel(&mut self) {
        let now = self.link.now_ms();
        match self.op {
            Op::Print { .. } => {
                self.packets.note("cancel");
                self.set_state(ClientState::Cancelling);
                self.link.session.cancel(now);
            }
            Op::Settings { .. } => self.link.session.cancel(now),
            _ => match self.pending.take() {
                Some(Pending::Print(_)) => {
                    self.emit
                        .send(ConnEvent::PrintFinished(Err(ClientError::session(
                            ptouch::Error::Cancelled,
                            Stage::Print,
                        ))));
                }
                Some(Pending::Read(_)) => {
                    self.emit
                        .send(ConnEvent::SettingsRead(Err(ClientError::session(
                            ptouch::Error::Cancelled,
                            Stage::Settings,
                        ))))
                }
                Some(Pending::Write(_)) => {
                    self.emit
                        .send(ConnEvent::SettingsWritten(Err(ClientError::session(
                            ptouch::Error::Cancelled,
                            Stage::Settings,
                        ))))
                }
                None => {}
            },
        }
    }

    /// Disconnect: a running job is cancelled first (best effort, ≤ 1.5 s), a settings
    /// transaction gets its epilogue out (≤ 0.5 s), then the link closes.
    fn close_gracefully(&mut self) {
        let now = self.link.now_ms();
        let wait = match self.op {
            Op::Print { .. } => CLOSE_CANCEL_WAIT,
            _ if self.state == ClientState::Cancelling => CLOSE_CANCEL_WAIT,
            Op::Settings { .. } => CLOSE_SETTINGS_WAIT,
            _ => Duration::ZERO,
        };
        if matches!(
            self.op,
            Op::Print { .. } | Op::Settings { .. } | Op::Connecting
        ) {
            self.link.session.cancel(now);
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline
            && matches!(
                self.link.session.state(),
                SessionState::Printing { .. } | SessionState::Recovering | SessionState::Settings
            )
        {
            if self.link.pump(Duration::from_millis(50)).is_err() {
                break;
            }
        }
        let _ = self.link.flush();
        self.packets.note("disconnected");
    }

    // ----- keepalive ------------------------------------------------------------------------

    fn start_keepalive(&mut self) {
        self.keepalive_at = self
            .config
            .keepalive
            .filter(|d| !d.is_zero())
            .map(|d| Instant::now() + d);
    }

    fn stop_keepalive(&mut self) {
        self.keepalive_at = None;
    }

    /// Never send anything while a page prints; only when idle and the session is ready.
    fn keepalive_due(&self) -> bool {
        self.keepalive_at.is_some_and(|t| Instant::now() >= t)
            && self.idle()
            && self.link.session.state() == SessionState::Ready
    }

    fn until_keepalive(&self) -> Duration {
        self.keepalive_at.map_or(Duration::MAX, |t| {
            t.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1))
        })
    }

    // ----- session events -------------------------------------------------------------------

    fn on_event(&mut self, ev: SessionEvent) {
        match ev {
            SessionEvent::Status(status) => {
                let info = self.info(status);
                self.emit.send(ConnEvent::Status(info.clone()));
                if let Op::Status { refresh } = self.op {
                    self.op = Op::None;
                    if self.state == ClientState::NoReply
                        && self.link.session.state() == SessionState::Ready
                    {
                        self.set_state(ClientState::Connected);
                    }
                    self.start_keepalive();
                    if refresh {
                        self.emit.send(ConnEvent::StatusRefreshed(Ok(info)));
                    }
                }
            }
            SessionEvent::Ready(status) => {
                let info = self.info(status);
                if self.op == Op::Connecting {
                    self.op = Op::None;
                    self.packets.note(format!(
                        "ready: {}, {} mm {}",
                        info.model_name().unwrap_or("printer"),
                        info.status.media_width_mm,
                        info.status.media_type
                    ));
                    self.set_state(ClientState::Connected);
                    self.start_keepalive();
                    self.emit.send(ConnEvent::Connected(info));
                } else {
                    // Back to ready after recovery (printer error) or cancel.
                    self.set_state(ClientState::Connected);
                    self.start_keepalive();
                }
            }
            SessionEvent::PageStarted { page } => {
                if let Op::Print { of } = self.op {
                    self.emit.send(ConnEvent::Progress(PrintProgress {
                        page,
                        of,
                        phase: PrintPhase::Printing,
                    }));
                }
            }
            SessionEvent::PageCompleted { page } => {
                if let Op::Print { of } = self.op
                    && page < of
                {
                    self.emit.send(ConnEvent::Progress(PrintProgress {
                        page: page.saturating_add(1),
                        of,
                        phase: PrintPhase::Sending,
                    }));
                }
            }
            SessionEvent::JobCompleted => {
                if let Op::Print { of } = self.op {
                    self.op = Op::None;
                    self.emit.send(ConnEvent::Progress(PrintProgress {
                        page: of,
                        of,
                        phase: PrintPhase::Done,
                    }));
                    self.packets.note("job completed");
                    self.set_state(ClientState::Connected);
                    self.start_keepalive();
                    self.emit.send(ConnEvent::PrintFinished(Ok(())));
                }
            }
            SessionEvent::Notification(n) => self.emit.send(ConnEvent::Notification(n)),
            SessionEvent::SettingsRead(report) => {
                if self.settings_done() {
                    self.emit.send(ConnEvent::SettingsRead(Ok(report)));
                }
            }
            SessionEvent::SettingsWritten { report, verified } => {
                if self.settings_done() {
                    self.emit
                        .send(ConnEvent::SettingsWritten(Ok(SettingsWriteResult {
                            report,
                            verified,
                        })));
                }
            }
            SessionEvent::Failed {
                error,
                resume_from_page,
            } => self.on_failed(error, resume_from_page),
            _ => {}
        }
    }

    /// A settings transaction ended (the session is back to ready).
    fn settings_done(&mut self) -> bool {
        if !matches!(self.op, Op::Settings { .. }) {
            return false;
        }
        self.op = Op::None;
        self.set_state(ClientState::Connected);
        self.start_keepalive();
        true
    }

    fn session_failed(&self) -> bool {
        self.link.session.state() == SessionState::Failed
    }

    /// The studio's `#onFailed`.
    fn on_failed(&mut self, error: ptouch::Error, resume_from_page: Option<u16>) {
        self.packets
            .note(format!("failed: {} {error}", error.code()));
        let off = error == ptouch::Error::PrinterOff;
        let timeout = matches!(error, ptouch::Error::Timeout(_));
        match self.op {
            Op::Connecting => {
                self.op = Op::None;
                self.dead = true;
                self.set_state(if timeout {
                    ClientState::NoReply
                } else {
                    ClientState::Error
                });
                self.emit
                    .send(ConnEvent::ConnectFailed(ClientError::session(
                        error,
                        Stage::Handshake,
                    )));
            }
            Op::Print { .. } => {
                self.op = Op::None;
                let cancelled = error == ptouch::Error::Cancelled;
                // The session now recovers (cancel sequence + status polls) and emits Ready
                // again; PRINTER_OFF or exhausted recovery leaves it failed.
                if off {
                    self.set_state(ClientState::Lost);
                } else if !cancelled {
                    self.set_state(ClientState::Error);
                }
                self.emit
                    .send(ConnEvent::PrintFinished(Err(ClientError::Session {
                        error,
                        stage: Stage::Print,
                        resume_from_page,
                    })));
                if self.session_failed() {
                    self.dead = true;
                }
            }
            Op::Settings { write } => {
                self.op = Op::None;
                if off {
                    self.set_state(ClientState::Lost);
                    self.dead = true;
                } else {
                    self.set_state(ClientState::Connected);
                    self.start_keepalive();
                }
                let err = ClientError::session(error, Stage::Settings);
                self.emit.send(if write {
                    ConnEvent::SettingsWritten(Err(err))
                } else {
                    ConnEvent::SettingsRead(Err(err))
                });
            }
            Op::Status { refresh } => {
                self.op = Op::None;
                if timeout {
                    self.set_state(ClientState::NoReply);
                } else if off {
                    self.set_state(ClientState::Lost);
                    self.dead = true;
                }
                let stage = if refresh { Stage::Status } else { Stage::Idle };
                let err = ClientError::session(error, stage);
                if refresh {
                    self.emit.send(ConnEvent::StatusRefreshed(Err(err)));
                } else if timeout || off {
                    self.emit.send(ConnEvent::Failure(err, Stage::Idle));
                } else {
                    self.start_keepalive();
                }
            }
            Op::None => {
                // Unsolicited (recovery failed, printer turned off while idle…).
                self.set_state(if off {
                    ClientState::Lost
                } else if timeout {
                    ClientState::NoReply
                } else {
                    ClientState::Error
                });
                self.emit.send(ConnEvent::Failure(
                    ClientError::session(error, Stage::Idle),
                    Stage::Idle,
                ));
                if self.session_failed() {
                    self.dead = true;
                }
            }
        }
        if self.dead {
            self.stop_keepalive();
        }
    }

    /// The studio's `#onLost`: fail whatever is pending, state `lost`, close.
    fn on_lost(&mut self, error: &TransportError) {
        let lost = ClientError::LinkLost(error.to_string());
        self.packets.note(format!("link lost: {error}"));
        self.dead = true;
        self.stop_keepalive();
        let op = std::mem::replace(&mut self.op, Op::None);
        self.set_state(ClientState::Lost);
        match op {
            Op::Connecting => self.emit.send(ConnEvent::ConnectFailed(lost)),
            Op::Print { .. } => self.emit.send(ConnEvent::PrintFinished(Err(lost))),
            Op::Settings { write: true } => self.emit.send(ConnEvent::SettingsWritten(Err(lost))),
            Op::Settings { write: false } => self.emit.send(ConnEvent::SettingsRead(Err(lost))),
            Op::Status { refresh: true } => self.emit.send(ConnEvent::StatusRefreshed(Err(lost))),
            Op::Status { refresh: false } | Op::None => {
                self.emit.send(ConnEvent::Failure(lost, Stage::Idle));
            }
        }
        match self.pending.take() {
            Some(Pending::Print(_)) => {
                self.emit
                    .send(ConnEvent::PrintFinished(Err(ClientError::LinkLost(
                        error.to_string(),
                    ))))
            }
            Some(Pending::Read(_)) => {
                self.emit
                    .send(ConnEvent::SettingsRead(Err(ClientError::LinkLost(
                        error.to_string(),
                    ))))
            }
            Some(Pending::Write(_)) => {
                self.emit
                    .send(ConnEvent::SettingsWritten(Err(ClientError::LinkLost(
                        error.to_string(),
                    ))))
            }
            None => {}
        }
    }
}
