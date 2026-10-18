//! What the UI shows about a connected printer, decoded from a status frame (the studio's
//! `#applyStatus`: tape detection, colours, battery, errors, model).

use ptouch::status::Battery;
use ptouch::{
    MediaType, ModelProfile, Notification, PrinterError, Status, TapeColor, TapeSpec, TextColor,
};

use crate::packet_log::hex;

/// A status frame plus everything derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrinterInfo {
    /// The model (detected from the status, or the one the session expects); `None` for a model
    /// that is not in ptouch's table.
    pub model: Option<&'static ModelProfile>,
    /// The decoded frame (its `raw` field holds the 32 bytes).
    pub status: Status,
    /// The loaded tape, when the model knows this width and type (`None` without tape).
    pub tape: Option<&'static TapeSpec>,
    /// Battery level and power source, decoded for the model's battery format.
    pub battery: Battery,
}

impl PrinterInfo {
    /// Decodes `status` for `model`.
    #[must_use]
    pub fn new(model: Option<&'static ModelProfile>, status: Status) -> Self {
        let tape = model.filter(|_| status.media_width_mm > 0).and_then(|m| {
            ptouch::tape_for_status(m, status.media_width_mm, status.media_type.to_byte()).ok()
        });
        let battery = status.battery(model.and_then(|m| m.battery_format));
        Self {
            model,
            status,
            tape,
            battery,
        }
    }

    /// Model name, e.g. `PT-E720BT`.
    #[must_use]
    pub fn model_name(&self) -> Option<&'static str> {
        self.model.map(|m| m.name)
    }

    /// Loaded tape width in mm (the media width byte: 3.5 mm tape reports 4); `None` without
    /// tape.
    #[must_use]
    pub fn tape_width_mm(&self) -> Option<u8> {
        (self.status.media_width_mm > 0 && self.status.has_media())
            .then_some(self.status.media_width_mm)
    }

    /// Tape width for display, e.g. `12 mm` or `3.5 mm`.
    #[must_use]
    pub fn tape_width_text(&self) -> Option<String> {
        self.tape_width_mm().map(mm_text)
    }

    /// The media type byte, decoded (laminated, non-laminated, heat-shrink tube…).
    #[must_use]
    pub fn media_type(&self) -> MediaType {
        self.status.media_type
    }

    /// Tape (background) colour.
    #[must_use]
    pub fn tape_color(&self) -> TapeColor {
        self.status.tape_color
    }

    /// Text (ink) colour.
    #[must_use]
    pub fn text_color(&self) -> TextColor {
        self.status.text_color
    }

    /// The low-battery bit (valid on every model).
    #[must_use]
    pub fn weak_battery(&self) -> bool {
        self.status.weak_battery()
    }

    /// Every printer error in the frame, in headline order. Cover-open signals that are not an
    /// error bit (phase `0014`, notification `01`) are reported as [`PrinterError::CoverOpen`].
    #[must_use]
    pub fn errors(&self) -> Vec<PrinterError> {
        let mut errors: Vec<PrinterError> = self.status.errors.iter().collect();
        let cover = self.status.phase.is_cover_open_error()
            || self.status.notification == Notification::CoverOpen;
        if cover && !errors.contains(&PrinterError::CoverOpen) {
            errors.insert(0, PrinterError::CoverOpen);
        }
        errors
    }

    /// The §4.3 ready predicate: no error and idle.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status.is_ready()
    }

    /// The raw frame as hex (`80 20 42 …`), for diagnostics.
    #[must_use]
    pub fn raw_hex(&self) -> String {
        hex(&self.status.raw)
    }
}

/// `12 mm`; 3.5 mm tape reports a width byte of 4.
#[must_use]
pub fn mm_text(width_byte: u8) -> String {
    if width_byte == 4 {
        String::from("3.5 mm")
    } else {
        format!("{width_byte} mm")
    }
}
