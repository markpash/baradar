//! Port of web/tests/unit/doc/secrets.test.ts: Wi-Fi passwords never leave by default.

use baradar_core::doc::secrets::{doc_has_secrets, strip_secrets};
use baradar_core::doc::*;

fn wifi_code(ssid: &str, password: &str, content: CodeContent) -> Item {
    let Item::Code(mut c) = Item::new(ItemKind::Code) else {
        unreachable!()
    };
    c.content = content;
    c.wifi = Some(WifiSettings {
        ssid: ssid.into(),
        password: password.into(),
        ..WifiSettings::default()
    });
    Item::Code(c)
}

fn batch(columns: &[&str], rows: &[&[&str]]) -> BatchData {
    BatchData {
        enabled: true,
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect(),
        ..BatchData::default()
    }
}

#[test]
fn finds_wifi_passwords_also_on_codes_switched_back_to_text() {
    assert!(!doc_has_secrets(&LabelDoc::new()));
    assert!(!doc_has_secrets(&LabelDoc::with_items(vec![wifi_code(
        "Home",
        "",
        CodeContent::Wifi
    )])));
    assert!(doc_has_secrets(&LabelDoc::with_items(vec![wifi_code(
        "Home",
        "pw",
        CodeContent::Wifi
    )])));
    assert!(doc_has_secrets(&LabelDoc::with_items(vec![wifi_code(
        "Home",
        "pw",
        CodeContent::Text
    )])));
}

#[test]
fn blanks_every_password_without_touching_the_input() {
    let doc = LabelDoc::with_items(vec![
        Item::new(ItemKind::Text),
        wifi_code("Home", "one", CodeContent::Wifi),
        wifi_code("Home", "two", CodeContent::Text),
    ]);
    let before = doc.clone();
    let out = strip_secrets(&doc);
    assert_eq!(out.removed, 2);
    assert_eq!(doc, before);
    let json = serde_json::to_string(&out.doc).unwrap();
    assert!(!json.contains("\"one\"") && !json.contains("\"two\""));
    let Item::Code(c) = &out.doc.items[1] else {
        panic!()
    };
    let w = c.wifi.as_ref().unwrap();
    assert_eq!((w.ssid.as_str(), w.password.as_str()), ("Home", ""));
    assert!(!doc_has_secrets(&out.doc));
}

#[test]
fn returns_the_same_doc_when_there_is_nothing_to_strip() {
    let doc = LabelDoc::new();
    let out = strip_secrets(&doc);
    assert_eq!((out.doc, out.removed, out.columns), (doc, 0, vec![]));
}

#[test]
fn blanks_the_batch_column_a_placeholder_password_names() {
    let mut doc = LabelDoc::with_items(vec![wifi_code("{{net}}", "{{pw}}", CodeContent::Wifi)]);
    doc.batch = Some(batch(
        &["net", "pw"],
        &[
            &["Guest1", "S3cretPassw0rd!"],
            &["Guest2", "Another-Secret-9"],
            &["Guest3", ""],
        ],
    ));
    let before = doc.clone();
    let out = strip_secrets(&doc);
    assert_eq!(doc, before);
    assert_eq!(out.columns, vec!["pw"]);
    assert_eq!(out.removed, 3); // the field + two non-empty cells
    let b = out.doc.batch.as_ref().unwrap();
    assert_eq!(
        b.rows,
        vec![vec!["Guest1", ""], vec!["Guest2", ""], vec!["Guest3", ""]]
    );
    assert_eq!(b.counters, before.batch.unwrap().counters);
    let json = serde_json::to_string(&out.doc).unwrap();
    assert!(!json.contains("S3cret") && !json.contains("Another-Secret"));
}

#[test]
fn a_literal_password_that_only_contains_braces_names_no_column() {
    let mut doc = LabelDoc::with_items(vec![wifi_code("Home", "x{{pw}}z", CodeContent::Wifi)]);
    doc.batch = Some(batch(&["pw"], &[&["visible"]]));
    let out = strip_secrets(&doc);
    assert!(out.columns.is_empty());
    assert_eq!(out.doc.batch.unwrap().rows, vec![vec!["visible"]]);
}
