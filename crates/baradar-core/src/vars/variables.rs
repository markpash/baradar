//! Variables and batch labels (port of `web/src/doc/variables.ts`).
//!
//! Placeholders are `{{name}}` in `TextItem.text`, `CodeItem.data` and `CodeItem.wifi`
//! ssid/password. A Wi-Fi password is a placeholder only when the WHOLE field is one `{{name}}`;
//! braces anywhere else in a password are literal characters, never substituted and never
//! "missing". Names resolve, in this order, to:
//!   1. a batch column (`BatchData.columns`, value from the current row);
//!   2. a batch counter (`BatchData.counters`: start + i × step, zero-padded);
//!   3. built-ins: `{{today}}`, `{{today+30d}}` / `{{today-7d}}` (local date in
//!      `BatchData.date_format`, iso without batch data), `{{ssid}}` (the first Wi-Fi code's
//!      network name).
//!
//! Anything else is "missing": it stays visible as `{{name}}` and is listed in
//! [`ResolvedDoc::missing`]. Resolution is a single pass: a value that itself contains `{{…}}` is
//! printed verbatim.

use std::borrow::Cow;

use time::{Date, Duration};

use crate::doc::schema::{
    BatchCounter, BatchData, CodeContent, CodeItem, DateFormat, Item, LabelDoc, Symbology,
};

/// `LIMITS.batchRows`: labels in one batch, rows in a table.
pub const BATCH_ROWS: usize = 500;
/// `LIMITS.batchColumns`.
pub const BATCH_COLUMNS: usize = 20;
/// `LIMITS.batchCellChars` (UTF-16 code units, as JavaScript counts).
pub const BATCH_CELL_CHARS: usize = 500;
/// `LIMITS.batchCounters`.
pub const BATCH_COUNTERS: usize = 4;
/// `LIMITS.counterValue`: counters' start and step lie in ±this.
pub const COUNTER_VALUE: i64 = 1_000_000_000;
/// `LIMITS.counterPad.min`.
pub const COUNTER_PAD_MIN: u8 = 0;
/// `LIMITS.counterPad.max`.
pub const COUNTER_PAD_MAX: u8 = 12;
/// `LIMITS.batchTotalChars` (UTF-16 code units).
pub const BATCH_TOTAL_CHARS: usize = 200_000;
/// Largest `{{today±Nd}}` offset (10 years). `MAX_DATE_OFFSET`.
pub const MAX_DATE_OFFSET: i64 = 3650;

pub use crate::doc::validate::is_variable_name;

/// What "today" is for date variables, and the locale of the 'long' format. `VariableContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableContext {
    /// The local calendar day (the TS takes a `Date` and uses its local date parts; callers
    /// convert to local time first).
    pub today: Date,
    /// Locale for the 'long' date format (BCP 47, e.g. `en-GB`). `None`: `en-US`.
    pub locale: Option<String>,
}

impl VariableContext {
    pub fn new(today: Date) -> Self {
        Self {
            today,
            locale: None,
        }
    }

    pub fn with_locale(today: Date, locale: impl Into<String>) -> Self {
        Self {
            today,
            locale: Some(locale.into()),
        }
    }
}

/// The result of [`resolve_doc`]. `ResolvedDoc`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDoc<'a> {
    /// The label with every known placeholder replaced; borrowed (the input itself) when nothing
    /// changed.
    pub doc: Cow<'a, LabelDoc>,
    /// Unknown placeholder names, deduplicated, in order of appearance.
    pub missing: Vec<String>,
}

/// What a placeholder name refers to. `VariableKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableKind {
    Column,
    Counter,
    Date,
    Ssid,
    Missing,
}

/// JavaScript's `String.prototype.trim` whitespace (WhiteSpace + LineTerminator).
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `s.trim()` with JavaScript's whitespace set.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// Length in UTF-16 code units (JavaScript's `length`).
pub(crate) fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` in UTF-16 code units. A surrogate pair that would be split is dropped whole
/// (JavaScript would keep a lone high surrogate, which a Rust string cannot hold).
pub(crate) fn utf16_prefix(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// JavaScript's `Number.prototype.toFixed(digits)` for finite values below 1e21: exact decimal
/// value, ties rounded away from zero (Rust's `{:.N}` rounds ties to even).
pub(crate) fn js_to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() {
        return x.to_string();
    }
    let exact = format!("{:.80}", x.abs());
    let (int, frac) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let mut kept: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut k = kept.len();
        loop {
            if k == 0 {
                kept.insert(0, b'1');
                break;
            }
            k -= 1;
            if kept[k] == b'9' {
                kept[k] = b'0';
            } else {
                kept[k] += 1;
                break;
            }
        }
    }
    let split = kept.len() - digits;
    let (i, f) = kept.split_at(split);
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(i).unwrap_or("0"));
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(f).unwrap_or(""));
    }
    out
}

/// Every `{{…}}` match of `PLACEHOLDER_RE` (`/\{\{([^{}]*)\}\}/g`): (byte range of the whole
/// match, raw inner text).
fn placeholders(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            let mut j = i + 2;
            while j < b.len() && b[j] != b'{' && b[j] != b'}' {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'}' && b[j + 1] == b'}' {
                out.push((i..j + 2, &text[i + 2..j]));
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn push_unique(out: &mut Vec<String>, name: &str) {
    if !out.iter().any(|n| n == name) {
        out.push(name.to_owned());
    }
}

/// Placeholder names in `text` (trimmed, deduplicated, in order of appearance). `placeholderNames`.
pub fn placeholder_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !text.contains("{{") {
        return out;
    }
    for (_, raw) in placeholders(text) {
        push_unique(&mut out, js_trim(raw));
    }
    out
}

/// `{{name}}` → its placeholder name when the WHOLE string is one placeholder (Wi-Fi passwords).
/// `passwordVariable`.
pub fn password_variable(password: &str) -> Option<&str> {
    let inner = password.strip_prefix("{{")?.strip_suffix("}}")?;
    if inner.contains(['{', '}']) {
        return None;
    }
    Some(js_trim(inner))
}

/// A QR code with content 'wifi' encodes `wifi`; every other code encodes `data`.
fn encodes_wifi(item: &CodeItem) -> bool {
    item.content == CodeContent::Wifi && item.symbology == Symbology::Qr
}

/// The printed strings of an item that may hold placeholders.
fn item_fields(item: &Item) -> Vec<&str> {
    match item {
        Item::Text(t) => vec![t.text.as_str()],
        Item::Code(c) if !encodes_wifi(c) => vec![c.data.as_str()],
        Item::Code(c) => match &c.wifi {
            None => vec![],
            Some(w) if password_variable(&w.password).is_none() => vec![w.ssid.as_str()],
            Some(w) => vec![w.ssid.as_str(), w.password.as_str()],
        },
        _ => vec![],
    }
}

/// Names of all `{{…}}` placeholders in the doc (deduplicated, in order of appearance).
/// `findVariables`.
pub fn find_variables(doc: &LabelDoc) -> Vec<String> {
    let mut out = Vec::new();
    for item in &doc.items {
        for field in item_fields(item) {
            for name in placeholder_names(field) {
                push_unique(&mut out, &name);
            }
        }
    }
    out
}

/// Labels the batch prints: rows (or `count` without rows) when the batch is enabled, else 0 (a
/// plain label). Capped at [`BATCH_ROWS`]. `batchSize`.
pub fn batch_size(doc: &LabelDoc) -> usize {
    match &doc.batch {
        Some(b) if b.enabled => {
            let n = if b.rows.is_empty() {
                (b.count as usize).max(1)
            } else {
                b.rows.len()
            };
            n.min(BATCH_ROWS)
        }
        _ => 0,
    }
}

/// Value of `counter` for label `index` (0-based): start + index × step, zero-padded to `pad`
/// digits after the sign (pad 2: -1 → "-01"). `counterValue`.
pub fn counter_value(counter: &BatchCounter, index: usize) -> String {
    let v = i128::from(counter.start)
        .saturating_add((index as i128).saturating_mul(i128::from(counter.step)));
    let width = usize::from(counter.pad.min(COUNTER_PAD_MAX));
    let digits = format!("{:0>width$}", v.unsigned_abs());
    if v < 0 { format!("-{digits}") } else { digits }
}

/// `date` shifted by `days` calendar days. `addDays` (out of `time`'s range: unchanged).
pub fn add_days(date: Date, days: i64) -> Date {
    date.checked_add(Duration::days(days)).unwrap_or(date)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// English locales whose long date is "8 October 2026" (day first) rather than en-US's
/// "October 8, 2026" (ICU `dateStyle: 'long'`).
const DAY_FIRST_REGIONS: [&str; 9] = ["gb", "au", "nz", "ie", "in", "sg", "mt", "hk", "001"];

fn long_date(date: Date, locale: Option<&str>) -> String {
    let month = MONTHS[usize::from(u8::from(date.month())) - 1];
    let (day, year) = (date.day(), year4(date.year()));
    let loc = locale
        .unwrap_or("en-US")
        .to_ascii_lowercase()
        .replace('_', "-");
    let region = loc
        .split('-')
        .skip(1)
        .find(|p| p.len() == 2 || p.len() == 3);
    let day_first = loc.starts_with("en") && region.is_some_and(|r| DAY_FIRST_REGIONS.contains(&r));
    if day_first {
        format!("{day} {month} {year}")
    } else {
        format!("{month} {day}, {year}")
    }
}

/// `String(year).padStart(4, '0')`.
fn year4(year: i32) -> String {
    format!("{:0>4}", year.to_string())
}

/// A local calendar date in `format`. `formatDate`.
///
/// 'long' follows `Intl.DateTimeFormat(locale, {dateStyle: 'long'})` for English locales only:
/// en-GB style ("8 October 2026") for the day-first regions, en-US style ("October 8, 2026")
/// otherwise, including non-English locales.
pub fn format_date(date: Date, format: DateFormat, locale: Option<&str>) -> String {
    let y = year4(date.year());
    let m = format!("{:02}", u8::from(date.month()));
    let d = format!("{:02}", date.day());
    match format {
        DateFormat::Dmy => format!("{d}/{m}/{y}"),
        DateFormat::Mdy => format!("{m}/{d}/{y}"),
        DateFormat::Long => long_date(date, locale),
        DateFormat::Iso => format!("{y}-{m}-{d}"),
    }
}

/// Day offset of a date placeholder (`today` 0, `today+30d` 30, `today-7d` −7), or `None` when
/// `name` is not one (or the offset is over [`MAX_DATE_OFFSET`]). `dateOffset`.
pub fn date_offset(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("today")?;
    if rest.is_empty() {
        return Some(0);
    }
    let (sign, rest) = match rest.as_bytes()[0] {
        b'+' => (1, &rest[1..]),
        b'-' => (-1, &rest[1..]),
        _ => return None,
    };
    let digits = rest.strip_suffix('d')?;
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    (n <= MAX_DATE_OFFSET).then_some(sign * n)
}

/// Index of the first Wi-Fi QR code of the label (the `{{ssid}}` source).
fn first_wifi(doc: &LabelDoc) -> Option<usize> {
    doc.items
        .iter()
        .position(|i| matches!(i, Item::Code(c) if encodes_wifi(c) && c.wifi.is_some()))
}

/// What `name` refers to in `doc` (independent of the row). `variableKind`.
pub fn variable_kind(doc: &LabelDoc, name: &str) -> VariableKind {
    if let Some(b) = &doc.batch
        && is_variable_name(name)
    {
        if b.columns.iter().any(|c| c == name) {
            return VariableKind::Column;
        }
        if b.counters.iter().any(|c| c.name == name) {
            return VariableKind::Counter;
        }
    }
    if date_offset(name).is_some() {
        return VariableKind::Date;
    }
    if name == "ssid" && first_wifi(doc).is_some() {
        return VariableKind::Ssid;
    }
    VariableKind::Missing
}

/// Placeholder names in the doc that nothing defines (in order of appearance).
/// `missingVariables`.
pub fn missing_variables(doc: &LabelDoc) -> Vec<String> {
    find_variables(doc)
        .into_iter()
        .filter(|n| variable_kind(doc, n) == VariableKind::Missing)
        .collect()
}

/// Batch columns the label actually uses (by placeholder). `usedColumns`.
pub fn used_columns(doc: &LabelDoc) -> Vec<String> {
    let used = find_variables(doc);
    doc.batch
        .as_ref()
        .map(|b| {
            b.columns
                .iter()
                .filter(|c| used.contains(c))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Columns the label uses while the enabled batch has no rows: every label would print them
/// blank, so printing is blocked until data is added. `columnsWithoutData`.
pub fn columns_without_data(doc: &LabelDoc) -> Vec<String> {
    match &doc.batch {
        Some(b) if b.enabled && b.rows.is_empty() => used_columns(doc),
        _ => Vec::new(),
    }
}

/// The label prints a `{{today…}}` date (its render depends on the calendar day). `usesDates`.
pub fn uses_dates(doc: &LabelDoc) -> bool {
    find_variables(doc).iter().any(|n| date_offset(n).is_some())
}

/// The calendar day as YYYY-MM-DD: part of the render key of dated labels. `localDay`.
pub fn local_day(today: Date) -> String {
    format_date(today, DateFormat::Iso, None)
}

/// Replaces known placeholders in `text`; unknown ones stay and are added to `missing`.
fn substitute<'t>(
    text: &'t str,
    lookup: &dyn Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
) -> Cow<'t, str> {
    if !text.contains("{{") {
        return Cow::Borrowed(text);
    }
    let matches = placeholders(text);
    if matches.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut changed = false;
    for (range, raw) in matches {
        let name = js_trim(raw);
        out.push_str(&text[last..range.start]);
        match lookup(name) {
            Some(v) => {
                changed |= v != text[range.clone()];
                out.push_str(&v);
            }
            None => {
                push_unique(missing, name);
                out.push_str(&text[range.clone()]);
            }
        }
        last = range.end;
    }
    if !changed {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
}

/// The doc as label `index` (0-based) of its batch, placeholders replaced. Without batch data
/// (or with it disabled and empty) index 0 is used for counters and the built-ins still resolve.
/// `resolveDoc`.
pub fn resolve_doc<'a>(doc: &'a LabelDoc, index: usize, ctx: &VariableContext) -> ResolvedDoc<'a> {
    let unchanged = || ResolvedDoc {
        doc: Cow::Borrowed(doc),
        missing: Vec::new(),
    };
    if !doc
        .items
        .iter()
        .any(|i| item_fields(i).iter().any(|f| f.contains("{{")))
    {
        return unchanged();
    }

    let b: Option<&BatchData> = doc.batch.as_ref();
    // A disabled batch still shows (and prints once) the preview row: data resolves either way.
    let i = match b {
        Some(b) if b.enabled || !b.rows.is_empty() => index,
        _ => 0,
    };
    let row: Option<&Vec<String>> = b
        .filter(|b| !b.rows.is_empty())
        .and_then(|b| b.rows.get(i.min(b.rows.len() - 1)));
    let date_format = b.map(|b| b.date_format).unwrap_or_default();
    let locale = ctx.locale.as_deref();

    let base = |name: &str| -> Option<String> {
        if let Some(b) = b
            && is_variable_name(name)
        {
            if let Some(col) = b.columns.iter().position(|c| c == name) {
                return Some(row.and_then(|r| r.get(col)).cloned().unwrap_or_default());
            }
            if let Some(counter) = b.counters.iter().find(|c| c.name == name) {
                return Some(counter_value(counter, i));
            }
        }
        let days = date_offset(name)?;
        Some(format_date(add_days(ctx.today, days), date_format, locale))
    };

    // `{{ssid}}`: the first Wi-Fi code's network name, itself resolved (without `{{ssid}}`). An
    // empty name keeps the placeholder visible (an obviously unfilled template), but is not missing.
    let wifi_at = first_wifi(doc);
    let ssid: Option<String> = wifi_at.and_then(|k| match &doc.items[k] {
        Item::Code(CodeItem { wifi: Some(w), .. }) => {
            Some(substitute(&w.ssid, &base, &mut Vec::new()).into_owned())
        }
        _ => None,
    });
    let lookup = |name: &str| -> Option<String> {
        if let Some(v) = base(name) {
            return Some(v);
        }
        match &ssid {
            Some(s) if name == "ssid" => Some(if s.is_empty() {
                "{{ssid}}".into()
            } else {
                s.clone()
            }),
            _ => None,
        }
    };

    let mut missing = Vec::new();
    let mut items: Option<Vec<Item>> = None;
    for (k, item) in doc.items.iter().enumerate() {
        let replaced: Option<Item> = match item {
            Item::Text(t) => match substitute(&t.text, &lookup, &mut missing) {
                Cow::Owned(text) => {
                    let mut t = t.clone();
                    t.text = text;
                    Some(Item::Text(t))
                }
                Cow::Borrowed(_) => None,
            },
            Item::Code(c) if !encodes_wifi(c) => match substitute(&c.data, &lookup, &mut missing) {
                Cow::Owned(data) => {
                    let mut c = c.clone();
                    c.data = data;
                    Some(Item::Code(c))
                }
                Cow::Borrowed(_) => None,
            },
            Item::Code(c) => match &c.wifi {
                None => None,
                Some(w) => {
                    // The `{{ssid}}` source cannot refer to itself.
                    let s = if Some(k) == wifi_at {
                        substitute(&w.ssid, &base, &mut missing)
                    } else {
                        substitute(&w.ssid, &lookup, &mut missing)
                    };
                    let p = if password_variable(&w.password).is_none() {
                        Cow::Borrowed(w.password.as_str())
                    } else {
                        substitute(&w.password, &lookup, &mut missing)
                    };
                    if matches!((&s, &p), (Cow::Borrowed(_), Cow::Borrowed(_))) {
                        None
                    } else {
                        let mut c = c.clone();
                        let (s, p) = (s.into_owned(), p.into_owned());
                        if let Some(w) = c.wifi.as_mut() {
                            w.ssid = s;
                            w.password = p;
                        }
                        Some(Item::Code(c))
                    }
                }
            },
            _ => None,
        };
        if let Some(new) = replaced {
            let list = items.get_or_insert_with(|| doc.items[..k].to_vec());
            list.push(new);
        } else if let Some(list) = items.as_mut() {
            list.push(item.clone());
        }
    }
    match items {
        Some(items) => ResolvedDoc {
            doc: Cow::Owned(with_items(doc, items)),
            missing,
        },
        None => ResolvedDoc {
            doc: Cow::Borrowed(doc),
            missing,
        },
    }
}

/// A copy of `doc` with `items` (the original items are not cloned).
fn with_items(doc: &LabelDoc, items: Vec<Item>) -> LabelDoc {
    LabelDoc {
        schema: doc.schema,
        id: doc.id.clone(),
        name: doc.name.clone(),
        created_at: doc.created_at.clone(),
        updated_at: doc.updated_at.clone(),
        tape: doc.tape.clone(),
        length: doc.length.clone(),
        margins_mm: doc.margins_mm,
        layout: doc.layout.clone(),
        frame: doc.frame,
        items,
        print: doc.print.clone(),
        batch: doc.batch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_scan_matches_the_regex() {
        assert_eq!(
            placeholder_names("{{a}} {{ b }} {{a}} {{}} {{x y}} {single} {{{c}}}"),
            ["a", "b", "", "x y", "c"]
        );
        assert!(placeholder_names("no placeholders {{ unclosed").is_empty());
        assert_eq!(placeholder_names("{{é}}"), ["é"]);
    }

    #[test]
    fn variable_names() {
        assert!(is_variable_name("_a1"));
        assert!(is_variable_name(&"x".repeat(32)));
        assert!(!is_variable_name(&"x".repeat(33)));
        assert!(!is_variable_name("1a"));
        assert!(!is_variable_name(""));
        assert!(!is_variable_name("a-b"));
    }

    #[test]
    fn utf16_helpers() {
        assert_eq!(utf16_len("a😀"), 3);
        assert_eq!(utf16_prefix("a😀b", 2), "a");
        assert_eq!(utf16_prefix("a😀b", 3), "a😀");
        assert_eq!(js_trim("\u{feff} x\u{a0}"), "x");
    }

    #[test]
    fn to_fixed_like_javascript() {
        assert_eq!(js_to_fixed(1.125, 2), "1.13");
        assert_eq!(js_to_fixed(0.25, 1), "0.3");
        assert_eq!(js_to_fixed(1.005, 2), "1.00"); // 1.00499999…
        assert_eq!(js_to_fixed(9.999, 2), "10.00");
        assert_eq!(js_to_fixed(1.234, 2), "1.23");
        assert_eq!(js_to_fixed(24.0, 1), "24.0");
        assert_eq!(js_to_fixed(-1.25, 1), "-1.3");
        assert_eq!(js_to_fixed(0.4, 0), "0");
    }
}
