//! macOS IOBluetooth backend (feature `macos-rfcomm`, macOS only).
//!
//! # Contract (PROTOCOL.md §2.2, normative steps 1–13)
//! - Threading: IOBluetooth objects are `!Send` and live on the **main thread**, whose
//!   `CFRunLoop` is pumped by [`run_main_loop`]. [`rfcomm::MacRfcommTransport`] is a `Send`
//!   handle that forwards open/write/close requests to the main thread (jobs over an `mpsc`
//!   channel, picked up between 10 ms run-loop slices) and receives data through a channel
//!   filled by the `rfcommChannelData:` delegate.
//! - Host run loop mode ([`use_host_run_loop`]): for GUI applications whose main thread runs
//!   its own run loop (`NSApplication`), so that [`run_main_loop`] can never run. The same jobs
//!   then go to the main dispatch queue, which the host's run loop services; the main thread is
//!   never blocked or pumped by this crate, and [`run_main_loop`] just calls its closure.
//! - Open: SDP cache → `performSDPQuery:` + poll `getLastServicesUpdate` (200 ms, ≤ 10 s) →
//!   `getRFCOMMChannelID:`; channel priming when SPP ≠ 1; sync open, fail fast on
//!   `kIOReturnTimeout`, else async open ≤ 10 s; candidate channel accepted only after a valid
//!   status reply (§2.1 fallback order: SDP, `rfcomm_channel_observed`, 1, 2).
//! - Write: chunks of `min(remaining, mtu)`; MTU 0 ⇒ wait for open-complete, then 320; never
//!   a zero-length write; retry `kIOReturnNotOpen` ≤ 2 s after open. Chunks go out with
//!   `writeAsync:length:refcon:` and the worker waits for `rfcommChannelWriteComplete:` with
//!   `OpenOptions::write_timeout`, so a stalled link (no RFCOMM credits) ends in
//!   `WriteTimeout` instead of blocking the main thread in `writeSync`. If a channel never
//!   reports a completion, the backend falls back to step 7's `writeSync` with a bounded
//!   wait.
//! - Close: `closeChannel` with the delegate set, wait for `rfcommChannelClosed:` (≤ 1 s),
//!   then clear the delegate; `closeConnection` if we opened the baseband or after a failed
//!   open; pump ~250 ms; then the `OpenOptions::close_wait` (500 ms, §2.1 "Timing").
//! - Stale cache (step 4): when the cached SPP channel fails to open, a fresh SDP query runs
//!   and its channel is retried once, even if it is the same channel.
//! - `IOReturn` constants are defined locally (crate-private upstream); codes in
//!   `TransportError::Bluetooth` are zero-extended (`0xe00002d6`, as documented).
//!
//! Reference prototype (verified steps 4–8 on hardware): scratchpad `proto/rfcomm-macos`.
//! Its ideas are ported here; its `getMTU() == 0 → 65535` shortcut is not.
//!
//! **UNVERIFIED in Rust on hardware** (PROTOCOL.md §2.2): baseband open (step 3),
//! multiplexer priming (step 10), the close sequence (step 9), inquiry, MTU-chunked multi-KB
//! writes, and whether `rfcommChannelWriteComplete:` is delivered on macOS 26/27.

#![allow(unsafe_code)] // Objective-C FFI is confined to this module tree.

mod delegates;
pub mod rfcomm;
mod runtime;

use std::ffi::c_int;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_io_bluetooth::{IOBluetoothDevice, IOBluetoothDeviceInquiry};

use crate::BtAddr;
use crate::TransportError;

/// `kIOReturnTimeout`.
pub const K_IO_RETURN_TIMEOUT: i32 = 0xE000_02D6_u32 as i32;
/// `kIOReturnNotOpen`.
pub const K_IO_RETURN_NOT_OPEN: i32 = 0xE000_02CD_u32 as i32;
/// `kIOReturnSuccess`.
pub const K_IO_RETURN_SUCCESS: i32 = 0;

/// An `IOReturn` as the code stored in [`TransportError::Bluetooth`]: zero-extended, so that it
/// prints as the documented 32-bit value (`0xe00002d6`, not `0xffffffffe00002d6`).
#[must_use]
pub fn io_return_code(code: c_int) -> i64 {
    i64::from(u32::from_ne_bytes(code.to_ne_bytes()))
}

/// Sends `Quit` to the main loop when dropped (also when the application panics).
struct QuitOnDrop(mpsc::Sender<runtime::Msg>);

impl Drop for QuitOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(runtime::Msg::Quit);
    }
}

/// Runs `f` on a worker thread while pumping the main `CFRunLoop` (see
/// [`crate::run_with_event_loop`]). Must be called on the main thread; on any other thread it
/// simply calls `f` (Bluetooth operations then fail with
/// [`TransportError::EventLoopNotRunning`]). In host run loop mode ([`use_host_run_loop`]) it
/// also just calls `f`, on the calling thread.
///
/// After `f` returns, channels it left open are closed and the run loop is drained ~250 ms
/// (PROTOCOL.md §2.2 step 9). A panic in `f` is resumed on the calling thread.
pub fn run_main_loop<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if MainThreadMarker::new().is_none() || runtime::is_running() {
        return f();
    }
    let (tx, rx) = runtime::install();
    // `thread::spawn` only fails when the OS is out of threads/memory.
    let handle = std::thread::spawn(move || {
        let _quit = QuitOnDrop(tx);
        f()
    });
    runtime::run(&rx);
    match handle.join() {
        Ok(r) => r,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Switches the backend to host run loop mode, for applications whose main thread runs its own
/// run loop (e.g. `NSApplication` in a GUI toolkit) instead of [`run_main_loop`].
///
/// Every IOBluetooth call is then sent to the main dispatch queue and runs there between the
/// host's run-loop iterations; delegate callbacks arrive the same way. The crate never blocks or
/// pumps the main thread in this mode, so:
/// - Bluetooth operations (open, write, read, close, [`inquiry`]) must be made from a worker
///   thread (from the main thread they fail with [`TransportError::EventLoopNotRunning`] instead
///   of deadlocking);
/// - the host must keep running its main run loop (in the default or common modes) while they
///   run;
/// - [`crate::run_with_event_loop`] / [`run_main_loop`] just call their closure.
///
/// Call it once at startup, before the first Bluetooth operation; it cannot be turned off. If a
/// [`run_main_loop`] is already running, it keeps serving jobs until it returns.
pub fn use_host_run_loop() {
    runtime::enable_host();
}

/// `true` once [`use_host_run_loop`] was called.
#[must_use]
pub fn host_run_loop_enabled() -> bool {
    runtime::host_enabled()
}

/// Host run loop mode: closes every channel and inquiry left open (PROTOCOL.md §2.2 step 9),
/// e.g. when the application quits. [`run_main_loop`] does this itself when its closure returns.
///
/// From a worker thread it waits for the close callbacks and the drain (~750 ms) while the host
/// keeps pumping. From the main thread it closes at once without waiting (it never blocks the
/// host's run loop), so callbacks still in flight are delivered only if the run loop keeps
/// running for a moment. A no-op outside host run loop mode.
pub fn shutdown_host_run_loop() {
    runtime::host_shutdown();
}

/// Runs `f` on the main thread, where IOBluetooth objects live, and waits for its result:
/// through [`run_main_loop`] when it runs, or the main dispatch queue in host run loop mode. For
/// IOBluetooth work this crate does not wrap (e.g. `IOBluetoothDevicePair`), so that it is
/// serialised with the backend's own calls.
///
/// # Errors
/// [`TransportError::EventLoopNotRunning`] when neither loop is available, or when called on the
/// main thread (that would deadlock).
pub fn run_on_main_thread<R, F>(f: F) -> Result<R, TransportError>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    runtime::call(move |_| f())
}

/// A paired Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// Address.
    pub addr: BtAddr,
    /// Device name, e.g. `PT-P710BTxxxx`.
    pub name: String,
    /// Class of device (0x042680 on PT-P710BT).
    pub class_of_device: u32,
}

/// IOBluetooth device object for `addr`.
fn device_for(addr: BtAddr) -> Option<Retained<IOBluetoothDevice>> {
    let s = NSString::from_str(&addr.to_dashed_lower());
    // SAFETY: plain class-method call with a valid NSString.
    unsafe { IOBluetoothDevice::deviceWithAddressString(Some(&s)) }
}

/// Snapshot of an `IOBluetoothDevice` (name may be unknown → address string).
fn describe(dev: &IOBluetoothDevice) -> Option<PairedDevice> {
    // SAFETY: getters on a live IOBluetoothDevice.
    let (addr, name, cod) = unsafe {
        #[allow(deprecated)] // `getName` returns an Option; `name` assumes non-nil.
        let name = dev.getName().map(|s| s.to_string());
        (
            dev.addressString().map(|s| s.to_string()),
            name,
            dev.classOfDevice(),
        )
    };
    let addr: BtAddr = addr?.parse().ok()?;
    Some(PairedDevice {
        addr,
        name: name.unwrap_or_else(|| addr.to_string()),
        class_of_device: cod,
    })
}

fn paired_devices_now() -> Vec<PairedDevice> {
    // SAFETY: plain class-method call.
    let Some(arr) = (unsafe { IOBluetoothDevice::pairedDevices() }) else {
        return Vec::new();
    };
    arr.to_vec()
        .into_iter()
        .filter_map(|o| o.downcast::<IOBluetoothDevice>().ok())
        .filter_map(|d| describe(&d))
        .collect()
}

/// Paired devices (`+[IOBluetoothDevice pairedDevices]`, no CoD filter; PROTOCOL.md §2.2
/// step 1). Routed through the main thread when [`run_main_loop`] runs or in host run loop mode
/// (unless called on the main thread); otherwise the (synchronous) list is read on the calling
/// thread.
///
/// # Errors
/// [`TransportError::EventLoopNotRunning`] if the loop stopped while the call was queued.
/// Bluetooth being off or access being denied (TCC) yields an empty list.
pub fn paired_devices() -> Result<Vec<PairedDevice>, TransportError> {
    if runtime::is_running() && MainThreadMarker::new().is_none() {
        runtime::call(|_| paired_devices_now())
    } else {
        Ok(paired_devices_now())
    }
}

/// Runs a classic Bluetooth inquiry for `duration` (clamped to 1–48 s) and returns every
/// device found (no CoD filter). Requires [`run_main_loop`] or [`use_host_run_loop`] (then call
/// it from a worker thread).
///
/// Inquiry results without a cached SDP record are not queried here; the open path does SDP.
///
/// # Errors
/// [`TransportError::EventLoopNotRunning`] outside [`run_main_loop`] and host run loop mode;
/// [`TransportError::Bluetooth`] if the inquiry cannot start.
pub fn inquiry(duration: Duration) -> Result<Vec<PairedDevice>, TransportError> {
    let secs = u8::try_from(duration.as_secs().clamp(1, 48)).unwrap_or(48);
    let (tx, rx) = mpsc::channel::<c_int>();
    let started = runtime::call(move |state| {
        let delegate = delegates::InquiryDelegate::new(tx);
        // SAFETY: IOBluetooth calls on the main thread; the delegate is kept alive below.
        let inq = unsafe { IOBluetoothDeviceInquiry::inquiryWithDelegate(Some(&delegate)) };
        state.keep_alive.push(Retained::into_super(delegate));
        let Some(inq) = inq else {
            return Err(K_IO_RETURN_NOT_OPEN);
        };
        // SAFETY: as above.
        let st = unsafe {
            inq.setInquiryLength(secs);
            inq.setUpdateNewDeviceNames(true);
            inq.start()
        };
        if st != K_IO_RETURN_SUCCESS {
            // SAFETY: as above.
            unsafe { inq.setDelegate(None) };
            return Err(st);
        }
        let id = state.next_handle();
        state.inquiries.insert(id, inq);
        Ok(id)
    })?;
    let id = started.map_err(|code| TransportError::Bluetooth {
        op: "IOBluetoothDeviceInquiry start",
        code: io_return_code(code),
    })?;
    // Name updates continue after the inquiry window; allow some slack.
    let deadline = Instant::now() + Duration::from_secs(u64::from(secs) + 15);
    let _ = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    runtime::call(move |state| {
        let Some(inq) = state.inquiries.remove(&id) else {
            return Vec::new();
        };
        // SAFETY: IOBluetooth calls on the main thread with a live inquiry.
        unsafe {
            inq.stop();
            inq.setDelegate(None);
        }
        // SAFETY: as above.
        let Some(arr) = (unsafe { inq.foundDevices() }) else {
            return Vec::new();
        };
        arr.to_vec()
            .into_iter()
            .filter_map(|o| o.downcast::<IOBluetoothDevice>().ok())
            .filter_map(|d| describe(&d))
            .collect()
    })
}
