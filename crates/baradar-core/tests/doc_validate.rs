//! Port of the validateDoc / schema parts of web/tests/unit/doc/schema.test.ts.

use baradar_core::doc::validate::{LIMITS, validate_doc};
use baradar_core::doc::*;
use serde_json::{Value, json};

/// Numbers as f64, so `24` and `24.0` compare equal (JavaScript has one number type).
fn norm(v: Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.into_iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, norm(v))).collect()),
        other => other,
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    norm(serde_json::to_value(v).unwrap())
}

/// `createDoc({items: []})` as JSON, with `patch` merged over the top level.
fn base_with(patch: Value) -> Value {
    let mut d = serde_json::to_value(LabelDoc::with_items(vec![])).unwrap();
    for (k, v) in patch.as_object().unwrap() {
        d[k] = v.clone();
    }
    d
}

fn ok(raw: &Value) -> validate::Validated {
    validate_doc(raw).unwrap_or_else(|p| panic!("rejected: {p:?}"))
}

fn item_json(kind: ItemKind, patch: Value) -> Value {
    let mut v = serde_json::to_value(Item::new(kind)).unwrap();
    for (k, x) in patch.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[test]
fn every_factory_item_survives_validation_unchanged() {
    let d = LabelDoc::with_items(ItemKind::ALL.iter().map(|k| Item::new(*k)).collect());
    let v = ok(&serde_json::to_value(&d).unwrap());
    assert_eq!(v.doc, d);
    assert!(v.notices.is_empty(), "{:?}", v.notices);
}

#[test]
fn rejects_things_that_are_not_current_schema_labels() {
    for raw in [
        json!(null),
        json!(42),
        json!("x"),
        json!([]),
        json!({}),
        json!({"schema": 1, "items": []}),
        json!({"schema": 2, "items": []}),
        json!({"schema": 3, "items": []}),
        json!({"schema": 5, "items": []}),
        json!({"schema": 4}),
        json!({"schema": 4, "items": {}}),
    ] {
        let p = validate_doc(&raw).expect_err(&raw.to_string());
        assert!(p[0].contains("not a ptouch label document"), "{p:?}");
    }
}

#[test]
fn accepts_36mm_tape_and_maps_other_widths_to_the_nearest() {
    let wide = ok(&base_with(
        json!({"tape": {"widthMm": 36, "model": "PT-P910BT"}}),
    ));
    assert_eq!(
        to_json(&wide.doc.tape),
        json!({"widthMm": 36.0, "model": "PT-P910BT"})
    );
    let over = ok(&base_with(json!({"tape": {"widthMm": 40}})));
    assert_eq!(over.doc.tape.width_mm, 36.0);
    assert!(
        over.notices
            .join("\n")
            .contains("width 40 mm is not a TZe width; using 36 mm")
    );
}

#[test]
fn fills_a_minimal_document_with_defaults() {
    let v = ok(&json!({"schema": 4, "items": []}));
    let d = to_json(&v.doc);
    assert_eq!(d["tape"], json!({"widthMm": 24.0}));
    assert_eq!(d["length"], json!({"mode": "auto"}));
    assert_eq!(d["marginsMm"], json!({"start": 2.0, "end": 2.0}));
    assert_eq!(
        d["layout"],
        json!({"mode": "flow", "gapMm": 3.0, "align": "center"})
    );
    assert_eq!(v.doc.print, PrintSettings::default());
    assert_eq!(v.doc.id.len(), 36);
    assert!(v.doc.id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    assert_eq!(v.doc.name, "Untitled label");
}

#[test]
fn clamps_numbers_and_repairs_enums_with_readable_notices() {
    let v = ok(&base_with(json!({
        "tape": {"widthMm": 13},
        "length": {"mode": "fixed", "mm": 5000},
        "print": {"copies": 500, "autoCut": "yes", "chain": false, "mirror": false, "threshold": -4},
        "items": [
            {"id": "c", "kind": "code", "symbology": "pdf417", "data": 123, "moduleDots": 0, "quietZone": true, "ecc": "Z", "showText": false},
            {"id": "t", "kind": "text", "text": "Hi", "fontFamily": "Comic Sans", "fontWeight": 650, "italic": false, "size": {"mode": "mm", "mm": 999}, "align": "middle", "lineHeight": "x", "invert": false},
        ],
    })));
    assert_eq!(v.doc.tape.width_mm, 12.0);
    assert_eq!(
        v.doc.length,
        LengthSettings::Fixed {
            mm: 1000.0,
            shrink: false
        }
    );
    assert_eq!(
        v.doc.print,
        PrintSettings {
            copies: 99,
            auto_cut: true,
            chain: false,
            mirror: false,
            threshold: 0
        }
    );
    let Item::Code(c) = &v.doc.items[0] else {
        panic!()
    };
    assert_eq!(
        (c.symbology, c.data.as_str(), c.module_dots, c.ecc),
        (Symbology::Qr, "123", ModuleSize::Dots(1), QrEcc::M)
    );
    let Item::Text(t) = &v.doc.items[1] else {
        panic!()
    };
    assert_eq!(t.font_family, "fira-sans");
    assert_eq!(t.font_weight, 600);
    assert_eq!(t.size, TextSize::Mm { mm: 100.0 });
    assert_eq!(t.align, Align::Center);
    assert_eq!(t.line_height, 1.1);
    let all = v.notices.join("\n");
    for want in [
        "tape: width 13 mm is not a TZe width; using 12 mm",
        "print: copies 500 out of range; clamped to 99",
        "print: autoCut was not true/false; reset to true",
        "print: threshold -4 out of range; clamped to 0",
        "length: mm 5000 out of range; clamped to 1000",
        "item 1 (code): symbology \"pdf417\" is not supported; using \"qr\"",
        "item 1 (code): moduleDots 0 out of range; clamped to 1",
        "item 1 (code): ecc \"Z\" is not supported; using \"M\"",
        "item 2 (text): fontFamily \"Comic Sans\" is not supported; using \"fira-sans\"",
        "item 2 (text): mm 999 out of range; clamped to 100",
        "item 2 (text): align \"middle\" is not supported; using \"center\"",
        "item 2 (text): lineHeight was not a number; reset to 1.1",
    ] {
        assert!(all.contains(want), "missing {want:?} in\n{all}");
    }
}

#[test]
fn drops_unknown_kinds_and_fixes_duplicate_ids() {
    let v = ok(&base_with(json!({"items": [
        {"id": "a", "kind": "spacer", "widthMm": 4},
        {"id": "a", "kind": "spacer", "widthMm": 4},
        {"id": "z", "kind": "hologram"},
        "junk",
    ]})));
    assert_eq!(v.doc.items.len(), 2);
    assert_eq!(v.doc.items[0].id(), "a");
    assert_ne!(v.doc.items[1].id(), "a");
    let all = v.notices.join("\n");
    assert!(all.contains("item 2 has a duplicate id; a new one was assigned"));
    assert!(all.contains("unknown kind \"hologram\"; dropped"));
    assert!(all.contains("item 4 is not an object; dropped"));
}

#[test]
fn keeps_inlined_image_data_urls_and_drops_anything_else() {
    let img = item_json(ItemKind::Image, json!({"blobRef": "sha-1"}));
    let mut a = img.clone();
    a["dataUrl"] = json!("data:image/png;base64,AAAA");
    let mut b = img;
    b["id"] = json!("x");
    b["dataUrl"] = json!("javascript:alert(1)");
    let v = ok(&base_with(json!({"items": [a, b]})));
    let Item::Image(a) = &v.doc.items[0] else {
        panic!()
    };
    assert_eq!(a.data_url.as_deref(), Some("data:image/png;base64,AAAA"));
    let Item::Image(b) = &v.doc.items[1] else {
        panic!()
    };
    assert_eq!(b.data_url, None);
    assert!(
        v.notices
            .join("\n")
            .contains("dataUrl is not an inlined image; removed")
    );
}

#[test]
fn strips_unknown_fields() {
    let mut raw = serde_json::to_value(LabelDoc::new()).unwrap();
    raw["extra"] = json!(true);
    let v = ok(&raw);
    assert!(serde_json::to_value(&v.doc).unwrap().get("extra").is_none());
}

#[test]
fn caps_the_number_of_blocks_and_the_total_text() {
    let many: Vec<Value> = (0..37_000)
        .map(|_| json!({"kind": "text", "text": "x"}))
        .collect();
    let r = ok(&base_with(json!({"items": many})));
    assert_eq!(r.doc.items.len(), 200);
    assert!(r.notices.join(" ").contains("first 200"));
    let long: Vec<Value> = (0..50)
        .map(|_| json!({"kind": "text", "text": "y".repeat(4000)}))
        .collect();
    let r2 = ok(&base_with(json!({"items": long})));
    assert_eq!(r2.doc.items.len(), 10);
    assert!(
        r2.notices
            .join(" ")
            .contains("more than 40000 characters of text; blocks from 11 on were dropped")
    );
}

#[test]
fn counts_characters_in_utf16_units() {
    // 2000 emoji = 4000 UTF-16 units: exactly at the limit, not truncated.
    let at_limit = "😀".repeat(2000);
    let over = format!("{at_limit}a");
    let v = ok(&base_with(json!({"items": [
        {"kind": "text", "text": at_limit},
        {"kind": "text", "text": over},
    ]})));
    let Item::Text(a) = &v.doc.items[0] else {
        panic!()
    };
    let Item::Text(b) = &v.doc.items[1] else {
        panic!()
    };
    assert_eq!(a.text, "😀".repeat(2000));
    assert_eq!(b.text, "😀".repeat(2000));
    assert_eq!(
        v.notices,
        vec!["item 2 (text): text truncated to 4000 characters"]
    );
}

#[test]
fn rejects_oversized_ids_media_ids_timestamps_and_non_colour_strings() {
    let r = ok(&base_with(json!({
        "id": "i".repeat(100_000),
        "createdAt": "c".repeat(50_000),
        "tape": {"widthMm": 24, "mediaId": "m".repeat(70_000), "colors": {"tape": "url(x);".repeat(1000), "ink": "expression()"}},
    })));
    assert!(r.doc.id.len() <= 64);
    assert!(r.doc.created_at.len() <= 40);
    assert_eq!(r.doc.tape.media_id, None);
    assert_eq!(r.doc.tape.colors, None);
    let good = ok(&base_with(
        json!({"tape": {"widthMm": 12, "mediaId": "tze231-12", "colors": {"tape": "#FFD400", "ink": "#000"}}}),
    ));
    assert_eq!(
        to_json(&good.doc.tape),
        json!({"widthMm": 12.0, "mediaId": "tze231-12", "colors": {"tape": "#FFD400", "ink": "#000"}})
    );
}

#[test]
fn keeps_point_sizes_every_library_font_and_shrink() {
    let items: Vec<Value> = FONT_FAMILY_IDS
        .iter()
        .enumerate()
        .map(|(i, f)| item_json(ItemKind::Text, json!({"id": format!("t{i}"), "fontFamily": f, "size": {"mode": "pt", "pt": 4.0 + i as f64 * 5.5}})))
        .collect();
    let v = ok(&base_with(
        json!({"length": {"mode": "fixed", "mm": 40, "shrink": true}, "items": items.clone()}),
    ));
    assert!(v.notices.is_empty(), "{:?}", v.notices);
    assert_eq!(to_json(&v.doc.items), norm(Value::Array(items)));
    assert_eq!(
        v.doc.length,
        LengthSettings::Fixed {
            mm: 40.0,
            shrink: true
        }
    );
}

#[test]
fn clamps_point_sizes_and_repairs_a_malformed_one() {
    let v = ok(&base_with(json!({"items": [
        item_json(ItemKind::Text, json!({"id": "a", "size": {"mode": "pt", "pt": 1}})),
        item_json(ItemKind::Text, json!({"id": "b", "size": {"mode": "pt", "pt": 1000}})),
        item_json(ItemKind::Text, json!({"id": "c", "size": {"mode": "pt", "pt": "huge"}})),
        item_json(ItemKind::Text, json!({"id": "d", "size": {"mode": "em", "em": 2}})),
    ]})));
    let sizes: Vec<TextSize> = v
        .doc
        .items
        .iter()
        .map(|i| match i {
            Item::Text(t) => t.size,
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        sizes,
        vec![
            TextSize::Pt {
                pt: LIMITS.size_pt.min
            },
            TextSize::Pt {
                pt: LIMITS.size_pt.max
            },
            TextSize::Pt { pt: 12.0 },
            TextSize::Fit
        ]
    );
    assert_eq!(v.notices.len(), 4, "{:?}", v.notices);
}

#[test]
fn accepts_a_point_size_only_on_text_blocks() {
    let v = ok(&base_with(
        json!({"items": [item_json(ItemKind::Icon, json!({"id": "i", "size": {"mode": "pt", "pt": 12}}))]}),
    ));
    let Item::Icon(i) = &v.doc.items[0] else {
        panic!()
    };
    assert_eq!(i.size, ItemSize::Fit);
    assert!(v.notices.join(",").contains("size was malformed"));
}

#[test]
fn shrink_is_only_kept_on_a_fixed_length_and_only_when_on() {
    let read = |length: Value| ok(&base_with(json!({"length": length}))).doc.length;
    assert_eq!(
        read(json!({"mode": "fixed", "mm": 30, "shrink": false})),
        LengthSettings::Fixed {
            mm: 30.0,
            shrink: false
        }
    );
    assert_eq!(
        read(json!({"mode": "fixed", "mm": 30})),
        LengthSettings::Fixed {
            mm: 30.0,
            shrink: false
        }
    );
    assert_eq!(
        read(json!({"mode": "auto", "shrink": true})),
        LengthSettings::Auto
    );
    assert_eq!(
        read(json!({"mode": "fixed", "mm": 30, "shrink": "yes"})),
        LengthSettings::Fixed {
            mm: 30.0,
            shrink: false
        }
    );
}

#[test]
fn keeps_wifi_datamatrix_quiet_zones_auto_modules_and_custom_fonts() {
    let items = json!([
        {"id": "w", "kind": "code", "symbology": "qr", "content": "wifi", "data": "", "wifi": {"ssid": "Home", "password": "pw;1", "security": "wep", "hidden": true}, "moduleDots": "auto", "quietZone": "compact", "ecc": "Q", "showText": false},
        {"id": "d", "kind": "code", "symbology": "datamatrix", "content": "text", "data": "A-001", "moduleDots": 4, "quietZone": "none", "ecc": "M", "showText": false},
        item_json(ItemKind::Text, json!({"id": "u", "customFont": {"kind": "user", "ref": "sha256-0123456789abcdef0123456789abcdef", "family": "Inter"}})),
        item_json(ItemKind::Text, json!({"id": "l", "customFont": {"kind": "local", "postscriptName": "Helvetica-Bold", "family": "Helvetica"}})),
    ]);
    let v = ok(&base_with(json!({"items": items.clone()})));
    assert!(v.notices.is_empty(), "{:?}", v.notices);
    assert_eq!(to_json(&v.doc.items), norm(items));
}

#[test]
fn accepts_a_boolean_quiet_zone_and_repairs_bad_v2_values() {
    let v = ok(&base_with(json!({"items": [
        {"id": "a", "kind": "code", "symbology": "qr", "data": "x", "moduleDots": "big", "quietZone": false, "ecc": "M", "showText": false},
        {"id": "b", "kind": "code", "symbology": "qr", "content": "telepathy", "data": "x", "moduleDots": 3, "quietZone": "wide", "ecc": "M", "showText": false, "wifi": {"ssid": "s".repeat(100), "security": "wpa9", "hidden": "no"}},
        item_json(ItemKind::Text, json!({"id": "c", "customFont": {"kind": "user", "ref": "../../etc/passwd", "family": "X"}})),
        item_json(ItemKind::Text, json!({"id": "d", "customFont": {"kind": "local", "postscriptName": "Evil\") ; url(x", "family": "X"}})),
    ]})));
    let Item::Code(a) = &v.doc.items[0] else {
        panic!()
    };
    assert_eq!(
        (a.content, a.module_dots, a.quiet_zone),
        (CodeContent::Text, ModuleSize::Dots(3), QuietZone::None)
    );
    let Item::Code(b) = &v.doc.items[1] else {
        panic!()
    };
    assert_eq!(
        (b.content, b.quiet_zone),
        (CodeContent::Text, QuietZone::Standard)
    );
    assert_eq!(
        b.wifi,
        Some(WifiSettings {
            ssid: "s".repeat(64),
            password: String::new(),
            security: WifiSecurity::Wpa,
            hidden: false
        })
    );
    let Item::Text(c) = &v.doc.items[2] else {
        panic!()
    };
    let Item::Text(d) = &v.doc.items[3] else {
        panic!()
    };
    assert_eq!((&c.custom_font, &d.custom_font), (&None, &None));
    let all = v.notices.join("\n");
    assert!(all.contains("customFont was malformed"));
    assert!(all.contains("item 1 (code): moduleDots was not a number; reset to 3"));
    assert!(all.contains("wifi: ssid truncated to 64 characters"));
}

#[test]
fn validates_batch_data_names_caps_counters() {
    let rows: Vec<Value> = (0..600)
        .map(|i| json!([format!("r{i}"), i, {"x": 1}]))
        .collect();
    let v = ok(&base_with(json!({"batch": {
        "enabled": true,
        "columns": ["room", "room", "1bad", "ok_2"],
        "rows": rows,
        "count": 9999,
        "counters": [{"name": "n", "start": 5, "step": 2, "pad": 3}, {"name": "room", "start": 1, "step": 1, "pad": 0}, {"name": "x y"}],
        "dateFormat": "klingon",
    }})));
    let b = v.doc.batch.unwrap();
    assert_eq!(b.columns, ["room", "col2", "col3", "ok_2"]);
    assert_eq!(b.rows.len(), LIMITS.batch_rows);
    assert_eq!(b.rows[1], ["r1", "1", "", ""]);
    assert_eq!(b.count, 500);
    assert_eq!(
        b.counters,
        vec![BatchCounter {
            name: "n".into(),
            start: 5,
            step: 2,
            pad: 3
        }]
    );
    assert_eq!(b.date_format, DateFormat::Iso);
    assert!(b.enabled);
    let all = v.notices.join("\n");
    assert!(all.contains("batch: column \"room\" is not a valid, unique variable name; renamed"));
    assert!(all.contains("batch: 600 rows; only the first 500 were kept"));
    assert!(all.contains("batch: counter {\"name\":\"room\",\"start\":1,\"step\":1,\"pad\"… is malformed or its name is taken; dropped"), "{all}");
    assert!(all.contains("batch: dateFormat \"klingon\" is not supported; using \"iso\""));
}

#[test]
fn a_doc_without_batch_data_has_no_batch_field() {
    let v = ok(&base_with(json!({})));
    assert_eq!(v.doc.batch, None);
    let w = ok(&base_with(
        json!({"batch": serde_json::to_value(BatchData::default()).unwrap()}),
    ));
    assert_eq!(w.doc.batch, Some(BatchData::default()));
    assert!(w.notices.is_empty());
}

#[test]
fn frames_and_numeric_strings() {
    let v = ok(&base_with(json!({
        "frame": {"thicknessMm": "1.5", "radiusMm": 99},
        "marginsMm": {"start": " 4 ", "end": "wide"},
        "items": [{"kind": "spacer", "widthMm": 3, "frame": {"xMm": 2000, "rotation": 45}}, {"kind": "spacer", "frame": 5}],
    })));
    assert_eq!(
        v.doc.frame,
        Some(LabelFrame {
            thickness_mm: 1.5,
            radius_mm: 50.0,
            inset_mm: 0.0
        })
    );
    assert_eq!(
        v.doc.margins_mm,
        Margins {
            start: 4.0,
            end: 2.0
        }
    );
    assert_eq!(
        v.doc.items[0].frame(),
        Some(&ItemFrame {
            x_mm: 1000.0,
            y_mm: 0.0,
            w_mm: 10.0,
            h_mm: 10.0,
            rotation: 0
        })
    );
    assert_eq!(v.doc.items[1].frame(), None);
    assert_eq!(
        v.notices,
        vec![
            "margins: end was not a number; reset to 2",
            "frame: radiusMm 99 out of range; clamped to 50",
            "item 1 (spacer): xMm 2000 out of range; clamped to 1000",
            "item 1 (spacer): rotation 45 is not supported; using 0",
            "item 2 (spacer): frame was malformed; removed",
        ]
    );
}
