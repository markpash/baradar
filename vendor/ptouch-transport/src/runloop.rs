//! Main-thread event loop helper (WP6).
//!
//! # Contract (PROTOCOL.md §2.2 step 6)
//! On macOS with `macos-rfcomm`, IOBluetooth delivers every RFCOMM/inquiry delegate callback
//! on the main dispatch queue, so the main thread must pump its `CFRunLoop` while the
//! application blocks elsewhere. [`run_with_event_loop`] must be called **from the main
//! thread**: it runs `f` on a worker thread and pumps the main run loop until `f` returns,
//! then drains ~250 ms so `bluetoothd` can tear links down before the process exits
//! (§2.2 step 9). On every other platform/build it just calls `f` on the current thread.
//!
//! GUI applications whose main thread runs its own run loop call
//! `ptouch_transport::macos::use_host_run_loop()` at startup instead; [`run_with_event_loop`]
//! then just calls `f`.

/// Runs `f` while the platform event loop (if any) is serviced on the calling (main) thread.
/// Panics inside `f` are propagated (resumed) on the calling thread.
pub fn run_with_event_loop<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    #[cfg(all(target_os = "macos", feature = "macos-rfcomm"))]
    {
        crate::macos::run_main_loop(f)
    }
    #[cfg(not(all(target_os = "macos", feature = "macos-rfcomm")))]
    {
        f()
    }
}
