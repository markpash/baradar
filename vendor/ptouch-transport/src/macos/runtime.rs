//! The main-thread IOBluetooth runtime (PROTOCOL.md §2.2 step 6).
//!
//! [`super::run_main_loop`] turns the main thread into the "BT thread": it executes jobs sent
//! from other threads (every IOBluetooth call goes through [`call`]) and pumps the main
//! `CFRunLoop` in short slices in between, so that the main-queue callbacks of IOBluetooth are
//! delivered. All IOBluetooth objects live in [`BtState`] and never leave the main thread.
//!
//! In host run loop mode ([`super::use_host_run_loop`]) the application already runs the main
//! run loop (e.g. `NSApplication`): jobs are sent to the main dispatch queue instead and run
//! against a main-thread-local [`BtState`]. Nothing here then blocks or pumps the main thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_core_foundation::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode};
use objc2_io_bluetooth::{IOBluetoothDevice, IOBluetoothDeviceInquiry, IOBluetoothRFCOMMChannel};

use super::delegates::RfcommDelegate;
use crate::TransportError;

/// A unit of work executed on the main thread.
pub(crate) type Job = Box<dyn FnOnce(&mut BtState) + Send>;

/// Messages to the main-thread loop.
pub(crate) enum Msg {
    /// Run a job.
    Job(Job),
    /// Leave the loop (the application closure returned).
    Quit,
}

/// Sender to the running loop, if any.
static RUNTIME: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

/// Host run loop mode is on ([`enable_host`]).
static HOST: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The main thread's state in host run loop mode (only ever touched on the main thread).
    static HOST_STATE: RefCell<BtState> = RefCell::new(BtState::default());
}

/// Wait for `rfcommChannelClosed:` callbacks before releasing channels at shutdown.
const SHUTDOWN_CLOSE_DRAIN: Duration = Duration::from_millis(500);
/// Final drain at shutdown so `bluetoothd` tears the links down.
const SHUTDOWN_DRAIN: Duration = Duration::from_millis(250);

/// Longest single `CFRunLoopRunInMode` slice; bounds job pick-up latency.
const PUMP_SLICE: Duration = Duration::from_millis(10);

/// An open RFCOMM channel owned by the main thread.
pub(crate) struct OpenChannel {
    pub(crate) channel: Retained<IOBluetoothRFCOMMChannel>,
    pub(crate) delegate: Retained<RfcommDelegate>,
    pub(crate) device: Retained<IOBluetoothDevice>,
    /// Call `closeConnection` on close (this code opened the baseband).
    pub(crate) close_baseband: bool,
    /// Buffers handed to `writeAsync`, by refcon, kept alive until the writer has seen their
    /// completion (or the channel is closed).
    pub(crate) pending_writes: HashMap<usize, Vec<u8>>,
}

/// State owned by the main thread (IOBluetooth objects are `!Send`).
#[derive(Default)]
pub(crate) struct BtState {
    pub(crate) channels: HashMap<u32, OpenChannel>,
    next_id: u32,
    /// Delegates/targets that IOBluetooth does not retain; kept until shutdown.
    pub(crate) keep_alive: Vec<Retained<NSObject>>,
    /// Running inquiries.
    pub(crate) inquiries: HashMap<u32, Retained<IOBluetoothDeviceInquiry>>,
    /// Any Bluetooth work happened (drain on exit).
    touched: bool,
}

impl BtState {
    /// Stores an open channel and returns its handle.
    pub(crate) fn insert_channel(&mut self, ch: OpenChannel) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.channels.insert(id, ch);
        id
    }

    /// Allocates a handle for a non-channel object (inquiry).
    pub(crate) fn next_handle(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Switches to host run loop mode (see the module docs). Irreversible.
pub(crate) fn enable_host() {
    HOST.store(true, Ordering::SeqCst);
}

/// `true` in host run loop mode.
pub(crate) fn host_enabled() -> bool {
    HOST.load(Ordering::SeqCst)
}

/// `true` while [`super::run_main_loop`] is running, or in host run loop mode.
pub(crate) fn is_running() -> bool {
    host_enabled()
        || RUNTIME
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
}

/// Runs `f` on the main thread and waits for its result.
///
/// # Errors
/// [`TransportError::EventLoopNotRunning`] when no loop runs (and host run loop mode is off) or
/// when called on the main thread (that would deadlock).
pub(crate) fn call<R, F>(f: F) -> Result<R, TransportError>
where
    R: Send + 'static,
    F: FnOnce(&mut BtState) -> R + Send + 'static,
{
    submit(f)?
        .recv()
        .map_err(|_| TransportError::EventLoopNotRunning)
}

/// [`call`] with a bound on the wait: `Ok(None)` when the main thread did not answer within
/// `timeout` (it is blocked, e.g. inside a stalled `writeSync`). The job still runs later; its
/// result is then dropped.
///
/// # Errors
/// As [`call`].
pub(crate) fn call_timeout<R, F>(f: F, timeout: Duration) -> Result<Option<R>, TransportError>
where
    R: Send + 'static,
    F: FnOnce(&mut BtState) -> R + Send + 'static,
{
    match submit(f)?.recv_timeout(timeout) {
        Ok(r) => Ok(Some(r)),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::EventLoopNotRunning),
    }
}

/// Queues `f` for the main thread; the receiver yields its result. A running
/// [`super::run_main_loop`] takes the job; otherwise, in host run loop mode, the main dispatch
/// queue does.
fn submit<R, F>(f: F) -> Result<Receiver<R>, TransportError>
where
    R: Send + 'static,
    F: FnOnce(&mut BtState) -> R + Send + 'static,
{
    if MainThreadMarker::new().is_some() {
        return Err(TransportError::EventLoopNotRunning);
    }
    let (rtx, rrx) = mpsc::sync_channel(1);
    let job: Job = Box::new(move |st: &mut BtState| {
        st.touched = true;
        let _ = rtx.send(f(st));
    });
    let private = RUNTIME.lock().unwrap_or_else(PoisonError::into_inner).clone();
    match private {
        Some(tx) => tx
            .send(Msg::Job(job))
            .map_err(|_| TransportError::EventLoopNotRunning)?,
        None if host_enabled() => run_on_host(job),
        None => return Err(TransportError::EventLoopNotRunning),
    }
    Ok(rrx)
}

/// Host run loop mode: runs `job` from the main dispatch queue against [`HOST_STATE`].
fn run_on_host(job: Job) {
    DispatchQueue::main().exec_async(move || {
        HOST_STATE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut state) => job(&mut state),
            // Only if a job spun a nested run loop that drained the main queue: run this one
            // after it.
            Err(_) => run_on_host(job),
        });
    });
}

/// Pumps the current thread's run loop once for at most `slice`. Private loop only: in host run
/// loop mode the host pumps, and the main thread must never block here.
pub(crate) fn pump(slice: Duration) {
    // SAFETY: reading an immutable CoreFoundation constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    let r = CFRunLoop::run_in_mode(mode, slice.as_secs_f64(), true);
    if r == CFRunLoopRunResult::Finished {
        // No sources yet (before the first IOBluetooth call): don't spin.
        std::thread::sleep(slice.min(Duration::from_millis(5)));
    }
}

/// Pumps the run loop for `total`.
pub(crate) fn pump_for(total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        pump(PUMP_SLICE.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Installs the loop's sender globally; returns the receiver to run.
pub(crate) fn install() -> (Sender<Msg>, Receiver<Msg>) {
    let (tx, rx) = mpsc::channel();
    *RUNTIME.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx.clone());
    (tx, rx)
}

/// Main-thread loop: run jobs, pump, until `Quit`; then tear everything down.
pub(crate) fn run(rx: &Receiver<Msg>) {
    let mut state = BtState::default();
    'outer: loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Job(job)) => job(&mut state),
                Ok(Msg::Quit) | Err(mpsc::TryRecvError::Disconnected) => break 'outer,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        pump(PUMP_SLICE);
    }
    // Uninstall first so that late callers get `EventLoopNotRunning` instead of hanging.
    *RUNTIME.lock().unwrap_or_else(PoisonError::into_inner) = None;
    // Run (and thereby answer) jobs that were queued before the uninstall.
    while let Ok(msg) = rx.try_recv() {
        if let Msg::Job(job) = msg {
            job(&mut state);
        }
    }
    shutdown(&mut state);
}

/// Closes whatever the application left open (PROTOCOL.md §2.2 step 9) and drains the run
/// loop so `bluetoothd` tears the links down before the process exits.
fn shutdown(state: &mut BtState) {
    if begin_shutdown(state) {
        pump_for(SHUTDOWN_CLOSE_DRAIN);
    }
    if finish_shutdown(state) {
        pump_for(SHUTDOWN_DRAIN);
    }
}

/// Host run loop mode: [`shutdown`] without blocking the main thread. From a worker thread the
/// steps run as main-queue jobs and the drains are sleeps on the worker (the host keeps pumping
/// meanwhile). On the main thread everything is closed at once without draining; the host's run
/// loop then delivers the close callbacks as long as the process keeps running. No-op outside
/// host run loop mode.
pub(crate) fn host_shutdown() {
    if !host_enabled() {
        return;
    }
    if MainThreadMarker::new().is_some() {
        HOST_STATE.with(|cell| {
            if let Ok(mut state) = cell.try_borrow_mut() {
                begin_shutdown(&mut state);
                finish_shutdown(&mut state);
            }
        });
        return;
    }
    if matches!(call(begin_shutdown), Ok(true)) {
        std::thread::sleep(SHUTDOWN_CLOSE_DRAIN);
    }
    if matches!(call(finish_shutdown), Ok(true)) {
        std::thread::sleep(SHUTDOWN_DRAIN);
    }
}

/// Shutdown, part 1: stops inquiries and calls `closeChannel` on every open channel (delegates
/// still set). `true` if a channel was open (wait for `rfcommChannelClosed:` before part 2).
fn begin_shutdown(state: &mut BtState) -> bool {
    for inq in state.inquiries.values() {
        // SAFETY: IOBluetooth calls on the main thread with live objects.
        unsafe {
            inq.stop();
            inq.setDelegate(None);
        }
    }
    state.inquiries.clear();
    for ch in state.channels.values() {
        // SAFETY: as above; the delegate is still set so the close completes cleanly.
        unsafe {
            ch.channel.closeChannel();
        }
    }
    !state.channels.is_empty()
}

/// Shutdown, part 2: clears delegates, releases basebands we opened and leaks the delegates.
/// `true` if any Bluetooth work happened (drain before the process exits).
fn finish_shutdown(state: &mut BtState) -> bool {
    for (_, ch) in state.channels.drain() {
        // SAFETY: IOBluetooth calls on the main thread with live objects.
        unsafe {
            ch.channel.setDelegate(None);
            if ch.close_baseband {
                ch.device.closeConnection();
            }
        }
        state.keep_alive.push(Retained::into_super(ch.delegate));
    }
    // A callback still queued on the main queue must never reach a deallocated delegate:
    // intentionally leak the (few, tiny) delegate objects.
    std::mem::forget(std::mem::take(&mut state.keep_alive));
    std::mem::take(&mut state.touched)
}
