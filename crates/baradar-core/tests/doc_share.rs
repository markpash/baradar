//! Port of web/tests/unit/persist/share.test.ts: deflate-raw + base64url in `#d=`, round trip,
//! image rules, size caps, damaged links, and the Wi-Fi password / custom font rules.

use std::collections::HashMap;

use baradar_core::doc::files::{
    base64_to_bytes, base64url_to_bytes, bytes_to_base64url, content_ref, internalize_images,
};
use baradar_core::doc::share::*;
use baradar_core::doc::*;
use serde_json::json;

const BASE: &str = "https://fjanicki.github.io/ptouch/";

fn hash_of(url: &str) -> &str {
    &url[url.find('#').unwrap()..]
}

fn png_1px() -> Vec<u8> {
    base64_to_bytes("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==")
        .unwrap()
}

/// Deterministic, poorly compressible bytes (xorshift32, as the TS helper).
fn noise(n: usize, seed: u32) -> Vec<u8> {
    let mut x = if seed == 0 { 1 } else { seed };
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xff) as u8
        })
        .collect()
}

fn image(blob_ref: &str) -> Item {
    let Item::Image(mut i) = Item::new(ItemKind::Image) else {
        unreachable!()
    };
    i.blob_ref = blob_ref.into();
    Item::Image(i)
}

fn sample_doc(blob_ref: &str) -> LabelDoc {
    let mut doc = LabelDoc::new();
    doc.name = "Shelf 3 — screws M3".into();
    doc.items.push(Item::new(ItemKind::Code));
    doc.items.push(image(blob_ref));
    doc
}

type Store = HashMap<String, (Vec<u8>, String)>;

fn put(store: &mut Store, bytes: Vec<u8>) -> String {
    let r = content_ref(&bytes);
    store.insert(r.clone(), (bytes, "image/png".into()));
    r
}

fn no_blobs(_: &str) -> Option<(Vec<u8>, String)> {
    None
}

fn decoded(url: &str) -> String {
    let bytes = base64url_to_bytes(&hash_of(url)[SHARE_PREFIX.len()..]).unwrap();
    String::from_utf8(inflate_raw(&bytes, SHARE_MAX_JSON_BYTES).unwrap()).unwrap()
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

fn wifi_doc() -> LabelDoc {
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        unreachable!()
    };
    t.text = "{{ssid}}".into();
    let mut doc = LabelDoc::with_items(vec![wifi_code("Guest", "hunter2-secret"), Item::Text(t)]);
    doc.name = "Guest Wi-Fi".into();
    doc
}

#[test]
fn round_trips_a_document_through_the_url_fragment() {
    let mut doc = LabelDoc::new();
    doc.name = "Garage — fuse box ⚡ ÄÖÜ".into();
    let link = create_share_link(&doc, BASE, no_blobs, false).unwrap();
    assert!(link.notices.is_empty());
    assert!(link.url.starts_with(&format!("{BASE}{SHARE_PREFIX}")));
    let payload = &hash_of(&link.url)[3..];
    assert!(
        payload
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    );

    let back = read_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    // A shared label is a copy: it never overwrites the recipient's library entry.
    assert_ne!(back.id, doc.id);
    let same = LabelDoc {
        id: doc.id.clone(),
        created_at: doc.created_at.clone(),
        updated_at: doc.updated_at.clone(),
        ..back
    };
    assert_eq!(same, doc);
}

#[test]
fn compresses_a_typical_label() {
    let link = create_share_link(&LabelDoc::new(), BASE, no_blobs, false).unwrap();
    assert!(hash_of(&link.url).len() < 1000);
}

#[test]
fn replaces_an_existing_fragment_in_the_base_url() {
    let link = create_share_link(
        &LabelDoc::new(),
        &format!("{BASE}#diagnostics"),
        no_blobs,
        false,
    )
    .unwrap();
    assert_eq!(link.url.find('#'), Some(BASE.len()));
}

#[test]
fn ignores_fragments_that_are_not_share_links() {
    assert_eq!(read_share_fragment("").unwrap(), None);
    assert_eq!(read_share_fragment("#diagnostics").unwrap(), None);
    assert!(parse_share_fragment("#x=1").unwrap().is_none());
}

#[test]
fn accepts_the_fragment_without_the_leading_hash() {
    let mut doc = LabelDoc::new();
    doc.name = "no hash".into();
    let link = create_share_link(&doc, BASE, no_blobs, false).unwrap();
    assert_eq!(
        read_share_fragment(&hash_of(&link.url)[1..])
            .unwrap()
            .unwrap()
            .name,
        "no hash"
    );
    // Anything after `&` is ignored.
    assert_eq!(
        read_share_fragment(&format!("{}&utm=x", hash_of(&link.url)))
            .unwrap()
            .unwrap()
            .name,
        "no hash"
    );
}

#[test]
fn inlines_small_images_and_hands_them_back_for_storage() {
    let mut sender = Store::new();
    let r = put(&mut sender, png_1px());
    let link = create_share_link(&sample_doc(&r), BASE, |r| sender.get(r).cloned(), false).unwrap();
    assert!(link.notices.is_empty(), "{:?}", link.notices);

    let parsed = parse_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    assert_eq!(parsed.images.len(), 1);
    let img = &parsed.images[0];
    assert_eq!(img.content_ref, r); // content-addressed: same bytes, same ref
    assert_eq!(img.bytes, png_1px());
    assert_eq!(img.mime, "image/png");
    let mut doc = parsed.doc;
    internalize_images(&mut doc, &parsed.images);
    let Item::Image(i) = &doc.items[2] else {
        panic!()
    };
    assert_eq!((i.blob_ref.as_str(), &i.data_url), (r.as_str(), &None));
}

#[test]
fn leaves_out_images_over_32kb_with_a_notice() {
    let mut store = Store::new();
    let big = put(&mut store, noise(SHARE_IMAGE_LIMIT_BYTES + 1, 1));
    let link =
        create_share_link(&sample_doc(&big), BASE, |r| store.get(r).cloned(), false).unwrap();
    assert_eq!(link.notices.len(), 1);
    assert_eq!(
        link.notices[0],
        "1 image was too large for a link and was left out. Use “Export file” to share it."
    );
    let back = read_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    assert!(!back.items.iter().any(|i| i.kind() == ItemKind::Image));
    assert_eq!(back.items.len(), 2);
}

#[test]
fn reports_images_it_cannot_resolve() {
    let link = create_share_link(&sample_doc("sha256-missing"), BASE, no_blobs, false).unwrap();
    assert_eq!(
        link.notices,
        vec!["1 image could not be found and was left out."]
    );
}

#[test]
fn keeps_every_link_under_the_cap_by_dropping_the_largest_images_first() {
    let mut store = Store::new();
    let mut doc = LabelDoc::new();
    for s in 1..=3 {
        let r = put(&mut store, noise(30_000, s));
        doc.items.push(image(&r));
    }
    let link = create_share_link(&doc, BASE, |r| store.get(r).cloned(), false).unwrap();
    assert!(hash_of(&link.url).len() - SHARE_PREFIX.len() <= SHARE_MAX_FRAGMENT_CHARS);
    assert!(link.notices.join(" ").contains("too large"));
    let back = read_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    assert!(
        back.items
            .iter()
            .filter(|i| i.kind() == ItemKind::Image)
            .count()
            < 3
    );
}

#[test]
fn refuses_a_label_that_cannot_fit_at_all() {
    // ~150 K characters of incompressible CJK text.
    let text: String = noise(150_000, 1)
        .iter()
        .map(|b| char::from_u32(0x4e00 + u32::from(*b)).unwrap())
        .collect();
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        unreachable!()
    };
    t.text = text;
    let doc = LabelDoc::with_items(vec![Item::Text(t)]);
    let e = create_share_link(&doc, BASE, no_blobs, false).unwrap_err();
    assert!(e.0.contains("too large for a share link"));
}

#[test]
fn rejects_damaged_links() {
    let link = create_share_link(&LabelDoc::new(), BASE, no_blobs, false).unwrap();
    let cut = &hash_of(&link.url)[..20];
    assert!(read_share_fragment(cut).unwrap_err().0.contains("damaged"));
    assert!(
        read_share_fragment("#d=***")
            .unwrap_err()
            .0
            .contains("damaged")
    );
    assert_eq!(
        read_share_fragment("#d=").unwrap_err().0,
        "This share link is empty."
    );
    let padded = format!("{}=", hash_of(&link.url));
    assert!(read_share_fragment(&padded).is_err());
    let huge = format!("#d={}", "A".repeat(SHARE_READ_MAX_FRAGMENT_CHARS + 1));
    assert_eq!(
        read_share_fragment(&huge).unwrap_err().0,
        "This share link is too large to open."
    );
}

#[test]
fn validates_the_payload_schema() {
    let payload = encode_share_payload(&json!({"hello": "world"}));
    let e = read_share_fragment(&format!("#d={payload}")).unwrap_err();
    assert!(e.0.contains("valid label"), "{e}");
}

#[test]
fn refuses_decompression_bombs() {
    let payload = bytes_to_base64url(&deflate_raw(&vec![0u8; 2 * 1024 * 1024]));
    assert!(payload.len() < 10_000);
    let e = read_share_fragment(&format!("#d={payload}")).unwrap_err();
    assert!(e.0.contains("too large"), "{e}");
}

#[test]
fn opens_links_made_by_a_newer_app_read_only() {
    let mut future = serde_json::to_value(LabelDoc::new()).unwrap();
    future["schema"] = json!(7);
    let parsed = parse_share_fragment(&format!("#d={}", encode_share_payload(&future)))
        .unwrap()
        .unwrap();
    assert!(parsed.read_only);
    assert!(parsed.notices[0].contains("newer version"));
}

#[test]
fn inflate_rejects_trailing_data_and_truncation() {
    let mut z = deflate_raw(b"hello world");
    assert_eq!(inflate_raw(&z, 100).unwrap(), b"hello world");
    assert_eq!(
        inflate_raw(&z[..z.len() - 2], 100),
        Err(InflateError::Damaged)
    );
    z.push(0);
    assert_eq!(inflate_raw(&z, 100), Err(InflateError::Damaged));
    assert_eq!(
        inflate_raw(&deflate_raw(&[1; 200]), 100),
        Err(InflateError::TooLarge)
    );
}

#[test]
fn leaves_wifi_passwords_out_by_default_and_keeps_the_ssid() {
    let link = create_share_link(&wifi_doc(), BASE, no_blobs, false).unwrap();
    assert_eq!(link.notices, vec![NOTICE_WIFI_PASSWORD_OMITTED]);
    assert!(!decoded(&link.url).contains("hunter2-secret"));
    let back = read_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    let Item::Code(c) = &back.items[0] else {
        panic!()
    };
    assert_eq!(
        c.wifi,
        Some(WifiSettings {
            ssid: "Guest".into(),
            password: String::new(),
            security: WifiSecurity::Wpa,
            hidden: false
        })
    );
}

#[test]
fn keeps_wifi_passwords_only_when_the_user_opts_in() {
    let link = create_share_link(&wifi_doc(), BASE, no_blobs, true).unwrap();
    assert!(link.notices.is_empty());
    assert!(decoded(&link.url).contains("hunter2-secret"));
}

#[test]
fn also_blanks_a_code_switched_back_to_text_and_counts_several() {
    let mut doc = wifi_doc();
    let mut other = doc.items[0].clone();
    other.set_id("other".into());
    if let Item::Code(c) = &mut other {
        c.content = CodeContent::Text;
    }
    doc.items.push(other);
    let link = create_share_link(&doc, BASE, no_blobs, false).unwrap();
    assert_eq!(link.notices, vec![NOTICE_WIFI_PASSWORDS_OMITTED]);
    assert!(!decoded(&link.url).contains("hunter2-secret"));
}

#[test]
fn leaves_out_passwords_from_a_batch_column_with_a_notice_naming_it() {
    let mut doc = LabelDoc::with_items(vec![wifi_code("{{net}}", "{{pw}}")]);
    doc.name = "Guest stickers".into();
    doc.batch = Some(BatchData {
        enabled: true,
        columns: vec!["net".into(), "pw".into()],
        rows: vec![
            vec!["Guest1".into(), "S3cretPassw0rd!".into()],
            vec!["Guest2".into(), "Another-Secret-9".into()],
        ],
        ..BatchData::default()
    });
    let link = create_share_link(&doc, BASE, no_blobs, false).unwrap();
    assert_eq!(
        link.notices,
        vec![notice_wifi_password_columns(&["pw".to_string()])]
    );
    assert!(link.notices[0].contains("{{pw}} column"));
    let json = decoded(&link.url);
    assert!(!json.contains("S3cretPassw0rd") && !json.contains("Another-Secret-9"));
    let back = read_share_fragment(hash_of(&link.url)).unwrap().unwrap();
    assert_eq!(
        back.batch.unwrap().rows,
        vec![vec!["Guest1", ""], vec!["Guest2", ""]]
    );
    let opted = create_share_link(&doc, BASE, no_blobs, true).unwrap();
    assert!(decoded(&opted.url).contains("S3cretPassw0rd!"));
}

#[test]
fn shareable_doc_is_unchanged_when_there_is_nothing_to_strip_and_notes_custom_fonts() {
    let plain = LabelDoc::new();
    assert_eq!(shareable_doc(&plain, false), (plain.clone(), vec![]));
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        unreachable!()
    };
    t.custom_font = Some(FontSource::Local {
        postscript_name: "Inter-Regular".into(),
        family: "Inter".into(),
    });
    let fonts = LabelDoc::with_items(vec![Item::Text(t)]);
    assert_eq!(shareable_doc(&fonts, false).1, vec![NOTICE_CUSTOM_FONTS]);
}
