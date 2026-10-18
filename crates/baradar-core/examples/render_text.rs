//! Renders one text label: `cargo run -p baradar-core --example render_text -- out.png "text" [tape_mm] [weight]`.

use std::sync::Arc;

use baradar_core::doc::{Item, ItemKind, LabelDoc};
use baradar_core::export::png::export_png;
use baradar_core::render::fonts::FontRegistry;
use baradar_core::render::renderer::{RenderOptions, RenderTarget, Renderer};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "text.png".into());
    let text = args.next().unwrap_or_else(|| "Label".into());
    let width: u8 = args.next().and_then(|w| w.parse().ok()).unwrap_or(12);
    let weight: u16 = args.next().and_then(|w| w.parse().ok()).unwrap_or(600);
    let mut item = Item::new(ItemKind::Text);
    if let Item::Text(t) = &mut item {
        t.text = text.replace("\\n", "\n");
        t.font_weight = weight;
    }
    let doc = LabelDoc::with_items(vec![item]);
    let profile = ptouch::profile(ptouch::Model::PtE720bt).expect("model");
    let tape = profile.tape(width, ptouch::TapeKind::Tze).expect("tape");
    let r = Renderer::new(Arc::new(FontRegistry::new()))
        .render(&doc, &RenderTarget { profile, tape }, &RenderOptions::default())?;
    for w in &r.warnings {
        eprintln!("warning: {}", w.message);
    }
    std::fs::write(&out, export_png(&r.bitmap, profile.dpi)?)?;
    eprintln!("{} × {} dots", r.length_dots, r.height_dots);
    Ok(())
}
