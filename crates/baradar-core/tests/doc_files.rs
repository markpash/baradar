//! Port of web/tests/unit/persist/files.test.ts: `.ptlabel.json` export/import, id conflicts,
//! bad input, and the Wi-Fi password / custom font rules.

use std::collections::HashMap;

use baradar_core::doc::files::*;
use baradar_core::doc::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A tiny but real PNG (1×1, opaque black).
fn png_1px() -> Vec<u8> {
    base64_to_bytes("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==")
        .unwrap()
}

/// A label with text, QR and an image item pointing at `blob_ref`.
fn sample_doc(blob_ref: &str) -> LabelDoc {
    let mut doc = LabelDoc::new();
    doc.name = "Shelf 3 — screws M3".into();
    doc.items.push(Item::new(ItemKind::Code));
    let mut img = Item::new(ItemKind::Image);
    if let Item::Image(i) = &mut img {
        i.blob_ref = blob_ref.into();
    }
    doc.items.push(img);
    doc
}

type Store = HashMap<String, (Vec<u8>, String)>;

fn store_with(bytes: Vec<u8>, mime: &str) -> (Store, String) {
    let r = content_ref(&bytes);
    (HashMap::from([(r.clone(), (bytes, mime.to_string()))]), r)
}

fn wifi_code(ssid: &str, password: &str) -> Item {
    let Item::Code(mut c) = Item::new(ItemKind::Code) else {
        unreachable!()
    };
    c.content = CodeContent::Wifi;
    c.wifi = Some(WifiSettings {
        ssid: ssid.into(),
        password: password.into(),
        ..WifiSettings::default()
    });
    Item::Code(c)
}

fn image_of(items: &[Item]) -> Option<&ImageItem> {
    items.iter().find_map(|i| match i {
        Item::Image(img) => Some(img),
        _ => None,
    })
}

#[test]
fn names_files_after_the_label() {
    assert_eq!(label_file_name("Cable tags"), "cable-tags.ptlabel.json");
    assert_eq!(
        label_file_name("Größe / M3 × 12"),
        "grosse-m3-12.ptlabel.json"
    );
    assert_eq!(label_file_name("   "), "label.ptlabel.json");
    assert_eq!(
        label_file_name("Ünïcödé — ½ ﬁle"),
        "unicode-1-2-file.ptlabel.json"
    );
    assert_eq!(
        label_file_name(&"a".repeat(80)).len(),
        60 + FILE_EXTENSION.len()
    );
}

#[test]
fn exports_a_self_contained_file_and_imports_it_elsewhere() {
    let (store, r) = store_with(png_1px(), "image/png");
    let doc = sample_doc(&r);
    let (text, notices) = serialize_label_file(&doc, |r| store.get(r).cloned(), false);
    assert!(notices.is_empty(), "{notices:?}");
    assert!(text.ends_with("}\n"));
    assert!(text.starts_with(
        "{\n  \"format\": \"ptouch-label\",\n  \"version\": 1,\n  \"exportedAt\": \""
    ));
    assert!(text.contains("\n  \"generator\": \"baradar\",\n"));
    // JavaScript number formatting: `24`, not `24.0`.
    assert!(text.contains("\"widthMm\": 24\n"), "{text}");
    let env: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(env["format"], FILE_FORMAT);
    assert_eq!(env["version"], 1);
    let img = env["doc"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "image")
        .unwrap();
    assert!(
        img["dataUrl"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert_eq!(img["blobRef"], r.as_str());

    let parsed = parse_label_file(&text).unwrap();
    assert!(parsed.notices.is_empty(), "{:?}", parsed.notices);
    assert!(!parsed.read_only);
    assert_eq!(parsed.doc.id, doc.id);
    assert_eq!(parsed.doc.name, doc.name);
    assert_eq!(
        parsed.doc.items.iter().map(Item::kind).collect::<Vec<_>>(),
        doc.items.iter().map(Item::kind).collect::<Vec<_>>()
    );
    assert_eq!(parsed.images.len(), 1);
    assert_eq!(parsed.images[0].bytes, png_1px());
    assert_eq!(parsed.images[0].mime, "image/png");
    assert_eq!(parsed.images[0].content_ref, r);

    let mut imported = parsed.doc.clone();
    internalize_images(&mut imported, &parsed.images);
    let img = image_of(&imported.items).unwrap();
    assert_eq!(img.data_url, None);
    assert_eq!(img.blob_ref, r);
    assert_eq!(imported, doc);
}

#[test]
fn imports_a_copy_when_the_id_already_exists() {
    let mut doc = LabelDoc::new();
    doc.name = "Mine".into();
    let (text, _) = serialize_label_file(&doc, |_| None, false);
    let mut r = parse_label_file(&text).unwrap();
    // The caller found the id in its library.
    let notice = import_as_copy(&mut r.doc);
    assert_ne!(r.doc.id, doc.id);
    assert_eq!(r.doc.created_at, r.doc.updated_at);
    assert!(notice.contains("copy"));
}

#[test]
fn accepts_a_bare_document_and_older_formats() {
    let r = parse_label_file(&fixture("schema-1.json").to_string()).unwrap();
    assert_eq!(r.doc.name, "Cable tags");
    assert_eq!(r.migrated_from, Some(1));
    let old = parse_label_file(&fixture("schema-0.json").to_string()).unwrap();
    assert!(old.notices.join(" ").contains("upgraded"));
}

#[test]
fn opens_files_from_a_newer_schema_read_only() {
    let text =
        json!({"format": FILE_FORMAT, "version": 1, "doc": fixture("schema-99.json")}).to_string();
    let r = parse_label_file(&text).unwrap();
    assert!(r.read_only);
    assert_eq!(r.notices, vec![NOTICE_READ_ONLY]);
}

#[test]
fn notes_images_missing_from_the_library_on_export() {
    let (text, notices) = serialize_label_file(&sample_doc("sha256-gone"), |_| None, false);
    assert!(notices.join(" ").contains("could not be found"));
    let env: Value = serde_json::from_str(&text).unwrap();
    assert!(env["doc"]["items"][2].get("dataUrl").is_none());
}

#[test]
fn rejects_bad_input() {
    for (text, msg) in [
        ("{nope".to_string(), "not valid JSON"),
        (
            r#"{"hello":"world"}"#.to_string(),
            "not a valid ptouch label",
        ),
        (
            json!({"format": FILE_FORMAT, "version": 9, "doc": {}}).to_string(),
            "newer version",
        ),
    ] {
        let e = parse_label_file(&text).unwrap_err();
        assert!(e.join(" ").contains(msg), "{e:?}");
    }
    // A damaged inlined image fails the import, as internalizeImages does.
    let mut doc = sample_doc("");
    if let Item::Image(i) = &mut doc.items[2] {
        i.data_url = Some("data:image/png;base64,***".into());
    }
    let e = parse_label_file(&serde_json::to_string(&doc).unwrap()).unwrap_err();
    assert!(e[0].contains("could not be read"), "{e:?}");
}

#[test]
fn leaves_wifi_passwords_out_by_default_with_a_notice() {
    let mut doc = LabelDoc::with_items(vec![wifi_code("Guest", "hunter2-secret")]);
    doc.name = "Guest Wi-Fi".into();
    let (text, notices) = serialize_label_file(&doc, |_| None, false);
    assert!(!text.contains("hunter2-secret"));
    assert!(text.contains("\"ssid\": \"Guest\""));
    assert_eq!(notices, vec!["The Wi-Fi password was left out."]);
    // Opted in: kept, no notice.
    let (text, notices) = serialize_label_file(&doc, |_| None, true);
    assert!(text.contains("hunter2-secret"));
    assert!(notices.is_empty());
}

#[test]
fn leaves_out_passwords_from_a_batch_column() {
    let mut doc = LabelDoc::with_items(vec![wifi_code("Guest", "{{pw}}")]);
    doc.batch = Some(BatchData {
        enabled: true,
        columns: vec!["room".into(), "pw".into()],
        rows: vec![vec!["101".into(), "S3cretPassw0rd!".into()]],
        ..BatchData::default()
    });
    let (text, notices) = serialize_label_file(&doc, |_| None, false);
    assert!(!text.contains("S3cretPassw0rd!"));
    let env: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(env["doc"]["batch"]["rows"], json!([["101", ""]]));
    assert!(notices.join(" ").contains("{{pw}} column"));
}

#[test]
fn never_embeds_font_files() {
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        unreachable!()
    };
    let font = FontSource::User {
        reference: format!("sha256-{}", "a".repeat(32)),
        family: "Inter".into(),
    };
    t.custom_font = Some(font);
    let doc = LabelDoc::with_items(vec![Item::Text(t)]);
    let (text, notices) = serialize_label_file(&doc, |_| None, false);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("Custom fonts are not included"));
    let env: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        env["doc"]["items"][0]["customFont"],
        json!({"kind": "user", "ref": format!("sha256-{}", "a".repeat(32)), "family": "Inter"})
    );
}
