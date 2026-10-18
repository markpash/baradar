//! Host run loop mode (patch, see PATCHES.md), without Bluetooth hardware and without any
//! IOBluetooth call: the main thread runs its `CFRunLoop` the way `NSApplication` does, and jobs
//! submitted from worker threads through the backend's runtime (`macos::run_on_main_thread`,
//! the same path every IOBluetooth call takes) must run on the main thread.
//!
//! `harness = false`: `main` runs on the process's main thread, which libtest never offers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(unsafe_code)] // one read of a CoreFoundation constant

#[cfg(target_os = "macos")]
fn main() {
    host::run();
    println!("host_run_loop: ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod host {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use objc2::MainThreadMarker;
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
    use ptouch_transport::TransportError;
    use ptouch_transport::macos;

    const WORKERS: usize = 4;
    const JOBS_PER_WORKER: usize = 25;

    /// Runs the main run loop in short slices (like `-[NSApplication run]`) until `done`.
    fn run_host_loop_until(done: &AtomicBool, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !done.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "jobs did not run on the main thread within {limit:?}"
            );
            // SAFETY: reading an immutable CoreFoundation constant.
            let mode = unsafe { kCFRunLoopDefaultMode };
            CFRunLoop::run_in_mode(mode, 0.05, false);
        }
    }

    pub fn run() {
        assert!(
            MainThreadMarker::new().is_some(),
            "harness = false runs main on the main thread"
        );
        let main_id = thread::current().id();

        // Without a loop (private or host), worker submissions are refused at once.
        let before = thread::spawn(|| macos::run_on_main_thread(|| ())).join().unwrap();
        assert!(
            matches!(before, Err(TransportError::EventLoopNotRunning)),
            "{before:?}"
        );

        macos::use_host_run_loop();
        assert!(macos::host_run_loop_enabled());

        // On the main thread a submission would deadlock: refused, not blocking.
        let on_main = macos::run_on_main_thread(|| ());
        assert!(
            matches!(on_main, Err(TransportError::EventLoopNotRunning)),
            "{on_main:?}"
        );
        // run_with_event_loop just calls the closure on the calling thread.
        assert_eq!(
            ptouch_transport::run_with_event_loop(|| thread::current().id()),
            main_id
        );

        // Several workers submit jobs concurrently while the main thread runs its run loop.
        let ran = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let all_done = Arc::new(AtomicBool::new(false));
        let workers: Vec<_> = (0..WORKERS)
            .map(|w| {
                let ran = Arc::clone(&ran);
                let finished = Arc::clone(&finished);
                let all_done = Arc::clone(&all_done);
                thread::spawn(move || {
                    let mut seen = Vec::new();
                    for i in 0..JOBS_PER_WORKER {
                        let ran = Arc::clone(&ran);
                        let r = macos::run_on_main_thread(move || {
                            ran.fetch_add(1, Ordering::SeqCst);
                            (
                                thread::current().id(),
                                MainThreadMarker::new().is_some(),
                                w * 1000 + i,
                            )
                        });
                        seen.push(r);
                    }
                    if finished.fetch_add(1, Ordering::SeqCst) + 1 == WORKERS {
                        // Nothing is open: shutdown from a worker returns after the drain.
                        macos::shutdown_host_run_loop();
                        all_done.store(true, Ordering::SeqCst);
                    }
                    seen
                })
            })
            .collect();

        run_host_loop_until(&all_done, Duration::from_secs(20));

        for (w, handle) in workers.into_iter().enumerate() {
            let seen = handle.join().unwrap();
            assert_eq!(seen.len(), JOBS_PER_WORKER);
            for (i, r) in seen.into_iter().enumerate() {
                let (id, is_main, tag) = r.unwrap();
                assert_eq!(id, main_id, "job ran on {id:?}, not the main thread");
                assert!(is_main);
                assert_eq!(tag, w * 1000 + i, "results arrive in submission order");
            }
        }
        assert_eq!(ran.load(Ordering::SeqCst), WORKERS * JOBS_PER_WORKER);

        // Shutdown on the main thread never blocks.
        let t = Instant::now();
        macos::shutdown_host_run_loop();
        assert!(t.elapsed() < Duration::from_millis(100));
    }
}
