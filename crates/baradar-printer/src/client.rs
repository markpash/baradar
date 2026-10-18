//! The printer client: a thread that owns the printer connection and talks to the UI through
//! async channels, the port of the web studio's `ConnectionManager` + `PrinterClient`.
//!
//! ```text
//! UI ──Command──▶ manager thread ──ConnCmd──▶ connection thread (one per attempt)
//!    ◀──Event────               ◀─ConnEvent─  transport + ptouch::Session
//!    ◀──PacketEntry (bounded) ─────────────────
//! ```
//!
//! The manager keeps the snapshot (state, status, problem), starts a new connection thread for
//! every connect attempt and drops events of superseded attempts, so a cancelled attempt that is
//! still stuck in a Bluetooth open never touches the UI. The previous connection's thread is
//! joined by the next one before it opens, so an old close never overlaps a new open. Scanning,
//! pairing and the link probe run on their own threads.
//!
//! # Use from a GPUI app (macOS)
//! ```ignore
//! // In main(), before the first Bluetooth use (GPUI's main thread runs NSApplication):
//! ptouch_transport::macos::use_host_run_loop();
//! let ClientChannels { client, events, packets } = PrinterClient::spawn(ClientConfig::default());
//! cx.spawn(async move |cx| {
//!     while let Ok(event) = events.recv().await { /* update the model */ }
//! }).detach();
//! client.send(Command::Connect(ConnectTarget::Auto))?;
//! ```
//! All Bluetooth work happens on the client's threads, never on the main thread.

use std::fmt;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use ptouch::{
    EncodedJob, ModelProfile, Notification, SessionConfig, SettingId, SettingReport, SettingValue,
};
use ptouch_transport::{Endpoint, OpenOptions, Transport, TransportError, TransportInfo};

use crate::conn::{self, ConnCmd, ConnEvent, ConnSpawn, PacketSink};
use crate::error::{ClientError, LinkKind, Stage};
use crate::info::PrinterInfo;
use crate::packet_log::{LOG_CAPACITY, PacketEntry};
use crate::probe::{self, ProbeReport};
use crate::problems::{
    self, Platform, Problem, ProblemContext, cooling_problem, cover_open_problem, describe_error,
    describe_status,
};

/// Connection state (the studio's `ClientState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientState {
    /// Not connected.
    Idle,
    /// Opening the transport.
    Opening,
    /// Retrying the open, or a slow first Bluetooth open: "Waking printer…".
    Waking,
    /// Reset + status request sent, waiting for the first status.
    Handshaking,
    /// Connected and idle.
    Connected,
    /// A job is printing.
    Printing,
    /// A job is being cancelled (the printer is reset, then polled until ready).
    Cancelling,
    /// A printer-settings read or write runs.
    Settings,
    /// The printer did not answer (asleep, or a dead serial port). Reconnect re-opens it.
    NoReply,
    /// The link dropped or the printer turned off. Reconnect re-opens it.
    Lost,
    /// The open or a job failed. After a job error the session recovers and the state returns
    /// to [`ClientState::Connected`].
    Error,
}

impl ClientState {
    /// Opening, waking or handshaking.
    #[must_use]
    pub fn is_connecting(self) -> bool {
        matches!(self, Self::Opening | Self::Waking | Self::Handshaking)
    }
}

/// Phase of a page in [`PrintProgress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrintPhase {
    /// The page is being sent (or waits for the previous page to finish).
    Sending,
    /// The printer reported that the page is printing.
    Printing,
    /// The whole job completed.
    Done,
}

/// Print progress, derived from the printer's status frames (never from write completion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrintProgress {
    /// 1-based page.
    pub page: u16,
    /// Pages in the job.
    pub of: u16,
    /// Phase of `page`.
    pub phase: PrintPhase,
}

/// Result of a settings write: the read-back values, and whether each equals what was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsWriteResult {
    /// The read-back, in request order.
    pub report: Vec<SettingReport>,
    /// Every read-back value equals the value written.
    pub verified: bool,
}

/// Opens a caller-supplied transport (tests, simulated printers with custom behaviour).
pub type TransportFactory =
    Arc<dyn Fn() -> Result<Box<dyn Transport>, TransportError> + Send + Sync>;

/// What to connect to.
#[derive(Clone)]
pub enum ConnectTarget {
    /// The paired PT-E720BT, or else another paired or attached P-touch printer.
    Auto,
    /// An endpoint: `bt:…`, `serial:…`, `usb:`, `tcp:…`, `virtual:12`.
    Endpoint(Endpoint),
    /// A transport opened by `open` (called on the connection thread, again on reconnect).
    Custom {
        /// Name for logs.
        label: String,
        /// Opens the transport.
        open: TransportFactory,
    },
}

impl fmt::Debug for ConnectTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("Auto"),
            Self::Endpoint(e) => write!(f, "Endpoint({e})"),
            Self::Custom { label, .. } => write!(f, "Custom({label:?})"),
        }
    }
}

/// The link a connect attempt opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedLink {
    /// The endpoint (resolved for [`ConnectTarget::Auto`]; `None` for a custom transport).
    /// Remember it to reconnect to the same printer later.
    pub endpoint: Option<Endpoint>,
    /// Link kind.
    pub link: LinkKind,
    /// Device or endpoint name.
    pub label: String,
    /// The transport's description.
    pub transport: TransportInfo,
}

/// A failed operation: the error and its user-facing problem.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// What failed.
    pub error: ClientError,
    /// How to tell the user.
    pub problem: Problem,
}

/// A command for the client.
pub enum Command {
    /// Connect (a new attempt replaces any connection or attempt in progress).
    Connect(ConnectTarget),
    /// Re-open the current printer (after `lost` / `no-reply`); a problem when there is none.
    Reconnect,
    /// Cancel the connect attempt, the running job or the settings transaction.
    Cancel,
    /// Close the connection (a running job is cancelled first).
    Disconnect,
    /// Ask the printer for a fresh status (no-op without a connection).
    RefreshStatus,
    /// Print an encoded job; progress and the result arrive as events.
    Print(EncodedJob),
    /// Read printer settings (`None`: every setting the model supports).
    ReadSettings(Option<Vec<SettingId>>),
    /// Write printer settings, then read them back.
    WriteSettings(Vec<(SettingId, SettingValue)>),
    /// Diagnostics link probe: open the target without a session, send the handshake and a
    /// status request, wait up to 5 s for a status frame, close; `cycles` times (1–5). `None`
    /// probes the current printer. Refused while connected.
    Probe {
        /// What to probe.
        target: Option<ConnectTarget>,
        /// Repetitions.
        cycles: u8,
    },
    /// Clear the current problem.
    DismissProblem,
    /// Run a Bluetooth inquiry (1–48 s) and report nearby devices.
    #[cfg(target_os = "macos")]
    Scan(Duration),
    /// Pair with a Bluetooth device.
    #[cfg(target_os = "macos")]
    Pair(ptouch_transport::BtAddr),
    /// Close everything and stop the client thread.
    Shutdown,
}

/// Something the UI should know.
#[derive(Debug, Clone)]
pub enum Event {
    /// The connection state changed.
    State(ClientState),
    /// Before each open attempt: "Waking printer… (attempt 2 of 3)".
    OpenProgress {
        /// 1-based attempt.
        attempt: u8,
        /// Attempts allowed.
        of: u8,
    },
    /// The transport opened; the handshake starts.
    Opened(OpenedLink),
    /// Handshake done: the first status.
    Connected(PrinterInfo),
    /// The connect attempt failed (not sent for a cancelled attempt).
    ConnectFailed(Failure),
    /// Every status frame (tape, colours, battery, errors).
    Status(PrinterInfo),
    /// A printer notification (cover open/closed, cooling started/finished…).
    Notification(Notification),
    /// The current problem banner changed (`None`: cleared).
    Problem(Option<Problem>),
    /// Print progress.
    Progress(PrintProgress),
    /// A print job ended.
    PrintFinished(Result<(), Failure>),
    /// Reply to [`Command::RefreshStatus`].
    StatusRefreshed(Result<PrinterInfo, Failure>),
    /// Reply to [`Command::ReadSettings`].
    SettingsRead(Result<Vec<SettingReport>, Failure>),
    /// Reply to [`Command::WriteSettings`].
    SettingsWritten(Result<SettingsWriteResult, Failure>),
    /// Reply to [`Command::Probe`].
    Probe(ProbeReport),
    /// A failure nobody waited for: link lost or printer off while idle, keepalive timeout.
    Error(Failure),
    /// Reply to [`Command::Scan`].
    #[cfg(target_os = "macos")]
    ScanFinished(Result<Vec<crate::bluetooth::NearbyDevice>, ClientError>),
    /// Pairing progress (a passkey to show, a PIN sent…).
    #[cfg(target_os = "macos")]
    Pairing(crate::bluetooth::PairEvent),
    /// Reply to [`Command::Pair`].
    #[cfg(target_os = "macos")]
    PairFinished(Result<(), ClientError>),
    /// The client thread stopped (after [`Command::Shutdown`] or when every handle was dropped).
    Stopped,
}

/// Client options. The defaults follow the studio.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Expected model (`None`: detect it from the first status, as the studio does). With
    /// `Some`, another model fails the handshake with `UNKNOWN_MODEL`.
    pub model: Option<&'static ModelProfile>,
    /// Transport options (default: PT-E720BT model hint, which also picks the model a
    /// `virtual:` endpoint simulates).
    pub open_options: OpenOptions,
    /// Session timing (`None`: ptouch's defaults; shorter handshake timeouts on macOS serial
    /// nodes).
    pub session: Option<SessionConfig>,
    /// Idle keepalive status request interval (60 s; `None` = off).
    pub keepalive: Option<Duration>,
    /// Open attempts (3).
    pub open_attempts: u8,
    /// Delay before retry n (300 ms, 1 s; the last value repeats).
    pub open_backoff: Vec<Duration>,
    /// Switch `opening` to `waking` after this long on a Bluetooth open (1.5 s; zero = only on
    /// a retry).
    pub slow_open: Duration,
    /// Longest wait for input before commands are checked again (50 ms).
    pub poll_interval: Duration,
    /// Platform for problem wording (default: this OS).
    pub platform: Platform,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            model: None,
            open_options: crate::printer::open_options(),
            session: None,
            keepalive: Some(Duration::from_secs(60)),
            open_attempts: 3,
            open_backoff: vec![Duration::from_millis(300), Duration::from_millis(1000)],
            slow_open: Duration::from_millis(1500),
            poll_interval: Duration::from_millis(50),
            platform: Platform::current(),
        }
    }
}

/// Messages to the manager thread.
pub(crate) enum Input {
    Cmd(Command),
    Conn {
        attempt: u64,
        ev: ConnEvent,
    },
    Probe(ProbeReport),
    #[cfg(target_os = "macos")]
    Scan(Result<Vec<crate::bluetooth::NearbyDevice>, ClientError>),
    #[cfg(target_os = "macos")]
    Pairing(crate::bluetooth::PairEvent),
    #[cfg(target_os = "macos")]
    PairDone(Result<(), ClientError>),
    /// Every [`PrinterClient`] handle was dropped.
    HandlesDropped,
}

/// The client thread has stopped; the command was not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the printer client has stopped")]
pub struct ClientStopped;

/// Sends [`Input::HandlesDropped`] when the last [`PrinterClient`] clone goes away.
struct HandleGuard(Sender<Input>);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        let _ = self.0.try_send(Input::HandlesDropped);
    }
}

/// Handle to the client thread. Cheap to clone; sending never blocks.
#[derive(Clone)]
pub struct PrinterClient {
    tx: Sender<Input>,
    _guard: Arc<HandleGuard>,
}

/// What [`PrinterClient::spawn`] returns.
pub struct ClientChannels {
    /// Sends commands.
    pub client: PrinterClient,
    /// Every [`Event`], in order (unbounded).
    pub events: Receiver<Event>,
    /// The hex packet log (bounded to [`LOG_CAPACITY`] entries: the oldest is dropped when
    /// nobody reads). Feed it into a [`crate::PacketLog`] for the diagnostics view.
    pub packets: Receiver<PacketEntry>,
}

impl PrinterClient {
    /// Starts the client thread. On macOS, Bluetooth needs either
    /// `ptouch_transport::macos::use_host_run_loop()` (GUI apps) or `run_with_event_loop`
    /// around the code that uses the client (command line).
    #[must_use]
    pub fn spawn(config: ClientConfig) -> ClientChannels {
        let (tx, rx) = async_channel::unbounded();
        let (events_tx, events) = async_channel::unbounded();
        let (packets_tx, packets) = async_channel::bounded(LOG_CAPACITY);
        let manager = Manager::new(
            config,
            tx.clone(),
            events_tx,
            PacketSink {
                tx: packets_tx,
                start: Instant::now(),
            },
        );
        // If the OS is out of threads, `rx` is dropped and every send reports ClientStopped.
        let _ = thread::Builder::new()
            .name(String::from("printer-client"))
            .spawn(move || manager.run(&rx));
        ClientChannels {
            client: Self {
                tx: tx.clone(),
                _guard: Arc::new(HandleGuard(tx)),
            },
            events,
            packets,
        }
    }

    /// Queues a command.
    ///
    /// # Errors
    /// [`ClientStopped`] after the client thread ended.
    pub fn send(&self, command: Command) -> Result<(), ClientStopped> {
        self.tx
            .try_send(Input::Cmd(command))
            .map_err(|_| ClientStopped)
    }
}

/// The current connection thread.
struct ConnHandle {
    attempt: u64,
    tx: mpsc::Sender<ConnCmd>,
    join: JoinHandle<()>,
    /// The thread reported [`ConnEvent::Closed`].
    closed: bool,
}

/// The manager thread's state (the studio's `ConnectionManager` snapshot).
struct Manager {
    config: ClientConfig,
    input: Sender<Input>,
    events: Sender<Event>,
    packets: PacketSink,
    attempt: u64,
    conn: Option<ConnHandle>,
    /// Threads of closed connections, joined by the next connection (or probe) before it
    /// opens.
    closing: Vec<JoinHandle<()>>,
    /// What Reconnect re-opens: the current attempt's target (resolved endpoint once known).
    target: Option<ConnectTarget>,
    state: ClientState,
    link: Option<LinkKind>,
    status: Option<ptouch::Status>,
    problem: Option<Problem>,
    /// The current problem came from a status frame or notification (clears itself).
    status_problem: bool,
    /// Tape width byte of the last job (wrong-media wording).
    design_width: Option<u8>,
    probing: bool,
    #[cfg(target_os = "macos")]
    scanning: bool,
    #[cfg(target_os = "macos")]
    pairing: bool,
}

impl Manager {
    fn new(
        config: ClientConfig,
        input: Sender<Input>,
        events: Sender<Event>,
        packets: PacketSink,
    ) -> Self {
        Self {
            config,
            input,
            events,
            packets,
            attempt: 0,
            conn: None,
            closing: Vec::new(),
            target: None,
            state: ClientState::Idle,
            link: None,
            status: None,
            problem: None,
            status_problem: false,
            design_width: None,
            probing: false,
            #[cfg(target_os = "macos")]
            scanning: false,
            #[cfg(target_os = "macos")]
            pairing: false,
        }
    }

    fn run(mut self, rx: &Receiver<Input>) {
        while let Ok(input) = rx.recv_blocking() {
            match input {
                Input::Cmd(Command::Shutdown) | Input::HandlesDropped => break,
                Input::Cmd(cmd) => self.command(cmd),
                Input::Conn { attempt, ev } => self.conn_event(attempt, ev),
                Input::Probe(report) => {
                    self.probing = false;
                    self.emit(Event::Probe(report));
                }
                #[cfg(target_os = "macos")]
                Input::Scan(result) => {
                    self.scanning = false;
                    self.emit(Event::ScanFinished(result));
                }
                #[cfg(target_os = "macos")]
                Input::Pairing(ev) => self.emit(Event::Pairing(ev)),
                #[cfg(target_os = "macos")]
                Input::PairDone(result) => {
                    self.pairing = false;
                    self.emit(Event::PairFinished(result));
                }
            }
        }
        self.close_conn();
        for previous in self.take_closing() {
            let _ = previous.join();
        }
        self.emit(Event::Stopped);
    }

    fn emit(&self, event: Event) {
        let _ = self.events.try_send(event);
    }

    fn set_state(&mut self, state: ClientState) {
        if self.state != state {
            self.state = state;
            self.emit(Event::State(state));
        }
    }

    fn set_problem(&mut self, problem: Option<Problem>) {
        if self.problem != problem {
            self.problem.clone_from(&problem);
            self.emit(Event::Problem(problem));
        }
    }

    fn context(&self, stage: Stage) -> ProblemContext {
        ProblemContext {
            stage,
            link: self.link,
            status: self.status.clone(),
            design_width_mm: self.design_width,
            platform: self.config.platform,
        }
    }

    fn failure(&self, error: ClientError, stage: Stage) -> Failure {
        let problem = describe_error(&error, &self.context(stage));
        Failure { error, problem }
    }

    /// A live connection thread for the current attempt.
    fn live_conn(&self) -> Option<&ConnHandle> {
        self.conn
            .as_ref()
            .filter(|c| c.attempt == self.attempt && !c.closed)
    }

    /// Asks the current connection thread to close; its handle is chained for the next open.
    fn close_conn(&mut self) {
        // Finished threads need no joining (dropping the handle detaches it).
        self.closing.retain(|h| !h.is_finished());
        if let Some(conn) = self.conn.take() {
            let _ = conn.tx.send(ConnCmd::Close);
            self.closing.push(conn.join);
        }
    }

    /// Starts a new attempt: invalidates and closes the previous one.
    fn begin(&mut self) -> u64 {
        self.attempt = self.attempt.wrapping_add(1);
        self.close_conn();
        self.attempt
    }

    fn take_closing(&mut self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut self.closing)
    }

    // ----- commands -----------------------------------------------------------------------

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Connect(target) => self.connect(target),
            Command::Reconnect => self.reconnect(),
            Command::Cancel => {
                if self.state.is_connecting() {
                    self.cancel_connect();
                } else if let Some(conn) = self.live_conn() {
                    let _ = conn.tx.send(ConnCmd::Cancel);
                }
            }
            Command::Disconnect => self.disconnect(),
            Command::RefreshStatus => {
                if let Some(conn) = self.live_conn() {
                    let _ = conn.tx.send(ConnCmd::RefreshStatus);
                }
            }
            Command::Print(job) => self.print(job),
            Command::ReadSettings(ids) => {
                if let Some(conn) = self.live_conn() {
                    let _ = conn.tx.send(ConnCmd::ReadSettings(ids));
                } else {
                    let f = self.failure(ClientError::NotConnected, Stage::Settings);
                    self.emit(Event::SettingsRead(Err(f)));
                }
            }
            Command::WriteSettings(values) => {
                if let Some(conn) = self.live_conn() {
                    let _ = conn.tx.send(ConnCmd::WriteSettings(values));
                } else {
                    let f = self.failure(ClientError::NotConnected, Stage::Settings);
                    self.emit(Event::SettingsWritten(Err(f)));
                }
            }
            Command::Probe { target, cycles } => self.probe(target, cycles),
            Command::DismissProblem => {
                self.status_problem = false;
                self.set_problem(None);
            }
            #[cfg(target_os = "macos")]
            Command::Scan(duration) => self.scan(duration),
            #[cfg(target_os = "macos")]
            Command::Pair(addr) => self.pair(addr),
            Command::Shutdown => {}
        }
    }

    fn connect(&mut self, target: ConnectTarget) {
        let attempt = self.begin();
        self.target = Some(target.clone());
        self.link = match &target {
            ConnectTarget::Endpoint(e) => Some(LinkKind::of(e)),
            ConnectTarget::Custom { .. } => Some(LinkKind::Custom),
            ConnectTarget::Auto => None,
        };
        self.status = None;
        self.status_problem = false;
        self.set_problem(None);
        self.set_state(ClientState::Opening);
        let (tx, cmds) = mpsc::channel();
        let spawn = ConnSpawn {
            attempt,
            target,
            config: self.config.clone(),
            previous: self.take_closing(),
            cmds,
            out: self.input.clone(),
            packets: self.packets.clone(),
        };
        match thread::Builder::new()
            .name(format!("printer-conn-{attempt}"))
            .spawn(move || conn::run(spawn))
        {
            Ok(join) => {
                self.conn = Some(ConnHandle {
                    attempt,
                    tx,
                    join,
                    closed: false,
                });
            }
            Err(e) => {
                self.set_state(ClientState::Error);
                let f = self.failure(ClientError::LinkLost(e.to_string()), Stage::Open);
                self.set_problem(Some(f.problem.clone()));
                self.emit(Event::ConnectFailed(f));
            }
        }
    }

    fn reconnect(&mut self) {
        // A reconnect already running (double-click): let it finish.
        if self.state.is_connecting() && self.live_conn().is_some() {
            return;
        }
        match self.target.clone() {
            Some(target) => self.connect(target),
            None => self.set_problem(Some(problems::no_remembered_printer(self.config.platform))),
        }
    }

    /// Abandons the attempt in flight (also a hanging open or a silent handshake). The target is
    /// kept for Reconnect; the state returns to idle without a problem.
    fn cancel_connect(&mut self) {
        self.begin();
        self.packets.note("connect cancelled");
        self.status = None;
        self.set_problem(None);
        self.set_state(ClientState::Idle);
    }

    fn disconnect(&mut self) {
        self.begin();
        self.status = None;
        self.link = None;
        self.status_problem = false;
        self.set_problem(None);
        self.set_state(ClientState::Idle);
    }

    fn print(&mut self, job: EncodedJob) {
        self.design_width = Some(job.media_width_byte);
        let Some(conn) = self.live_conn() else {
            let f = self.failure(ClientError::NotConnected, Stage::Print);
            self.set_problem(Some(f.problem.clone()));
            self.emit(Event::PrintFinished(Err(f)));
            return;
        };
        let _ = conn.tx.send(ConnCmd::Print(job));
        self.status_problem = false;
        self.set_problem(None);
    }

    fn probe(&mut self, target: Option<ConnectTarget>, cycles: u8) {
        let refuse = |msg: &str| ProbeReport::refused(msg);
        if self.probing {
            self.emit(Event::Probe(refuse("A probe is already running.")));
            return;
        }
        if self.live_conn().is_some() {
            self.emit(Event::Probe(refuse(
                "Disconnect the printer first: a port can only be opened by one connection at a \
                 time.",
            )));
            return;
        }
        let Some(target) = target.or_else(|| self.target.clone()) else {
            self.emit(Event::Probe(refuse("No device chosen.")));
            return;
        };
        self.probing = true;
        let input = self.input.clone();
        let previous = self.take_closing();
        let options = self.config.open_options.clone();
        let packets = self.packets.clone();
        let spawned = thread::Builder::new()
            .name(String::from("printer-probe"))
            .spawn(move || {
                for previous in previous {
                    let _ = previous.join();
                }
                let report = probe::run(&target, &options, cycles, &packets);
                let _ = input.try_send(Input::Probe(report));
            });
        if spawned.is_err() {
            self.probing = false;
            self.emit(Event::Probe(refuse("Could not start the probe.")));
        }
    }

    #[cfg(target_os = "macos")]
    fn scan(&mut self, duration: Duration) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let input = self.input.clone();
        let spawned = thread::Builder::new()
            .name(String::from("printer-scan"))
            .spawn(move || {
                let result = crate::bluetooth::scan(duration)
                    .map_err(|e| ClientError::Bluetooth(format!("{e:#}")));
                let _ = input.try_send(Input::Scan(result));
            });
        if spawned.is_err() {
            self.scanning = false;
            self.emit(Event::ScanFinished(Err(ClientError::Bluetooth(
                String::from("could not start the scan"),
            ))));
        }
    }

    #[cfg(target_os = "macos")]
    fn pair(&mut self, addr: ptouch_transport::BtAddr) {
        if self.pairing {
            return;
        }
        self.pairing = true;
        let input = self.input.clone();
        let spawned = thread::Builder::new()
            .name(String::from("printer-pair"))
            .spawn(move || {
                let progress = input.clone();
                let result = crate::bluetooth::pair(addr, Duration::from_secs(30), |ev| {
                    let _ = progress.try_send(Input::Pairing(ev));
                })
                .map_err(|e| ClientError::Bluetooth(format!("{e:#}")));
                let _ = input.try_send(Input::PairDone(result));
            });
        if spawned.is_err() {
            self.pairing = false;
            self.emit(Event::PairFinished(Err(ClientError::Bluetooth(
                String::from("could not start pairing"),
            ))));
        }
    }

    // ----- connection events --------------------------------------------------------------

    fn conn_event(&mut self, attempt: u64, ev: ConnEvent) {
        if let ConnEvent::Closed = ev {
            if let Some(conn) = self.conn.as_mut().filter(|c| c.attempt == attempt) {
                conn.closed = true;
            }
            return;
        }
        if attempt != self.attempt {
            return; // a superseded attempt
        }
        match ev {
            ConnEvent::OpenProgress { attempt, of } => {
                self.emit(Event::OpenProgress { attempt, of });
            }
            ConnEvent::SlowOpen => {
                if self.state == ClientState::Opening {
                    self.set_state(ClientState::Waking);
                }
            }
            ConnEvent::Opened(opened) => {
                self.link = Some(opened.link);
                if let Some(endpoint) = &opened.endpoint {
                    self.target = Some(ConnectTarget::Endpoint(endpoint.clone()));
                }
                self.emit(Event::Opened(opened));
            }
            ConnEvent::State(state) => self.set_state(state),
            ConnEvent::Connected(info) => {
                self.apply_status(&info);
                self.emit(Event::Connected(info));
            }
            ConnEvent::ConnectFailed(error) => {
                if error.is_cancel() {
                    return;
                }
                let stage = match error {
                    ClientError::Session { stage, .. } => stage,
                    ClientError::LinkLost(_) => Stage::Handshake,
                    _ => Stage::Open,
                };
                let f = self.failure(error, stage);
                self.set_problem(Some(f.problem.clone()));
                self.emit(Event::ConnectFailed(f));
            }
            ConnEvent::Status(info) => {
                self.apply_status(&info);
                self.emit(Event::Status(info));
            }
            ConnEvent::Notification(n) => {
                self.on_notification(n);
                self.emit(Event::Notification(n));
            }
            ConnEvent::Progress(p) => self.emit(Event::Progress(p)),
            ConnEvent::PrintFinished(result) => {
                let result = result.map_err(|e| {
                    let f = self.failure(e, Stage::Print);
                    if !f.error.is_cancel() {
                        self.status_problem = false;
                        self.set_problem(Some(f.problem.clone()));
                    }
                    f
                });
                self.emit(Event::PrintFinished(result));
            }
            ConnEvent::StatusRefreshed(result) => {
                let result = result.map_err(|e| {
                    let f = self.failure(e, Stage::Status);
                    // BUSY (printing, recovering) is not worth a banner.
                    if !f.error.is_busy() {
                        self.set_problem(Some(f.problem.clone()));
                    }
                    f
                });
                self.emit(Event::StatusRefreshed(result));
            }
            ConnEvent::SettingsRead(result) => {
                let result = result.map_err(|e| self.settings_failure(e));
                self.emit(Event::SettingsRead(result));
            }
            ConnEvent::SettingsWritten(result) => {
                let result = result.map_err(|e| self.settings_failure(e));
                self.emit(Event::SettingsWritten(result));
            }
            ConnEvent::Failure(error, stage) => {
                let f = self.failure(error, stage);
                self.status_problem = false;
                self.set_problem(Some(f.problem.clone()));
                self.emit(Event::Error(f));
            }
            ConnEvent::Closed => {}
        }
    }

    /// Settings failures are phrased by the settings UI; only a lost link also sets the banner.
    fn settings_failure(&mut self, error: ClientError) -> Failure {
        let f = self.failure(error, Stage::Settings);
        if matches!(self.state, ClientState::Lost | ClientState::Idle) {
            self.set_problem(Some(f.problem.clone()));
        }
        f
    }

    /// Status-derived problems (cover open, no tape…) appear while not printing and clear
    /// themselves (the studio's `#applyStatus`).
    fn apply_status(&mut self, info: &PrinterInfo) {
        self.status = Some(info.status.clone());
        if matches!(self.state, ClientState::Printing | ClientState::Cancelling) {
            return;
        }
        match describe_status(&info.status, &self.context(Stage::Status)) {
            Some(p) => {
                self.status_problem = true;
                self.set_problem(Some(p));
            }
            None => {
                if self.status_problem
                    && self
                        .problem
                        .as_ref()
                        .is_some_and(|p| p.id.is_status_problem())
                {
                    self.status_problem = false;
                    self.set_problem(None);
                }
            }
        }
    }

    /// The studio's `#onNotification`.
    fn on_notification(&mut self, n: Notification) {
        match n {
            Notification::CoverOpen => {
                self.status_problem = true;
                let p = cover_open_problem(&self.context(Stage::Status));
                self.set_problem(Some(p));
            }
            Notification::CoolingStarted => {
                self.status_problem = true;
                self.set_problem(Some(cooling_problem()));
            }
            Notification::CoverClosed | Notification::CoolingFinished => {
                if self.status_problem {
                    self.set_problem(None);
                }
                self.status_problem = false;
            }
            _ => {}
        }
    }
}
