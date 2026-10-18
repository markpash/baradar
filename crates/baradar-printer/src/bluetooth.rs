//! Bluetooth scanning and pairing (macOS).
//!
//! Scanning reuses `ptouch-transport`'s IOBluetooth inquiry. Pairing is not in upstream yet, so
//! it is done here with `IOBluetoothDevicePair`, following what Brother's macOS SDK does
//! (ptouch `docs/PROTOCOL.md` §2.2 step 2): Secure Simple Pairing without a PIN, plus a reply
//! of `0000` if the printer asks for a legacy PIN.
//!
//! IOBluetooth objects live on the main thread, and their delegate callbacks are delivered
//! there. Everything here must run on a worker thread, either inside
//! [`ptouch_transport::run_with_event_loop`] (command line) or with
//! `ptouch_transport::macos::use_host_run_loop()` on (GUI app); work is sent to the main thread
//! through `ptouch_transport::macos::run_on_main_thread`, so it is serialised with the
//! transport's own IOBluetooth calls.

use std::cell::RefCell;
use std::ffi::c_int;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::NSString;
use objc2_io_bluetooth::{BluetoothPINCode, IOBluetoothDevice, IOBluetoothDevicePair};
use ptouch_transport::BtAddr;
use ptouch_transport::discovery::is_printer_candidate;
use ptouch_transport::macos::{PairedDevice, inquiry, paired_devices, run_on_main_thread};

/// Legacy PIN that Brother printers accept (community-reported; SSP needs none).
const LEGACY_PIN: &[u8] = b"0000";

/// A Bluetooth device seen by [`scan`].
#[derive(Debug, Clone)]
pub struct NearbyDevice {
    /// Address.
    pub addr: BtAddr,
    /// Device name (the address when the name is unknown).
    pub name: String,
    /// Already paired with this Mac.
    pub paired: bool,
    /// Looks like a P-touch printer (printer class of device, or a `PT-` name).
    pub is_printer: bool,
}

/// Runs a Bluetooth inquiry for `duration` (1–48 s) and returns every device found, plus
/// paired printers that did not answer. Printers must be switched on to be found.
///
/// macOS returns an empty list, not an error, when this process lacks Bluetooth permission.
pub fn scan(duration: Duration) -> Result<Vec<NearbyDevice>> {
    let paired = paired_devices()?;
    let is_paired = |addr: BtAddr| paired.iter().any(|p| p.addr == addr);
    let to_nearby = |d: PairedDevice, paired: bool| NearbyDevice {
        is_printer: is_printer_candidate(&d.name, Some(d.class_of_device)),
        addr: d.addr,
        name: d.name,
        paired,
    };
    let mut out: Vec<NearbyDevice> = inquiry(duration)
        .context("running the Bluetooth inquiry")?
        .into_iter()
        .map(|d| {
            let p = is_paired(d.addr);
            to_nearby(d, p)
        })
        .collect();
    for d in paired.iter().cloned().map(|d| to_nearby(d, true)) {
        if d.is_printer && !out.iter().any(|o| o.addr == d.addr) {
            out.push(d);
        }
    }
    Ok(out)
}

/// Pairs with the device at `addr` and waits up to `timeout` for the result. Returns `Ok`
/// straight away if it is already paired.
///
/// `on_event` is called with progress messages (e.g. a passkey to check on the printer).
pub fn pair(addr: BtAddr, timeout: Duration, mut on_event: impl FnMut(PairEvent)) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let address = addr.to_dashed_lower();
    let started = on_main(move || start_pairing(&address, tx))?;
    match started.map_err(|e| anyhow!(e))? {
        Started::AlreadyPaired => return Ok(()),
        Started::Pairing => {}
    }

    let deadline = Instant::now() + timeout;
    let result = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(PairEvent::Finished(0)) => break Ok(()),
            Ok(PairEvent::Finished(code)) => {
                break Err(anyhow!("pairing failed (IOReturn {:#010x})", code as u32));
            }
            Ok(ev) => on_event(ev),
            Err(_) => break Err(anyhow!("pairing timed out after {} s", timeout.as_secs())),
        }
    };
    // Stop (harmless once finished) and release the pairing object and its delegate.
    let _ = on_main(|| {
        PAIRING.with(|p| {
            if let Some((pair, _delegate)) = p.borrow_mut().take() {
                // SAFETY: IOBluetooth calls on the main thread with a live pairing object.
                unsafe {
                    pair.stop();
                    pair.setDelegate(None);
                }
            }
        });
    });
    result
}

/// Progress reported while pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairEvent {
    /// The printer asked for a legacy PIN; `0000` was sent.
    PinRequested,
    /// Numeric comparison: the printer should show this number. It is accepted automatically.
    ConfirmNumber(u32),
    /// The printer asks for this passkey to be typed on it.
    Passkey(u32),
    /// Secure Simple Pairing completed with this HCI status (0 = success).
    SimplePairingComplete(u8),
    /// Pairing finished with this `IOReturn` (0 = success).
    Finished(c_int),
}

enum Started {
    AlreadyPaired,
    Pairing,
}

thread_local! {
    /// The pairing in progress (main thread only). IOBluetooth retains neither the pairing
    /// object nor its delegate, so both are kept here until [`pair`] finishes.
    static PAIRING: RefCell<Option<(Retained<IOBluetoothDevicePair>, Retained<PairDelegate>)>> =
        const { RefCell::new(None) };
}

/// Main thread: looks the device up and starts pairing.
fn start_pairing(address: &str, tx: Sender<PairEvent>) -> Result<Started, String> {
    let s = NSString::from_str(address);
    // SAFETY: IOBluetooth calls on the main thread with valid arguments.
    unsafe {
        let device = IOBluetoothDevice::deviceWithAddressString(Some(&s))
            .ok_or_else(|| format!("invalid Bluetooth address {address}"))?;
        if device.isPaired() {
            return Ok(Started::AlreadyPaired);
        }
        let pair = IOBluetoothDevicePair::pairWithDevice(Some(&device))
            .ok_or("could not create an IOBluetoothDevicePair")?;
        let delegate = PairDelegate::new(tx);
        pair.setDelegate(Some(&delegate));
        let status = pair.start();
        if status != 0 {
            pair.setDelegate(None);
            return Err(format!(
                "could not start pairing (IOReturn {:#010x})",
                status as u32
            ));
        }
        PAIRING.with(|p| *p.borrow_mut() = Some((pair, delegate)));
    }
    Ok(Started::Pairing)
}

/// Runs `f` on the main thread and returns its result.
fn on_main<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R> {
    run_on_main_thread(f).context(
        "Bluetooth pairing needs a worker thread inside run_with_event_loop, or host run loop mode",
    )
}

/// Calls `f` with the pairing object in progress (main thread, from a delegate callback).
fn with_pairing(f: impl FnOnce(&IOBluetoothDevicePair)) {
    PAIRING.with(|p| {
        if let Ok(p) = p.try_borrow()
            && let Some((pair, _)) = p.as_ref()
        {
            f(pair);
        }
    });
}

/// Instance variables of [`PairDelegate`].
struct PairIvars {
    tx: Sender<PairEvent>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "BaradarPairDelegate"]
    #[ivars = PairIvars]
    struct PairDelegate;

    unsafe impl NSObjectProtocol for PairDelegate {}

    // Selectors of the informal protocol IOBluetoothDevicePairDelegate.
    impl PairDelegate {
        #[unsafe(method(devicePairingPINCodeRequest:))]
        fn pin_code_request(&self, _sender: Option<&AnyObject>) {
            let mut pin = BluetoothPINCode { data: [0; 16] };
            pin.data[..LEGACY_PIN.len()].copy_from_slice(LEGACY_PIN);
            // SAFETY: IOBluetooth call on the main thread; `pin` outlives the call.
            with_pairing(|pair| unsafe { pair.replyPINCode_PINCode(LEGACY_PIN.len() as u64, &mut pin) });
            self.send(PairEvent::PinRequested);
        }

        #[unsafe(method(devicePairingUserConfirmationRequest:numericValue:))]
        fn user_confirmation_request(&self, _sender: Option<&AnyObject>, value: u32) {
            // SAFETY: IOBluetooth call on the main thread.
            with_pairing(|pair| unsafe { pair.replyUserConfirmation(true) });
            self.send(PairEvent::ConfirmNumber(value));
        }

        #[unsafe(method(devicePairingUserPasskeyNotification:passkey:))]
        fn passkey_notification(&self, _sender: Option<&AnyObject>, passkey: u32) {
            self.send(PairEvent::Passkey(passkey));
        }

        #[unsafe(method(deviceSimplePairingComplete:status:))]
        fn simple_pairing_complete(&self, _sender: Option<&AnyObject>, status: u8) {
            self.send(PairEvent::SimplePairingComplete(status));
        }

        #[unsafe(method(devicePairingFinished:error:))]
        fn finished(&self, _sender: Option<&AnyObject>, error: c_int) {
            self.send(PairEvent::Finished(error));
        }
    }
);

impl PairDelegate {
    fn new(tx: Sender<PairEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(PairIvars { tx });
        // SAFETY: `init` on a freshly allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, ev: PairEvent) {
        let _ = self.ivars().tx.send(ev);
    }
}
