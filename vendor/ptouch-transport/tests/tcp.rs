//! `TcpTransport` against a local listener (no hardware).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use ptouch_transport::tcp::TcpTransport;
use ptouch_transport::{Endpoint, OpenOptions, Transport, TransportError, TransportKind};

const FIXTURE: &str =
    include_str!("fixtures/p710bt_24mm_laminated_idle.hex");

fn fixture() -> Vec<u8> {
    FIXTURE
        .split_whitespace()
        .map(|h| u8::from_str_radix(h, 16).unwrap())
        .collect()
}

/// A one-shot "printer": reads the request, answers with the fixture, then waits for EOF and
/// returns everything it received.
fn spawn_printer(listener: TcpListener) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        // Wait until the status request arrived, then answer.
        while !got.ends_with(&[0x1B, 0x69, 0x53]) {
            let n = sock.read(&mut buf).unwrap();
            assert!(n > 0, "client closed early");
            got.extend_from_slice(&buf[..n]);
        }
        sock.write_all(&fixture()).unwrap();
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
        }
        got
    })
}

#[test]
fn write_read_timeout_close() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let printer = spawn_printer(listener);

    let mut t = TcpTransport::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
    assert_eq!(t.info().kind, TransportKind::Tcp);

    // Nothing sent yet: a read times out with Ok(0), both with a real and a zero timeout.
    let mut buf = [0u8; 64];
    let start = Instant::now();
    assert_eq!(t.read(&mut buf, Duration::from_millis(100)).unwrap(), 0);
    assert!(start.elapsed() >= Duration::from_millis(90));
    assert_eq!(t.read(&mut buf, Duration::ZERO).unwrap(), 0);

    let mut request = vec![0u8; 100];
    request.extend_from_slice(&[0x1B, 0x40, 0x1B, 0x69, 0x53]);
    let big = vec![0x5Au8; 200_000];
    t.write_all(&big).unwrap();
    t.write_all(&request).unwrap();

    let mut reply = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while reply.len() < 32 && Instant::now() < deadline {
        let n = t.read(&mut buf, Duration::from_millis(500)).unwrap();
        reply.extend_from_slice(&buf[..n]);
    }
    assert_eq!(reply, fixture());

    t.close().unwrap();
    t.close().unwrap(); // idempotent
    assert!(matches!(t.write_all(b"x"), Err(TransportError::Closed)));
    assert!(matches!(
        t.read(&mut buf, Duration::from_millis(10)),
        Err(TransportError::Closed)
    ));

    let received = printer.join().unwrap();
    let mut expected = big;
    expected.extend_from_slice(&request);
    assert_eq!(received, expected);
}

#[test]
fn peer_close_is_closed_not_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (sock, _) = listener.accept().unwrap();
        drop(sock);
    });
    let mut t = TcpTransport::connect("localhost", port, Duration::from_secs(5)).unwrap();
    server.join().unwrap();
    let mut buf = [0u8; 8];
    let r = t.read(&mut buf, Duration::from_secs(2));
    assert!(matches!(r, Err(TransportError::Closed)), "{r:?}");
}

#[test]
fn open_tcp_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let printer = spawn_printer(listener);
    let ep: Endpoint = format!("tcp:127.0.0.1:{port}").parse().unwrap();
    let mut t = ptouch_transport::open(&ep, &OpenOptions::default()).unwrap();
    t.write_all(&[0x1B, 0x69, 0x53]).unwrap();
    let mut buf = [0u8; 64];
    let mut got = 0;
    while got < 32 {
        got += t.read(&mut buf[got..], Duration::from_secs(5)).unwrap();
    }
    assert_eq!(&buf[..32], fixture().as_slice());
    drop(t); // close on drop
    printer.join().unwrap();
}

#[test]
fn connect_refused_is_io_error() {
    // Bind then drop to get a port that is (almost certainly) closed.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let r = TcpTransport::connect("127.0.0.1", port, Duration::from_secs(2));
    assert!(matches!(r, Err(TransportError::Io(_))), "{r:?}");
}

/// Review regression: `read` accepts any timeout, `Duration::MAX` included.
#[test]
fn read_accepts_duration_max() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let printer = spawn_printer(listener);
    let mut t = TcpTransport::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
    t.write_all(&[0x1B, 0x69, 0x53]).unwrap();
    let mut buf = [0u8; 64];
    let mut got = 0;
    while got < 32 {
        got += t.read(&mut buf[got..], Duration::MAX).unwrap();
    }
    assert_eq!(&buf[..32], fixture().as_slice());
    t.close().unwrap();
    printer.join().unwrap();
}
