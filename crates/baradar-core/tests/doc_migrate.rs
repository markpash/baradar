//! Port of web/tests/unit/persist/migrate.test.ts: schema migrations against the fixtures.

use baradar_core::doc::migrate::{CURRENT_SCHEMA, migrate, migration_step};
use baradar_core::doc::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

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

fn with(mut v: Value, patch: Value) -> Value {
    for (k, x) in patch.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[test]
fn has_a_migration_step_for_every_older_schema() {
    let raw = json!({"items": []});
    for n in 0..CURRENT_SCHEMA {
        assert!(
            migration_step(n, raw.as_object().unwrap()).is_some(),
            "step {n}"
        );
    }
    assert!(migration_step(CURRENT_SCHEMA, raw.as_object().unwrap()).is_none());
}

#[test]
fn accepts_a_current_document_unchanged() {
    let raw = fixture("schema-4.json");
    let r = migrate(&raw).unwrap();
    assert!(!r.read_only);
    assert_eq!(r.migrated_from, None);
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    assert_eq!(to_json(&r.doc), norm(raw));
    assert_eq!(
        to_json(&r.doc.tape),
        json!({"widthMm": 36.0, "mediaId": "tze560-36", "model": "PT-P910BT", "colors": {"tape": "#ffd400", "ink": "#000000"}})
    );
}

#[test]
fn schema_3_to_4_changes_nothing_but_the_version() {
    let raw = fixture("schema-3.json");
    let r = migrate(&raw).unwrap();
    assert!(!r.read_only);
    assert_eq!(r.migrated_from, Some(3));
    assert_eq!(to_json(&r.doc), norm(with(raw, json!({"schema": 4}))));
    assert_eq!(r.doc.tape.model, None);
    let Item::Text(t) = &r.doc.items[1] else {
        panic!()
    };
    assert_eq!(
        (t.font_family.as_str(), t.size),
        ("oswald", TextSize::Pt { pt: 10.5 })
    );
    assert_eq!(
        r.doc.length,
        LengthSettings::Fixed {
            mm: 36.0,
            shrink: true
        }
    );
}

#[test]
fn schema_2_to_3_marks_fixed_size_text_clip_tall() {
    let text = |size: Value| {
        with(
            serde_json::to_value(Item::new(ItemKind::Text)).unwrap(),
            json!({"size": size}),
        )
    };
    let fit = text(json!({"mode": "fit"}));
    let mm = text(json!({"mode": "mm", "mm": 6}));
    let legacy = text(json!(6));
    let icon = with(
        serde_json::to_value(Item::new(ItemKind::Icon)).unwrap(),
        json!({"size": {"mode": "mm", "mm": 6}}),
    );
    let raw = with(
        serde_json::to_value(LabelDoc::with_items(vec![])).unwrap(),
        json!({"schema": 2, "items": [fit, mm, legacy, icon]}),
    );
    let r = migrate(&raw).unwrap();
    assert_eq!(to_json(&r.doc.items[0]), norm(fit.clone()));
    assert_eq!(
        to_json(&r.doc.items[1]),
        norm(with(mm.clone(), json!({"clipTall": true})))
    );
    let Item::Text(l) = &r.doc.items[2] else {
        panic!()
    };
    assert_eq!((l.size, l.clip_tall), (TextSize::Mm { mm: 6.0 }, true));
    assert_eq!(to_json(&r.doc.items[3]), norm(icon));
    // A schema-4 document keeps the flag through validation.
    let again = migrate(&serde_json::to_value(&r.doc).unwrap()).unwrap();
    assert_eq!(
        to_json(&again.doc.items[1]),
        norm(with(mm.clone(), json!({"clipTall": true})))
    );
    // Only `true` is read; anything else is dropped.
    let odd = with(
        serde_json::to_value(&r.doc).unwrap(),
        json!({"items": [with(mm.clone(), json!({"clipTall": "yes"}))]}),
    );
    assert_eq!(to_json(&migrate(&odd).unwrap().doc.items[0]), norm(mm));
}

#[test]
fn upgrades_schema_2_by_changing_nothing_but_the_version() {
    let raw = fixture("schema-2.json");
    let r = migrate(&raw).unwrap();
    assert_eq!(r.migrated_from, Some(2));
    assert!(!r.read_only);
    assert_eq!(to_json(&r.doc), norm(with(raw, json!({"schema": 4}))));
    assert_eq!(
        r.doc.length,
        LengthSettings::Fixed {
            mm: 40.0,
            shrink: false
        }
    );
    // The schema-2 fields survive: Wi-Fi, a custom font, batch data, counter padding.
    let Item::Code(w) = &r.doc.items[1] else {
        panic!()
    };
    assert_eq!(w.wifi.as_ref().unwrap().password, "correct horse");
    let b = r.doc.batch.as_ref().unwrap();
    assert_eq!((b.counters[0].pad, b.date_format), (3, DateFormat::Dmy));
}

#[test]
fn newer_schemas_open_read_only() {
    let raw = with(
        fixture("schema-3.json"),
        json!({"schema": CURRENT_SCHEMA + 1}),
    );
    assert!(migrate(&raw).unwrap().read_only);
    let raw = with(
        fixture("schema-4.json"),
        json!({"schema": CURRENT_SCHEMA + 1}),
    );
    let r = migrate(&raw).unwrap();
    assert!(r.read_only);
    assert_eq!(r.migrated_from, Some(CURRENT_SCHEMA + 1));
    assert_eq!(r.doc.tape.width_mm, 36.0);
    assert_eq!(r.doc.schema, CURRENT_SCHEMA);
}

#[test]
fn keeps_a_well_formed_model_name_and_drops_anything_else() {
    let base = fixture("schema-4.json");
    let tape = |model: &str| {
        with(
            base.clone(),
            json!({"tape": with(base["tape"].clone(), json!({"model": model}))}),
        )
    };
    let odd = migrate(&tape("<script>")).unwrap();
    assert_eq!(odd.doc.tape.model, None);
    assert!(
        odd.notices
            .contains(&"tape: the printer model is not a model name; removed".to_string())
    );
    let future = migrate(&tape("PT-Z999BT")).unwrap();
    assert_eq!(future.doc.tape.model.as_deref(), Some("PT-Z999BT"));
}

#[test]
fn upgrades_schema_1_so_it_prints_exactly_as_before() {
    let raw = fixture("schema-1.json");
    let r = migrate(&raw).unwrap();
    assert_eq!(r.migrated_from, Some(1));
    assert!(!r.read_only);
    assert_eq!(r.doc.schema, CURRENT_SCHEMA);
    assert_eq!(to_json(&r.doc.items[0]), norm(raw["items"][0].clone()));
    assert_eq!(
        to_json(&r.doc.items[1]),
        norm(with(
            raw["items"][1].clone(),
            json!({"content": "text", "quietZone": "standard", "moduleDots": 2})
        ))
    );
    assert_eq!(r.doc.batch, None);
    let doc = to_json(&r.doc);
    for (k, v) in raw.as_object().unwrap() {
        if k != "schema" && k != "items" {
            assert_eq!(doc[k], norm(v.clone()), "{k}");
        }
    }
}

#[test]
fn maps_a_schema_1_quiet_zone_of_false_to_none_and_keeps_every_module_size() {
    let mut raw = fixture("schema-1.json");
    let code = raw["items"][1].clone();
    raw["items"] = Value::Array(
        [5, 1, 12]
            .iter()
            .enumerate()
            .map(|(i, m)| {
                with(
                    code.clone(),
                    json!({"id": format!("c{i}"), "moduleDots": m, "quietZone": i != 1}),
                )
            })
            .collect(),
    );
    let r = migrate(&raw).unwrap();
    let got: Vec<(ModuleSize, QuietZone)> = r
        .doc
        .items
        .iter()
        .map(|i| match i {
            Item::Code(c) => (c.module_dots, c.quiet_zone),
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (ModuleSize::Dots(5), QuietZone::Standard),
            (ModuleSize::Dots(1), QuietZone::None),
            (ModuleSize::Dots(12), QuietZone::Standard)
        ]
    );
}

#[test]
fn does_not_mutate_its_input() {
    let raw = fixture("schema-0.json");
    let before = raw.clone();
    let _ = migrate(&raw);
    assert_eq!(raw, before);
}

#[test]
fn upgrades_unversioned_json_by_filling_defaults() {
    let r = migrate(&fixture("schema-0.json")).unwrap();
    assert_eq!(r.migrated_from, Some(0));
    assert!(!r.read_only);
    assert_eq!(r.doc.schema, CURRENT_SCHEMA);
    assert_eq!(r.doc.name, "Prototype label");
    assert_eq!(r.doc.tape.width_mm, 24.0);
    assert_eq!(r.doc.length, LengthSettings::Auto);
    assert_eq!(r.doc.print.copies, 1);
    assert_eq!(r.doc.items.len(), 1);
    assert_eq!(r.doc.items[0].id(), "7a2b3c4d-0000-4000-8000-000000000001");
    assert!(!r.doc.id.is_empty());
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    // An unnamed schema-0 doc is "Imported label".
    assert_eq!(
        migrate(&json!({"items": [], "name": ""})).unwrap().doc.name,
        "Imported label"
    );
}

#[test]
fn opens_documents_from_a_newer_schema_read_only() {
    let r = migrate(&fixture("schema-99.json")).unwrap();
    assert!(r.read_only);
    assert_eq!(r.migrated_from, Some(99));
    assert_eq!(r.doc.name, "From the future");
    assert_eq!(r.doc.tape.width_mm, 18.0);
    // Unknown fields are dropped.
    assert!(
        serde_json::to_value(&r.doc)
            .unwrap()
            .get("someFutureField")
            .is_none()
    );
}

#[test]
fn round_trips_a_fresh_document_through_json() {
    let doc = LabelDoc::new();
    let r = migrate(&serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap()).unwrap();
    assert_eq!(r.doc, doc);
}

#[test]
fn rejects_non_documents() {
    for raw in [
        json!(null),
        json!([1, 2]),
        json!("hello"),
        json!({"name": "x"}),
        json!({"schema": -1, "items": []}),
        json!({"schema": 1.5, "items": []}),
    ] {
        let p = migrate(&raw).expect_err(&raw.to_string());
        assert!(!p.is_empty());
    }
    let p = migrate(&json!({"schema": 99, "items": {}})).unwrap_err();
    assert!(
        p[0].starts_with("made by a newer version of ptouch studio (schema 99)"),
        "{p:?}"
    );
}

#[test]
fn schema_0_never_copies_prototype_keys() {
    let raw: Value = serde_json::from_str(
        r#"{"items":[],"__proto__":{"frame":{"thicknessMm":3},"polluted":1}}"#,
    )
    .unwrap();
    let out = migration_step(0, raw.as_object().unwrap()).unwrap();
    assert!(!out.contains_key("__proto__") && !out.contains_key("polluted"));
    let r = migrate(&raw).unwrap();
    assert_eq!(r.doc.frame, None);
}

#[test]
fn caps_the_error_text_built_from_a_hostile_schema_value() {
    let p = migrate(&json!({"schema": "z".repeat(20_000), "items": []})).unwrap_err();
    assert!(p.join(" ").len() < 200, "{p:?}");
}
