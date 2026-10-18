//! Endpoint addressing and opening.
//!
//! # Contract
//! Grammar (case-insensitive scheme):
//! ```text
//! serial:<path>                     /dev/cu.PT-P710BTxxxx, COM5, /dev/rfcomm0
//! <path starting with / or COM>     shorthand for serial:
//! bt:<XX:XX:XX:XX:XX:XX>[@<ch>]     Bluetooth RFCOMM (macOS IOBluetooth / Linux socket)
//! usb:[<vid>:<pid>]                 first matching Brother USB printer (vid defaults to 04f9)
//! tcp:<host>[:<port>]               raw TCP, port defaults to 9100
//! virtual:[<width_mm>]              in-process VirtualPrinter (PT-P710BT, default 24 mm)
//! ```
//! `Display` prints the canonical form, and `parse(display(e)) == e`.
//! Never log or persist real Bluetooth addresses in the repository (examples use `XX`).
//!
//! Accepted variants beyond the canonical grammar:
//! - `usb:<pid>` (vendor 04f9) and `usb:<vid>:` (any product of that vendor); hex digits with
//!   an optional `0x` prefix.
//! - `tcp:[<ipv6>]:<port>` and a bare IPv6 address without a port.
//! - Bluetooth addresses with `:` or `-` separators, or 12 hex digits without separators.
//! - RFCOMM channels 1–30.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use crate::{Transport, TransportError};

pub mod rfcomm;

/// Brother USB vendor ID (PROTOCOL.md §2.8).
const BROTHER_VID: u16 = 0x04F9;
/// Default raw TCP port (PROTOCOL.md §2.9).
const DEFAULT_TCP_PORT: u16 = 9100;
/// Default virtual tape width.
const DEFAULT_VIRTUAL_WIDTH: u8 = 24;

fn invalid(input: &str, reason: &'static str) -> TransportError {
    TransportError::InvalidEndpoint {
        input: input.to_owned(),
        reason,
    }
}

/// A Bluetooth device address, most-significant byte first as printed (`XX:XX:…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BtAddr(pub [u8; 6]);

impl BtAddr {
    /// IOBluetooth's string form: lower-case, `-`-separated (`xx-xx-xx-xx-xx-xx`).
    #[must_use]
    pub fn to_dashed_lower(&self) -> String {
        let b = self.0;
        format!(
            "{:02x}-{:02x}-{:02x}-{:02x}-{:02x}-{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

impl FromStr for BtAddr {
    type Err = TransportError;

    /// Parses `XX:XX:XX:XX:XX:XX`, `XX-XX-XX-XX-XX-XX` (any case) or `XXXXXXXXXXXX`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let groups: Vec<&str> = if t.contains(':') {
            t.split(':').collect()
        } else if t.contains('-') {
            t.split('-').collect()
        } else {
            if t.len() != 12 || !t.is_ascii() {
                return Err(invalid(s, "Bluetooth address must have 6 bytes"));
            }
            (0..6).filter_map(|i| t.get(i * 2..i * 2 + 2)).collect()
        };
        if groups.len() != 6 {
            return Err(invalid(s, "Bluetooth address must have 6 bytes"));
        }
        let mut out = [0u8; 6];
        for (slot, g) in out.iter_mut().zip(&groups) {
            if g.len() != 2 || !g.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid(s, "Bluetooth address bytes must be two hex digits"));
            }
            *slot = u8::from_str_radix(g, 16)
                .map_err(|_| invalid(s, "Bluetooth address bytes must be two hex digits"))?;
        }
        Ok(Self(out))
    }
}

impl fmt::Display for BtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

/// Where a printer is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Endpoint {
    /// OS serial device node.
    Serial {
        /// Path or port name.
        path: String,
    },
    /// Bluetooth Classic RFCOMM.
    Bluetooth {
        /// Device address.
        addr: BtAddr,
        /// Fixed channel; `None` = SDP lookup, then the §2.1 fallback order.
        channel: Option<u8>,
    },
    /// USB printer.
    Usb {
        /// Vendor ID (0x04F9).
        vendor_id: u16,
        /// Product ID; `None` = first Brother printer-class device.
        product_id: Option<u16>,
    },
    /// Raw TCP.
    Tcp {
        /// Host name or address.
        host: String,
        /// Port (9100).
        port: u16,
    },
    /// In-process virtual printer.
    Virtual {
        /// Loaded tape width (media width byte).
        width_mm: u8,
    },
}

/// `true` for `COM<digits>` (any case).
fn is_com_port(s: &str) -> bool {
    s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("COM"))
        && s.len() > 3
        && s.get(3..)
            .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
}

fn parse_hex_u16(s: &str) -> Option<u16> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

fn parse_bt(input: &str, rest: &str) -> Result<Endpoint, TransportError> {
    let (addr, channel) = match rest.split_once('@') {
        Some((a, ch)) => {
            let ch: u8 = ch
                .trim()
                .parse()
                .map_err(|_| invalid(input, "RFCOMM channel must be a number 1-30"))?;
            if !(1..=30).contains(&ch) {
                return Err(invalid(input, "RFCOMM channel must be a number 1-30"));
            }
            (a, Some(ch))
        }
        None => (rest, None),
    };
    let addr = addr.parse::<BtAddr>().map_err(|e| match e {
        TransportError::InvalidEndpoint { reason, .. } => invalid(input, reason),
        other => other,
    })?;
    Ok(Endpoint::Bluetooth { addr, channel })
}

fn parse_usb(input: &str, rest: &str) -> Result<Endpoint, TransportError> {
    let rest = rest.trim();
    let bad = || invalid(input, "USB ids must be 1-4 hex digits (usb:[<vid>:<pid>])");
    let (vendor_id, product_id) = if rest.is_empty() {
        (BROTHER_VID, None)
    } else if let Some((vid, pid)) = rest.split_once(':') {
        let vid = parse_hex_u16(vid).ok_or_else(bad)?;
        let pid = if pid.is_empty() {
            None
        } else {
            Some(parse_hex_u16(pid).ok_or_else(bad)?)
        };
        (vid, pid)
    } else {
        (BROTHER_VID, Some(parse_hex_u16(rest).ok_or_else(bad)?))
    };
    Ok(Endpoint::Usb {
        vendor_id,
        product_id,
    })
}

fn parse_port(input: &str, s: &str) -> Result<u16, TransportError> {
    match s.parse::<u16>() {
        Ok(p) if p != 0 => Ok(p),
        _ => Err(invalid(input, "TCP port must be 1-65535")),
    }
}

fn parse_tcp(input: &str, rest: &str) -> Result<Endpoint, TransportError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(invalid(input, "missing host"));
    }
    let (host, port) = if let Some(after) = rest.strip_prefix('[') {
        let (host, tail) = after
            .split_once(']')
            .ok_or_else(|| invalid(input, "unterminated '[' in IPv6 host"))?;
        let port = match tail {
            "" => DEFAULT_TCP_PORT,
            t => {
                let p = t
                    .strip_prefix(':')
                    .ok_or_else(|| invalid(input, "expected ':<port>' after ']'"))?;
                parse_port(input, p)?
            }
        };
        (host, port)
    } else if rest.matches(':').count() > 1 {
        // Bare IPv6 address without a port.
        (rest, DEFAULT_TCP_PORT)
    } else if let Some((h, p)) = rest.split_once(':') {
        (h, parse_port(input, p)?)
    } else {
        (rest, DEFAULT_TCP_PORT)
    };
    if host.is_empty() {
        return Err(invalid(input, "missing host"));
    }
    Ok(Endpoint::Tcp {
        host: host.to_owned(),
        port,
    })
}

fn parse_virtual(input: &str, rest: &str) -> Result<Endpoint, TransportError> {
    let rest = rest.trim();
    let rest = rest.strip_suffix("mm").unwrap_or(rest);
    let width_mm = if rest.is_empty() {
        DEFAULT_VIRTUAL_WIDTH
    } else {
        match rest.parse::<u8>() {
            Ok(w) if w != 0 => w,
            _ => return Err(invalid(input, "virtual tape width must be 1-255 mm")),
        }
    };
    Ok(Endpoint::Virtual { width_mm })
}

impl FromStr for Endpoint {
    type Err = TransportError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t.is_empty() {
            return Err(invalid(s, "empty endpoint"));
        }
        if t.starts_with('/') || t.starts_with(r"\\") || is_com_port(t) {
            return Ok(Self::Serial { path: t.to_owned() });
        }
        let Some((scheme, rest)) = t.split_once(':') else {
            return Err(invalid(
                s,
                "expected <scheme>:<address> (serial, bt, usb, tcp, virtual) or a device path",
            ));
        };
        match scheme.to_ascii_lowercase().as_str() {
            "serial" => {
                let path = rest.trim();
                if path.is_empty() {
                    Err(invalid(s, "missing serial device path"))
                } else {
                    Ok(Self::Serial {
                        path: path.to_owned(),
                    })
                }
            }
            "bt" | "bluetooth" | "rfcomm" => parse_bt(s, rest),
            "usb" => parse_usb(s, rest),
            "tcp" => parse_tcp(s, rest),
            "virtual" => parse_virtual(s, rest),
            _ => Err(invalid(
                s,
                "unknown scheme (expected serial, bt, usb, tcp or virtual)",
            )),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serial { path } => write!(f, "serial:{path}"),
            Self::Bluetooth { addr, channel } => match channel {
                Some(ch) => write!(f, "bt:{addr}@{ch}"),
                None => write!(f, "bt:{addr}"),
            },
            Self::Usb {
                vendor_id,
                product_id,
            } => match product_id {
                Some(pid) => write!(f, "usb:{vendor_id:04x}:{pid:04x}"),
                None if *vendor_id == BROTHER_VID => write!(f, "usb:"),
                None => write!(f, "usb:{vendor_id:04x}:"),
            },
            Self::Tcp { host, port } => {
                if host.contains(':') {
                    write!(f, "tcp:[{host}]:{port}")
                } else {
                    write!(f, "tcp:{host}:{port}")
                }
            }
            Self::Virtual { width_mm } => write!(f, "virtual:{width_mm}"),
        }
    }
}

/// Options for [`open`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Connect/open timeout (default 10 s; SDP + RFCOMM open on macOS can take that long).
    pub connect_timeout: Duration,
    /// Settle time after connect (default 500 ms, PROTOCOL.md §2.1 "Timing").
    pub settle: Duration,
    /// Model hint (from discovery / `--model`) for the RFCOMM channel fallback order; also the
    /// simulated model of a `virtual:` endpoint (default PT-P710BT).
    pub model_hint: Option<&'static ptouch::ModelProfile>,
    /// Write timeout (default 60 s; USB and RFCOMM back-pressure for the print duration).
    /// Used by USB, TCP, Linux RFCOMM (`SO_SNDTIMEO`) and macOS RFCOMM (per write
    /// completion).
    pub write_timeout: Duration,
    /// Wait after closing a serial or Bluetooth link before it may be reopened (default
    /// 500 ms, PROTOCOL.md §2.1 "Timing"; a UI adds the rest of the 3 s after an error).
    pub close_wait: Duration,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            settle: Duration::from_millis(500),
            model_hint: None,
            write_timeout: Duration::from_secs(60),
            close_wait: Duration::from_millis(500),
        }
    }
}

/// Opens a transport for `endpoint`.
///
/// Serial and Bluetooth links wait [`OpenOptions::settle`] after connecting (PROTOCOL.md §2.1
/// "Timing"). Bluetooth links without a fixed channel are only returned after a candidate
/// channel answered a status request (see [`rfcomm`]).
///
/// On macOS, Bluetooth endpoints require [`crate::run_with_event_loop`] to be running and
/// `open` to be called from inside its closure, or host run loop mode
/// (`macos::use_host_run_loop`) and a call from a worker thread.
///
/// # Errors
/// [`TransportError::Unsupported`] if the backend is not compiled in / not on this OS;
/// otherwise the backend's open error.
pub fn open(endpoint: &Endpoint, opts: &OpenOptions) -> Result<Box<dyn Transport>, TransportError> {
    match endpoint {
        Endpoint::Serial { path } => {
            let mut t = crate::serial::SerialTransport::open(path)?;
            t.set_close_wait(opts.close_wait);
            std::thread::sleep(opts.settle);
            Ok(Box::new(t))
        }
        Endpoint::Bluetooth { addr, channel } => open_bluetooth(*addr, *channel, opts),
        Endpoint::Usb {
            vendor_id,
            product_id,
        } => open_usb(*vendor_id, *product_id, opts),
        Endpoint::Tcp { host, port } => {
            let mut t = crate::tcp::TcpTransport::connect(host, *port, opts.connect_timeout)?;
            t.set_write_timeout(opts.write_timeout)?;
            Ok(Box::new(t))
        }
        Endpoint::Virtual { width_mm } => open_virtual(*width_mm, opts.model_hint),
    }
}

#[cfg(all(target_os = "macos", feature = "macos-rfcomm"))]
fn open_bluetooth(
    addr: BtAddr,
    channel: Option<u8>,
    opts: &OpenOptions,
) -> Result<Box<dyn Transport>, TransportError> {
    Ok(Box::new(crate::macos::rfcomm::MacRfcommTransport::connect(
        addr, channel, opts,
    )?))
}

#[cfg(all(target_os = "linux", feature = "linux-rfcomm"))]
fn open_bluetooth(
    addr: BtAddr,
    channel: Option<u8>,
    opts: &OpenOptions,
) -> Result<Box<dyn Transport>, TransportError> {
    Ok(Box::new(
        crate::linux::rfcomm::LinuxRfcommTransport::connect(addr, channel, opts)?,
    ))
}

#[cfg(not(any(
    all(target_os = "macos", feature = "macos-rfcomm"),
    all(target_os = "linux", feature = "linux-rfcomm")
)))]
fn open_bluetooth(
    _addr: BtAddr,
    _channel: Option<u8>,
    _opts: &OpenOptions,
) -> Result<Box<dyn Transport>, TransportError> {
    Err(TransportError::Unsupported(
        "Bluetooth RFCOMM (use the OS serial port, e.g. serial:COM5)",
    ))
}

#[cfg(feature = "usb")]
fn open_usb(
    vendor_id: u16,
    product_id: Option<u16>,
    opts: &OpenOptions,
) -> Result<Box<dyn Transport>, TransportError> {
    Ok(Box::new(crate::usb::UsbTransport::open(
        vendor_id,
        product_id,
        opts.write_timeout,
    )?))
}

#[cfg(not(feature = "usb"))]
fn open_usb(
    _vendor_id: u16,
    _product_id: Option<u16>,
    _opts: &OpenOptions,
) -> Result<Box<dyn Transport>, TransportError> {
    Err(TransportError::Unsupported("USB (feature `usb`)"))
}

#[cfg(feature = "virtual")]
fn open_virtual(
    width_mm: u8,
    model_hint: Option<&'static ptouch::ModelProfile>,
) -> Result<Box<dyn Transport>, TransportError> {
    // The model hint (`--model`) picks the simulated model; PT-P710BT otherwise.
    let profile = match model_hint {
        Some(p) => p,
        None => ptouch::profile(ptouch::Model::PtP710bt)
            .ok_or(TransportError::NotFound(String::from("PT-P710BT profile")))?,
    };
    Ok(Box::new(crate::virtual_transport::VirtualTransport::new(
        profile,
        width_mm,
        ptouch::Behaviour::Normal,
    )?))
}

#[cfg(not(feature = "virtual"))]
fn open_virtual(
    _width_mm: u8,
    _model_hint: Option<&'static ptouch::ModelProfile>,
) -> Result<Box<dyn Transport>, TransportError> {
    Err(TransportError::Unsupported(
        "the virtual printer (feature `virtual`)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(s: &str) -> Endpoint {
        let e: Endpoint = s.parse().unwrap_or_else(|err| panic!("{s}: {err}"));
        let again: Endpoint = e
            .to_string()
            .parse()
            .unwrap_or_else(|err| panic!("{e}: {err}"));
        assert_eq!(e, again, "round trip of {s}");
        e
    }

    #[test]
    fn com_port_detection() {
        assert!(is_com_port("COM5"));
        assert!(is_com_port("com12"));
        assert!(!is_com_port("COM"));
        assert!(!is_com_port("COMX"));
        assert!(!is_com_port("CO"));
    }

    #[test]
    fn usb_forms() {
        assert_eq!(
            rt("usb:"),
            Endpoint::Usb {
                vendor_id: 0x04F9,
                product_id: None
            }
        );
        assert_eq!(
            rt("usb:20af"),
            Endpoint::Usb {
                vendor_id: 0x04F9,
                product_id: Some(0x20AF)
            }
        );
        assert_eq!(
            rt("usb:1234:"),
            Endpoint::Usb {
                vendor_id: 0x1234,
                product_id: None
            }
        );
    }
}
