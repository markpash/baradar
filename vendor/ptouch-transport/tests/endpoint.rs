//! Endpoint / BtAddr parsing and display (no hardware).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ptouch_transport::{BtAddr, Endpoint, TransportError};

fn parse(s: &str) -> Endpoint {
    s.parse().unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

/// Parses, displays, re-parses; asserts the canonical display and the round trip.
fn round_trip(s: &str, canonical: &str) -> Endpoint {
    let e = parse(s);
    assert_eq!(e.to_string(), canonical, "display of {s:?}");
    assert_eq!(parse(&e.to_string()), e, "round trip of {s:?}");
    e
}

fn reason(s: &str) -> &'static str {
    match s.parse::<Endpoint>() {
        Err(TransportError::InvalidEndpoint { input, reason }) => {
            assert_eq!(input, s);
            reason
        }
        other => panic!("{s:?}: expected InvalidEndpoint, got {other:?}"),
    }
}

const ADDR: BtAddr = BtAddr([0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45]);

#[test]
fn bt_addr_parsing() {
    for s in [
        "AB:CD:EF:01:23:45",
        "ab:cd:ef:01:23:45",
        "aB:Cd:eF:01:23:45",
        "ab-cd-ef-01-23-45",
        "ABCDEF012345",
        "  AB:CD:EF:01:23:45 ",
    ] {
        assert_eq!(s.parse::<BtAddr>().unwrap(), ADDR, "{s:?}");
    }
    assert_eq!(ADDR.to_string(), "AB:CD:EF:01:23:45");
    assert_eq!(ADDR.to_dashed_lower(), "ab-cd-ef-01-23-45");
    for bad in [
        "",
        "AB:CD:EF:01:23",
        "AB:CD:EF:01:23:45:67",
        "AB:CD:EF:01:23:4",
        "AB:CD:EF:01:23:456",
        "AB:CD:EF:01:23:GG",
        "ABCDEF01234",
        "ABCDEF0123456",
        "AB:CD-EF:01:23:45",
        "+B:CD:EF:01:23:45",
    ] {
        assert!(
            matches!(
                bad.parse::<BtAddr>(),
                Err(TransportError::InvalidEndpoint { .. })
            ),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn serial_endpoints() {
    let e = round_trip(
        "serial:/dev/cu.PT-P710BTxxxx",
        "serial:/dev/cu.PT-P710BTxxxx",
    );
    assert_eq!(
        e,
        Endpoint::Serial {
            path: "/dev/cu.PT-P710BTxxxx".into()
        }
    );
    round_trip("/dev/rfcomm0", "serial:/dev/rfcomm0");
    round_trip("COM5", "serial:COM5");
    round_trip("com12", "serial:com12");
    round_trip("SERIAL:COM5", "serial:COM5");
    round_trip(r"\\.\COM10", r"serial:\\.\COM10");
    assert_eq!(reason("serial:"), "missing serial device path");
}

#[test]
fn bluetooth_endpoints() {
    let e = round_trip("bt:ab:cd:ef:01:23:45", "bt:AB:CD:EF:01:23:45");
    assert_eq!(
        e,
        Endpoint::Bluetooth {
            addr: ADDR,
            channel: None
        }
    );
    let e = round_trip("BT:AB-CD-EF-01-23-45@2", "bt:AB:CD:EF:01:23:45@2");
    assert_eq!(
        e,
        Endpoint::Bluetooth {
            addr: ADDR,
            channel: Some(2)
        }
    );
    round_trip("bt:ABCDEF012345@30", "bt:AB:CD:EF:01:23:45@30");
    assert_eq!(
        reason("bt:AB:CD:EF:01:23:45@0"),
        "RFCOMM channel must be a number 1-30"
    );
    assert_eq!(
        reason("bt:AB:CD:EF:01:23:45@31"),
        "RFCOMM channel must be a number 1-30"
    );
    assert_eq!(
        reason("bt:AB:CD:EF:01:23:45@x"),
        "RFCOMM channel must be a number 1-30"
    );
    assert_eq!(
        reason("bt:AB:CD:EF:01:23"),
        "Bluetooth address must have 6 bytes"
    );
    assert_eq!(reason("bt:"), "Bluetooth address must have 6 bytes");
}

#[test]
fn usb_endpoints() {
    assert_eq!(
        round_trip("usb:", "usb:"),
        Endpoint::Usb {
            vendor_id: 0x04F9,
            product_id: None
        }
    );
    assert_eq!(
        round_trip("usb:04f9:20AF", "usb:04f9:20af"),
        Endpoint::Usb {
            vendor_id: 0x04F9,
            product_id: Some(0x20AF)
        }
    );
    round_trip("usb:0x20af", "usb:04f9:20af");
    round_trip("USB:4f9:0x20af", "usb:04f9:20af");
    round_trip("usb:1234:", "usb:1234:");
    for bad in ["usb:xyz", "usb:12345", "usb:04f9:zz", "usb::20af"] {
        assert_eq!(
            reason(bad),
            "USB ids must be 1-4 hex digits (usb:[<vid>:<pid>])",
            "{bad:?}"
        );
    }
}

#[test]
fn tcp_endpoints() {
    assert_eq!(
        round_trip("tcp:192.0.2.10", "tcp:192.0.2.10:9100"),
        Endpoint::Tcp {
            host: "192.0.2.10".into(),
            port: 9100
        }
    );
    round_trip("tcp:printer.local:9101", "tcp:printer.local:9101");
    round_trip("tcp:[2001:db8::1]:9100", "tcp:[2001:db8::1]:9100");
    round_trip("tcp:[2001:db8::1]", "tcp:[2001:db8::1]:9100");
    round_trip("tcp:2001:db8::1", "tcp:[2001:db8::1]:9100");
    assert_eq!(reason("tcp:"), "missing host");
    assert_eq!(reason("tcp::9100"), "missing host");
    assert_eq!(reason("tcp:host:0"), "TCP port must be 1-65535");
    assert_eq!(reason("tcp:host:70000"), "TCP port must be 1-65535");
    assert_eq!(reason("tcp:host:abc"), "TCP port must be 1-65535");
    assert_eq!(reason("tcp:[::1"), "unterminated '[' in IPv6 host");
    assert_eq!(reason("tcp:[::1]9100"), "expected ':<port>' after ']'");
}

#[test]
fn virtual_endpoints() {
    assert_eq!(
        round_trip("virtual:", "virtual:24"),
        Endpoint::Virtual { width_mm: 24 }
    );
    round_trip("virtual:12", "virtual:12");
    round_trip("VIRTUAL:9mm", "virtual:9");
    assert_eq!(reason("virtual:0"), "virtual tape width must be 1-255 mm");
    assert_eq!(
        reason("virtual:wide"),
        "virtual tape width must be 1-255 mm"
    );
}

#[test]
fn malformed_endpoints() {
    assert_eq!(reason(""), "empty endpoint");
    assert_eq!(reason("   "), "empty endpoint");
    assert_eq!(
        reason("printer"),
        "expected <scheme>:<address> (serial, bt, usb, tcp, virtual) or a device path"
    );
    assert_eq!(
        reason("ftp:host"),
        "unknown scheme (expected serial, bt, usb, tcp or virtual)"
    );
}
