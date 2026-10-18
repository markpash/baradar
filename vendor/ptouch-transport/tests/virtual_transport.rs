//! `VirtualTransport` (feature `virtual`): fragmentation and a `Session` round trip.

#![cfg(feature = "virtual")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use ptouch::{Behaviour, Event, Model, Session, SessionConfig, SessionState};
use ptouch_transport::virtual_transport::VirtualTransport;
use ptouch_transport::{Endpoint, OpenOptions, Transport, TransportError};

const FIXTURE: &str =
    include_str!("fixtures/p710bt_24mm_laminated_idle.hex");

fn fixture() -> Vec<u8> {
    FIXTURE
        .split_whitespace()
        .map(|h| u8::from_str_radix(h, 16).unwrap())
        .collect()
}

fn p710bt() -> &'static ptouch::ModelProfile {
    ptouch::profile(Model::PtP710bt).unwrap()
}

#[test]
fn status_reply_is_fragmented_7_25() {
    let mut t = VirtualTransport::new(p710bt(), 24, Behaviour::Normal)
        .unwrap()
        .with_fragmentation(vec![7, 25]);
    t.write_all(&[0x1B, 0x40, 0x1B, 0x69, 0x53]).unwrap();
    let mut buf = [0u8; 64];
    let mut chunks = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while chunks.iter().map(Vec::len).sum::<usize>() < 32 && Instant::now() < deadline {
        let n = t.read(&mut buf, Duration::from_millis(200)).unwrap();
        if n > 0 {
            chunks.push(buf[..n].to_vec());
        }
    }
    assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [7, 25]);
    assert_eq!(chunks.concat(), fixture());
    // Nothing else pending: timeout → Ok(0).
    assert_eq!(t.read(&mut buf, Duration::from_millis(20)).unwrap(), 0);
    t.close().unwrap();
    t.close().unwrap();
    assert!(matches!(t.write_all(b"x"), Err(TransportError::Closed)));
}

#[test]
fn unknown_width_is_rejected() {
    let r = VirtualTransport::new(p710bt(), 7, Behaviour::Normal);
    assert!(matches!(r, Err(TransportError::Protocol(_))), "{r:?}");
}

#[test]
fn open_virtual_endpoint() {
    let ep: Endpoint = "virtual:12".parse().unwrap();
    let mut t = ptouch_transport::open(&ep, &OpenOptions::default()).unwrap();
    t.write_all(&[0x1B, 0x69, 0x53]).unwrap();
    let mut buf = [0u8; 32];
    let mut got = 0;
    while got < 32 {
        let n = t.read(&mut buf[got..], Duration::from_secs(1)).unwrap();
        assert!(n > 0, "virtual printer did not answer");
        got += n;
    }
    assert_eq!(buf[10], 12, "media width byte");
}

/// Drives a `Session` handshake over the virtual transport exactly like the CLI driver loop.
#[test]
fn session_handshake_over_fragmented_link() {
    let mut t = VirtualTransport::new(p710bt(), 24, Behaviour::Normal)
        .unwrap()
        .with_fragmentation(vec![7, 25]);
    let mut s = Session::new(None, SessionConfig::default());
    let start = Instant::now();
    let now = || u64::try_from(start.elapsed().as_millis()).unwrap();
    s.connect(now());
    let mut ready = None;
    let mut buf = [0u8; 64];
    while ready.is_none() && start.elapsed() < Duration::from_secs(10) {
        while let Some(out) = s.poll_transmit() {
            t.write_all(&out).unwrap();
        }
        while let Some(ev) = s.poll_event() {
            match ev {
                Event::Ready(st) => ready = Some(st),
                Event::Failed { error, .. } => panic!("handshake failed: {error}"),
                _ => {}
            }
        }
        if ready.is_some() {
            break;
        }
        let wait = s
            .poll_timeout()
            .map_or(Duration::from_millis(100), |d| {
                Duration::from_millis(d.saturating_sub(now()))
            })
            .min(Duration::from_millis(100));
        let n = t.read(&mut buf, wait).unwrap();
        if n > 0 {
            s.handle_input(&buf[..n], now());
        } else {
            s.handle_timeout(now());
        }
    }
    let st = ready.expect("session became ready");
    assert_eq!(st.media_width_mm, 24);
    assert_eq!(s.state(), SessionState::Ready);
}

/// Review regression: `read` with `Duration::MAX` ("wait for data") must not panic on
/// `Instant + Duration` overflow.
#[test]
fn read_accepts_duration_max() {
    let mut t = VirtualTransport::new(p710bt(), 24, Behaviour::Normal).unwrap();
    t.write_all(&[0x1B, 0x40, 0x1B, 0x69, 0x53]).unwrap();
    let mut buf = [0u8; 64];
    let mut got = 0;
    while got < 32 {
        got += t.read(&mut buf[got..], Duration::MAX).unwrap();
    }
    assert_eq!(&buf[..32], fixture().as_slice());
}
