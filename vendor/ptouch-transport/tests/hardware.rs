//! Hardware smoke tests. **Never run in CI**: they are `#[ignore]`d and only run with
//! `PTOUCH_TEST_DEVICE=<endpoint> cargo test -p ptouch-transport --test hardware -- --ignored`.
//!
//! They only ever send invalidate + `ESC @` + `ESC i S` (no printing, no feeding) and expect a
//! valid 32-byte `80 20 42` status reply. Examples: `serial:/dev/cu.PT-P710BTxxxx`, `usb:`,
//! `bt:XX:XX:XX:XX:XX:XX` (Linux only: on macOS the Bluetooth backend needs the main-thread
//! event loop, which the libtest harness does not provide; use the CLI there).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use ptouch_transport::endpoint::rfcomm::probe_status;
use ptouch_transport::{Endpoint, OpenOptions};

fn device() -> Option<Endpoint> {
    let s = std::env::var("PTOUCH_TEST_DEVICE").ok()?;
    Some(
        s.parse()
            .unwrap_or_else(|e| panic!("PTOUCH_TEST_DEVICE: {e}")),
    )
}

#[test]
#[ignore = "hardware: set PTOUCH_TEST_DEVICE; sends only ESC i S"]
fn status_request_round_trip() {
    let Some(ep) = device() else {
        eprintln!("PTOUCH_TEST_DEVICE not set; skipping");
        return;
    };
    let mut t = ptouch_transport::open(&ep, &OpenOptions::default()).unwrap();
    eprintln!("opened {:?}", t.info());
    let mut ok = false;
    for _ in 0..3 {
        if probe_status(&mut *t, Duration::from_secs(5)).unwrap() {
            ok = true;
            break;
        }
    }
    t.close().unwrap();
    assert!(ok, "no valid status reply (printer asleep? power-cycle it)");
}

#[test]
#[ignore = "hardware: lists devices only"]
fn discover_lists_something() {
    let found = ptouch_transport::discover(&ptouch_transport::DiscoverOptions::default()).unwrap();
    for d in &found {
        eprintln!("{} {:?} {:?}", d.endpoint, d.label, d.model.map(|m| m.name));
    }
}
