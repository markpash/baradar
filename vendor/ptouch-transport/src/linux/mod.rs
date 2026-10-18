//! Linux BlueZ RFCOMM backend (feature `linux-rfcomm`, Linux only).
//!
//! # Contract (PROTOCOL.md §2.3)
//! - Raw `socket(AF_BLUETOOTH, SOCK_STREAM, BTPROTO_RFCOMM)` + `connect(sockaddr_rc)` via
//!   `libc` (bdaddr byte-reversed), blocking I/O with `poll` for read timeouts.
//! - No SDP in this backend: channel = explicit, else the §2.1 fallback order
//!   (`rfcomm_channel_observed`, 1, 2), each accepted only after a valid status reply.
//! - `EBUSY` on an immediate reconnect: retry for up to 5 s.
//! - Paired-device listing reads BlueZ state only if cheaply available; otherwise discovery
//!   skips Bluetooth on Linux (documented limitation).
//!
//! # Paired devices
//! BlueZ keeps bonded devices under `/var/lib/bluetooth/<adapter>/<device>/info` (an INI file
//! with `Name=` and `Class=` under `[General]`, and a `[LinkKey]` section once bonded). That
//! directory is usually readable by root only; when it is not readable, [`paired_devices`]
//! returns an empty list and discovery falls back to `/dev/rfcomm*` nodes. Pair with
//! `bluetoothctl pair` + `trust`.

#![allow(unsafe_code)] // libc socket FFI is confined to this module tree.

pub mod rfcomm;

use std::path::Path;

use crate::BtAddr;

/// A bonded Bluetooth device known to BlueZ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// Address.
    pub addr: BtAddr,
    /// Device name (`Name=`), or the address string if unknown.
    pub name: String,
    /// Class of device (`Class=0x…`), if recorded.
    pub class_of_device: Option<u32>,
}

/// BlueZ storage root.
const BLUEZ_STORAGE: &str = "/var/lib/bluetooth";

/// Bonded devices from BlueZ storage; empty when the storage is not readable.
#[must_use]
pub fn paired_devices() -> Vec<PairedDevice> {
    let mut out = Vec::new();
    let Ok(adapters) = std::fs::read_dir(BLUEZ_STORAGE) else {
        return out;
    };
    for adapter in adapters.flatten() {
        let Ok(devices) = std::fs::read_dir(adapter.path()) else {
            continue;
        };
        for dev in devices.flatten() {
            let file_name = dev.file_name();
            let Some(addr) = file_name.to_str().and_then(|s| s.parse::<BtAddr>().ok()) else {
                continue;
            };
            if let Some(d) = read_info(&dev.path().join("info"), addr)
                && !out.iter().any(|o: &PairedDevice| o.addr == d.addr)
            {
                out.push(d);
            }
        }
    }
    out
}

fn read_info(path: &Path, addr: BtAddr) -> Option<PairedDevice> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_info(&text, addr)
}

/// Parses a BlueZ `info` file; `None` unless the device is bonded (has a `[LinkKey]`).
fn parse_info(text: &str, addr: BtAddr) -> Option<PairedDevice> {
    let mut section = "";
    let mut name = None;
    let mut class = None;
    let mut bonded = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s;
            if s == "LinkKey" {
                bonded = true;
            }
            continue;
        }
        if section != "General" {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("Class=") {
            let v = v.trim_start_matches("0x").trim_start_matches("0X");
            class = u32::from_str_radix(v, 16).ok();
        }
    }
    bonded.then(|| PairedDevice {
        addr,
        name: name.unwrap_or_else(|| addr.to_string()),
        class_of_device: class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bluez_info() {
        let addr = BtAddr([0, 1, 2, 3, 4, 5]);
        let text = "[General]\nName=PT-P710BTxxxx\nClass=0x042680\n\n[LinkKey]\nKey=00\n";
        let d = parse_info(text, addr).unwrap();
        assert_eq!(d.name, "PT-P710BTxxxx");
        assert_eq!(d.class_of_device, Some(0x04_2680));
        assert!(parse_info("[General]\nName=X\n", addr).is_none());
    }
}
