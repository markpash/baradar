//! Writes golden-comparison cases: for each test document, a share link the web studio opens
//! and baradar's own render as row-major bits (`1` = ink).
//!
//! `cargo run -p baradar-core --example golden_docs -- out.json`
//!
//! To compare, open each `url` in a browser (https://fjanicki.github.io/ptouch/), read the
//! preview `<canvas>` pixels (dark = ink; the canvas is the exact print bitmap) and diff them
//! with `bits`. On 2026-10-18 (studio at 485dab4) all 26 cases had identical dimensions; codes,
//! icons and the Silkscreen pixel font matched dot for dot, shapes and frames within 0.1 % of
//! ink, and anti-aliased text within about 4 % after `TEXT_EMBOLDEN_PX`.

use std::sync::Arc;

use baradar_core::doc::share::{STUDIO_BASE_URL, create_share_link};
use baradar_core::doc::{
    Align, Item, ItemKind, LabelDoc, LabelFrame, LayoutSettings, LengthSettings, QuietZone,
    ShapeKind, Symbology, TextSize,
};
use baradar_core::render::fonts::FontRegistry;
use baradar_core::render::renderer::{RenderOptions, RenderTarget, Renderer};
use baradar_core::templates::{TemplateBand, templates};
use baradar_core::vars::variables::{VariableContext, resolve_doc};
use serde_json::json;

fn text(s: &str, size: TextSize, family: &str) -> Item {
    let mut i = Item::new(ItemKind::Text);
    if let Item::Text(t) = &mut i {
        t.text = s.into();
        t.size = size;
        t.font_family = family.into();
    }
    i
}

fn cases() -> Vec<(String, LabelDoc)> {
    let mut out = Vec::new();
    let mut add = |name: &str, width: f64, items: Vec<Item>, edit: &dyn Fn(&mut LabelDoc)| {
        let mut d = LabelDoc::with_items(items);
        d.name = name.into();
        d.tape.width_mm = width;
        edit(&mut d);
        out.push((name.to_owned(), d));
    };
    let none = &|_: &mut LabelDoc| {};
    add("fit-12", 12.0, vec![text("Label", TextSize::Fit, "fira-sans")], none);
    add("fit-24-two-lines", 24.0, vec![text("Rack B\nCable 7", TextSize::Fit, "fira-sans")], none);
    add("pt-18", 18.0, vec![text("Points 14", TextSize::Pt { pt: 14.0 }, "lexend")], none);
    add("mm-24-mono", 24.0, vec![text("MONO 123", TextSize::Mm { mm: 6.0 }, "jetbrains-mono")], none);
    add("silkscreen-12", 12.0, vec![text("PIXEL", TextSize::Fit, "silkscreen")], none);
    let mut qr = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut qr {
        c.data = "https://example.com".into();
    }
    add("qr-24", 24.0, vec![qr.clone()], none);
    let mut qrc = qr.clone();
    if let Item::Code(c) = &mut qrc {
        c.quiet_zone = QuietZone::Compact;
    }
    add("qr-compact-12", 12.0, vec![qrc], none);
    let mut ean = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut ean {
        c.symbology = Symbology::Ean13;
        c.data = "590123412345".into();
        c.show_text = true;
    }
    add("ean13-text-24", 24.0, vec![ean], none);
    let mut c128 = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut c128 {
        c.symbology = Symbology::Code128;
        c.data = "ABC-1234".into();
        c.show_text = false;
    }
    add("code128-12", 12.0, vec![c128], none);
    let mut dm = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut dm {
        c.symbology = Symbology::Datamatrix;
        c.data = "DATAMATRIX".into();
    }
    add("datamatrix-18", 18.0, vec![dm], none);
    let mut icon = Item::new(ItemKind::Icon);
    if let Item::Icon(i) = &mut icon {
        i.icon_id = "bolt".into();
    }
    add("icon-24", 24.0, vec![icon], none);
    let mut ellipse = Item::new(ItemKind::Shape);
    if let Item::Shape(s) = &mut ellipse {
        s.shape = ShapeKind::Ellipse;
    }
    let mut line = Item::new(ItemKind::Shape);
    if let Item::Shape(s) = &mut line {
        s.shape = ShapeKind::Line;
        s.width_mm = 15.0;
    }
    add("shapes-24", 24.0, vec![Item::new(ItemKind::Shape), ellipse, line], none);
    add(
        "frame-fixed-24",
        24.0,
        vec![text("Framed", TextSize::Fit, "oswald")],
        &|d| {
            d.frame = Some(LabelFrame::UI_DEFAULT);
            d.length = LengthSettings::Fixed { mm: 60.0, shrink: false };
        },
    );
    add(
        "shrink-12",
        12.0,
        vec![text("Much too long for thirty", TextSize::Fit, "fira-sans")],
        &|d| d.length = LengthSettings::Fixed { mm: 30.0, shrink: true },
    );
    add(
        "align-end-gap-24",
        24.0,
        vec![text("Top", TextSize::Mm { mm: 4.0 }, "fira-sans"), text("End", TextSize::Mm { mm: 4.0 }, "fira-sans")],
        &|d| d.layout = LayoutSettings::Flow { gap_mm: 6.0, align: Align::End },
    );
    let mut inv = text("INVERT", TextSize::Fit, "anton");
    if let Item::Text(t) = &mut inv {
        t.invert = true;
    }
    add("invert-18", 18.0, vec![inv], none);
    for t in templates() {
        let band = match t.tape_width_mm as u32 {
            12 => 70,
            18 => 112,
            24 => 128,
            9 => 50,
            6 => 32,
            _ => 128,
        };
        let doc = t.build(Some(TemplateBand { band_dots: band, dpi: 180 }));
        out.push((format!("template-{}", t.id), doc));
    }
    out
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "golden.json".into());
    let renderer = Renderer::new(Arc::new(FontRegistry::new()));
    let profile = ptouch::profile(ptouch::Model::PtP710bt).expect("model");
    let today = time::OffsetDateTime::now_utc().date();
    let mut list = Vec::new();
    for (name, mut doc) in cases() {
        doc.tape.model = Some("PT-P710BT".into());
        let byte = if doc.tape.width_mm == 3.5 { 4 } else { doc.tape.width_mm as u8 };
        let Some(tape) = profile.tape(byte, ptouch::TapeKind::Tze) else {
            eprintln!("skip {name}: no tape");
            continue;
        };
        let resolved = resolve_doc(&doc, 0, &VariableContext::new(today)).doc.into_owned();
        let r = renderer.render(&resolved, &RenderTarget { profile, tape }, &RenderOptions::default())?;
        let link = match create_share_link(&doc, STUDIO_BASE_URL, |_| None, true) {
            Ok(l) => l.url,
            Err(e) => {
                eprintln!("skip {name}: {e}");
                continue;
            }
        };
        let (len, h) = (r.bitmap.length(), r.bitmap.height());
        let mut bits = String::with_capacity((len as usize) * (h as usize));
        for y in 0..h {
            for x in 0..len {
                bits.push(if r.bitmap.get(x, y) { '1' } else { '0' });
            }
        }
        list.push(json!({
            "name": name, "url": link, "length": len, "height": h, "bits": bits,
            "warnings": r.warnings.iter().map(|w| w.message.clone()).collect::<Vec<_>>(),
        }));
    }
    eprintln!("{} cases", list.len());
    std::fs::write(&out, serde_json::to_vec(&list)?)?;
    Ok(())
}
