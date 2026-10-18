//! Printer discovery.
//!
//! # Contract
//! - Lists candidates without opening them: serial nodes with `PT-` in the name (and
//!   `/dev/rfcomm*`), paired Bluetooth printers (macOS: `IOBluetoothDevice.pairedDevices`,
//!   CoD major 6 / minor printer or name starting with `PT-`; Linux: BlueZ paired devices when
//!   available), Brother USB devices (feature `usb`).
//! - Every result carries a ready-to-use [`Endpoint`] and a model hint from
//!   `ptouch::profile_by_bt_name` / `profile_by_usb_pid`.
//! - Optional inquiry (macOS) is bounded by `DiscoverOptions::inquiry`.
//! - Backends that are unavailable are skipped silently; only a failure of every enabled
//!   source is an error.
//!
//! Results are ordered by preference: native Bluetooth, then USB, then serial nodes. On macOS
//! a `/dev/cu.<name>` node whose `<name>` matches a paired Bluetooth printer that was also
//! listed is omitted, because IOBluetooth is the default macOS backend and the serial node is
//! an opt-in fallback only (PROTOCOL.md §2.2 step 12); it can still be opened explicitly with
//! `serial:/dev/cu.<name>`.
//!
//! Bluetooth on macOS needs [`crate::run_with_event_loop`]; outside it, the paired-device list
//! is still read (it is synchronous) but an inquiry is skipped.

use std::time::Duration;

use crate::endpoint::rfcomm::is_printer_class_of_device;
use crate::{Endpoint, TransportError};

/// Which sources to query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    /// Serial device nodes.
    pub serial: bool,
    /// Paired Bluetooth devices.
    pub bluetooth: bool,
    /// USB devices.
    pub usb: bool,
    /// Also run a Bluetooth inquiry for this long (unpaired devices); `None` = paired only.
    pub inquiry: Option<Duration>,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            serial: true,
            bluetooth: true,
            usb: true,
            inquiry: None,
        }
    }
}

/// Where a device was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiscoverySource {
    /// Serial port enumeration.
    Serial,
    /// Paired Bluetooth device list.
    BluetoothPaired,
    /// Bluetooth inquiry.
    BluetoothInquiry,
    /// USB enumeration.
    Usb,
}

/// One discovered candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    /// How to open it.
    pub endpoint: Endpoint,
    /// Display label (device or port name).
    pub label: String,
    /// Model hint (not authoritative; `st[4]` is).
    pub model: Option<&'static ptouch::ModelProfile>,
    /// Source.
    pub source: DiscoverySource,
}

/// The Bluetooth device name embedded in a serial node path: `/dev/cu.PT-P710BTxxxx` →
/// `PT-P710BTxxxx`; `None` for nodes without a name (`/dev/rfcomm0`, `COM5`).
#[must_use]
pub fn device_name_from_port(path: &str) -> Option<&str> {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let name = file
        .strip_prefix("cu.")
        .or_else(|| file.strip_prefix("tty."))?;
    // macOS sometimes appends "-SerialPort" / "-SPP" style service suffixes.
    let name = name
        .strip_suffix("-SerialPort")
        .or_else(|| name.strip_suffix("-SPP"))
        .unwrap_or(name);
    (!name.is_empty()).then_some(name)
}

/// `true` when a Bluetooth device looks like a P-touch printer: printer Class of Device or a
/// name starting with `PT-` (PROTOCOL.md §2.1 "Device identification").
#[must_use]
pub fn is_printer_candidate(name: &str, class_of_device: Option<u32>) -> bool {
    class_of_device.is_some_and(is_printer_class_of_device)
        || name.trim_start().to_ascii_uppercase().starts_with("PT-")
}

/// Lists candidate printers.
///
/// # Errors
/// Only if every enabled source failed.
pub fn discover(opts: &DiscoverOptions) -> Result<Vec<DiscoveredDevice>, TransportError> {
    let mut found = Vec::new();
    let mut first_err: Option<TransportError> = None;
    let mut any_ok = false;
    let mut record = |r: Result<Vec<DiscoveredDevice>, TransportError>,
                      found: &mut Vec<DiscoveredDevice>| match r {
        Ok(v) => {
            any_ok = true;
            found.extend(v);
        }
        Err(TransportError::Unsupported(_)) => any_ok = true,
        Err(e) => {
            if first_err.is_none() {
                first_err = Some(e);
            }
        }
    };
    let mut enabled = false;

    if opts.bluetooth {
        enabled = true;
        record(bluetooth_devices(opts.inquiry), &mut found);
    }
    if opts.usb {
        enabled = true;
        record(usb_devices(), &mut found);
    }
    if opts.serial {
        enabled = true;
        let bt_names: Vec<String> = found
            .iter()
            .filter(|d| matches!(d.endpoint, Endpoint::Bluetooth { .. }))
            .map(|d| d.label.clone())
            .collect();
        let serial = serial_devices().map(|v| {
            v.into_iter()
                .filter(|d| {
                    !cfg!(target_os = "macos")
                        || !matches!(&d.endpoint, Endpoint::Serial { path }
                            if device_name_from_port(path).is_some_and(|n| bt_names.iter().any(|b| b == n)))
                })
                .collect()
        });
        record(serial, &mut found);
    }

    match first_err {
        Some(e) if enabled && !any_ok => Err(e),
        _ => Ok(found),
    }
}

fn serial_devices() -> Result<Vec<DiscoveredDevice>, TransportError> {
    Ok(crate::serial::SerialTransport::list_candidates()?
        .into_iter()
        .map(|path| {
            let model = device_name_from_port(&path).and_then(ptouch::profile_by_bt_name);
            let label = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
            DiscoveredDevice {
                endpoint: Endpoint::Serial { path },
                label,
                model,
                source: DiscoverySource::Serial,
            }
        })
        .collect())
}

#[cfg(feature = "usb")]
fn usb_devices() -> Result<Vec<DiscoveredDevice>, TransportError> {
    Ok(crate::usb::UsbTransport::list()?
        .into_iter()
        .filter(|c| !crate::usb::UsbTransport::is_editor_lite_pid(c.product_id))
        .map(|c| {
            let model = ptouch::profile_by_usb_pid(c.product_id);
            let label = c
                .product
                .clone()
                .or_else(|| model.map(|m| m.name.to_owned()))
                .unwrap_or_else(|| format!("USB {:04x}:{:04x}", c.vendor_id, c.product_id));
            DiscoveredDevice {
                endpoint: Endpoint::Usb {
                    vendor_id: c.vendor_id,
                    product_id: Some(c.product_id),
                },
                label,
                model,
                source: DiscoverySource::Usb,
            }
        })
        .collect())
}

#[cfg(not(feature = "usb"))]
fn usb_devices() -> Result<Vec<DiscoveredDevice>, TransportError> {
    Err(TransportError::Unsupported("USB (feature `usb`)"))
}

#[cfg(all(target_os = "macos", feature = "macos-rfcomm"))]
fn bluetooth_devices(inquiry: Option<Duration>) -> Result<Vec<DiscoveredDevice>, TransportError> {
    let to_device = |d: crate::macos::PairedDevice, source| DiscoveredDevice {
        endpoint: Endpoint::Bluetooth {
            addr: d.addr,
            channel: None,
        },
        model: ptouch::profile_by_bt_name(&d.name),
        label: d.name,
        source,
    };
    let mut out: Vec<DiscoveredDevice> = crate::macos::paired_devices()?
        .into_iter()
        .filter(|d| is_printer_candidate(&d.name, Some(d.class_of_device)))
        .map(|d| to_device(d, DiscoverySource::BluetoothPaired))
        .collect();
    if let Some(duration) = inquiry {
        // An inquiry needs the main-thread event loop; skip it silently otherwise.
        if let Ok(devices) = crate::macos::inquiry(duration) {
            for d in devices {
                let known = out.iter().any(
                    |o| matches!(o.endpoint, Endpoint::Bluetooth { addr, .. } if addr == d.addr),
                );
                if !known && is_printer_candidate(&d.name, Some(d.class_of_device)) {
                    out.push(to_device(d, DiscoverySource::BluetoothInquiry));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(all(target_os = "linux", feature = "linux-rfcomm"))]
fn bluetooth_devices(_inquiry: Option<Duration>) -> Result<Vec<DiscoveredDevice>, TransportError> {
    Ok(crate::linux::paired_devices()
        .into_iter()
        .filter(|d| is_printer_candidate(&d.name, d.class_of_device))
        .map(|d| DiscoveredDevice {
            endpoint: Endpoint::Bluetooth {
                addr: d.addr,
                channel: None,
            },
            model: ptouch::profile_by_bt_name(&d.name),
            label: d.name,
            source: DiscoverySource::BluetoothPaired,
        })
        .collect())
}

#[cfg(not(any(
    all(target_os = "macos", feature = "macos-rfcomm"),
    all(target_os = "linux", feature = "linux-rfcomm")
)))]
fn bluetooth_devices(_inquiry: Option<Duration>) -> Result<Vec<DiscoveredDevice>, TransportError> {
    Err(TransportError::Unsupported("Bluetooth discovery"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_ports() {
        assert_eq!(
            device_name_from_port("/dev/cu.PT-P710BTxxxx"),
            Some("PT-P710BTxxxx")
        );
        assert_eq!(device_name_from_port("/dev/rfcomm0"), None);
        assert_eq!(device_name_from_port("COM5"), None);
        assert_eq!(device_name_from_port("/dev/cu."), None);
    }

    #[test]
    fn printer_candidates() {
        assert!(is_printer_candidate("PT-P710BTxxxx", None));
        assert!(is_printer_candidate("Office", Some(0x04_2680)));
        assert!(!is_printer_candidate("Headphones", Some(0x24_0404)));
    }

    #[test]
    fn nothing_enabled_is_empty() {
        let opts = DiscoverOptions {
            serial: false,
            bluetooth: false,
            usb: false,
            inquiry: None,
        };
        assert_eq!(discover(&opts).unwrap(), Vec::new());
    }
}
