//! USB printer-class transport via `nusb` (feature `usb`).
//!
//! # Contract (PROTOCOL.md §2.8)
//! - Finds VID 0x04F9 (+ optional PID) with a printer-class (0x07) interface 0, claims it
//!   (detaching a kernel driver on Linux where needed), bulk OUT 0x02 / IN 0x81.
//! - Writes in 16 KiB transfers with `write_timeout` (the printer back-pressures OUT for the
//!   print duration); never retries a partially sent buffer.
//! - Reads: one outstanding IN transfer of 64 bytes, polled with the read timeout; an
//!   in-flight IN transfer is never abandoned (it would eat the next frame) — it is kept and
//!   completed on the next `read`.
//! - Editor Lite PIDs (0x2064, 0x2065) are rejected with a hint to switch Editor Lite off.

use std::collections::VecDeque;
use std::time::Duration;

use nusb::transfer::{Buffer, Bulk, In, Out, TransferError};
use nusb::{Endpoint, Interface, MaybeFuture};

use crate::{Transport, TransportError, TransportInfo, TransportKind};

/// Brother vendor ID.
const BROTHER_VID: u16 = 0x04F9;
/// Interface number of the printer function.
const INTERFACE: u8 = 0;
/// USB interface class "printer".
const PRINTER_CLASS: u8 = 0x07;
/// Bulk OUT endpoint (data).
const EP_OUT: u8 = 0x02;
/// Bulk IN endpoint (status).
const EP_IN: u8 = 0x81;
/// Bytes per OUT transfer.
const WRITE_CHUNK: usize = 16 * 1024;
/// Bytes requested per IN transfer (max packet size 64).
const READ_LEN: usize = 64;
/// Editor Lite (mass-storage) product IDs: PT-P700, PT-P750W.
const EDITOR_LITE_PIDS: [u16; 2] = [0x2064, 0x2065];

/// A Brother USB device found during enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbCandidate {
    /// Vendor ID.
    pub vendor_id: u16,
    /// Product ID.
    pub product_id: u16,
    /// Product string, if readable.
    pub product: Option<String>,
    /// Serial string, if readable (do not parse; PROTOCOL.md §2.8).
    pub serial: Option<String>,
}

/// USB transport.
pub struct UsbTransport {
    info: TransportInfo,
    write_timeout: Duration,
    /// Endpoints and interface; `None` after `close`.
    io: Option<UsbIo>,
    /// Bytes received but not yet returned by `read`.
    leftover: VecDeque<u8>,
}

struct UsbIo {
    out: Endpoint<Bulk, Out>,
    inp: Endpoint<Bulk, In>,
    _interface: Interface,
}

impl std::fmt::Debug for UsbTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbTransport")
            .field("info", &self.info)
            .field("open", &self.io.is_some())
            .finish_non_exhaustive()
    }
}

fn usb_err(op: &'static str, e: impl std::fmt::Display) -> TransportError {
    TransportError::Usb {
        op,
        message: e.to_string(),
    }
}

fn transfer_err(op: &'static str, e: TransferError) -> TransportError {
    match e {
        TransferError::Disconnected => TransportError::Closed,
        other => usb_err(op, other),
    }
}

fn has_printer_interface(d: &nusb::DeviceInfo) -> bool {
    // Some platforms do not report interfaces before opening; accept those and let the
    // claim fail instead.
    let mut it = d.interfaces().peekable();
    it.peek().is_none()
        || d.interfaces()
            .any(|i| i.interface_number() == INTERFACE && i.class() == PRINTER_CLASS)
}

impl UsbTransport {
    /// Opens the first matching device.
    ///
    /// # Errors
    /// [`TransportError::NotFound`] if none matches; [`TransportError::Usb`] on open/claim
    /// failure; [`TransportError::EditorLiteMode`] if the only match is in Editor Lite mode.
    pub fn open(
        vendor_id: u16,
        product_id: Option<u16>,
        write_timeout: Duration,
    ) -> Result<Self, TransportError> {
        let devices: Vec<nusb::DeviceInfo> = nusb::list_devices()
            .wait()
            .map_err(|e| usb_err("list_devices", e))?
            .filter(|d| d.vendor_id() == vendor_id)
            .filter(|d| product_id.is_none_or(|p| d.product_id() == p))
            .collect();
        let mut editor_lite = None;
        let info = devices.into_iter().find(|d| {
            if vendor_id == BROTHER_VID && EDITOR_LITE_PIDS.contains(&d.product_id()) {
                editor_lite = Some(d.product_id());
                return false;
            }
            has_printer_interface(d)
        });
        let Some(info) = info else {
            return Err(match editor_lite {
                Some(product_id) => TransportError::EditorLiteMode { product_id },
                None => TransportError::NotFound(match product_id {
                    Some(p) => format!("USB printer {vendor_id:04x}:{p:04x}"),
                    None => format!("USB printer with vendor {vendor_id:04x}"),
                }),
            });
        };
        let pid = info.product_id();
        let device = info.open().wait().map_err(|e| usb_err("open", e))?;
        let interface = device
            .detach_and_claim_interface(INTERFACE)
            .wait()
            .map_err(|e| usb_err("claim_interface", e))?;
        let out = interface
            .endpoint::<Bulk, Out>(EP_OUT)
            .map_err(|e| usb_err("open OUT endpoint 0x02", e))?;
        let inp = interface
            .endpoint::<Bulk, In>(EP_IN)
            .map_err(|e| usb_err("open IN endpoint 0x81", e))?;
        Ok(Self {
            info: TransportInfo {
                kind: TransportKind::Usb,
                label: format!("USB {vendor_id:04x}:{pid:04x}"),
                max_write: Some(WRITE_CHUNK),
            },
            write_timeout,
            io: Some(UsbIo {
                out,
                inp,
                _interface: interface,
            }),
            leftover: VecDeque::new(),
        })
    }

    /// Lists Brother USB devices (any product, including Editor Lite mode).
    ///
    /// # Errors
    /// [`TransportError::Usb`] if enumeration fails.
    pub fn list() -> Result<Vec<UsbCandidate>, TransportError> {
        Ok(nusb::list_devices()
            .wait()
            .map_err(|e| usb_err("list_devices", e))?
            .filter(|d| d.vendor_id() == BROTHER_VID)
            .map(|d| UsbCandidate {
                vendor_id: d.vendor_id(),
                product_id: d.product_id(),
                product: d.product_string().map(str::to_owned),
                serial: d.serial_number().map(str::to_owned),
            })
            .collect())
    }

    /// `true` when `product_id` is an Editor Lite (mass-storage) PID.
    #[must_use]
    pub fn is_editor_lite_pid(product_id: u16) -> bool {
        EDITOR_LITE_PIDS.contains(&product_id)
    }
}

impl Transport for UsbTransport {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let io = self.io.as_mut().ok_or(TransportError::Closed)?;
        for chunk in bytes.chunks(WRITE_CHUNK) {
            io.out.submit(Buffer::from(chunk));
            let completion = match io.out.wait_next_complete(self.write_timeout) {
                Some(c) => c,
                None => {
                    // Cancel and reap the transfer so the endpoint stays consistent; the
                    // job is NOT retried (PROTOCOL.md §2.1 "Writes").
                    io.out.cancel_all();
                    let _ = io.out.wait_next_complete(Duration::from_secs(2));
                    return Err(TransportError::WriteTimeout);
                }
            };
            completion.status.map_err(|e| transfer_err("bulk OUT", e))?;
            if completion.actual_len != chunk.len() {
                return Err(TransportError::Usb {
                    op: "bulk OUT",
                    message: format!(
                        "short write: {} of {} bytes",
                        completion.actual_len,
                        chunk.len()
                    ),
                });
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.leftover.is_empty() {
            let io = self.io.as_mut().ok_or(TransportError::Closed)?;
            // Keep exactly one IN transfer in flight; a pending one from an earlier timed-out
            // read is reused, never cancelled.
            if io.inp.pending() == 0 {
                let len = READ_LEN.max(io.inp.max_packet_size());
                io.inp.submit(Buffer::new(len));
            }
            let Some(completion) = io.inp.wait_next_complete(timeout) else {
                return Ok(0);
            };
            completion.status.map_err(|e| transfer_err("bulk IN", e))?;
            let data = completion
                .buffer
                .get(..completion.actual_len)
                .unwrap_or(&[]);
            self.leftover.extend(data);
            if self.leftover.is_empty() {
                // Zero-length packet; the caller polls again.
                return Ok(0);
            }
        }
        let n = buf.len().min(self.leftover.len());
        for (dst, src) in buf.iter_mut().zip(self.leftover.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }

    fn close(&mut self) -> Result<(), TransportError> {
        // Dropping the endpoints and interface releases the claim.
        if let Some(mut io) = self.io.take()
            && io.inp.pending() > 0
        {
            io.inp.cancel_all();
            while io.inp.pending() > 0 {
                if io.inp.wait_next_complete(Duration::from_secs(1)).is_none() {
                    break;
                }
            }
        }
        Ok(())
    }

    fn info(&self) -> &TransportInfo {
        &self.info
    }
}

impl Drop for UsbTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
