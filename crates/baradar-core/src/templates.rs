//! The "New from template" catalogue (port of `web/src/ui/templates/templates.ts`).
//!
//! A template is a plain [`LabelDoc`] with obvious placeholder text; [`Template::build`] returns a
//! fresh doc (new ids) every call. Sizes follow the PT-P710BT at 180 dpi: printable band 12 mm →
//! 70 dots (9.9 mm), 24 mm → 128 dots (18.1 mm); the printer adds a ≈ 2 mm feed margin at each
//! end of every label (not counted in `length`). The 36 mm variants follow the PT-P910BT at
//! 360 dpi: 36 mm → 454 dots (32 mm).
//!
//! `build(band)` takes the printable band the template is opened on, so quick size "M" is M on
//! that band; other sizes are physical (mm) and fit every model's band of the template's width.

use crate::doc::schema::{
    Align, BatchCounter, BatchData, CodeContent, CodeItem, Item, ItemKind, ItemSize, LabelDoc,
    LabelFrame, LayoutSettings, LengthSettings, Margins, ModuleSize, QrEcc, QuietZone, TextSize,
    WifiSettings,
};
use crate::render::units::{dots_to_mm, js_round};
use crate::vars::variables::{VariableContext, js_to_fixed, resolve_doc};

/// Gallery sections. `TemplateCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemplateCategory {
    Network,
    Cables,
    Storage,
    Office,
}

impl TemplateCategory {
    /// The TS id (`'network'`, `'cables'`, `'storage'`, `'office'`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Cables => "cables",
            Self::Storage => "storage",
            Self::Office => "office",
        }
    }

    /// The section heading.
    pub fn label(self) -> &'static str {
        match self {
            Self::Network => "Wi-Fi",
            Self::Cables => "Cables",
            Self::Storage => "Shelves, bins & drawers",
            Self::Office => "Office",
        }
    }
}

/// Gallery sections, in display order. `TEMPLATE_CATEGORIES`.
pub const TEMPLATE_CATEGORIES: [TemplateCategory; 4] = [
    TemplateCategory::Network,
    TemplateCategory::Cables,
    TemplateCategory::Storage,
    TemplateCategory::Office,
];

/// The printable band a template is opened on (the active model's print area for its width).
/// `TemplateBand`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateBand {
    pub band_dots: u32,
    pub dpi: u16,
}

/// One catalogue entry. `TemplateDef`.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    pub id: &'static str,
    /// Card title (unique; also the new label's name without the size suffix).
    pub name: &'static str,
    /// One sentence for the card: what it is for and how to adapt it.
    pub description: &'static str,
    pub category: TemplateCategory,
    pub tape_width_mm: f64,
    build_fn: fn(Option<TemplateBand>) -> LabelDoc,
}

impl Template {
    /// A fresh doc. `band`: the printable band it is opened on, so quick sizes are that band's;
    /// without it, the default band of the tape. `TemplateDef.build`.
    pub fn build(&self, band: Option<TemplateBand>) -> LabelDoc {
        (self.build_fn)(band)
    }

    /// The length a card shows before its thumbnail is rendered: the fixed length, or a typical
    /// short label (40 mm). `TemplateThumb.svelte guessMm`.
    pub fn guess_length_mm(&self) -> f64 {
        match self.build(None).length {
            LengthSettings::Fixed { mm, .. } => mm,
            LengthSettings::Auto => 40.0,
        }
    }

    /// The card's length chip (the editor's headline: printed length plus the feed margin at both
    /// ends), measured by rendering the template as the thumbnail does: built on `band`, Wi-Fi
    /// codes filled with [`SAMPLE_WIFI`], variables resolved as label 0 of `ctx`'s day.
    /// `render` returns the renderer's `(length_mm, feed_margin_mm)` for that doc.
    /// (`TemplateThumb.svelte draw` → `onrender`.)
    pub fn gallery_length_mm<E>(
        &self,
        band: Option<TemplateBand>,
        ctx: &VariableContext,
        render: impl FnOnce(&LabelDoc) -> Result<(f64, f64), E>,
    ) -> Result<f64, E> {
        let doc = with_sample_data(&self.build(band));
        let shown = resolve_doc(&doc, 0, ctx).doc;
        let (length_mm, feed_margin_mm) = render(&shown)?;
        Ok(length_mm + 2.0 * feed_margin_mm)
    }

    /// The card's length chip text: "34 mm label", plus " with a sample network" for templates
    /// with a Wi-Fi code. (`TemplateGallery.svelte`.)
    pub fn length_chip(&self, length_mm: f64) -> String {
        let sampled = self
            .build(None)
            .items
            .iter()
            .any(|i| matches!(i, Item::Code(c) if c.content == CodeContent::Wifi));
        let suffix = if sampled {
            " with a sample network"
        } else {
            ""
        };
        format!("{} label{suffix}", format_mm(length_mm))
    }
}

/// Default cable diameter of the cable flag (mm). `CABLE_FLAG_DIAMETER_MM`.
pub const CABLE_FLAG_DIAMETER_MM: f64 = 6.0;
/// Blank middle of the cable flag: one turn around the cable (π × Ø, rounded to 0.5 mm).
/// `CABLE_FLAG_WRAP_MM` = `Math.round(Math.PI * 6 * 2) / 2`.
pub const CABLE_FLAG_WRAP_MM: f64 = 19.0;
/// Default cable diameter of the cable wrap (mm). `CABLE_WRAP_DIAMETER_MM`.
pub const CABLE_WRAP_DIAMETER_MM: f64 = 8.0;
/// The overlap that the end of the cable wrap sticks to (mm). `CABLE_WRAP_OVERLAP_MM`.
pub const CABLE_WRAP_OVERLAP_MM: f64 = 15.0;
/// Gridfinity bins sit on a 42 mm grid; 32 mm printed + the printer's 2 × ≈ 2 mm feed margin
/// gives a ≈ 36 mm piece of 12 mm tape that fits the label tab of a 1-unit bin.
/// `GRIDFINITY_LENGTH_MM`.
pub const GRIDFINITY_LENGTH_MM: f64 = 32.0;

/// Quick size M (half the printable band) of 12 mm (35 dots) and 24 mm (64 dots) tape on the
/// PT-P710BT, and of 36 mm (227 dots at 360 dpi) on the PT-P910BT, in mm. `QUICK_M_MM`.
pub fn quick_m_mm(width_mm: f64) -> Option<f64> {
    match width_mm {
        12.0 => Some(4.94),
        24.0 => Some(9.03),
        36.0 => Some(16.02),
        _ => None,
    }
}

/// Quick size M of `band` (`render/text-size.ts quickTextSizes(...).m`: half the band, whole
/// dots, mm rounded to 0.01), else of the default band of that tape. `quickM`.
fn quick_m(width_mm: f64, band: Option<TemplateBand>) -> TextSize {
    let mm = match band {
        Some(b) if b.dpi > 0 => {
            let band_dots = b.band_dots.max(1) as f64;
            let dots = js_round(band_dots * 0.5).max(1) as f64;
            js_round(dots_to_mm(dots, b.dpi) * 100.0) as f64 / 100.0
        }
        _ => quick_m_mm(width_mm).unwrap_or(4.94),
    };
    TextSize::Mm { mm }
}

const fn mm(mm: f64) -> TextSize {
    TextSize::Mm { mm }
}

/// `text(t, patch)`: a default text block with `t`, adjusted by `f`.
fn text(t: &str, f: impl FnOnce(&mut crate::doc::schema::TextItem)) -> Item {
    let mut item = Item::new(ItemKind::Text);
    if let Item::Text(x) = &mut item {
        x.text = t.into();
        f(x);
    }
    item
}

/// `icon(iconId, patch)`; `size` `None` keeps fit.
fn icon(icon_id: &str, size_mm: Option<f64>) -> Item {
    let mut item = Item::new(ItemKind::Icon);
    if let Item::Icon(x) = &mut item {
        x.icon_id = icon_id.into();
        if let Some(mm) = size_mm {
            x.size = ItemSize::Mm { mm };
        }
    }
    item
}

/// `spacer(widthMm)`.
fn spacer(width_mm: f64) -> Item {
    let mut item = Item::new(ItemKind::Spacer);
    if let Item::Spacer(x) = &mut item {
        x.width_mm = width_mm;
    }
    item
}

/// `code(patch)`.
fn code(f: impl FnOnce(&mut CodeItem)) -> Item {
    let mut item = Item::new(ItemKind::Code);
    if let Item::Code(x) = &mut item {
        f(x);
    }
    item
}

/// A QR code for a Wi-Fi network (the network is filled in the code editor). `wifiQr`.
fn wifi_qr(quiet_zone: QuietZone, ecc: QrEcc) -> Item {
    code(|c| {
        c.content = CodeContent::Wifi;
        c.data = String::new();
        c.wifi = Some(WifiSettings::default());
        c.module_dots = ModuleSize::Auto;
        c.quiet_zone = quiet_zone;
        c.ecc = ecc;
    })
}

/// `label(name, widthMm, items, patch)` = `createDoc({name, tape: {widthMm}, items, ...patch})`.
fn label(name: &str, width_mm: f64, items: Vec<Item>, f: impl FnOnce(&mut LabelDoc)) -> LabelDoc {
    let mut doc = LabelDoc::with_items(items);
    doc.name = name.into();
    doc.tape.width_mm = width_mm;
    f(&mut doc);
    doc
}

fn flow(gap_mm: f64) -> LayoutSettings {
    LayoutSettings::Flow {
        gap_mm,
        align: Align::Center,
    }
}

fn margins(start: f64, end: f64) -> Margins {
    Margins { start, end }
}

fn fixed(mm: f64) -> LengthSettings {
    LengthSettings::Fixed { mm, shrink: true }
}

static TEMPLATES: [Template; 14] = [
    Template {
        id: "wifi-12",
        name: "Wi-Fi sticker (12 mm)",
        description: "Small sticker: guests scan the QR code with the iPhone Camera app to join. Enter your network in the QR block.",
        category: TemplateCategory::Network,
        tape_width_mm: 12.0,
        // ECC L keeps a typical WPA payload at version 3 (29 modules): 2 dots per module on the
        // 70-dot band. "Wi-Fi" over the network name in Archivo Narrow, 5.5 mm for both lines.
        build_fn: |_| {
            let items = vec![
                wifi_qr(QuietZone::Compact, QrEcc::L),
                text("Wi-Fi\n{{ssid}}", |t| {
                    t.font_family = "archivo-narrow".into();
                    t.size = mm(5.5);
                    t.align = Align::Start;
                }),
            ];
            label("Wi-Fi sticker", 12.0, items, |d| {
                d.margins_mm = margins(1.5, 2.0);
                d.layout = flow(2.0);
            })
        },
    },
    Template {
        id: "wifi-24",
        name: "Wi-Fi sticker (24 mm)",
        description: "Larger sticker with a Wi-Fi icon, the network name and a QR code that phones join from.",
        category: TemplateCategory::Network,
        tape_width_mm: 24.0,
        // Two lines at M (9 mm for both): ≈ 71 mm with "MyHomeNetwork".
        build_fn: |band| {
            let items = vec![
                icon("wifi", Some(9.0)),
                text("Wi-Fi\n{{ssid}}", |t| {
                    t.line_height = 1.15;
                    t.align = Align::Start;
                    t.size = quick_m(24.0, band);
                }),
                wifi_qr(QuietZone::Standard, QrEcc::M),
            ];
            label("Wi-Fi sticker", 24.0, items, |d| d.layout = flow(2.5))
        },
    },
    Template {
        id: "wifi-36",
        name: "Wi-Fi sticker (36 mm)",
        description: "Big sticker for a wall or a router: Wi-Fi icon, the network name and a QR code that phones join from across the room.",
        category: TemplateCategory::Network,
        tape_width_mm: 36.0,
        // The 24 mm design on the 32 mm band of a 360 dpi model.
        build_fn: |band| {
            let items = vec![
                icon("wifi", Some(14.0)),
                text("Wi-Fi\n{{ssid}}", |t| {
                    t.line_height = 1.15;
                    t.align = Align::Start;
                    t.size = quick_m(36.0, band);
                }),
                wifi_qr(QuietZone::Standard, QrEcc::M),
            ];
            label("Wi-Fi sticker", 36.0, items, |d| d.layout = flow(3.5))
        },
    },
    Template {
        id: "cable-flag",
        name: "Cable flag",
        description: "The same text on both ends; the blank middle (19 mm ≈ π × 6 mm) wraps around the cable and the ends stick together, so it reads from either side. Other cables: spacer = π × diameter.",
        category: TemplateCategory::Cables,
        tape_width_mm: 12.0,
        build_fn: |band| {
            let name = || {
                text("Cable name", |t| {
                    t.font_family = "archivo-narrow".into();
                    t.size = quick_m(12.0, band);
                })
            };
            let items = vec![name(), spacer(CABLE_FLAG_WRAP_MM), name()];
            label("Cable flag", 12.0, items, |d| {
                d.margins_mm = margins(1.0, 1.0);
                d.layout = flow(2.0);
            })
        },
    },
    Template {
        id: "cable-wrap",
        name: "Cable wrap",
        description: "Repeated text that wraps once around a cable up to 8 mm thick (π × Ø + 15 mm overlap), so the name shows from every side.",
        category: TemplateCategory::Cables,
        tape_width_mm: 9.0,
        build_fn: |_| {
            let items = vec![text("LAN 1 · LAN 1 · LAN 1", |t| {
                t.font_family = "archivo-narrow".into();
                t.size = mm(4.0);
            })];
            label("Cable wrap", 9.0, items, |d| {
                d.length = fixed(js_round(
                    std::f64::consts::PI * CABLE_WRAP_DIAMETER_MM + CABLE_WRAP_OVERLAP_MM,
                ) as f64);
                d.margins_mm = margins(0.0, 0.0);
            })
        },
    },
    Template {
        id: "bin-24",
        name: "Shelf or bin label (24 mm)",
        description: "Icon and contents for shelves, boxes and bins. Pick another icon in the icon block.",
        category: TemplateCategory::Storage,
        tape_width_mm: 24.0,
        build_fn: |band| {
            let items = vec![
                icon("archive", Some(14.0)),
                text("Contents", |t| t.size = quick_m(24.0, band)),
            ];
            label("Shelf label", 24.0, items, |_| {})
        },
    },
    Template {
        id: "bin-36",
        name: "Shelf or bin label (36 mm)",
        description: "Large icon and contents for shelves, crates and archive boxes, readable from a distance.",
        category: TemplateCategory::Storage,
        tape_width_mm: 36.0,
        build_fn: |band| {
            let items = vec![
                icon("archive", Some(22.0)),
                text("Contents", |t| t.size = quick_m(36.0, band)),
            ];
            label("Shelf label", 36.0, items, |d| d.layout = flow(3.0))
        },
    },
    Template {
        id: "drawer-12",
        name: "Drawer label (12 mm)",
        description: "Compact icon and contents for drawers and small boxes.",
        category: TemplateCategory::Storage,
        tape_width_mm: 12.0,
        build_fn: |band| {
            let items = vec![
                icon("package", None),
                text("Contents", |t| t.size = quick_m(12.0, band)),
            ];
            label("Drawer label", 12.0, items, |d| d.layout = flow(2.0))
        },
    },
    Template {
        id: "gridfinity-12",
        name: "Gridfinity bin (12 mm)",
        description: "≈ 36 mm piece (32 mm + feed margins): fits the label tab of a 1-unit (42 mm) Gridfinity bin.",
        category: TemplateCategory::Storage,
        tape_width_mm: 12.0,
        build_fn: |_| {
            let items = vec![icon("nut", Some(8.0)), text("M3 × 8", |t| t.size = mm(6.0))];
            label("Gridfinity label", 12.0, items, |d| {
                d.length = fixed(GRIDFINITY_LENGTH_MM);
                d.margins_mm = margins(1.0, 1.0);
                d.layout = flow(1.5);
            })
        },
    },
    Template {
        id: "asset-tag",
        name: "Asset tag",
        description: "Small numbered tags (about 3 cm each) with a QR code: prints Asset 0001 to 0010 in one job. Change the count and start in the batch panel.",
        category: TemplateCategory::Office,
        tape_width_mm: 12.0,
        // A short QR payload ("0001": version 1, 21 modules) gets 3 dots per module on the 70-dot
        // band; the two-line text has a fixed size so every tag stays short.
        build_fn: |_| {
            let items = vec![
                code(|c| {
                    c.data = "{{id}}".into();
                    c.quiet_zone = QuietZone::Compact;
                    c.ecc = QrEcc::M;
                }),
                text("Asset\n{{id}}", |t| {
                    t.size = mm(7.0);
                    t.line_height = 1.1;
                }),
            ];
            label("Asset tag", 12.0, items, |d| {
                d.margins_mm = margins(1.5, 1.5);
                d.layout = flow(1.5);
                d.batch = Some(BatchData {
                    enabled: true,
                    count: 10,
                    counters: vec![BatchCounter {
                        name: "id".into(),
                        start: 1,
                        step: 1,
                        pad: 4,
                    }],
                    ..BatchData::default()
                });
            })
        },
    },
    Template {
        id: "folder-spine",
        name: "Folder spine",
        description: "Large title for the spine of a binder or file box (100 mm long); a long title gets smaller to fit.",
        category: TemplateCategory::Office,
        tape_width_mm: 24.0,
        build_fn: |_| {
            let items = vec![text("Folder title", |t| {
                t.font_weight = 700;
                t.size = mm(15.0);
            })];
            label("Folder spine", 24.0, items, |d| d.length = fixed(100.0))
        },
    },
    Template {
        id: "folder-spine-36",
        name: "Folder spine (36 mm)",
        description: "Large title for the wide spine of a lever-arch file or archive box (150 mm long); a long title gets smaller to fit.",
        category: TemplateCategory::Office,
        tape_width_mm: 36.0,
        build_fn: |_| {
            let items = vec![text("Folder title", |t| {
                t.font_weight = 700;
                t.size = mm(22.0);
            })];
            label("Folder spine", 36.0, items, |d| d.length = fixed(150.0))
        },
    },
    Template {
        id: "name-tag",
        name: "Name tag",
        description: "Two lines with a thin border: a name and a team or role.",
        category: TemplateCategory::Office,
        tape_width_mm: 24.0,
        build_fn: |_| {
            let items = vec![text("Your Name\nTeam or role", |t| t.line_height = 1.15)];
            label("Name tag", 24.0, items, |d| {
                d.margins_mm = margins(4.0, 4.0);
                d.frame = Some(LabelFrame {
                    thickness_mm: 0.4,
                    radius_mm: 2.0,
                    inset_mm: 0.0,
                });
            })
        },
    },
    Template {
        id: "name-tag-36",
        name: "Name tag (36 mm)",
        description: "A large name tag for events and doors: two lines with a thin rounded border.",
        category: TemplateCategory::Office,
        tape_width_mm: 36.0,
        build_fn: |_| {
            let items = vec![text("Your Name\nTeam or role", |t| t.line_height = 1.15)];
            label("Name tag", 36.0, items, |d| {
                d.margins_mm = margins(6.0, 6.0);
                d.frame = Some(LabelFrame {
                    thickness_mm: 0.5,
                    radius_mm: 3.0,
                    inset_mm: 0.0,
                });
            })
        },
    },
];

/// The catalogue, in gallery order. `TEMPLATES`.
pub fn templates() -> &'static [Template] {
    &TEMPLATES
}

/// `templateById`.
pub fn template_by_id(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Templates of `category`, in catalogue order. `templatesIn`.
pub fn templates_in(category: TemplateCategory) -> Vec<&'static Template> {
    TEMPLATES
        .iter()
        .filter(|t| t.category == category)
        .collect()
}

/// The active model takes the template's tape width. `templateFits`.
pub fn template_fits(t: &Template, widths: &[f64]) -> bool {
    widths.iter().any(|w| (w - t.tape_width_mm).abs() < 0.01)
}

/// What [`template_model`] needs to know about a printer model (`ModelInfo` in the TS).
#[derive(Debug, Clone, Copy)]
pub struct ModelWidths<'a> {
    /// Canonical name, e.g. `PT-P710BT`.
    pub name: &'a str,
    /// Other names the model answers to (matched case-insensitively, like `name`).
    pub aliases: &'a [&'a str],
    /// The tape widths the editor offers for it (`models.ts tapeWidthsOf`).
    pub tape_widths_mm: &'a [f64],
}

/// The model a template card is drawn and opened with: `active` when it takes the template's
/// width (or the table does not know it), else the first table model that takes the width
/// (`models.ts standInModel`); `None` when no model takes it. `templateModel`.
pub fn template_model(t: &Template, active: &str, models: &[ModelWidths<'_>]) -> Option<String> {
    let n = active.to_lowercase();
    let info = models
        .iter()
        .find(|m| m.name.to_lowercase() == n || m.aliases.iter().any(|a| a.to_lowercase() == n));
    match info {
        None => Some(active.to_owned()),
        Some(m) if template_fits(t, m.tape_widths_mm) => Some(m.name.to_owned()),
        Some(_) => models
            .iter()
            .find(|m| template_fits(t, m.tape_widths_mm))
            .map(|m| m.name.to_owned()),
    }
}

/// `view-model.ts formatMm(mm, 1)`: "24 mm", "3.5 mm".
fn format_mm(mm: f64) -> String {
    let s = js_to_fixed(mm, 1);
    let s = s.strip_suffix(".0").unwrap_or(&s);
    format!("{s} mm")
}

/// "Needs 36 mm tape · PT-P710BT takes up to 24 mm" (a width above the model's widest) or
/// "Needs 9 mm tape · PT-X doesn’t take it" (a gap); `None` when the model takes the width (or
/// `widths` is unknown). `needsWidthText`.
pub fn needs_width_text(width_mm: f64, model: &str, widths: &[f64]) -> Option<String> {
    if widths.is_empty() || widths.iter().any(|w| (w - width_mm).abs() < 0.01) {
        return None;
    }
    let max = widths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let why = if width_mm > max {
        format!("takes up to {}", format_mm(max))
    } else {
        "doesn’t take it".to_owned()
    };
    Some(format!(
        "Needs {} tape · {model} {why}",
        format_mm(width_mm)
    ))
}

/// The template is designed for the tape that is loaded in the printer. `fitsLoadedTape`.
pub fn fits_loaded_tape(t: &Template, loaded_width_mm: Option<f64>) -> bool {
    loaded_width_mm.is_some_and(|w| (w - t.tape_width_mm).abs() < 0.01)
}

/// The block to select after opening a template: the Wi-Fi code (its network must be entered),
/// else the first text block, else the first block. `primaryItemId`.
pub fn primary_item_id(doc: &LabelDoc) -> Option<&str> {
    doc.items
        .iter()
        .find(|i| matches!(i, Item::Code(c) if c.content == CodeContent::Wifi))
        .or_else(|| doc.items.iter().find(|i| matches!(i, Item::Text(_))))
        .or_else(|| doc.items.first())
        .map(Item::id)
}

/// Sample network for gallery thumbnails and tests (never stored in a label): (ssid, password).
/// `SAMPLE_WIFI`.
pub const SAMPLE_WIFI: (&str, &str) = ("Home Wi-Fi", "correct-horse-42");

/// A copy of `doc` with the Wi-Fi codes filled with [`SAMPLE_WIFI`] (so thumbnails show a QR).
/// `withSampleData`.
pub fn with_sample_data(doc: &LabelDoc) -> LabelDoc {
    let mut out = doc.clone();
    for item in &mut out.items {
        if let Item::Code(c) = item
            && c.content == CodeContent::Wifi
        {
            let w = c.wifi.get_or_insert_with(WifiSettings::default);
            w.ssid = SAMPLE_WIFI.0.into();
            w.password = SAMPLE_WIFI.1.into();
        }
    }
    out
}
