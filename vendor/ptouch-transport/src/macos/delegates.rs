//! Objective-C delegate classes for IOBluetooth callbacks.
//!
//! IOBluetooth does **not** retain its delegates/targets, and on macOS 26/27 it delivers
//! every callback asynchronously on the main queue (PROTOCOL.md §2.2 step 6). A delegate may
//! therefore be called after the Rust side stopped caring about it; the runtime keeps every
//! delegate it created alive until the event loop shuts down, and the callbacks only ever
//! send into an `mpsc` channel whose receiver may already be gone (sends are ignored then).

use std::ffi::{c_int, c_void};
use std::sync::mpsc::Sender;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_io_bluetooth::{IOBluetoothDevice, IOBluetoothDeviceInquiry, IOBluetoothRFCOMMChannel};

/// Events from an RFCOMM channel delegate.
#[derive(Debug)]
pub(crate) enum Event {
    /// `rfcommChannelOpenComplete:status:`.
    OpenComplete(c_int),
    /// `rfcommChannelData:data:length:` (copied).
    Data(Vec<u8>),
    /// `rfcommChannelClosed:`.
    Closed,
    /// `rfcommChannelWriteComplete:refcon:status:` for the `writeAsync` with this refcon.
    WriteComplete {
        /// The refcon passed to `writeAsync:length:refcon:` (an opaque counter).
        refcon: usize,
        /// `IOReturn` of the write.
        status: c_int,
    },
}

/// Instance variables of [`RfcommDelegate`].
pub(crate) struct RfcommIvars {
    tx: Sender<Event>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "PtouchTransportRfcommDelegate"]
    #[ivars = RfcommIvars]
    pub(crate) struct RfcommDelegate;

    unsafe impl NSObjectProtocol for RfcommDelegate {}

    // Selectors of the informal protocol IOBluetoothRFCOMMChannelDelegate.
    impl RfcommDelegate {
        // - (void)rfcommChannelData:(IOBluetoothRFCOMMChannel*)ch data:(void*)p length:(size_t)n;
        #[unsafe(method(rfcommChannelData:data:length:))]
        fn rfcomm_channel_data(
            &self,
            _ch: Option<&IOBluetoothRFCOMMChannel>,
            data: *mut c_void,
            length: usize,
        ) {
            if data.is_null() || length == 0 {
                return;
            }
            // SAFETY: IOBluetooth guarantees `data` points at `length` readable bytes for the
            // duration of the callback; they are copied out immediately.
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            let _ = self.ivars().tx.send(Event::Data(bytes.to_vec()));
        }

        // - (void)rfcommChannelOpenComplete:(IOBluetoothRFCOMMChannel*)ch status:(IOReturn)error;
        #[unsafe(method(rfcommChannelOpenComplete:status:))]
        fn rfcomm_channel_open_complete(
            &self,
            _ch: Option<&IOBluetoothRFCOMMChannel>,
            status: c_int,
        ) {
            let _ = self.ivars().tx.send(Event::OpenComplete(status));
        }

        // - (void)rfcommChannelWriteComplete:(IOBluetoothRFCOMMChannel*)ch refcon:(void*)r
        //                            status:(IOReturn)error;
        #[unsafe(method(rfcommChannelWriteComplete:refcon:status:))]
        fn rfcomm_channel_write_complete(
            &self,
            _ch: Option<&IOBluetoothRFCOMMChannel>,
            refcon: *mut c_void,
            status: c_int,
        ) {
            // The refcon is an integer the writer chose, never dereferenced.
            let refcon = refcon.addr();
            let _ = self.ivars().tx.send(Event::WriteComplete { refcon, status });
        }

        // - (void)rfcommChannelClosed:(IOBluetoothRFCOMMChannel*)ch;
        #[unsafe(method(rfcommChannelClosed:))]
        fn rfcomm_channel_closed(&self, _ch: Option<&IOBluetoothRFCOMMChannel>) {
            let _ = self.ivars().tx.send(Event::Closed);
        }
    }
);

impl RfcommDelegate {
    /// A delegate forwarding into `tx`.
    pub(crate) fn new(tx: Sender<Event>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(RfcommIvars { tx });
        // SAFETY: `init` on a freshly allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "PtouchTransportSdpQueryTarget"]
    pub(crate) struct SdpQueryTarget;

    unsafe impl NSObjectProtocol for SdpQueryTarget {}

    impl SdpQueryTarget {
        // - (void)sdpQueryComplete:(IOBluetoothDevice *)device status:(IOReturn)status;
        // Never fires on macOS 26/27 (PROTOCOL.md §2.2 step 4); completion is polled instead.
        #[unsafe(method(sdpQueryComplete:status:))]
        fn sdp_query_complete(&self, _device: Option<&IOBluetoothDevice>, _status: c_int) {}
    }
);

impl SdpQueryTarget {
    /// A no-op target for `performSDPQuery:`.
    pub(crate) fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: `init` on a freshly allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }
}

/// Instance variables of [`InquiryDelegate`].
pub(crate) struct InquiryIvars {
    tx: Sender<c_int>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "PtouchTransportInquiryDelegate"]
    #[ivars = InquiryIvars]
    pub(crate) struct InquiryDelegate;

    unsafe impl NSObjectProtocol for InquiryDelegate {}

    impl InquiryDelegate {
        // - (void)deviceInquiryComplete:(IOBluetoothDeviceInquiry*)sender
        //                         error:(IOReturn)error aborted:(BOOL)aborted;
        #[unsafe(method(deviceInquiryComplete:error:aborted:))]
        fn complete(
            &self,
            _sender: Option<&IOBluetoothDeviceInquiry>,
            error: c_int,
            _aborted: bool,
        ) {
            let _ = self.ivars().tx.send(error);
        }
    }
);

impl InquiryDelegate {
    /// A delegate reporting completion status into `tx`.
    pub(crate) fn new(tx: Sender<c_int>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(InquiryIvars { tx });
        // SAFETY: `init` on a freshly allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }
}
