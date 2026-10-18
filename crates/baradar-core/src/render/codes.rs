//! Codes: module matrices, quiet zones, module sizes, readability and the human-readable line
//! (port of `web/src/render/codes.ts`, the code part of `renderer.ts` `prepareCode`, and
//! `crates/ptouch-wasm/src/codes.rs`).
//!
//! The encoder ([`encode`], [`code128_syntax`]) is copied from fjanicki/ptouch
//! `crates/ptouch-wasm/src/codes.rs` (<https://github.com/fjanicki/ptouch>, MIT OR Apache-2.0),
//! with the wasm bindings removed and errors mapped to [`CodeError`].
//!
//! Quiet zones come in three modes (modules along the label / across the tape):
//!
//! | Mode     | QR     | DataMatrix | Linear  |
//! |----------|--------|------------|---------|
//! | standard | 4 / 4  | 1 / 1      | 10 / 0  |
//! | compact  | 2 / 2  | 1 / 1      | 5 / 0   |
//! | none     | 0 / 0  | 0 / 0      | 0 / 0   |
//!
//! The renderer pads the matrix with light modules ([`pad_matrix`]) and blits it with
//! `quiet_zone = false`; the core clears and protects the padded zone.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use barcoders::sym::code128::Code128;
use barcoders::sym::ean13::EAN13;
use datamatrix::{DataMatrix, SymbolList};
use fast_qr::{ECL, QRBuilder};
pub use ptouch::ModuleMatrix;

use crate::doc::{CodeContent, CodeItem, ModuleSize, QrEcc, QuietZone, Symbology, WifiSettings};
use crate::render::units::{dots_to_mm, js_round, mm_to_dots};
use crate::render::wifi::{wifi_payload, wifi_warnings};

/// Narrowest module that scans reliably (`MIN_SCANNABLE_MODULE_MM` in renderer.ts).
pub const MIN_SCANNABLE_MODULE_MM: f64 = 0.25;
/// Shortest bars accepted silently (`MIN_BAR_MM` in renderer.ts).
pub const MIN_BAR_MM: f64 = 3.0;
/// Largest module size `moduleDots: 'auto'` gives a linear code (at 180 dpi; [`dots_at`]).
pub const AUTO_LINEAR_MAX_DOTS: i64 = 4;
/// Linear `'auto'` module size when nothing limits the length (auto-length label; at 180 dpi).
pub const AUTO_LINEAR_DEFAULT_DOTS: i64 = 2;
/// Entries kept by the encode cache (`CACHE_SIZE`).
pub const CACHE_SIZE: usize = 64;

// ------------------------------------------------------------------------------------------
// Errors and specs
// ------------------------------------------------------------------------------------------

/// Why a code cannot be encoded. `Display` is the user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    /// Bad data for the symbology (`INVALID_INPUT` in the wasm encoder). Cached.
    #[error("{0}")]
    Invalid(String),
    /// Incomplete Wi-Fi settings (`wifiPayload` throws). Not cached.
    #[error("{0}")]
    Wifi(String),
}

impl CodeError {
    /// The wasm error code: `INVALID_INPUT` for data errors, `WIFI` for Wi-Fi settings.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "INVALID_INPUT",
            Self::Wifi(_) => "WIFI",
        }
    }

    /// The user-facing message.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::Wifi(m) => m,
        }
    }

    fn invalid(message: &str) -> Self {
        Self::Invalid(message.to_owned())
    }
}

/// What to encode (the wasm `CodeSpec` DTO).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CodeSpec {
    Qr { data: String, ecc: Option<QrEcc> },
    Code128 { data: String },
    Ean13 { data: String },
    Datamatrix { data: String },
}

impl CodeSpec {
    /// The spec `codeMatrix` encodes for `symbology` (ECC only for QR).
    pub fn new(symbology: Symbology, data: String, ecc: QrEcc) -> Self {
        match symbology {
            Symbology::Qr => Self::Qr {
                data,
                ecc: Some(ecc),
            },
            Symbology::Code128 => Self::Code128 { data },
            Symbology::Ean13 => Self::Ean13 { data },
            Symbology::Datamatrix => Self::Datamatrix { data },
        }
    }
}

// ------------------------------------------------------------------------------------------
// Encoder (copied from ptouch-wasm codes.rs)
// ------------------------------------------------------------------------------------------

/// Encodes a QR / Code 128 / EAN-13 / DataMatrix symbol into a module matrix for
/// `LabelRaster::blit_code` (ptouch-wasm `codes::encode`, wasm `encodeCode`).
///
/// # Errors
/// [`CodeError::Invalid`] for empty data, data too long for QR or DataMatrix, non-ASCII
/// Code 128, bad EAN-13 digits or check digit, with a message the UI can show.
pub fn encode(spec: &CodeSpec) -> Result<ModuleMatrix, CodeError> {
    match spec {
        CodeSpec::Qr { data, ecc } => qr(data, ecc.unwrap_or_default()),
        CodeSpec::Code128 { data } => {
            let syntax = code128_syntax(data)?;
            let bars = Code128::new(syntax)
                .map_err(|_| CodeError::invalid("Code 128 cannot encode this text"))?
                .encode();
            Ok(linear(bars))
        }
        CodeSpec::Ean13 { data } => {
            let digits = data.trim();
            if !(digits.len() == 12 || digits.len() == 13)
                || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(CodeError::invalid(
                    "EAN-13 needs 12 digits (or 13 with the check digit)",
                ));
            }
            let code = EAN13::new(digits).map_err(|e| match e {
                barcoders::error::Error::Checksum => {
                    CodeError::invalid("EAN-13 check digit is wrong")
                }
                _ => CodeError::invalid("EAN-13 needs 12 digits (or 13 with the check digit)"),
            })?;
            Ok(linear(code.encode()))
        }
        CodeSpec::Datamatrix { data } => datamatrix(data),
    }
}

/// Square ECC 200 symbol, finder/timing border included, no quiet zone. Latin-1 text is
/// encoded as is; anything else gets the UTF-8 ECI (`encode_str`), which some scanners ignore.
fn datamatrix(data: &str) -> Result<ModuleMatrix, CodeError> {
    if data.is_empty() {
        return Err(CodeError::invalid("DataMatrix data is empty"));
    }
    let code = DataMatrix::encode_str(data, SymbolList::default().enforce_square())
        .map_err(|_| CodeError::invalid("too much data for a DataMatrix code"))?;
    let bitmap = code.bitmap();
    let (w, h) = (bitmap.width(), bitmap.height());
    let mut modules = vec![0u8; w * h];
    for (x, y) in bitmap.pixels() {
        if let Some(m) = modules.get_mut(y * w + x) {
            *m = 1;
        }
    }
    Ok(ModuleMatrix {
        width: u32::try_from(w).unwrap_or(0),
        height: u32::try_from(h).unwrap_or(0),
        modules,
    })
}

/// One-row matrix from a bar pattern (1 = bar).
fn linear(bars: Vec<u8>) -> ModuleMatrix {
    ModuleMatrix {
        width: u32::try_from(bars.len()).unwrap_or(u32::MAX),
        height: 1,
        modules: bars,
    }
}

fn qr(data: &str, ecc: QrEcc) -> Result<ModuleMatrix, CodeError> {
    if data.is_empty() {
        return Err(CodeError::invalid("QR code data is empty"));
    }
    let ecl = match ecc {
        QrEcc::L => ECL::L,
        QrEcc::M => ECL::M,
        QrEcc::Q => ECL::Q,
        QrEcc::H => ECL::H,
    };
    let code = QRBuilder::new(data.as_bytes())
        .ecl(ecl)
        .build()
        .map_err(|_| CodeError::invalid("too much data for a QR code"))?;
    let size = code.size;
    let modules: Vec<u8> = code
        .data
        .get(..size * size)
        .unwrap_or_default()
        .iter()
        .map(|m| u8::from(m.value()))
        .collect();
    let side = u32::try_from(size).unwrap_or(0);
    Ok(ModuleMatrix {
        width: side,
        height: side,
        modules,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Set {
    A,
    B,
    C,
}

impl Set {
    /// barcoders' code-set marker (start or switch).
    fn marker(self) -> char {
        match self {
            Self::A => 'À',
            Self::B => 'Ɓ',
            Self::C => 'Ć',
        }
    }
}

/// Converts plain ASCII text into barcoders' Code 128 syntax (code-set markers), choosing sets
/// automatically: C for runs of ≥ 4 digits at the start/end or ≥ 6 in the middle (or an
/// all-digit text of ≥ 2), A for control characters, B otherwise (ptouch-wasm `code128_syntax`).
///
/// # Errors
/// [`CodeError::Invalid`] for empty or non-ASCII text.
pub fn code128_syntax(data: &str) -> Result<String, CodeError> {
    if data.is_empty() {
        return Err(CodeError::invalid("Code 128 data is empty"));
    }
    if !data.is_ascii() {
        return Err(CodeError::invalid(
            "Code 128 supports plain ASCII characters only",
        ));
    }
    let bytes = data.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n + 4);
    let mut set: Option<Set> = None;
    let mut i = 0;
    while i < n {
        let run = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        let at_start = i == 0;
        let at_end = i + run == n;
        let use_c = if at_start && at_end {
            run >= 2
        } else if at_start || at_end {
            run >= 4
        } else {
            run >= 6
        };
        if use_c {
            let even = run & !1;
            if set != Some(Set::C) {
                out.push(Set::C.marker());
                set = Some(Set::C);
            }
            out.push_str(&data[i..i + even]);
            i += even;
            continue;
        }
        let b = bytes[i];
        let needed = match b {
            0..=31 => Set::A,
            96..=127 => Set::B,
            _ => match set {
                Some(Set::A) => Set::A,
                _ => Set::B,
            },
        };
        if set != Some(needed) {
            out.push(needed.marker());
            set = Some(needed);
        }
        // barcoders spells DEL (0x7F) in set B as U+00F7.
        out.push(if b == 0x7F { '\u{00F7}' } else { char::from(b) });
        i += 1;
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------
// Payload and cache
// ------------------------------------------------------------------------------------------

/// The text a code item encodes: `data`, or the `WIFI:` string for content Wi-Fi (QR only)
/// (`codePayload`).
///
/// # Errors
/// [`CodeError::Wifi`] when the Wi-Fi settings are incomplete.
pub fn code_payload(item: &CodeItem) -> Result<String, CodeError> {
    if item.content == CodeContent::Wifi && item.symbology == Symbology::Qr {
        let default = WifiSettings::default();
        return wifi_payload(item.wifi.as_ref().unwrap_or(&default)).map_err(CodeError::Wifi);
    }
    Ok(item.data.clone())
}

type CacheKey = (Symbology, Option<QrEcc>, String);
type CacheEntry = Result<Arc<ModuleMatrix>, CodeError>;

/// A least-recently-used cache of encoded matrices, keyed by symbology, ECC (QR only) and
/// payload. Only data errors ([`CodeError::Invalid`]) are cached besides matrices.
#[derive(Debug, Default)]
pub struct CodeCache {
    entries: VecDeque<(CacheKey, CacheEntry)>,
}

impl CodeCache {
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    /// Encodes `codePayload(item)`, from the cache when possible (`codeMatrix`).
    ///
    /// # Errors
    /// The encoder's [`CodeError::Invalid`] (cached), or [`CodeError::Wifi`].
    pub fn get(&mut self, item: &CodeItem) -> Result<Arc<ModuleMatrix>, CodeError> {
        let data = code_payload(item)?;
        let ecc = (item.symbology == Symbology::Qr).then_some(item.ecc);
        let key = (item.symbology, ecc, data);
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            // LRU: move to the back.
            if let Some(hit) = self.entries.remove(i) {
                let out = hit.1.clone();
                self.entries.push_back(hit);
                return out;
            }
        }
        let entry = encode(&CodeSpec::new(item.symbology, key.2.clone(), item.ecc)).map(Arc::new);
        self.entries.push_back((key, entry.clone()));
        if self.entries.len() > CACHE_SIZE {
            self.entries.pop_front();
        }
        entry
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

static CACHE: Mutex<CodeCache> = Mutex::new(CodeCache::new());

/// [`CodeCache::get`] on a process-wide cache (`codeMatrix`).
///
/// # Errors
/// See [`CodeCache::get`].
pub fn code_matrix(item: &CodeItem) -> Result<Arc<ModuleMatrix>, CodeError> {
    CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(item)
}

// ------------------------------------------------------------------------------------------
// Geometry
// ------------------------------------------------------------------------------------------

/// A linear (1-D) code: one row, or every row identical, the core's rule (`isLinear`).
pub fn is_linear(m: &ModuleMatrix) -> bool {
    if m.height <= 1 || m.width == 0 {
        return true;
    }
    let w = m.width as usize;
    let Some(first) = m.modules.get(..w) else {
        return true;
    };
    m.modules.chunks(w).skip(1).all(|row| row == first)
}

/// Quiet zone in modules, `(along the label, across the tape)` (`quietModules`). `symbology`
/// tells a DataMatrix (1 module) from a QR code (the default for 2-D); linear codes are
/// detected from the matrix.
pub fn quiet_modules(
    m: &ModuleMatrix,
    quiet_zone: QuietZone,
    symbology: Option<Symbology>,
) -> (i64, i64) {
    if quiet_zone == QuietZone::None {
        return (0, 0);
    }
    let compact = quiet_zone == QuietZone::Compact;
    if is_linear(m) {
        return (if compact { 5 } else { 10 }, 0);
    }
    if symbology == Some(Symbology::Datamatrix) {
        return (1, 1);
    }
    if compact { (2, 2) } else { (4, 4) }
}

/// Footprint of a code, quiet zones included (`CodeSize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeSize {
    /// Along the label, dots.
    pub w: i64,
    /// Across the tape (linear codes: the band height), dots.
    pub h: i64,
}

/// Footprint of a code at `module_dots`; linear codes take the whole band (`codeSizeDots`).
pub fn code_size_dots(
    m: &ModuleMatrix,
    module_dots: i64,
    quiet_zone: QuietZone,
    band_dots: i64,
    symbology: Option<Symbology>,
) -> CodeSize {
    let md = module_dots.max(0);
    let (qx, qy) = quiet_modules(m, quiet_zone, symbology);
    let w = (i64::from(m.width) + 2 * qx) * md;
    if is_linear(m) {
        CodeSize { w, h: band_dots }
    } else {
        CodeSize {
            w,
            h: (i64::from(m.height) + 2 * qy) * md,
        }
    }
}

/// Largest integer module size whose symbol fits `band_dots` across the tape; 0 if impossible
/// (`maxModuleDots`). A 2-D symbol's vertical quiet zone must fit in the blank band plus
/// `margin_dots` of unprinted tape on each side; in compact mode any margin is zone enough.
/// Linear codes: `min(band, 255)`.
pub fn max_module_dots(
    m: &ModuleMatrix,
    band_dots: i64,
    quiet_zone: QuietZone,
    margin_dots: i64,
    symbology: Option<Symbology>,
) -> i64 {
    if is_linear(m) {
        return band_dots.clamp(0, 255);
    }
    let h = i64::from(m.height);
    if h <= 0 {
        return 0;
    }
    let fit = band_dots.div_euclid(h).clamp(0, 255);
    let (_, qy) = quiet_modules(m, quiet_zone, symbology);
    if qy == 0 || (quiet_zone == QuietZone::Compact && margin_dots > 0) {
        return fit;
    }
    let with_quiet = (band_dots + 2 * margin_dots.max(0)).div_euclid(h + 2 * qy);
    fit.min(with_quiet).max(0)
}

/// A module size given in 180 dpi dots, at `dpi`: the same physical size (`dotsAt`).
pub fn dots_at(dots180: i64, dpi: u16) -> i64 {
    js_round(dots180 as f64 * f64::from(dpi) / 180.0).max(1)
}

/// The module size a code prints with (`ModuleChoice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleChoice {
    /// Module size to print with, 1–255 (1 also when nothing fits: see `max`).
    pub md: u8,
    /// Largest size that fits (0: does not fit at all). Linear codes without a limit: 255.
    pub max: i64,
    /// A fixed size was lowered to `max`.
    pub reduced: bool,
}

/// Inputs of [`choose_module_dots`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleOptions {
    pub module_dots: ModuleSize,
    pub quiet_zone: QuietZone,
    pub symbology: Option<Symbology>,
    pub band_dots: i64,
    /// Unprinted tape beyond each band edge usable as vertical quiet zone (2-D codes).
    pub margin_dots: i64,
    /// Length available along the label (a fixed label length or a free frame).
    pub limit_w: Option<i64>,
    pub dpi: u16,
}

/// The module size the renderer prints a code with (`chooseModuleDots`). 2-D: auto = the
/// largest size that fits the band and `limit_w`; a fixed size is lowered to it. Linear: auto =
/// the largest size up to [`AUTO_LINEAR_MAX_DOTS`] that fits `limit_w`, else
/// [`AUTO_LINEAR_DEFAULT_DOTS`] (both physical, via [`dots_at`]); a fixed size is lowered only to
/// fit `limit_w`.
pub fn choose_module_dots(m: &ModuleMatrix, o: &ModuleOptions) -> ModuleChoice {
    let (qx, _) = quiet_modules(m, o.quiet_zone, o.symbology);
    let by_w = match o.limit_w {
        None => 255,
        Some(l) => l.max(0).div_euclid((i64::from(m.width) + 2 * qx).max(1)),
    };
    let fixed = match o.module_dots {
        ModuleSize::Auto => None,
        ModuleSize::Dots(n) => Some(i64::from(n).clamp(1, 255)),
    };
    let to_u8 = |v: i64| u8::try_from(v.clamp(1, 255)).unwrap_or(1);
    let max = if is_linear(m) {
        let max = by_w.min(255);
        if fixed.is_none() {
            let md = match o.limit_w {
                None => dots_at(AUTO_LINEAR_DEFAULT_DOTS, o.dpi),
                Some(_) => dots_at(AUTO_LINEAR_MAX_DOTS, o.dpi).min(max).max(1),
            };
            return ModuleChoice {
                md: to_u8(md),
                max,
                reduced: false,
            };
        }
        max
    } else {
        max_module_dots(m, o.band_dots, o.quiet_zone, o.margin_dots, o.symbology).min(by_w)
    };
    match fixed {
        None => ModuleChoice {
            md: to_u8(max.max(1)),
            max,
            reduced: false,
        },
        Some(f) => ModuleChoice {
            md: to_u8(f.min(max).max(1)),
            max,
            reduced: f > max && max >= 1,
        },
    }
}

/// `m` with `qx` light modules left and right and `qy` above and below (`padMatrix`).
pub fn pad_matrix(m: &ModuleMatrix, qx: u32, qy: u32) -> ModuleMatrix {
    if qx == 0 && qy == 0 {
        return m.clone();
    }
    let (sw, sh) = (m.width as usize, m.height as usize);
    let (qx, qy) = (qx as usize, qy as usize);
    let w = sw + 2 * qx;
    let h = sh + 2 * qy;
    let mut modules = vec![0u8; w * h];
    for y in 0..sh {
        for x in 0..sw {
            modules[(y + qy) * w + x + qx] = m.modules.get(y * sw + x).copied().unwrap_or(0);
        }
    }
    ModuleMatrix {
        width: u32::try_from(w).unwrap_or(u32::MAX),
        height: u32::try_from(h).unwrap_or(u32::MAX),
        modules,
    }
}

/// A copy of a linear code's first row repeated `rows` times (at least once) (`repeatRow`).
pub fn repeat_row(m: &ModuleMatrix, rows: u32) -> ModuleMatrix {
    let w = m.width as usize;
    let n = rows.max(1);
    let mut row: Vec<u8> = m.modules.iter().take(w).copied().collect();
    row.resize(w, 0);
    ModuleMatrix {
        width: m.width,
        height: n,
        modules: row.repeat(n as usize),
    }
}

/// The matrix rotated clockwise by `rotation` degrees (0/90/180/270; anything else is 0)
/// (`rotateMatrix`). 90°: new (x, y) ← old (y, h − 1 − x).
pub fn rotate_matrix(m: &ModuleMatrix, rotation: u16) -> ModuleMatrix {
    let (w, h) = (m.width as usize, m.height as usize);
    let at = |x: usize, y: usize| m.modules.get(y * w + x).copied().unwrap_or(0);
    match rotation {
        180 => ModuleMatrix {
            width: m.width,
            height: m.height,
            modules: (0..w * h)
                .map(|i| at(w - 1 - i % w, h - 1 - i / w))
                .collect(),
        },
        90 | 270 => {
            let nw = h;
            let modules = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % nw, i / nw);
                    if rotation == 90 {
                        at(y, h - 1 - x)
                    } else {
                        at(w - 1 - y, x)
                    }
                })
                .collect();
            ModuleMatrix {
                width: m.height,
                height: m.width,
                modules,
            }
        }
        _ => m.clone(),
    }
}

/// Unprinted tape on each side of the printable band, in dots (`tapeMarginDots`).
pub fn tape_margin_dots(tape_width_dots: i64, height_dots: i64) -> i64 {
    ((tape_width_dots - height_dots).div_euclid(2)).max(0)
}

// ------------------------------------------------------------------------------------------
// Readability, EAN-13, human-readable line
// ------------------------------------------------------------------------------------------

/// How well a code scans (`Readability`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Readability {
    Good,
    Ok,
    Poor,
}

impl Readability {
    /// `'good'` / `'ok'` / `'poor'`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Ok => "ok",
            Self::Poor => "poor",
        }
    }
}

/// A readability level with its user-facing sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadabilityRating {
    pub level: Readability,
    pub text: String,
}

/// How well a code at `md` dots per module scans at `dpi` (`readability`): ≥ 3 dots at 180 dpi
/// good for phones, 2 close up, less poor (6 / 4 at 360 dpi).
pub fn readability(md: i64, dpi: u16) -> ReadabilityRating {
    let at180 = md as f64 * 180.0 / f64::from(dpi);
    if at180 >= 3.0 - 1e-9 {
        return ReadabilityRating {
            level: Readability::Good,
            text: "Good for phone cameras.".into(),
        };
    }
    if at180 >= 2.0 - 1e-9 {
        return ReadabilityRating {
            level: Readability::Ok,
            text: "OK close up: hold the phone near the label.".into(),
        };
    }
    let size = if md == 1 {
        "1 dot".to_owned()
    } else {
        format!(
            "{md} dots ({} mm)",
            js_to_fixed(md as f64 * 25.4 / f64::from(dpi), 2)
        )
    };
    ReadabilityRating {
        level: Readability::Poor,
        text: format!(
            "Unreliable: modules of {size} often do not scan. Use wider tape or less data."
        ),
    }
}

/// EAN-13 check digit for the first 12 digits of `digits12` (`ean13CheckDigit`); `None` when
/// they are not 12 ASCII digits.
pub fn ean13_check_digit(digits12: &str) -> Option<u8> {
    let d = digits12.as_bytes().get(..12)?;
    if !d.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let sum: u32 = d
        .iter()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    u8::try_from((10 - sum % 10) % 10).ok()
}

/// Human-readable line under a linear code: `"5 901234 123457"` for EAN-13 (check digit filled
/// in), else the trimmed data (`humanReadable`).
pub fn human_readable(symbology: Symbology, data: &str) -> String {
    let d = data.trim();
    if symbology == Symbology::Ean13
        && (d.len() == 12 || d.len() == 13)
        && d.bytes().all(|b| b.is_ascii_digit())
    {
        let full = match ean13_check_digit(d) {
            Some(c) if d.len() == 12 => format!("{d}{c}"),
            _ => d.to_owned(),
        };
        return format!("{} {} {}", &full[..1], &full[1..7], &full[7..]);
    }
    d.to_owned()
}

/// Geometry of the human-readable strip under linear bars (from `prepareCode`). Dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeTextStrip {
    /// Strip height the text is fitted to: `max(mmToDots(1.6), round(band·0.2))`.
    pub text_h: i64,
    /// Space between bars and strip: `max(1, round(textH·0.25))`.
    pub gap: i64,
    /// Bar height: `band − textH − gap`.
    pub bar_h: i64,
    /// Top of the strip: `barH + gap`.
    pub top: i64,
}

/// The text strip for a `band`-tall linear code, or `None` when the bars would get shorter than
/// `max(mmToDots(3), band·0.5)` (the text is then left out with a warning).
pub fn code_text_strip(band: i64, dpi: u16) -> Option<CodeTextStrip> {
    let text_h = mm_to_dots(1.6, dpi).max(js_round(band as f64 * 0.2));
    let gap = js_round(text_h as f64 * 0.25).max(1);
    let bar_h = band - text_h - gap;
    let min_bar = (mm_to_dots(MIN_BAR_MM, dpi) as f64).max(band as f64 * 0.5);
    (bar_h as f64 >= min_bar).then_some(CodeTextStrip {
        text_h,
        gap,
        bar_h,
        top: bar_h + gap,
    })
}

/// The text renderer fits the human-readable line to `textH·f` px; when that block is wider
/// than the code box (`box_w_dots·f` px) it refits in mm mode at this target px:
/// `fontPx·w·f·0.98·(cap + desc)/block.width`.
pub fn code_text_refit_px(
    font_px: f64,
    block_width_px: f64,
    box_w_dots: i64,
    f: u32,
    cap: f64,
    desc: f64,
) -> f64 {
    font_px * box_w_dots as f64 * f64::from(f) * 0.98 * (cap + desc) / block_width_px
}

// ------------------------------------------------------------------------------------------
// prepareCode: size, blits and warnings
// ------------------------------------------------------------------------------------------

/// User-facing symbology names (`SYMBOLOGY_NAME`).
pub fn symbology_name(s: Symbology) -> &'static str {
    match s {
        Symbology::Qr => "QR code",
        Symbology::Code128 => "Code 128 barcode",
        Symbology::Ean13 => "EAN-13 barcode",
        Symbology::Datamatrix => "DataMatrix code",
    }
}

/// A warning raised while preparing a code. `code` is the studio's `RenderWarningCode`
/// (`code-invalid`, `code-too-small` or `content-overflow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeWarning {
    pub code: &'static str,
    pub message: String,
    pub blocking: bool,
}

/// One code blit relative to the item box (dots, unrotated). `m` already carries the quiet zone
/// as light modules, so it is blitted with `quiet_zone = false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlit {
    pub m: ModuleMatrix,
    pub dx: i64,
    pub dy: i64,
    pub md: u8,
}

/// The human-readable line the text module draws under linear bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeText {
    pub text: String,
    pub strip: CodeTextStrip,
}

/// Per-render inputs of [`plan_code`] (the renderer's `Ctx` fields it reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeContext {
    pub dpi: u16,
    /// Unprinted tape usable as vertical quiet zone (0 with a label frame or in a free frame).
    pub margin_dots: i64,
    /// Length a linear auto code may take on a fixed-length label (`sizeAutoLinearCodes`).
    pub auto_linear_w: Option<i64>,
}

/// A code measured and ready to draw (the code branch of the renderer's `Prepared`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodePlan {
    pub w: i64,
    pub h: i64,
    /// Code blits (empty when `invalid`).
    pub blits: Vec<CodeBlit>,
    /// Draw the dashed crossed box (`shapes::draw_invalid_code`) instead.
    pub invalid: bool,
    /// Human-readable line under linear bars, drawn in `CODE_TEXT_FONT` by the text module.
    pub text: Option<CodeText>,
    /// A linear auto-module code in the flow (re-planned by `sizeAutoLinearCodes`).
    pub auto_linear: bool,
    pub warnings: Vec<CodeWarning>,
}

fn warning(code: &'static str, message: String, blocking: bool) -> CodeWarning {
    CodeWarning {
        code,
        message,
        blocking,
    }
}

fn invalid_plan(w: i64, h: i64, warnings: Vec<CodeWarning>) -> CodePlan {
    CodePlan {
        w,
        h,
        blits: Vec::new(),
        invalid: true,
        text: None,
        auto_linear: false,
        warnings,
    }
}

/// Measures a code item in a `band`-tall slot (`prepareCode` without drawing): encodes it,
/// chooses the module size, lays out the padded matrix blits and the text strip, and collects
/// the warnings with the studio's strings. `max_w` is the free-frame width.
pub fn plan_code(item: &CodeItem, band: i64, c: &CodeContext, max_w: Option<i64>) -> CodePlan {
    let dpi = c.dpi;
    let name = symbology_name(item.symbology);
    let mut warnings = Vec::new();
    let encoded = if item.content != CodeContent::Wifi && item.data.is_empty() {
        Err(CodeError::Invalid("Enter the data to encode.".into()))
    } else {
        code_matrix(item)
    };
    let m = match encoded {
        Ok(m) => m,
        Err(e) => {
            warnings.push(warning("code-invalid", format!("{name}: {e}"), true));
            let side = band.min(mm_to_dots(12.0, dpi)).max(1);
            return invalid_plan(side, band, warnings);
        }
    };
    if item.content == CodeContent::Wifi
        && item.symbology == Symbology::Qr
        && let Some(wifi) = &item.wifi
    {
        for message in wifi_warnings(wifi) {
            warnings.push(warning("code-invalid", format!("{name}: {message}"), false));
        }
    }
    let auto = item.module_dots == ModuleSize::Auto;
    if item.module_dots == ModuleSize::Dots(0) {
        warnings.push(warning(
            "code-too-small",
            format!("{name}: the module size must be at least 1 dot."),
            true,
        ));
    }
    let (qx, qy) = quiet_modules(&m, item.quiet_zone, Some(item.symbology));
    let (mw, mh) = (i64::from(m.width), i64::from(m.height));
    let plural = |md: u8| if md == 1 { "" } else { "s" };

    if !is_linear(&m) {
        let choice = choose_module_dots(
            &m,
            &ModuleOptions {
                module_dots: item.module_dots,
                quiet_zone: item.quiet_zone,
                symbology: Some(item.symbology),
                band_dots: band,
                margin_dots: c.margin_dots,
                limit_w: max_w,
                dpi,
            },
        );
        if choice.max < 1 {
            let hint = if item.symbology == Symbology::Qr {
                "Shorten the data, lower the error correction or use wider tape."
            } else {
                "Shorten the data or use wider tape."
            };
            warnings.push(warning(
                "code-too-small",
                format!("{name}: {mh}×{mw} modules do not fit on this tape. {hint}"),
                true,
            ));
            return invalid_plan(band.max(1), band, warnings);
        }
        let md = choice.md;
        if choice.reduced {
            warnings.push(warning(
                "content-overflow",
                format!(
                    "{name} reduced to {md} dot{} per module to fit the tape.",
                    plural(md)
                ),
                false,
            ));
        }
        let mm = dots_to_mm(f64::from(md), dpi);
        if mm < MIN_SCANNABLE_MODULE_MM {
            warnings.push(warning(
                "code-too-small",
                format!(
                    "{name}: modules are {} mm and may not scan. Use wider tape or shorter data.",
                    js_to_fixed(mm, 2)
                ),
                false,
            ));
        }
        let mdi = i64::from(md);
        let sym_h = mh * mdi;
        let w = mw * mdi + 2 * qx * mdi;
        let h = band.min(sym_h + 2 * qy * mdi);
        let padded = pad_matrix(&m, qx as u32, qy as u32);
        return CodePlan {
            w,
            h,
            blits: vec![CodeBlit {
                m: padded,
                dx: 0,
                dy: (h - sym_h).div_euclid(2) - qy * mdi,
                md,
            }],
            invalid: false,
            text: None,
            auto_linear: false,
            warnings,
        };
    }

    // Linear: a frame limits the length; a fixed-length label limits auto codes.
    let limit_w = max_w.or(if auto { c.auto_linear_w } else { None });
    let choice = choose_module_dots(
        &m,
        &ModuleOptions {
            module_dots: item.module_dots,
            quiet_zone: item.quiet_zone,
            symbology: Some(item.symbology),
            band_dots: band,
            margin_dots: 0,
            limit_w,
            dpi,
        },
    );
    if let Some(mw_frame) = max_w
        && choice.max < 1
    {
        warnings.push(warning(
            "code-too-small",
            format!(
                "{name}: {mw} modules do not fit in its frame. Make the frame longer or shorten the data."
            ),
            true,
        ));
        return invalid_plan(mw_frame.max(1), band, warnings);
    }
    let md = choice.md;
    if choice.reduced {
        warnings.push(warning(
            "content-overflow",
            format!(
                "{name} reduced to {md} dot{} per module to fit its frame.",
                plural(md)
            ),
            false,
        ));
    }
    let mm = dots_to_mm(f64::from(md), dpi);
    if mm < MIN_SCANNABLE_MODULE_MM {
        warnings.push(warning(
            "code-too-small",
            format!(
                "{name}: bars are {} mm wide and may not scan. Use 2 or more dots per module.",
                js_to_fixed(mm, 2)
            ),
            false,
        ));
    }
    let mdi = i64::from(md);
    let w = mw * mdi + 2 * qx * mdi;
    let mut bar_h = band;
    let mut text = None;
    let label = if item.show_text {
        human_readable(item.symbology, &item.data)
    } else {
        String::new()
    };
    if !label.is_empty() {
        match code_text_strip(band, dpi) {
            Some(strip) => {
                bar_h = strip.bar_h;
                text = Some(CodeText { text: label, strip });
            }
            None => warnings.push(warning(
                "content-overflow",
                format!(
                    "{name}: there is not enough room for the text under the bars on this tape, so it is left out."
                ),
                false,
            )),
        }
    }
    let bar_mm = dots_to_mm(bar_h as f64, dpi);
    if bar_mm < MIN_BAR_MM {
        warnings.push(warning(
            "code-too-small",
            format!(
                "{name}: bars are only {} mm tall and may not scan.",
                js_to_fixed(bar_mm, 1)
            ),
            false,
        ));
    }
    // Bar height = rows × module; a second one-row blit tops up the remainder (the protected
    // dots of the first blit stay untouched).
    let rows = bar_h.div_euclid(mdi).max(1);
    let qxu = qx as u32;
    let mut blits = vec![CodeBlit {
        m: pad_matrix(&repeat_row(&m, u32::try_from(rows).unwrap_or(1)), qxu, 0),
        dx: 0,
        dy: 0,
        md,
    }];
    if rows * mdi < bar_h {
        blits.push(CodeBlit {
            m: pad_matrix(&repeat_row(&m, 1), qxu, 0),
            dx: 0,
            dy: bar_h - mdi,
            md,
        });
    }
    CodePlan {
        w,
        h: band,
        blits,
        invalid: false,
        text,
        auto_linear: auto && max_w.is_none(),
        warnings,
    }
}

// ------------------------------------------------------------------------------------------
// Number formatting
// ------------------------------------------------------------------------------------------

/// JavaScript's `Number.prototype.toFixed(digits)` for finite values: exact decimal value,
/// halves rounded away from zero (Rust's `{:.N}` rounds exact halves to even).
pub fn js_to_fixed(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    // Enough digits to hold the exact binary expansion of values down to ~1e-24.
    let exact = format!("{:.*}", digits + 80, v.abs());
    let (int_part, frac) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let mut digits_vec: Vec<u8> = int_part
        .bytes()
        .chain(frac.bytes().take(digits))
        .map(|b| b - b'0')
        .collect();
    let round_up = frac.as_bytes().get(digits).is_some_and(|&b| b >= b'5');
    if round_up {
        let mut i = digits_vec.len();
        loop {
            if i == 0 {
                digits_vec.insert(0, 1);
                break;
            }
            i -= 1;
            if digits_vec[i] == 9 {
                digits_vec[i] = 0;
            } else {
                digits_vec[i] += 1;
                break;
            }
        }
    }
    let s: String = digits_vec.iter().map(|d| char::from(b'0' + d)).collect();
    let split = s.len() - digits;
    let body = if digits == 0 {
        s
    } else {
        format!("{}.{}", &s[..split], &s[split..])
    };
    if v < 0.0 { format!("-{body}") } else { body }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_matches_js() {
        assert_eq!(js_to_fixed(0.125, 2), "0.13");
        assert_eq!(js_to_fixed(0.25, 1), "0.3");
        assert_eq!(js_to_fixed(1.005, 2), "1.00"); // 1.00499999…
        assert_eq!(js_to_fixed(9.995, 2), "9.99");
        assert_eq!(js_to_fixed(9.9951, 2), "10.00");
        assert_eq!(js_to_fixed(0.0, 2), "0.00");
        assert_eq!(js_to_fixed(3.0, 0), "3");
        assert_eq!(js_to_fixed(-0.001, 2), "-0.00");
        assert_eq!(js_to_fixed(-0.0, 2), "0.00");
        assert_eq!(js_to_fixed(-1.25, 1), "-1.3");
    }
}
