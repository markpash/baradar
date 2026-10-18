//! Lenient validation of untrusted label JSON (port of `validateDoc`, the `Reader` class,
//! `LIMITS` and the regexes in `web/src/doc/schema.ts`).
//!
//! Anything readable is repaired rather than rejected: numbers are clamped, unknown enum values
//! fall back to defaults, items of unknown kinds are dropped, duplicate or missing ids are
//! regenerated, and every repair is listed as a notice. Character limits count UTF-16 code units,
//! as JavaScript string lengths do.

use std::cell::RefCell;

use serde_json::{Map, Value};

use super::schema::*;

type Obj = Map<String, Value>;

/// An inclusive numeric range in [`LIMITS`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinMax {
    pub min: f64,
    pub max: f64,
}

const fn mm(min: f64, max: f64) -> MinMax {
    MinMax { min, max }
}

/// Valid ranges used by [`validate_doc`] (and by editors for their controls). `LIMITS`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub length_mm: MinMax,
    pub margin_mm: MinMax,
    pub gap_mm: MinMax,
    pub size_mm: MinMax,
    /// Text size in points (em; 1 pt = 2.5 dots at 180 dpi).
    pub size_pt: MinMax,
    pub line_height: MinMax,
    pub module_dots: MinMax,
    pub width_mm: MinMax,
    pub stroke_mm: MinMax,
    pub frame_thickness_mm: MinMax,
    pub frame_radius_mm: MinMax,
    pub frame_inset_mm: MinMax,
    pub copies: MinMax,
    pub threshold: MinMax,
    pub brightness: MinMax,
    pub gamma_x100: MinMax,
    /// Characters per text item / code payload.
    pub text_chars: usize,
    /// Blocks per label.
    pub items: usize,
    /// Characters of text + code data across the whole label.
    pub total_chars: usize,
    pub ssid_chars: usize,
    pub wifi_password_chars: usize,
    pub batch_rows: usize,
    pub batch_columns: usize,
    pub batch_cell_chars: usize,
    pub batch_counters: usize,
    /// Labels when a batch has no rows (counters only).
    pub batch_count: MinMax,
    /// |start|, |step| of a counter.
    pub counter_value: f64,
    pub counter_pad: MinMax,
    /// Characters of a batch table across all cells.
    pub batch_total_chars: usize,
    /// Custom font names.
    pub font_name_chars: usize,
}

/// `LIMITS` (verbatim from schema.ts).
pub const LIMITS: Limits = Limits {
    length_mm: mm(4.0, 1000.0),
    margin_mm: mm(0.0, 100.0),
    gap_mm: mm(0.0, 100.0),
    size_mm: mm(0.5, 100.0),
    size_pt: mm(4.0, 144.0),
    line_height: mm(0.5, 3.0),
    module_dots: mm(1.0, 20.0),
    width_mm: mm(0.0, 1000.0),
    stroke_mm: mm(0.1, 20.0),
    frame_thickness_mm: mm(0.0, 10.0),
    frame_radius_mm: mm(0.0, 50.0),
    frame_inset_mm: mm(0.0, 20.0),
    copies: mm(1.0, 99.0),
    threshold: mm(0.0, 255.0),
    brightness: mm(-100.0, 100.0),
    gamma_x100: mm(10.0, 1000.0),
    text_chars: 4000,
    items: 200,
    total_chars: 40_000,
    ssid_chars: 64,
    wifi_password_chars: 64,
    batch_rows: 500,
    batch_columns: 20,
    batch_cell_chars: 500,
    batch_counters: 4,
    batch_count: mm(1.0, 500.0),
    counter_value: 1_000_000_000.0,
    counter_pad: mm(0.0, 12.0),
    batch_total_chars: 200_000,
    font_name_chars: 100,
};

/// `VARIABLE_NAME_RE` (`/^[A-Za-z_][A-Za-z0-9_]{0,31}$/`).
pub fn is_variable_name(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b.first(), Some(c) if c.is_ascii_alphabetic() || *c == b'_')
        && b.len() <= 32
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// `CONTENT_REF_RE` (`/^sha256-[0-9a-f]{32}$/`).
pub fn is_content_ref(s: &str) -> bool {
    s.strip_prefix("sha256-")
        .is_some_and(|h| h.len() == 32 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')))
}

/// `MODEL_NAME_RE` (`/^[A-Za-z0-9][A-Za-z0-9 ._-]{0,31}$/`).
pub fn is_model_name(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b.first(), Some(c) if c.is_ascii_alphanumeric())
        && b.len() <= 32
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b' ' | b'.' | b'_' | b'-'))
}

/// `ID_RE` (`/^[A-Za-z0-9_-]{1,64}$/`): doc and item ids.
pub fn is_valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// `MEDIA_ID_RE` (`/^[a-z0-9-]{1,32}$/`).
fn is_media_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// `COLOR_RE` (`/^#[0-9a-f]{3,8}$/i`).
fn is_color(s: &str) -> bool {
    s.strip_prefix('#')
        .is_some_and(|h| (3..=8).contains(&h.len()) && h.bytes().all(|c| c.is_ascii_hexdigit()))
}

/// A local font's PostScript name: `/^[\x20-\x7e]{1,100}$/` without `"`, `'` or `\`.
fn is_postscript_name(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes()
            .all(|c| (0x20..=0x7e).contains(&c) && !matches!(c, b'"' | b'\'' | b'\\'))
}

/// JavaScript semantics the validator reproduces: number formatting, `JSON.stringify`,
/// `String(v)`, `Number(str)`, `Math.round`, UTF-16 lengths and `Date.parse`.
pub(crate) mod js {
    use serde_json::Value;

    /// `String(n)` / `Number.prototype.toString()` for a double.
    pub fn num(n: f64) -> String {
        if n.is_nan() {
            return "NaN".into();
        }
        if n.is_infinite() {
            return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
        }
        if n == 0.0 {
            return "0".into();
        }
        let sign = if n < 0.0 { "-" } else { "" };
        // Rust's `{:e}` gives the shortest round-trip digits, like ECMAScript's Number::toString.
        let e = format!("{:e}", n.abs());
        let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
        let digits: String = mant.chars().filter(|c| *c != '.').collect();
        let exp: i32 = exp.parse().unwrap_or(0);
        let k = digits.len() as i32;
        let point = exp + 1; // ECMAScript's n
        let body = if k <= point && point <= 21 {
            format!("{digits}{}", "0".repeat((point - k) as usize))
        } else if 0 < point && point <= 21 {
            let (a, b) = digits.split_at(point as usize);
            format!("{a}.{b}")
        } else if -6 < point && point <= 0 {
            format!("0.{}{digits}", "0".repeat((-point) as usize))
        } else {
            let (a, b) = digits.split_at(1);
            let frac = if b.is_empty() {
                String::new()
            } else {
                format!(".{b}")
            };
            let s = if point > 0 { "+" } else { "-" };
            format!("{a}{frac}e{s}{}", (point - 1).abs())
        };
        format!("{sign}{body}")
    }

    /// `JSON.stringify(v)` (compact).
    pub fn json(v: &Value) -> String {
        match v {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .map_or("null".into(), num),
            Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
            Value::Array(a) => format!("[{}]", a.iter().map(json).collect::<Vec<_>>().join(",")),
            Value::Object(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        json(v)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    /// `String(v)` for a JSON value.
    pub fn string(v: &Value) -> String {
        match v {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => num(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => s.clone(),
            Value::Array(a) => a
                .iter()
                .map(|e| {
                    if e.is_null() {
                        String::new()
                    } else {
                        string(e)
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
            Value::Object(_) => "[object Object]".into(),
        }
    }

    /// `shortJson(v, max)`: JSON of an untrusted value, at most `max` UTF-16 units.
    pub fn short_json(v: &Value, max: usize) -> String {
        let s = json(v);
        if u16_len(&s) > max {
            format!("{}…", truncate_u16(&s, max - 1))
        } else {
            s
        }
    }

    /// `s.length` (UTF-16 code units).
    pub fn u16_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    /// `s.slice(0, max)` on UTF-16 code units. A surrogate pair that would be split is dropped
    /// whole (Rust strings cannot hold a lone surrogate).
    pub fn truncate_u16(s: &str, max: usize) -> String {
        let mut n = 0;
        let mut end = 0;
        for (i, c) in s.char_indices() {
            n += c.len_utf16();
            if n > max {
                break;
            }
            end = i + c.len_utf8();
        }
        s[..end].to_string()
    }

    /// `String.prototype.trim()`.
    pub fn trim(s: &str) -> &str {
        s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
    }

    /// `Math.round`.
    pub fn round(x: f64) -> f64 {
        let f = x.floor();
        if x - f >= 0.5 { f + 1.0 } else { f }
    }

    /// `Number(s)` for a string (NaN when JavaScript would give NaN).
    pub fn number_from_str(s: &str) -> f64 {
        let t = trim(s);
        if t.is_empty() {
            return 0.0;
        }
        for (prefix, radix) in [
            ("0x", 16),
            ("0X", 16),
            ("0o", 8),
            ("0O", 8),
            ("0b", 2),
            ("0B", 2),
        ] {
            if let Some(d) = t.strip_prefix(prefix) {
                if d.is_empty() || !d.chars().all(|c| c.is_digit(radix)) {
                    return f64::NAN;
                }
                return d.chars().fold(0.0, |acc, c| {
                    acc * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64
                });
            }
        }
        match t {
            "Infinity" | "+Infinity" => return f64::INFINITY,
            "-Infinity" => return f64::NEG_INFINITY,
            _ => {}
        }
        if !t
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            return f64::NAN;
        }
        t.parse::<f64>().unwrap_or(f64::NAN)
    }

    /// `isTimestamp`: a string of at most 40 UTF-16 units that `Date.parse` accepts. Only the ISO
    /// 8601 forms (and a space instead of `T`) are recognised, not V8's legacy date formats.
    pub fn is_timestamp(v: &Value) -> bool {
        matches!(v, Value::String(s) if u16_len(s) <= 40 && parse_iso_date(s))
    }

    fn take_digits(s: &[u8], i: &mut usize, n: usize) -> Option<u32> {
        let d = s.get(*i..*i + n)?;
        if !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *i += n;
        Some(d.iter().fold(0, |a, c| a * 10 + u32::from(c - b'0')))
    }

    fn parse_iso_date(s: &str) -> bool {
        let b = s.as_bytes();
        let mut i = 0;
        // Year: YYYY or ±YYYYYY.
        if matches!(b.first(), Some(b'+' | b'-')) {
            i = 1;
            if take_digits(b, &mut i, 6).is_none() {
                return false;
            }
        } else if take_digits(b, &mut i, 4).is_none() {
            return false;
        }
        let mut month = 1;
        let mut day = 1;
        if b.get(i) == Some(&b'-') {
            i += 1;
            let Some(m) = take_digits(b, &mut i, 2) else {
                return false;
            };
            month = m;
            if b.get(i) == Some(&b'-') {
                i += 1;
                let Some(d) = take_digits(b, &mut i, 2) else {
                    return false;
                };
                day = d;
            }
        }
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return false;
        }
        if i == b.len() {
            return true;
        }
        if !matches!(b[i], b'T' | b't' | b' ') {
            return false;
        }
        i += 1;
        let Some(h) = take_digits(b, &mut i, 2) else {
            return false;
        };
        if b.get(i) != Some(&b':') {
            return false;
        }
        i += 1;
        let Some(min) = take_digits(b, &mut i, 2) else {
            return false;
        };
        let mut sec = 0;
        let mut frac = false;
        if b.get(i) == Some(&b':') {
            i += 1;
            let Some(sv) = take_digits(b, &mut i, 2) else {
                return false;
            };
            sec = sv;
            if b.get(i) == Some(&b'.') {
                i += 1;
                let start = i;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == start {
                    return false;
                }
                frac = b[start..i].iter().any(|c| *c != b'0');
            }
        }
        if h > 24 || min > 59 || sec > 59 || (h == 24 && (min > 0 || sec > 0 || frac)) {
            return false;
        }
        match b.get(i) {
            None => true,
            Some(b'Z' | b'z') => i + 1 == b.len(),
            Some(b'+' | b'-') => {
                i += 1;
                let Some(oh) = take_digits(b, &mut i, 2) else {
                    return false;
                };
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                let Some(om) = take_digits(b, &mut i, 2) else {
                    return false;
                };
                i == b.len() && oh <= 23 && om <= 59
            }
            _ => false,
        }
    }
}

/// A validated document plus the repairs made while reading it.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub doc: LabelDoc,
    /// One line per repair, e.g. `"print: copies 500 out of range; clamped to 99"`.
    pub notices: Vec<String>,
}

/// Collects repairs while reading a document (`Reader`).
struct Reader<'n> {
    at: String,
    /// Shared by nested readers so every repair of one document lands in one list.
    notes: &'n RefCell<Vec<String>>,
}

impl<'n> Reader<'n> {
    fn at(&self, at: impl Into<String>) -> Reader<'n> {
        Reader {
            at: at.into(),
            notes: self.notes,
        }
    }

    fn note(&self, msg: impl AsRef<str>) {
        self.notes
            .borrow_mut()
            .push(format!("{}: {}", self.at, msg.as_ref()));
    }

    fn num(&self, o: &Obj, key: &str, min: f64, max: f64, dflt: f64, int: bool) -> f64 {
        let v = o.get(key);
        let mut n = match v {
            Some(Value::Number(x)) => x.as_f64().unwrap_or(f64::NAN),
            Some(Value::String(s)) if !js::trim(s).is_empty() => js::number_from_str(s),
            _ => f64::NAN,
        };
        if !n.is_finite() {
            if v.is_some() {
                self.note(format!(
                    "{key} was not a number; reset to {}",
                    js::num(dflt)
                ));
            }
            return dflt;
        }
        if int {
            n = js::round(n);
        }
        if n < min || n > max {
            let c = max.min(min.max(n));
            self.note(format!(
                "{key} {} out of range; clamped to {}",
                js::num(n),
                js::num(c)
            ));
            return c;
        }
        n
    }

    fn range(&self, o: &Obj, key: &str, r: MinMax, dflt: f64) -> f64 {
        self.num(o, key, r.min, r.max, dflt, false)
    }

    fn int(&self, o: &Obj, key: &str, r: MinMax, dflt: f64) -> f64 {
        self.num(o, key, r.min, r.max, dflt, true)
    }

    fn bool(&self, o: &Obj, key: &str, dflt: bool) -> bool {
        match o.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(_) => {
                self.note(format!("{key} was not true/false; reset to {dflt}"));
                dflt
            }
            None => dflt,
        }
    }

    fn str(&self, o: &Obj, key: &str, dflt: &str, max_len: usize) -> String {
        match o.get(key) {
            Some(Value::String(s)) => {
                if js::u16_len(s) > max_len {
                    self.note(format!("{key} truncated to {max_len} characters"));
                    js::truncate_u16(s, max_len)
                } else {
                    s.clone()
                }
            }
            Some(Value::Number(n)) => js::num(n.as_f64().unwrap_or(f64::NAN)),
            Some(_) => {
                self.note(format!("{key} was not text; reset"));
                dflt.to_string()
            }
            None => dflt.to_string(),
        }
    }

    /// `oneOf` for string enums: `table` maps each allowed string to its value.
    fn one_of<T: Copy + PartialEq>(&self, o: &Obj, key: &str, table: &[(&str, T)], dflt: T) -> T {
        let v = o.get(key);
        if let Some(Value::String(s)) = v
            && let Some((_, t)) = table.iter().find(|(name, _)| name == s)
        {
            return *t;
        }
        if let Some(v) = v {
            let d = table
                .iter()
                .find(|(_, t)| *t == dflt)
                .map_or("", |(n, _)| n);
            self.note(format!(
                "{key} {} is not supported; using {}",
                js::json(v),
                js::json(&Value::String(d.into()))
            ));
        }
        dflt
    }

    /// `obj`: a nested object, or `{}` (with a note when the value is present but not an object).
    fn obj<'o>(&self, o: &'o Obj, key: &str, empty: &'o Obj) -> &'o Obj {
        match o.get(key) {
            Some(Value::Object(m)) => m,
            Some(_) => {
                self.note(format!("{key} was malformed; defaults used"));
                empty
            }
            None => empty,
        }
    }
}

const ALIGNS: [(&str, Align); 3] = [
    ("start", Align::Start),
    ("center", Align::Center),
    ("end", Align::End),
];
const SYMBOLOGIES: [(&str, Symbology); 4] = [
    ("qr", Symbology::Qr),
    ("code128", Symbology::Code128),
    ("ean13", Symbology::Ean13),
    ("datamatrix", Symbology::Datamatrix),
];
const CONTENTS: [(&str, CodeContent); 2] =
    [("text", CodeContent::Text), ("wifi", CodeContent::Wifi)];
const SECURITIES: [(&str, WifiSecurity); 3] = [
    ("wpa", WifiSecurity::Wpa),
    ("wep", WifiSecurity::Wep),
    ("open", WifiSecurity::Open),
];
const QUIET_ZONES: [(&str, QuietZone); 3] = [
    ("standard", QuietZone::Standard),
    ("compact", QuietZone::Compact),
    ("none", QuietZone::None),
];
const DATE_FORMATS: [(&str, DateFormat); 4] = [
    ("iso", DateFormat::Iso),
    ("dmy", DateFormat::Dmy),
    ("mdy", DateFormat::Mdy),
    ("long", DateFormat::Long),
];
const ECCS: [(&str, QrEcc); 4] = [
    ("L", QrEcc::L),
    ("M", QrEcc::M),
    ("Q", QrEcc::Q),
    ("H", QrEcc::H),
];
const DITHERS: [(&str, DitherKind); 5] = [
    ("threshold", DitherKind::Threshold),
    ("floyd-steinberg", DitherKind::FloydSteinberg),
    ("atkinson", DitherKind::Atkinson),
    ("bayer4", DitherKind::Bayer4),
    ("bayer8", DitherKind::Bayer8),
];
const SHAPES: [(&str, ShapeKind); 3] = [
    ("rect", ShapeKind::Rect),
    ("ellipse", ShapeKind::Ellipse),
    ("line", ShapeKind::Line),
];
const ROTATIONS: [u16; 4] = [0, 90, 180, 270];

fn read_size(r: &Reader, o: &Obj, key: &str) -> ItemSize {
    match o.get(key) {
        Some(Value::Object(v)) if v.get("mode").and_then(Value::as_str) == Some("mm") => {
            ItemSize::Mm {
                mm: r.range(v, "mm", LIMITS.size_mm, 6.0),
            }
        }
        Some(Value::Object(v)) if v.get("mode").and_then(Value::as_str) == Some("fit") => {
            ItemSize::Fit
        }
        Some(Value::Number(_)) => ItemSize::Mm {
            mm: r.range(o, key, LIMITS.size_mm, 6.0),
        },
        Some(_) => {
            r.note(format!("{key} was malformed; fit to tape"));
            ItemSize::Fit
        }
        None => ItemSize::Fit,
    }
}

/// A text size: an `ItemSize` or a point size (schema 3). `readTextSize`.
fn read_text_size(r: &Reader, o: &Obj) -> TextSize {
    if let Some(Value::Object(v)) = o.get("size")
        && v.get("mode").and_then(Value::as_str) == Some("pt")
    {
        return TextSize::Pt {
            pt: r.range(v, "pt", LIMITS.size_pt, 12.0),
        };
    }
    match read_size(r, o, "size") {
        ItemSize::Fit => TextSize::Fit,
        ItemSize::Mm { mm } => TextSize::Mm { mm },
    }
}

fn nearest_weight(v: Option<&Value>) -> Option<u16> {
    let v = v?.as_f64().filter(|f| f.is_finite())?;
    Some(FONT_WEIGHTS.iter().fold(400, |best, &w| {
        if (f64::from(w) - v).abs() < (f64::from(best) - v).abs() {
            w
        } else {
            best
        }
    }))
}

fn read_item_frame(r: &Reader, o: &Obj) -> Option<ItemFrame> {
    let v = o.get("frame")?;
    let Value::Object(v) = v else {
        r.note("frame was malformed; removed");
        return None;
    };
    let x_mm = r.num(v, "xMm", -1000.0, 1000.0, 0.0, false);
    let y_mm = r.num(v, "yMm", -100.0, 100.0, 0.0, false);
    let w_mm = r.num(v, "wMm", 0.5, 1000.0, 10.0, false);
    let h_mm = r.num(v, "hMm", 0.5, 100.0, 10.0, false);
    let rotation = match v.get("rotation") {
        None => 0,
        Some(x) => match x
            .as_f64()
            .and_then(|f| ROTATIONS.iter().copied().find(|&a| f64::from(a) == f))
        {
            Some(a) => a,
            None => {
                r.note(format!(
                    "rotation {} is not supported; using 0",
                    js::json(x)
                ));
                0
            }
        },
    };
    Some(ItemFrame {
        x_mm,
        y_mm,
        w_mm,
        h_mm,
        rotation,
    })
}

fn read_font_source(r: &Reader, o: &Obj) -> Option<FontSource> {
    let v = o.get("customFont")?;
    if let Value::Object(v) = v {
        let family = match v.get("family") {
            Some(Value::String(f)) => js::truncate_u16(js::trim(f), LIMITS.font_name_chars),
            _ => String::new(),
        };
        if !family.is_empty() {
            let kind = v.get("kind").and_then(Value::as_str);
            if kind == Some("user")
                && let Some(Value::String(rf)) = v.get("ref")
                && is_content_ref(rf)
            {
                return Some(FontSource::User {
                    reference: rf.clone(),
                    family,
                });
            }
            if kind == Some("local")
                && let Some(Value::String(ps)) = v.get("postscriptName")
                && is_postscript_name(ps)
            {
                return Some(FontSource::Local {
                    postscript_name: ps.clone(),
                    family,
                });
            }
        }
    }
    r.note("customFont was malformed; the bundled font is used");
    None
}

fn read_wifi(r: &Reader, o: &Obj) -> WifiSettings {
    WifiSettings {
        ssid: r.str(o, "ssid", "", LIMITS.ssid_chars),
        password: r.str(o, "password", "", LIMITS.wifi_password_chars),
        security: r.one_of(o, "security", &SECURITIES, WifiSecurity::Wpa),
        hidden: r.bool(o, "hidden", false),
    }
}

/// Schema-2 modes; a schema-1 style boolean is accepted too (true → standard, false → none).
fn read_quiet_zone(r: &Reader, o: &Obj) -> QuietZone {
    match o.get("quietZone") {
        Some(Value::Bool(true)) => QuietZone::Standard,
        Some(Value::Bool(false)) => QuietZone::None,
        _ => r.one_of(o, "quietZone", &QUIET_ZONES, QuietZone::Standard),
    }
}

fn read_batch(r: &Reader, o: &Obj) -> BatchData {
    let mut names: Vec<String> = Vec::new();
    let mut columns: Vec<String> = Vec::new();
    let raw_cols: &[Value] = match o.get("columns") {
        Some(Value::Array(a)) => a,
        Some(_) => {
            r.note("columns was malformed; removed");
            &[]
        }
        None => &[],
    };
    for c in raw_cols.iter().take(LIMITS.batch_columns) {
        let name = c.as_str().map(js::trim).unwrap_or("");
        if !is_variable_name(name) || names.iter().any(|n| n == name) {
            r.note(format!(
                "column {} is not a valid, unique variable name; renamed",
                js::short_json(c, 40)
            ));
            let mut i = columns.len() + 1;
            while names.contains(&format!("col{i}")) {
                i += 1;
            }
            columns.push(format!("col{i}"));
            names.push(format!("col{i}"));
            continue;
        }
        names.push(name.to_string());
        columns.push(name.to_string());
    }
    if raw_cols.len() > LIMITS.batch_columns {
        r.note(format!(
            "{} columns; only the first {} were kept",
            raw_cols.len(),
            LIMITS.batch_columns
        ));
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut raw_rows: &[Value] = match o.get("rows") {
        Some(Value::Array(a)) => a,
        Some(_) => {
            r.note("rows was malformed; removed");
            &[]
        }
        None => &[],
    };
    if raw_rows.len() > LIMITS.batch_rows {
        r.note(format!(
            "{} rows; only the first {} were kept",
            raw_rows.len(),
            LIMITS.batch_rows
        ));
        raw_rows = &raw_rows[..LIMITS.batch_rows];
    }
    let mut chars = 0usize;
    for row in raw_rows {
        let cells: &[Value] = row.as_array().map_or(&[], Vec::as_slice);
        let out: Vec<String> = (0..columns.len())
            .map(|j| {
                let v = match cells.get(j) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n
                        .as_f64()
                        .filter(|f| f.is_finite())
                        .map(js::num)
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if js::u16_len(&v) > LIMITS.batch_cell_chars {
                    js::truncate_u16(&v, LIMITS.batch_cell_chars)
                } else {
                    v
                }
            })
            .collect();
        chars += out.iter().map(|c| js::u16_len(c)).sum::<usize>();
        if chars > LIMITS.batch_total_chars {
            r.note(format!(
                "more than {} characters of batch data; rows from {} on were dropped",
                LIMITS.batch_total_chars,
                rows.len() + 1
            ));
            break;
        }
        rows.push(out);
    }

    let mut counters: Vec<BatchCounter> = Vec::new();
    let raw_counters: &[Value] = o
        .get("counters")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    for c in raw_counters.iter().take(LIMITS.batch_counters) {
        let name = c
            .as_object()
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str);
        let (Some(m), Some(name)) = (c.as_object(), name) else {
            r.note(format!(
                "counter {} is malformed or its name is taken; dropped",
                js::short_json(c, 40)
            ));
            continue;
        };
        if !is_variable_name(name) || names.iter().any(|n| n == name) {
            r.note(format!(
                "counter {} is malformed or its name is taken; dropped",
                js::short_json(c, 40)
            ));
            continue;
        }
        let rc = r.at(format!("counter {name}"));
        let lim = LIMITS.counter_value;
        names.push(name.to_string());
        counters.push(BatchCounter {
            name: name.to_string(),
            start: rc.num(m, "start", -lim, lim, 1.0, true) as i64,
            step: rc.num(m, "step", -lim, lim, 1.0, true) as i64,
            pad: rc.int(m, "pad", LIMITS.counter_pad, 0.0) as u8,
        });
    }

    BatchData {
        enabled: r.bool(o, "enabled", false),
        columns,
        rows,
        count: r.int(o, "count", LIMITS.batch_count, 10.0) as u32,
        counters,
        date_format: r.one_of(o, "dateFormat", &DATE_FORMATS, DateFormat::Iso),
    }
}

fn read_item(r: &Reader, o: &Obj, id: String) -> Option<Item> {
    let frame = read_item_frame(r, o);
    let empty = Obj::new();
    Some(match o.get("kind").and_then(Value::as_str)? {
        "text" => {
            let weight = nearest_weight(o.get("fontWeight"));
            if weight.is_none() && o.contains_key("fontWeight") {
                r.note("fontWeight was malformed; using 600");
            }
            let custom_font = read_font_source(r, o);
            let text = r.str(o, "text", "", LIMITS.text_chars);
            let fams: Vec<(&str, &str)> = FONT_FAMILY_IDS.iter().map(|f| (*f, *f)).collect();
            let font_family = r.one_of(o, "fontFamily", &fams, "fira-sans").to_string();
            Item::Text(TextItem {
                id,
                frame,
                text,
                font_family,
                custom_font,
                font_weight: weight.unwrap_or(600),
                italic: r.bool(o, "italic", false),
                size: read_text_size(r, o),
                align: r.one_of(o, "align", &ALIGNS, Align::Center),
                line_height: r.range(o, "lineHeight", LIMITS.line_height, 1.1),
                invert: r.bool(o, "invert", false),
                clip_tall: o.get("clipTall") == Some(&Value::Bool(true)),
            })
        }
        "icon" => Item::Icon(IconItem {
            id,
            frame,
            icon_id: r.str(o, "iconId", "bolt", 64),
            size: read_size(r, o, "size"),
        }),
        "code" => {
            let wifi = if o.contains_key("wifi") {
                let at = r.at("wifi");
                Some(read_wifi(&at, r.obj(o, "wifi", &empty)))
            } else {
                None
            };
            let symbology = r.one_of(o, "symbology", &SYMBOLOGIES, Symbology::Qr);
            let content = r.one_of(o, "content", &CONTENTS, CodeContent::Text);
            let data = r.str(o, "data", "", LIMITS.text_chars);
            let module_dots = if o.get("moduleDots").and_then(Value::as_str) == Some("auto") {
                ModuleSize::Auto
            } else {
                ModuleSize::Dots(r.int(o, "moduleDots", LIMITS.module_dots, 3.0) as u8)
            };
            Item::Code(CodeItem {
                id,
                frame,
                symbology,
                content,
                data,
                wifi,
                module_dots,
                quiet_zone: read_quiet_zone(r, o),
                ecc: r.one_of(o, "ecc", &ECCS, QrEcc::M),
                show_text: r.bool(o, "showText", false),
            })
        }
        "image" => {
            let adjust = r.obj(o, "adjust", &empty);
            let ra = r.at("image adjust");
            let blob_ref = r.str(o, "blobRef", "", 256);
            let size = read_size(r, o, "size");
            let dither = r.one_of(o, "dither", &DITHERS, DitherKind::FloydSteinberg);
            let adjust = ImageAdjust {
                brightness: ra.int(adjust, "brightness", LIMITS.brightness, 0.0) as i32,
                contrast: ra.int(adjust, "contrast", LIMITS.brightness, 0.0) as i32,
                gamma_x100: ra.int(adjust, "gammaX100", LIMITS.gamma_x100, 100.0) as u16,
                invert: ra.bool(adjust, "invert", false),
                level: ra.int(adjust, "level", LIMITS.threshold, 128.0) as u8,
            };
            let data_url = match o.get("dataUrl") {
                Some(Value::String(s)) if s.starts_with("data:image/") => Some(s.clone()),
                Some(_) => {
                    r.note("dataUrl is not an inlined image; removed");
                    None
                }
                None => None,
            };
            Item::Image(ImageItem {
                id,
                frame,
                blob_ref,
                data_url,
                size,
                dither,
                adjust,
            })
        }
        "shape" => Item::Shape(ShapeItem {
            id,
            frame,
            shape: r.one_of(o, "shape", &SHAPES, ShapeKind::Rect),
            width_mm: r.num(o, "widthMm", 0.1, LIMITS.width_mm.max, 10.0, false),
            size: read_size(r, o, "size"),
            stroke_mm: r.range(o, "strokeMm", LIMITS.stroke_mm, 0.5),
            fill: r.bool(o, "fill", false),
        }),
        "spacer" => Item::Spacer(SpacerItem {
            id,
            frame,
            width_mm: r.range(o, "widthMm", LIMITS.width_mm, 5.0),
        }),
        _ => return None,
    })
}

/// Text characters an item adds to the label total (text, code data, Wi-Fi SSID and password).
fn item_chars(item: &Item) -> usize {
    match item {
        Item::Text(t) => js::u16_len(&t.text),
        Item::Code(c) => {
            js::u16_len(&c.data)
                + c.wifi
                    .as_ref()
                    .map_or(0, |w| js::u16_len(&w.ssid) + js::u16_len(&w.password))
        }
        _ => 0,
    }
}

/// Structural validation of an already-migrated document (import, share links, storage).
/// Port of `validateDoc`.
///
/// Fails only when the input is not a current-schema label at all (not an object, `schema` not
/// 4, no `items` array). The result is a fresh document with unknown fields removed.
pub fn validate_doc(raw: &Value) -> Result<Validated, Vec<String>> {
    let Value::Object(raw) = raw else {
        return Err(vec![
            "not a ptouch label document (expected a JSON object)".into(),
        ]);
    };
    if raw.get("schema").and_then(Value::as_f64) != Some(f64::from(SCHEMA_VERSION)) {
        let s = raw.get("schema").unwrap_or(&Value::Null);
        return Err(vec![format!(
            "not a ptouch label document (schema {}, expected {SCHEMA_VERSION})",
            js::short_json(s, 40)
        )]);
    }
    let Some(Value::Array(raw_items)) = raw.get("items") else {
        return Err(vec![
            "not a ptouch label document (no \"items\" list)".into(),
        ]);
    };

    let notes = RefCell::new(Vec::new());
    let r = Reader {
        at: "label".into(),
        notes: &notes,
    };
    let empty = Obj::new();
    let now = now_iso();
    let id = match raw.get("id") {
        Some(Value::String(s)) if is_valid_id(s) => s.clone(),
        _ => {
            r.note("missing or invalid id; a new one was assigned");
            new_id()
        }
    };

    let tape_in = r.obj(raw, "tape", &empty);
    let tr = r.at("tape");
    let width_v = tape_in.get("widthMm");
    let width_n = width_v.and_then(Value::as_f64);
    let width_mm =
        match width_n.filter(|w| TAPE_WIDTHS_MM.contains(w)) {
            Some(w) => w,
            None => {
                let n = width_n.filter(|w| w.is_finite()).unwrap_or(24.0);
                let nearest = TAPE_WIDTHS_MM.iter().fold(24.0, |b: f64, &w| {
                    if (w - n).abs() < (b - n).abs() { w } else { b }
                });
                tr.note(format!(
                    "width {} mm is not a TZe width; using {} mm",
                    js::short_json(width_v.unwrap_or(&Value::Null), 40),
                    js::num(nearest)
                ));
                nearest
            }
        };
    let mut tape = TapeSettings {
        width_mm,
        media_id: None,
        model: None,
        colors: None,
    };
    if let Some(Value::String(m)) = tape_in.get("mediaId")
        && is_media_id(m)
    {
        tape.media_id = Some(m.clone());
    }
    match tape_in.get("model") {
        Some(Value::String(m)) if is_model_name(m) => tape.model = Some(m.clone()),
        Some(_) => tr.note("the printer model is not a model name; removed"),
        None => {}
    }
    match tape_in.get("colors") {
        Some(Value::Object(c))
            if c.get("tape").and_then(Value::as_str).is_some_and(is_color)
                && c.get("ink").and_then(Value::as_str).is_some_and(is_color) =>
        {
            tape.colors = Some(TapeColors {
                tape: c
                    .get("tape")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                ink: c
                    .get("ink")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        Some(_) => tr.note("tape colours are not #rrggbb colours; using the defaults"),
        None => {}
    }

    let length_in = r.obj(raw, "length", &empty);
    let length = if length_in.get("mode").and_then(Value::as_str) == Some("fixed") {
        let lr = r.at("length");
        let mm = lr.range(length_in, "mm", LIMITS.length_mm, 50.0);
        // Only written when on, so labels without it keep their exact JSON.
        let shrink = length_in.contains_key("shrink") && lr.bool(length_in, "shrink", false);
        LengthSettings::Fixed { mm, shrink }
    } else {
        LengthSettings::Auto
    };

    let m = r.obj(raw, "marginsMm", &empty);
    let mr = r.at("margins");
    let margins_mm = Margins {
        start: mr.range(m, "start", LIMITS.margin_mm, 2.0),
        end: mr.range(m, "end", LIMITS.margin_mm, 2.0),
    };

    let lay = r.obj(raw, "layout", &empty);
    let lr = r.at("layout");
    let layout = if lay.get("mode").and_then(Value::as_str) == Some("free") {
        LayoutSettings::Free
    } else {
        LayoutSettings::Flow {
            gap_mm: lr.range(lay, "gapMm", LIMITS.gap_mm, 3.0),
            align: lr.one_of(lay, "align", &ALIGNS, Align::Center),
        }
    };

    let frame = match raw.get("frame") {
        None | Some(Value::Null) => None,
        Some(_) => {
            let fr = r.obj(raw, "frame", &empty);
            let f = r.at("frame");
            Some(LabelFrame {
                thickness_mm: f.range(fr, "thicknessMm", LIMITS.frame_thickness_mm, 0.5),
                radius_mm: f.range(fr, "radiusMm", LIMITS.frame_radius_mm, 0.0),
                inset_mm: f.range(fr, "insetMm", LIMITS.frame_inset_mm, 0.0),
            })
        }
    };

    let p_in = r.obj(raw, "print", &empty);
    let pr = r.at("print");
    let print = PrintSettings {
        copies: pr.int(p_in, "copies", LIMITS.copies, 1.0) as u32,
        auto_cut: pr.bool(p_in, "autoCut", true),
        chain: pr.bool(p_in, "chain", false),
        mirror: pr.bool(p_in, "mirror", false),
        threshold: pr.int(p_in, "threshold", LIMITS.threshold, 128.0) as u8,
    };

    let mut items: Vec<Item> = Vec::new();
    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut raw_items: &[Value] = raw_items;
    if raw_items.len() > LIMITS.items {
        r.note(format!(
            "{} blocks; only the first {} were kept",
            raw_items.len(),
            LIMITS.items
        ));
        raw_items = &raw_items[..LIMITS.items];
    }
    let mut chars = 0usize;
    for (i, raw_item) in raw_items.iter().enumerate() {
        let Value::Object(o) = raw_item else {
            r.note(format!("item {} is not an object; dropped", i + 1));
            continue;
        };
        let mut item_id = match o.get("id") {
            Some(Value::String(s)) if is_valid_id(s) => s.clone(),
            _ => String::new(),
        };
        if item_id.is_empty() || ids.contains(&item_id) {
            if !item_id.is_empty() {
                r.note(format!(
                    "item {} has a duplicate id; a new one was assigned",
                    i + 1
                ));
            }
            item_id = new_id();
        }
        let kind = match o.get("kind") {
            None | Some(Value::Null) => "?".to_string(),
            Some(k) => js::string(k),
        };
        let Some(item) = read_item(
            &r.at(format!("item {} ({kind})", i + 1)),
            o,
            item_id.clone(),
        ) else {
            r.note(format!(
                "item {} has unknown kind {}; dropped",
                i + 1,
                js::json(o.get("kind").unwrap_or(&Value::Null))
            ));
            continue;
        };
        chars += item_chars(&item);
        if chars > LIMITS.total_chars {
            r.note(format!(
                "more than {} characters of text; blocks from {} on were dropped",
                LIMITS.total_chars,
                i + 1
            ));
            break;
        }
        ids.insert(item_id);
        items.push(item);
    }

    let name = r.str(raw, "name", "Untitled label", 200);
    let name = if name.is_empty() {
        "Untitled label".to_string()
    } else {
        name
    };
    let timestamp = |key: &str| match raw.get(key) {
        Some(v @ Value::String(s)) if js::is_timestamp(v) => s.clone(),
        _ => now.clone(),
    };
    let created_at = timestamp("createdAt");
    let updated_at = timestamp("updatedAt");
    let batch = match raw.get("batch") {
        None | Some(Value::Null) => None,
        Some(_) => {
            let br = r.at("batch");
            Some(read_batch(&br, r.obj(raw, "batch", &empty)))
        }
    };
    let doc = LabelDoc {
        schema: SCHEMA_VERSION,
        id,
        name,
        created_at,
        updated_at,
        tape,
        length,
        margins_mm,
        layout,
        frame,
        items,
        print,
        batch,
    };
    drop(r);
    Ok(Validated {
        doc,
        notices: notes.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::js;

    #[test]
    fn js_number_formatting() {
        for (n, s) in [
            (24.0, "24"),
            (0.5, "0.5"),
            (1.1, "1.1"),
            (-4.0, "-4"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (123456789012.5, "123456789012.5"),
            (-0.0, "0"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(js::num(n), s);
        }
    }

    #[test]
    fn js_number_parsing_and_rounding() {
        assert_eq!(js::number_from_str(" 12 "), 12.0);
        assert_eq!(js::number_from_str("0x1A"), 26.0);
        assert!(js::number_from_str("12px").is_nan());
        assert!(js::number_from_str("inf").is_nan());
        assert_eq!(js::round(2.5), 3.0);
        assert_eq!(js::round(-2.5), -2.0);
        assert_eq!(js::round(0.49999999999999994), 0.0);
    }

    #[test]
    fn utf16_truncation_and_timestamps() {
        assert_eq!(js::u16_len("a😀"), 3);
        assert_eq!(js::truncate_u16("a😀b", 2), "a");
        assert_eq!(js::truncate_u16("a😀b", 3), "a😀");
        let ts = |s: &str| js::is_timestamp(&serde_json::Value::String(s.into()));
        assert!(ts("2026-10-01T10:00:00.000Z"));
        assert!(ts("2026-10-01"));
        assert!(ts("2026-10-01T10:00+02:00"));
        assert!(!ts("2026-13-01"));
        assert!(!ts("yesterday"));
        assert!(!ts(&"c".repeat(50)));
    }
}
