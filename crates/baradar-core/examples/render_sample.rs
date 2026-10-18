//! Renders a sample label to a 1-bit PNG: `cargo run -p baradar-core --example render_sample -- out.png [tape_mm]`.

use std::sync::Arc;

use baradar_core::doc::{Item, ItemKind, LabelDoc, Symbology};
use baradar_core::export::png::export_png;
use baradar_core::render::fonts::FontRegistry;
use baradar_core::render::renderer::{RenderOptions, RenderTarget, Renderer};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "sample.png".into());
    let width: u8 = args.next().and_then(|w| w.parse().ok()).unwrap_or(24);

    let mut doc = LabelDoc::with_items(vec![]);
    let mut text = Item::new(ItemKind::Text);
    if let Item::Text(t) = &mut text {
        t.text = "Rack B\nCable 7".into();
    }
    let mut icon = Item::new(ItemKind::Icon);
    if let Item::Icon(i) = &mut icon {
        i.icon_id = "bolt".into();
    }
    let mut ean = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut ean {
        c.symbology = Symbology::Ean13;
        c.data = "590123412345".into();
        c.show_text = true;
    }
    doc.items = vec![icon, text, Item::new(ItemKind::Code), ean];
    doc.frame = Some(baradar_core::doc::LabelFrame::UI_DEFAULT);

    let profile = ptouch::profile(ptouch::Model::PtE720bt).expect("model");
    let tape = profile.tape(width, ptouch::TapeKind::Tze).expect("tape width");
    let renderer = Renderer::new(Arc::new(FontRegistry::new()));
    let r = renderer.render(&doc, &RenderTarget { profile, tape }, &RenderOptions::default())?;
    for w in &r.warnings {
        eprintln!("warning [{}]: {}", w.code, w.message);
    }
    std::fs::write(&out, export_png(&r.bitmap, profile.dpi)?)?;
    eprintln!("{} × {} dots ({:.1} mm) → {out}", r.length_dots, r.height_dots, r.length_mm);
    Ok(())
}
