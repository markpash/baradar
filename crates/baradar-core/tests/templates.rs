//! Port of web/tests/unit/ui/templates.test.ts (the catalogue and the template helpers; the
//! gallery's grid movement and task queue are UI code and are not ported). Model bands are the
//! core table's values for the models the TS test asks the wasm build about.

use baradar_core::doc::schema::{
    Align, CodeContent, CodeItem, Item, ItemKind, ItemSize, LabelDoc, LayoutSettings,
    LengthSettings, ModuleSize, QrEcc, QuietZone, Symbology, TAPE_WIDTHS_MM, TextItem, TextSize,
    WifiSettings,
};
use baradar_core::render::units::{js_round, mm_to_dots};
use baradar_core::templates::*;
use baradar_core::vars::variables::VariableContext;
use time::macros::date;

fn build(id: &str) -> LabelDoc {
    template_by_id(id).expect(id).build(None)
}

fn texts(d: &LabelDoc) -> Vec<&TextItem> {
    d.items
        .iter()
        .filter_map(|i| if let Item::Text(t) = i { Some(t) } else { None })
        .collect()
}

fn codes(d: &LabelDoc) -> Vec<&CodeItem> {
    d.items
        .iter()
        .filter_map(|i| if let Item::Code(c) = i { Some(c) } else { None })
        .collect()
}

fn kinds(d: &LabelDoc) -> Vec<ItemKind> {
    d.items.iter().map(Item::kind).collect()
}

fn band(band_dots: u32, dpi: u16) -> Option<TemplateBand> {
    Some(TemplateBand { band_dots, dpi })
}

/// `matchQuickSize(size, band, dpi) === 'm'`.
fn is_quick_m(size: &TextSize, band_dots: u32, dpi: u16) -> bool {
    matches!(size, TextSize::Mm { mm } if mm_to_dots(*mm, dpi) == js_round(f64::from(band_dots) * 0.5))
}

const P710: [f64; 6] = [3.5, 6.0, 9.0, 12.0, 18.0, 24.0];
const P300: [f64; 4] = [3.5, 6.0, 9.0, 12.0];
const P910: [f64; 7] = [3.5, 6.0, 9.0, 12.0, 18.0, 24.0, 36.0];

#[test]
fn has_every_template_with_unique_ids_and_names() {
    let ids: Vec<&str> = templates().iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        [
            "wifi-12",
            "wifi-24",
            "wifi-36",
            "cable-flag",
            "cable-wrap",
            "bin-24",
            "bin-36",
            "drawer-12",
            "gridfinity-12",
            "asset-tag",
            "folder-spine",
            "folder-spine-36",
            "name-tag",
            "name-tag-36",
        ]
    );
    let names: std::collections::HashSet<&str> = templates().iter().map(|t| t.name).collect();
    assert_eq!(names.len(), templates().len());
    for t in templates() {
        assert!(t.description.chars().count() > 20, "{}", t.id);
        assert!(TEMPLATE_CATEGORIES.contains(&t.category), "{}", t.id);
    }
    let total: usize = TEMPLATE_CATEGORIES
        .iter()
        .map(|c| templates_in(*c).len())
        .sum();
    assert_eq!(total, templates().len());
    for c in TEMPLATE_CATEGORIES {
        assert!(!templates_in(c).is_empty(), "{}", c.id());
    }
    assert!(template_by_id("nope").is_none());
    let labels: Vec<(&str, &str)> = TEMPLATE_CATEGORIES
        .iter()
        .map(|c| (c.id(), c.label()))
        .collect();
    assert_eq!(
        labels,
        [
            ("network", "Wi-Fi"),
            ("cables", "Cables"),
            ("storage", "Shelves, bins & drawers"),
            ("office", "Office")
        ]
    );
}

#[test]
fn every_template_is_a_plain_label_on_its_tape() {
    for t in templates() {
        let doc = t.build(None);
        // Round-trips through the studio's JSON unchanged.
        let json = serde_json::to_string(&doc).unwrap();
        let back: LabelDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc, "{}", t.id);
        assert_eq!(doc.tape.width_mm, t.tape_width_mm, "{}", t.id);
        assert!(TAPE_WIDTHS_MM.contains(&t.tape_width_mm), "{}", t.id);
        assert!(!doc.items.is_empty(), "{}", t.id);
        for c in codes(&doc) {
            assert_eq!(c.module_dots, ModuleSize::Auto, "{}", t.id);
        }
        let first = doc.name.split(' ').next().unwrap_or("");
        assert!(t.name.starts_with(first), "{}", t.id);
    }
}

#[test]
fn build_makes_fresh_ids_every_call() {
    for t in templates() {
        let a = t.build(None);
        let b = t.build(None);
        assert_ne!(a.id, b.id, "{}", t.id);
        let ids: Vec<&str> = a.items.iter().map(Item::id).collect();
        let unique: std::collections::HashSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{}", t.id);
        for id in &ids {
            assert!(!b.items.iter().any(|i| i.id() == *id), "{}", t.id);
        }
    }
}

#[test]
fn wifi_stickers() {
    let small = build("wifi-12");
    assert_eq!(small.tape.width_mm, 12.0);
    let qr = codes(&small)[0];
    assert_eq!(
        (
            qr.symbology,
            qr.content,
            qr.quiet_zone,
            qr.ecc,
            qr.module_dots
        ),
        (
            Symbology::Qr,
            CodeContent::Wifi,
            QuietZone::Compact,
            QrEcc::L,
            ModuleSize::Auto
        )
    );
    assert_eq!(qr.wifi, Some(WifiSettings::default()));
    assert_eq!(qr.data, "");
    let t = texts(&small)[0];
    assert_eq!(t.text, "Wi-Fi\n{{ssid}}");
    assert_eq!(
        (t.font_family.as_str(), t.size, t.align),
        ("archivo-narrow", TextSize::Mm { mm: 5.5 }, Align::Start)
    );
    assert_eq!(small.length, LengthSettings::Auto);
    assert_eq!((small.margins_mm.start, small.margins_mm.end), (1.5, 2.0));
    assert_eq!(
        small.layout,
        LayoutSettings::Flow {
            gap_mm: 2.0,
            align: Align::Center
        }
    );

    let large = build("wifi-24");
    assert_eq!(large.tape.width_mm, 24.0);
    assert_eq!(
        kinds(&large),
        [ItemKind::Icon, ItemKind::Text, ItemKind::Code]
    );
    assert!(
        matches!(&large.items[0], Item::Icon(i) if i.icon_id == "wifi" && i.size == ItemSize::Mm { mm: 9.0 })
    );
    assert_eq!(texts(&large)[0].text, "Wi-Fi\n{{ssid}}");
    assert_eq!(
        texts(&large)[0].size,
        TextSize::Mm {
            mm: quick_m_mm(24.0).unwrap()
        }
    );
    assert_eq!(texts(&large)[0].line_height, 1.15);
    let c = codes(&large)[0];
    assert_eq!(
        (c.content, c.quiet_zone, c.ecc),
        (CodeContent::Wifi, QuietZone::Standard, QrEcc::M)
    );
    assert_eq!(large.length, LengthSettings::Auto);
    assert_eq!(
        large.layout,
        LayoutSettings::Flow {
            gap_mm: 2.5,
            align: Align::Center
        }
    );
}

#[test]
fn cable_flag() {
    assert_eq!(
        CABLE_FLAG_WRAP_MM,
        (std::f64::consts::PI * CABLE_FLAG_DIAMETER_MM * 2.0).round() / 2.0
    );
    let doc = build("cable-flag");
    assert_eq!(
        kinds(&doc),
        [ItemKind::Text, ItemKind::Spacer, ItemKind::Text]
    );
    let t = texts(&doc);
    assert_eq!(t[0].text, t[1].text);
    assert_eq!(t[0].size, t[1].size);
    assert!(matches!(&doc.items[1], Item::Spacer(s) if s.width_mm == CABLE_FLAG_WRAP_MM));
    assert_eq!(doc.margins_mm.start, doc.margins_mm.end);
    assert!(matches!(
        doc.layout,
        LayoutSettings::Flow {
            align: Align::Center,
            ..
        }
    ));
    assert_eq!(doc.tape.width_mm, 12.0);
    let desc = template_by_id("cable-flag").unwrap().description;
    assert!(desc.contains(&format!(
        "({CABLE_FLAG_WRAP_MM} mm ≈ π × {CABLE_FLAG_DIAMETER_MM} mm)"
    )));
}

#[test]
fn cable_wrap() {
    let doc = build("cable-wrap");
    assert_eq!(doc.tape.width_mm, 9.0);
    assert_eq!(
        doc.length,
        LengthSettings::Fixed {
            mm: (std::f64::consts::PI * CABLE_WRAP_DIAMETER_MM + CABLE_WRAP_OVERLAP_MM).round(),
            shrink: true
        }
    );
    let t = &texts(&doc)[0].text;
    let words: Vec<&str> = t.split(" · ").collect();
    assert!(!words[0].is_empty());
    let repeats = words.iter().filter(|w| **w == words[0]).count();
    assert!((3..=4).contains(&repeats));
    let desc = template_by_id("cable-wrap").unwrap().description;
    assert!(desc.contains(&format!(
        "up to {CABLE_WRAP_DIAMETER_MM} mm thick (π × Ø + {CABLE_WRAP_OVERLAP_MM} mm overlap)"
    )));
}

#[test]
fn storage_labels() {
    for id in ["bin-24", "drawer-12", "gridfinity-12"] {
        assert_eq!(kinds(&build(id)), [ItemKind::Icon, ItemKind::Text], "{id}");
    }
    assert_eq!(build("bin-24").tape.width_mm, 24.0);
    assert_eq!(build("drawer-12").tape.width_mm, 12.0);
    let grid = build("gridfinity-12");
    assert_eq!(grid.tape.width_mm, 12.0);
    assert_eq!(
        grid.length,
        LengthSettings::Fixed {
            mm: GRIDFINITY_LENGTH_MM,
            shrink: true
        }
    );
    assert!((35.0..=37.8).contains(&(GRIDFINITY_LENGTH_MM + 4.0)));
    let desc = template_by_id("gridfinity-12").unwrap().description;
    assert!(desc.contains(&format!("({GRIDFINITY_LENGTH_MM} mm + feed margins)")));
}

#[test]
fn asset_tag() {
    let doc = build("asset-tag");
    let c = codes(&doc)[0];
    assert_eq!(
        (c.symbology, c.content, c.data.as_str()),
        (Symbology::Qr, CodeContent::Text, "{{id}}")
    );
    assert_eq!(texts(&doc)[0].text, "Asset\n{{id}}");
    assert_eq!(texts(&doc)[0].size, TextSize::Mm { mm: 7.0 });
    let json = serde_json::to_value(&doc.batch).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"enabled": true, "columns": [], "rows": [], "count": 10, "counters": [{"name": "id", "start": 1, "step": 1, "pad": 4}], "dateFormat": "iso"})
    );
}

#[test]
fn fixed_text_sizes_except_the_name_tag_and_fixed_lengths_shrink() {
    for t in templates() {
        let doc = t.build(None);
        for i in texts(&doc) {
            assert_eq!(
                i.size == TextSize::Fit,
                t.id.starts_with("name-tag"),
                "{}: {}",
                t.id,
                i.text
            );
        }
        if let LengthSettings::Fixed { shrink, .. } = doc.length {
            assert!(shrink, "{}", t.id);
        }
    }
}

#[test]
fn quick_m_mm_is_m_of_the_default_bands() {
    for (w, band_dots) in [(12.0, 70), (24.0, 128)] {
        let m = quick_m_mm(w).unwrap();
        assert!(is_quick_m(&TextSize::Mm { mm: m }, band_dots, 180));
        assert_eq!(mm_to_dots(m, 180), js_round(f64::from(band_dots) / 2.0));
    }
    assert!(is_quick_m(
        &TextSize::Mm {
            mm: quick_m_mm(36.0).unwrap()
        },
        454,
        360
    ));
    assert_eq!(
        texts(&build("drawer-12"))[0].size,
        TextSize::Mm { mm: 4.94 }
    );
    assert_eq!(texts(&build("bin-24"))[0].size, TextSize::Mm { mm: 9.03 });
}

#[test]
fn folder_spine_and_name_tag() {
    let spine = build("folder-spine");
    assert_eq!(spine.tape.width_mm, 24.0);
    assert_eq!(
        spine.length,
        LengthSettings::Fixed {
            mm: 100.0,
            shrink: true
        }
    );
    let tag = build("name-tag");
    assert_eq!(tag.tape.width_mm, 24.0);
    assert_eq!(texts(&tag)[0].text.split('\n').count(), 2);
    assert!(tag.frame.is_some());
}

#[test]
fn variants_for_36_mm() {
    let ids: Vec<&str> = templates()
        .iter()
        .filter(|t| t.tape_width_mm == 36.0)
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["wifi-36", "bin-36", "folder-spine-36", "name-tag-36"]);
    let wifi = build("wifi-36");
    assert_eq!(
        kinds(&wifi),
        [ItemKind::Icon, ItemKind::Text, ItemKind::Code]
    );
    let c = codes(&wifi)[0];
    assert_eq!(
        (c.content, c.quiet_zone, c.ecc, c.module_dots),
        (
            CodeContent::Wifi,
            QuietZone::Standard,
            QrEcc::M,
            ModuleSize::Auto
        )
    );
    assert_eq!(
        texts(&build("bin-36"))[0].size,
        TextSize::Mm {
            mm: quick_m_mm(36.0).unwrap()
        }
    );
    assert_eq!(
        build("folder-spine-36").length,
        LengthSettings::Fixed {
            mm: 150.0,
            shrink: true
        }
    );
    assert_eq!(texts(&build("name-tag-36"))[0].text.split('\n').count(), 2);
    assert!(build("name-tag-36").frame.is_some());
    assert_eq!(build("wifi-36").name, build("wifi-24").name);
    assert_eq!(build("bin-36").name, build("bin-24").name);
}

#[test]
fn build_without_a_band_equals_the_p710_band() {
    let strip = |mut d: LabelDoc| {
        d.id.clear();
        d.created_at.clear();
        d.updated_at.clear();
        for i in &mut d.items {
            i.set_id(String::new());
        }
        serde_json::to_string(&d).unwrap()
    };
    let p710 = |w: f64| match w {
        9.0 => 50,
        12.0 => 70,
        _ => 128,
    };
    for t in templates().iter().filter(|t| t.tape_width_mm != 36.0) {
        assert_eq!(
            strip(t.build(band(p710(t.tape_width_mm), 180))),
            strip(t.build(None)),
            "{}",
            t.id
        );
    }
    let w36 = template_by_id("wifi-36").unwrap().build(band(454, 360));
    assert_eq!(texts(&w36)[0].size, TextSize::Mm { mm: 16.02 });
}

#[test]
fn build_band_gives_m_of_that_band() {
    for id in ["wifi-24", "bin-24"] {
        let doc = template_by_id(id).unwrap().build(band(320, 360));
        let t = texts(&doc)[0];
        assert!(is_quick_m(&t.size, 320, 360), "{id}");
        assert_eq!(t.size, TextSize::Mm { mm: 11.29 });
    }
    for id in ["wifi-36", "bin-36"] {
        let doc = template_by_id(id).unwrap().build(band(454, 360));
        assert!(is_quick_m(&texts(&doc)[0].size, 454, 360), "{id}");
    }
    for id in ["drawer-12", "cable-flag"] {
        let doc = template_by_id(id).unwrap().build(band(64, 180));
        for t in texts(&doc) {
            assert!(is_quick_m(&t.size, 64, 180), "{id}");
        }
    }
}

#[test]
fn fixed_sizes_fit_every_band_of_the_width() {
    // (band dots, dpi) of the models' print areas per width (PT-P300BT, PT-P710BT, PT-P910BT).
    let bands: &[(f64, u32, u16)] = &[
        (9.0, 50, 180),
        (12.0, 64, 180),
        (12.0, 70, 180),
        (24.0, 128, 180),
        (24.0, 320, 360),
        (36.0, 454, 360),
    ];
    for &(w, dots, dpi) in bands {
        for t in templates().iter().filter(|t| t.tape_width_mm == w) {
            let doc = t.build(band(dots, dpi));
            for i in &doc.items {
                let mm = match i {
                    Item::Text(x) => match x.size {
                        TextSize::Mm { mm } => Some(mm),
                        _ => None,
                    },
                    Item::Icon(x) => match x.size {
                        ItemSize::Mm { mm } => Some(mm),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(mm) = mm {
                    assert!(
                        mm_to_dots(mm, dpi) <= i64::from(dots),
                        "{} on {dots}@{dpi}",
                        t.id
                    );
                }
            }
        }
    }
}

#[test]
fn template_fits_the_models_widths() {
    let missing = |widths: &[f64]| -> Vec<&str> {
        templates()
            .iter()
            .filter(|t| !template_fits(t, widths))
            .map(|t| t.id)
            .collect()
    };
    assert_eq!(
        missing(&P710),
        ["wifi-36", "bin-36", "folder-spine-36", "name-tag-36"]
    );
    assert!(missing(&P910).is_empty());
    let wide: Vec<&str> = templates()
        .iter()
        .filter(|t| t.tape_width_mm > 12.0)
        .map(|t| t.id)
        .collect();
    assert_eq!(missing(&P300), wide);
}

#[test]
fn template_model_active_or_stand_in() {
    let models = [
        ModelWidths {
            name: "PT-P710BT",
            aliases: &[],
            tape_widths_mm: &P710,
        },
        ModelWidths {
            name: "PT-P300BT",
            aliases: &["Cube"],
            tape_widths_mm: &P300,
        },
        ModelWidths {
            name: "PT-P910BT",
            aliases: &[],
            tape_widths_mm: &P910,
        },
    ];
    let t = |id| template_by_id(id).unwrap();
    assert_eq!(
        template_model(t("wifi-24"), "PT-P710BT", &models).as_deref(),
        Some("PT-P710BT")
    );
    assert_eq!(
        template_model(t("wifi-36"), "PT-P710BT", &models).as_deref(),
        Some("PT-P910BT")
    );
    assert_eq!(
        template_model(t("wifi-24"), "PT-P300BT", &models).as_deref(),
        Some("PT-P710BT")
    );
    assert_eq!(
        template_model(t("wifi-12"), "pt-p300bt", &models).as_deref(),
        Some("PT-P300BT")
    );
    assert_eq!(
        template_model(t("wifi-12"), "cube", &models).as_deref(),
        Some("PT-P300BT")
    );
    assert_eq!(
        template_model(t("wifi-36"), "PT-P710BT", &[]).as_deref(),
        Some("PT-P710BT")
    );
    assert_eq!(
        template_model(t("wifi-36"), "PT-P710BT", &models[..2]),
        None
    );
}

#[test]
fn needs_width_text_marks_a_width_the_model_lacks() {
    assert_eq!(
        needs_width_text(36.0, "PT-P710BT", &P710).as_deref(),
        Some("Needs 36 mm tape · PT-P710BT takes up to 24 mm")
    );
    assert_eq!(
        needs_width_text(24.0, "PT-P300BT", &P300).as_deref(),
        Some("Needs 24 mm tape · PT-P300BT takes up to 12 mm")
    );
    assert_eq!(
        needs_width_text(9.0, "PT-X", &[3.5, 12.0, 24.0]).as_deref(),
        Some("Needs 9 mm tape · PT-X doesn’t take it")
    );
    assert_eq!(
        needs_width_text(3.5, "PT-X", &[6.0, 12.0]).as_deref(),
        Some("Needs 3.5 mm tape · PT-X doesn’t take it")
    );
    assert_eq!(needs_width_text(24.0, "PT-P710BT", &P710), None);
    assert_eq!(needs_width_text(36.0, "PT-P710BT", &[]), None);
}

#[test]
fn fits_loaded_tape_compares_the_loaded_width() {
    let t = template_by_id("wifi-12").unwrap();
    assert!(fits_loaded_tape(t, Some(12.0)));
    assert!(!fits_loaded_tape(t, Some(24.0)));
    assert!(!fits_loaded_tape(t, None));
}

#[test]
fn primary_item_id_picks_wifi_then_text_then_first() {
    let wifi = build("wifi-24");
    assert_eq!(primary_item_id(&wifi), Some(wifi.items[2].id()));
    let mut bin = build("bin-24");
    assert_eq!(primary_item_id(&bin), Some(bin.items[1].id()));
    bin.items.truncate(1);
    assert_eq!(primary_item_id(&bin), Some(bin.items[0].id()));
    bin.items.clear();
    assert_eq!(primary_item_id(&bin), None);
}

#[test]
fn with_sample_data_fills_wifi_codes_only() {
    let doc = build("wifi-12");
    let sample = with_sample_data(&doc);
    let w = codes(&sample)[0].wifi.clone().unwrap();
    assert_eq!((w.ssid.as_str(), w.password.as_str()), SAMPLE_WIFI);
    assert_eq!(codes(&doc)[0].wifi.as_ref().unwrap().ssid, "");
    assert_eq!(sample.items[1], doc.items[1]);
    let bin = build("bin-24");
    assert_eq!(with_sample_data(&bin).items, bin.items);
}

#[test]
fn gallery_length_hook_renders_the_sampled_resolved_doc() {
    let ctx = VariableContext::new(date!(2026 - 10 - 08));
    let t = template_by_id("wifi-24").unwrap();
    let len = t
        .gallery_length_mm(band(128, 180), &ctx, |doc| {
            // The renderer sees the sample network, with {{ssid}} resolved.
            assert_eq!(texts(doc)[0].text, "Wi-Fi\nHome Wi-Fi");
            Ok::<_, ()>((60.0, 2.0))
        })
        .unwrap();
    assert_eq!(len, 64.0);
    assert_eq!(t.length_chip(len), "64 mm label with a sample network");
    assert_eq!(
        template_by_id("bin-24").unwrap().length_chip(52.5),
        "52.5 mm label"
    );
    assert_eq!(
        template_by_id("folder-spine").unwrap().guess_length_mm(),
        100.0
    );
    assert_eq!(template_by_id("bin-24").unwrap().guess_length_mm(), 40.0);
}
