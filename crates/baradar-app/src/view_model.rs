//! Pure display helpers (port of `web/src/ui/state/view-model.ts`).

use baradar_core::doc::{CodeContent, DitherKind, Item, ShapeKind, Symbology};

/// `formatMm`: "24 mm", "3.5 mm" (one decimal, trailing ".0" dropped).
pub fn format_mm(mm: f64) -> String {
    let s = format!("{mm:.1}");
    format!("{} mm", s.strip_suffix(".0").unwrap_or(&s))
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A tape/ink colour preset for the preview (`TAPE_PRESETS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapePreset {
    pub id: &'static str,
    pub label: &'static str,
    pub tape: &'static str,
    pub ink: &'static str,
}

pub const TAPE_PRESETS: [TapePreset; 9] = [
    TapePreset { id: "white-black", label: "Black on white", tape: "#ffffff", ink: "#000000" },
    TapePreset { id: "yellow-black", label: "Black on yellow", tape: "#ffd400", ink: "#000000" },
    TapePreset { id: "clear-black", label: "Black on clear", tape: "#eef1f4", ink: "#000000" },
    TapePreset { id: "white-blue", label: "Blue on white", tape: "#ffffff", ink: "#1d3fbf" },
    TapePreset { id: "white-red", label: "Red on white", tape: "#ffffff", ink: "#c8102e" },
    TapePreset { id: "red-white", label: "White on red", tape: "#c8102e", ink: "#ffffff" },
    TapePreset { id: "blue-white", label: "White on blue", tape: "#1d4fbf", ink: "#ffffff" },
    TapePreset { id: "black-white", label: "White on black", tape: "#111111", ink: "#ffffff" },
    TapePreset { id: "green-black", label: "Black on green", tape: "#3fae49", ink: "#000000" },
];

/// `DEFAULT_COLORS`: black on white.
pub const DEFAULT_COLORS: (&str, &str) = ("#ffffff", "#000000");

/// `printButtonText`.
pub fn print_button_text(labels: u32) -> String {
    if labels == 1 {
        "Print label".into()
    } else {
        format!("Print {labels} labels")
    }
}

/// Phase of the page being printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePhase {
    Sending,
    Printing,
    Done,
}

/// Print progress: page `page` of `of` (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintProgress {
    pub page: u32,
    pub of: u32,
    pub phase: PagePhase,
}

/// `progressText`.
pub fn progress_text(p: Option<PrintProgress>) -> String {
    let Some(p) = p else {
        return "Starting…".into();
    };
    let which = if p.of > 1 {
        format!("label {} of {}", p.page, p.of)
    } else {
        "label".into()
    };
    match p.phase {
        PagePhase::Sending => format!("Sending {which}…"),
        PagePhase::Printing => format!("Printing {which}…"),
        PagePhase::Done if p.page >= p.of => "Finishing…".into(),
        PagePhase::Done => format!("Printed {which}"),
    }
}

/// `progressFraction`: 0..1; each page counts sending 15 %, printing 50 %, done 100 %.
pub fn progress_fraction(p: Option<PrintProgress>) -> f32 {
    let Some(p) = p.filter(|p| p.of > 0) else {
        return 0.0;
    };
    let within = match p.phase {
        PagePhase::Sending => 0.15,
        PagePhase::Printing => 0.5,
        PagePhase::Done => 1.0,
    };
    ((p.page as f32 - 1.0 + within) / p.of as f32).clamp(0.0, 1.0)
}

/// Zoom steps as multiples of real size (`ZOOM_STEPS`).
pub const ZOOM_STEPS: [f32; 11] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0];

/// Logical pixels per printer dot at real size (`realScale`: 96 px per inch).
pub fn real_scale(dpi: u16) -> f32 {
    96.0 / f32::from(dpi)
}

/// `rulerSteps`: minor tick and labelled tick spacing in mm.
pub fn ruler_steps(px_per_mm: f32) -> (u32, u32) {
    let minor = [1, 2, 5, 10, 20, 50]
        .into_iter()
        .find(|&s| s as f32 * px_per_mm >= 5.0)
        .unwrap_or(50);
    let label = [5, 10, 20, 50, 100]
        .into_iter()
        .find(|&s| s as f32 * px_per_mm >= 36.0 && s % minor == 0)
        .unwrap_or(100);
    (minor, label)
}

/// Block kind labels (`KIND_META`).
pub fn kind_label(item: &Item) -> &'static str {
    match item {
        Item::Text(_) => "Text",
        Item::Icon(_) => "Icon",
        Item::Code(_) => "Code",
        Item::Image(_) => "Image",
        Item::Shape(_) => "Shape",
        Item::Spacer(_) => "Spacer",
    }
}

pub fn symbology_label(s: Symbology) -> &'static str {
    match s {
        Symbology::Qr => "QR",
        Symbology::Code128 => "Code 128",
        Symbology::Ean13 => "EAN-13",
        Symbology::Datamatrix => "DataMatrix",
    }
}

pub fn dither_label(d: DitherKind) -> &'static str {
    match d {
        DitherKind::Threshold => "Threshold",
        DitherKind::FloydSteinberg => "Floyd steinberg",
        DitherKind::Atkinson => "Atkinson",
        DitherKind::Bayer4 => "Bayer4",
        DitherKind::Bayer8 => "Bayer8",
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    } else {
        s.to_owned()
    }
}

/// `itemSummary`: title and one-line summary for the block list.
pub fn item_summary(item: &Item, icon_label: impl Fn(&str) -> Option<String>) -> (String, String) {
    match item {
        Item::Text(t) => {
            let line = t.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let summary = if line.trim().is_empty() {
                "(empty)".into()
            } else {
                truncate(line.trim(), 40)
            };
            ("Text".into(), summary)
        }
        Item::Icon(i) => (
            "Icon".into(),
            icon_label(&i.icon_id).unwrap_or_else(|| i.icon_id.clone()),
        ),
        Item::Code(c) if c.content == CodeContent::Wifi && c.symbology == Symbology::Qr => {
            // A Wi-Fi code shows its network name, never the password.
            let ssid = c.wifi.as_ref().map(|w| w.ssid.as_str()).unwrap_or("");
            let summary = if ssid.is_empty() {
                "(no network name)".into()
            } else {
                truncate(ssid, 40)
            };
            ("Wi-Fi QR".into(), summary)
        }
        Item::Code(c) => (
            symbology_label(c.symbology).into(),
            if c.data.is_empty() {
                "(empty)".into()
            } else {
                truncate(&c.data, 40)
            },
        ),
        Item::Image(i) => (
            "Image".into(),
            if i.blob_ref.is_empty() && i.data_url.is_none() {
                "No image chosen".into()
            } else {
                dither_label(i.dither).into()
            },
        ),
        Item::Shape(s) => {
            let name = match s.shape {
                ShapeKind::Rect => "Rectangle",
                ShapeKind::Ellipse => "Ellipse",
                ShapeKind::Line => "Line",
            };
            ("Shape".into(), format!("{name} · {}", format_mm(s.width_mm)))
        }
        Item::Spacer(s) => ("Spacer".into(), format_mm(s.width_mm)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use baradar_core::doc::{Item, ItemKind};

    #[test]
    fn formatting() {
        assert_eq!(format_mm(24.0), "24 mm");
        assert_eq!(format_mm(3.5), "3.5 mm");
        assert_eq!(print_button_text(3), "Print 3 labels");
        assert_eq!(ruler_steps(7.0866 * 0.53), (2, 10));
        let p = PrintProgress { page: 2, of: 3, phase: PagePhase::Sending };
        assert_eq!(progress_text(Some(p)), "Sending label 2 of 3…");
        assert!((progress_fraction(Some(p)) - 0.3833).abs() < 1e-3);
    }

    #[test]
    fn summaries() {
        let (t, s) = item_summary(&Item::new(ItemKind::Shape), |_| None);
        assert_eq!((t.as_str(), s.as_str()), ("Shape", "Rectangle · 10 mm"));
        let (t, s) = item_summary(&Item::new(ItemKind::Code), |_| None);
        assert_eq!((t.as_str(), s.as_str()), ("QR", "https://example.com"));
        let (_, s) = item_summary(&Item::new(ItemKind::Image), |_| None);
        assert_eq!(s, "No image chosen");
    }
}
