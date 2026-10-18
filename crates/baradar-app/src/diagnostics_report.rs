//! Pure helpers of the Diagnostics view: masking for "Copy diagnostics" (port of
//! `web/src/ui/diagnostics/report.ts`), the built-in test labels (`testlabels.ts`) and the
//! virtual printer's file names (`virtual.ts`).

use baradar_core::doc::{
    Align, Item, ItemFrame, ItemKind, ItemSize, LabelDoc, LabelFrame, LayoutSettings, LengthSettings, Margins,
    ModuleSize, QrEcc, ShapeItem, ShapeKind, TextItem, TextSize, new_id,
};

// ------------------------------------------------------------------------------------------
// Masking (report.ts `maskText`)
// ------------------------------------------------------------------------------------------

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Bluetooth addresses (`XX:XX:XX:XX:XX:XX`, `:` or `-` separated) → `XX:XX:XX:XX:XX:XX`.
fn mask_bt_addresses(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let boundary_before = i == 0 || !is_word(chars[i - 1]);
        if boundary_before && i + 17 <= chars.len() {
            let candidate = &chars[i..i + 17];
            let ok = candidate.iter().enumerate().all(|(k, c)| {
                if k % 3 == 2 {
                    *c == ':' || *c == '-'
                } else {
                    c.is_ascii_hexdigit()
                }
            });
            let boundary_after = i + 17 == chars.len() || !is_word(chars[i + 17]);
            if ok && boundary_after {
                out.push_str("XX:XX:XX:XX:XX:XX");
                i += 17;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Device-name suffixes: `PT-P710BT1234` → `PT-P710BTxxxx` (`MODEL_SUFFIX`, case-insensitive:
/// a `PT|QL|TD|RJ|PJ-` name whose last four characters follow `BT`, `B` or `W`).
fn mask_model_suffixes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let boundary_before = i == 0 || !is_word(chars[i - 1]);
        if boundary_before && i + 3 < chars.len() && chars[i + 2] == '-' {
            let prefix: String = chars[i..i + 2].iter().collect::<String>().to_ascii_uppercase();
            if matches!(prefix.as_str(), "PT" | "QL" | "TD" | "RJ" | "PJ") {
                let start = i + 3;
                let mut end = start;
                while end < chars.len() && chars[end].is_ascii_alphanumeric() {
                    end += 1;
                }
                let run = &chars[start..end];
                if run.len() >= 5 {
                    let head: String = run[..run.len() - 4].iter().collect::<String>().to_ascii_uppercase();
                    if head.ends_with('B') || head.ends_with("BT") || head.ends_with('W') {
                        out.extend(&chars[i..end - 4]);
                        out.push_str("xxxx");
                        i = end;
                        continue;
                    }
                }
                out.extend(&chars[i..end]);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Serial numbers after `serial number:` / `serial_number=` / `"serialNumber": "…"` → `***`.
fn mask_serial_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let starts_with = |at: usize, word: &str| {
        let w: Vec<char> = word.chars().collect();
        at + w.len() <= lower.len() && lower[at..at + w.len()] == w[..]
    };
    while i < chars.len() {
        if starts_with(i, "serial") {
            let mut j = i + 6;
            if j < chars.len() && matches!(chars[j], ' ' | '_' | '-') {
                j += 1;
            }
            if starts_with(j, "number") {
                j += 6;
                if j < chars.len() && matches!(chars[j], '"' | '\'') {
                    j += 1;
                }
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && matches!(chars[j], ':' | '=') {
                    j += 1;
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j < chars.len() && matches!(chars[j], '"' | '\'') {
                        j += 1;
                    }
                    let mut end = j;
                    while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], '"' | '\'' | ',' | ';' | ')') {
                        end += 1;
                    }
                    if end > j {
                        out.extend(&chars[i..j]);
                        out.push_str("***");
                        i = end;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Masks every device-name suffix, Bluetooth address and serial number in `text` (`maskText`).
pub fn mask_text(text: &str) -> String {
    mask_serial_numbers(&mask_model_suffixes(&mask_bt_addresses(text)))
}

/// `kv(k, v)`: a report line, the key padded to 18 columns; empty values print as `—`.
pub fn kv(lines: &mut Vec<String>, key: &str, value: impl Into<String>) {
    let v: String = value.into();
    let v = if v.is_empty() { "—".to_owned() } else { v };
    lines.push(format!("{:<18} {v}", format!("{key}:")));
}

// ------------------------------------------------------------------------------------------
// Test labels (testlabels.ts)
// ------------------------------------------------------------------------------------------

/// Which built-in label to run through the virtual printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestLabel {
    Orientation,
    Ruler,
    Current,
}

impl TestLabel {
    pub const ALL: [TestLabel; 3] = [Self::Orientation, Self::Ruler, Self::Current];

    pub fn label(self) -> &'static str {
        match self {
            Self::Orientation => "Orientation test",
            Self::Ruler => "Ruler test (60 mm)",
            Self::Current => "Current label",
        }
    }
}

fn text(t: &str, edit: impl FnOnce(&mut TextItem)) -> Item {
    let mut item = Item::new(ItemKind::Text);
    if let Item::Text(x) = &mut item {
        x.text = t.into();
        x.font_family = "atkinson-hyperlegible".into();
        x.font_weight = 700;
        edit(x);
    }
    item
}

/// START … END along the label, TOP/BOTTOM across it, an asymmetric "F" (reveals mirroring)
/// and a QR code; framed to show the printable band edges (`orientationDoc`).
pub fn orientation_doc(width_mm: f64) -> LabelDoc {
    let mut items = vec![
        text("START >", |t| t.align = Align::Start),
        text("TOP\nBOTTOM", |t| {
            t.font_weight = 600;
            t.line_height = 1.0;
        }),
        text("F", |t| {
            t.font_family = "fira-sans".into();
            t.font_weight = 800;
        }),
    ];
    if width_mm >= 9.0 {
        let mut code = Item::new(ItemKind::Code);
        if let Item::Code(c) = &mut code {
            c.data = "ptouch orientation test".into();
            c.ecc = QrEcc::L;
            c.module_dots = ModuleSize::Dots(if width_mm >= 18.0 { 3 } else { 2 });
        }
        items.push(code);
    }
    items.push(text("> END", |t| t.align = Align::End));
    let mut doc = LabelDoc::with_items(items);
    doc.name = format!("Orientation test {} mm", crate::view_model::format_mm(width_mm).trim_end_matches(" mm"));
    doc.tape.width_mm = width_mm;
    doc.length = LengthSettings::Auto;
    doc.margins_mm = Margins { start: 1.0, end: 1.0 };
    doc.layout = LayoutSettings::Flow {
        gap_mm: 3.0,
        align: Align::Center,
    };
    doc.frame = Some(LabelFrame {
        thickness_mm: 0.3,
        radius_mm: 0.0,
        inset_mm: 0.0,
    });
    doc
}

/// Fixed 60 mm, no margins, free layout: a tick every mm (taller every 5/10 mm) with numbers
/// every 10 mm (`rulerDoc`). `band_mm` is the printable band's height in mm.
pub fn ruler_doc(width_mm: f64, band_mm: f64, length_mm: u32) -> LabelDoc {
    let mut items = Vec::new();
    let tick_w = 0.25;
    let length = f64::from(length_mm);
    for mm in 0..=length_mm {
        let major = mm % 10 == 0;
        let mid = mm % 5 == 0;
        let h = band_mm * if major { 1.0 } else if mid { 0.5 } else { 0.25 };
        let x = f64::from(mm).min(length - tick_w);
        items.push(Item::Shape(ShapeItem {
            id: new_id(),
            frame: Some(ItemFrame {
                x_mm: x,
                y_mm: 0.0,
                w_mm: tick_w,
                h_mm: h,
                rotation: 0,
            }),
            shape: ShapeKind::Rect,
            width_mm: tick_w,
            size: ItemSize::Mm { mm: h },
            stroke_mm: tick_w,
            fill: true,
        }));
        if major && mm > 0 && mm < length_mm && band_mm >= 4.0 {
            let th = (band_mm * 0.4).min(5.0);
            items.push(text(&mm.to_string(), |t| {
                t.font_family = "jetbrains-mono".into();
                t.font_weight = 600;
                t.size = TextSize::Mm { mm: th };
                t.align = Align::Start;
                t.frame = Some(ItemFrame {
                    x_mm: f64::from(mm) + 0.6,
                    y_mm: band_mm - th,
                    w_mm: 8.0,
                    h_mm: th,
                    rotation: 0,
                });
            }));
        }
    }
    let mut doc = LabelDoc::with_items(items);
    doc.name = format!("Ruler test {} mm", crate::view_model::format_mm(width_mm).trim_end_matches(" mm"));
    doc.tape.width_mm = width_mm;
    doc.length = LengthSettings::Fixed {
        mm: length,
        shrink: false,
    };
    doc.margins_mm = Margins { start: 0.0, end: 0.0 };
    doc.layout = LayoutSettings::Free;
    doc
}

// ------------------------------------------------------------------------------------------
// File names (virtual.ts)
// ------------------------------------------------------------------------------------------

/// "Cable tags" → "cable-tags" (`slug`).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out: String = out.chars().take(40).collect();
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() { "label".into() } else { out }
}

/// Download base name for a run, e.g. "ptouch-orientation-test-24mm" (`runFileBase`).
pub fn run_file_base(doc_name: &str, width_mm: f64) -> String {
    let trimmed = doc_name.trim_end();
    // Drop a trailing "<n> mm" from the name.
    let name = match trimmed.strip_suffix("mm").map(str::trim_end) {
        Some(rest) => {
            let digits = rest.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
            if digits.len() < rest.len() { digits.trim_end() } else { trimmed }
        }
        None => trimmed,
    };
    let width = crate::view_model::format_mm(width_mm).trim_end_matches(" mm").replace('.', "_");
    format!("ptouch-{}-{width}mm", slug(name))
}

#[cfg(test)]
mod tests {
    use super::{mask_text, run_file_base, slug};

    #[test]
    fn masks_addresses_names_and_serials() {
        assert_eq!(mask_text("addr 00:80:77:31:AB:cd here"), "addr XX:XX:XX:XX:XX:XX here");
        assert_eq!(mask_text("00-80-77-31-AB-CD"), "XX:XX:XX:XX:XX:XX");
        assert_eq!(mask_text("cu.PT-P710BT1234 ok"), "cu.PT-P710BTxxxx ok");
        assert_eq!(mask_text("PT-E720BT9a7f"), "PT-E720BTxxxx");
        assert_eq!(mask_text("QL-820NWB5678,"), "QL-820NWBxxxx,");
        // Not a device name: no BT/B/W before the last four.
        assert_eq!(mask_text("PT-P9001234"), "PT-P9001234");
        assert_eq!(mask_text("serial number: A1B2C3, x"), "serial number: ***, x");
        assert_eq!(mask_text("\"serialNumber\": \"XYZ\""), "\"serialNumber\": \"***\"");
    }

    #[test]
    fn file_names() {
        assert_eq!(slug("Cable tags"), "cable-tags");
        assert_eq!(slug("!!!"), "label");
        assert_eq!(run_file_base("Orientation test 24 mm", 24.0), "ptouch-orientation-test-24mm");
        assert_eq!(run_file_base("Shelf", 3.5), "ptouch-shelf-3_5mm");
    }
}
