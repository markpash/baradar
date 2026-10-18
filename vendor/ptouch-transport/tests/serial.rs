//! `SerialTransport` over a pseudo-terminal pair (unix only, no hardware).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serialport::{SerialPort, TTYPort};

use ptouch_transport::serial::SerialTransport;
use ptouch_transport::{Transport, TransportError, TransportKind};

fn pair() -> (TTYPort, SerialTransport) {
    let (mut master, slave) = TTYPort::pair().expect("pty pair");
    master.set_timeout(Duration::from_secs(5)).unwrap();
    let t = SerialTransport::from_port(Box::new(slave), "pty".into());
    (master, t)
}

#[test]
fn write_reaches_the_other_end() {
    let (mut master, mut t) = pair();
    assert_eq!(t.info().kind, TransportKind::Serial);
    // Larger than one 4096-byte chunk; read concurrently so the pty buffer never fills.
    let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let expected = data.clone();
    let reader = std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut buf = [0u8; 1024];
        while got.len() < expected.len() {
            let n = master.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, expected);
        master
    });
    t.write_all(&data).unwrap();
    let _master = reader.join().unwrap();
}

#[test]
fn read_timeout_is_ok_zero_and_data_arrives() {
    let (mut master, mut t) = pair();
    let mut buf = [0u8; 64];
    let start = Instant::now();
    assert_eq!(t.read(&mut buf, Duration::from_millis(100)).unwrap(), 0);
    assert!(start.elapsed() >= Duration::from_millis(90));

    let frame = [0x80u8, 0x20, 0x42, 0x30, 0x76];
    master.write_all(&frame).unwrap();
    master.flush().unwrap();
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while got.len() < frame.len() && Instant::now() < deadline {
        let n = t.read(&mut buf, Duration::from_millis(500)).unwrap();
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, frame);
}

#[test]
fn close_is_idempotent() {
    let (_master, mut t) = pair();
    t.close().unwrap();
    t.close().unwrap();
    let mut buf = [0u8; 4];
    assert!(matches!(t.write_all(b"x"), Err(TransportError::Closed)));
    assert!(matches!(
        t.read(&mut buf, Duration::from_millis(1)),
        Err(TransportError::Closed)
    ));
}

#[test]
fn open_missing_port_fails() {
    let r = SerialTransport::open("/dev/ptouch-does-not-exist");
    assert!(matches!(r, Err(TransportError::Serial(_))), "{r:?}");
}

/// Review regression: a node that accepts writes but never drains them (half-open
/// `/dev/cu.*`, PROTOCOL.md §2.2 step 12) must not hang `write_all` (no `tcdrain`) nor
/// `close` (bounded drain, then the output is discarded).
#[test]
fn write_and_close_do_not_block_on_an_undrained_node() {
    let (_master, mut t) = pair(); // nobody reads the master side
    let probe: Vec<u8> = [vec![0u8; 100], vec![0x1B, 0x40, 0x1B, 0x69, 0x53]].concat();
    let start = Instant::now();
    t.write_all(&probe).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "write_all returned"
    );
    let start = Instant::now();
    t.close().unwrap();
    assert!(
        start.elapsed() < ptouch_transport::serial::CLOSE_DRAIN + Duration::from_secs(1),
        "close is bounded"
    );
}

/// Review regression: `read` accepts any timeout (no `Instant + Duration` overflow).
#[test]
fn read_accepts_duration_max() {
    let (mut master, mut t) = pair();
    master.write_all(&[0x80, 0x20, 0x42]).unwrap();
    let mut buf = [0u8; 8];
    assert!(t.read(&mut buf, Duration::MAX).unwrap() > 0);
}
