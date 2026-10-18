//! The `.ptlabel.json` label file (port of `web/src/doc/persist-files.ts`) and the byte/text
//! codecs it shares with share links and storage (port of `web/src/doc/persist-codec.ts`):
//! base64 / base64url, data URLs, content hashes, plus a JSON writer that formats numbers like
//! `JSON.stringify`.
//!
//! Storage glue (id-collision checks, putting images into the blob store) is left to the caller:
//! [`parse_label_file`] returns the decoded images with their content refs, and
//! [`import_as_copy`] / [`internalize_images`] apply the studio's rules.

use std::io;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::Serialize;
use serde_json::Value;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::migrate::migrate;
use super::schema::{Item, LabelDoc, new_id, now_iso};
use super::secrets::shareable_doc;
use super::validate::js;

/// `FILE_EXTENSION`.
pub const FILE_EXTENSION: &str = ".ptlabel.json";
/// `FILE_MIME`.
pub const FILE_MIME: &str = "application/json";
/// Marker of the file envelope. `FILE_FORMAT`.
pub const FILE_FORMAT: &str = "ptouch-label";
/// Envelope version written and the newest one read (independent of the doc schema).
pub const FILE_VERSION: u32 = 1;
/// Refuse absurd files before parsing. `MAX_IMPORT_BYTES`.
pub const MAX_IMPORT_BYTES: u64 = 32 * 1024 * 1024;
/// The envelope's `generator` (the studio writes "ptouch studio"; its reader ignores the value).
pub const GENERATOR: &str = "baradar";

/// `"This label was made with a newer version of ptouch studio and opens read-only."`
pub const NOTICE_READ_ONLY: &str =
    "This label was made with a newer version of ptouch studio and opens read-only.";
/// Shown after importing a file whose schema was older than the current one.
pub const NOTICE_UPGRADED: &str = "This label was upgraded from an older format.";
/// Shown when the library already has the imported doc's id (see [`import_as_copy`]).
pub const NOTICE_IMPORTED_AS_COPY: &str =
    "A label with the same id is already in your library, so this one was imported as a copy.";
/// Shown on export for each image whose blob could not be resolved.
pub const NOTICE_IMAGE_MISSING: &str = "An image could not be found and is missing from the file.";

// ------------------------------------------------------------------------------------------
// Codecs (persist-codec.ts)
// ------------------------------------------------------------------------------------------

/// Decoding failures of the codecs below.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("not a data: URL")]
    NotDataUrl,
    #[error("invalid base64")]
    InvalidBase64,
    #[error("invalid base64url")]
    InvalidBase64Url,
    #[error("URI malformed")]
    InvalidPercentEncoding,
}

/// `atob`'s forgiving decoder: padding optional, non-zero trailing bits accepted.
const FORGIVING: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);
const B64_STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, FORGIVING);
const B64_URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    FORGIVING
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

/// Standard base64 with padding. `bytesToBase64`.
pub fn bytes_to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Standard base64; whitespace is ignored and padding is optional. `base64ToBytes`.
pub fn base64_to_bytes(b64: &str) -> Result<Vec<u8>, CodecError> {
    let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    B64_STANDARD
        .decode(compact)
        .map_err(|_| CodecError::InvalidBase64)
}

/// RFC 4648 §5, no padding. `bytesToBase64Url`.
pub fn bytes_to_base64url(bytes: &[u8]) -> String {
    B64_URL.encode(bytes)
}

/// RFC 4648 §5 without padding; any character outside `[A-Za-z0-9_-]` is rejected.
/// `base64UrlToBytes`.
pub fn base64url_to_bytes(text: &str) -> Result<Vec<u8>, CodecError> {
    if !text
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(CodecError::InvalidBase64Url);
    }
    B64_URL.decode(text).map_err(|_| CodecError::InvalidBase64)
}

/// `data:<mime>;base64,<payload>`; an empty MIME type becomes `application/octet-stream`.
/// `blobToDataUrl`.
pub fn bytes_to_data_url(bytes: &[u8], mime: &str) -> String {
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
    format!("data:{mime};base64,{}", bytes_to_base64(bytes))
}

/// Decodes a base64 or percent-encoded data URL into `(bytes, mime)`. `dataUrlToBlob`, which
/// matches `/^data:([^,;]*)((?:;[^,;=]+=[^,;]*)*)(;base64)?,(.*)$/s`. The MIME type is
/// normalised as a `Blob` type is (lowercased; empty if it has characters outside U+0020–U+007E).
pub fn data_url_to_bytes(url: &str) -> Result<(Vec<u8>, String), CodecError> {
    let rest = url.strip_prefix("data:").ok_or(CodecError::NotDataUrl)?;
    let (header, payload) = rest.split_once(',').ok_or(CodecError::NotDataUrl)?;
    let (mime, params) = header
        .split_once(';')
        .map_or((header, None), |(m, p)| (m, Some(p)));
    let mut base64 = false;
    if let Some(params) = params {
        let segments: Vec<&str> = params.split(';').collect();
        for (i, seg) in segments.iter().enumerate() {
            if i + 1 == segments.len() && *seg == "base64" {
                base64 = true;
            } else if !seg.split_once('=').is_some_and(|(k, _)| !k.is_empty()) {
                return Err(CodecError::NotDataUrl);
            }
        }
    }
    let bytes = if base64 {
        base64_to_bytes(payload)?
    } else {
        percent_decode(payload)?
    };
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
    let mime = if mime.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
        mime.to_ascii_lowercase()
    } else {
        String::new()
    };
    Ok((bytes, mime))
}

/// `new TextEncoder().encode(decodeURIComponent(s))`: `%XX` escapes must form valid UTF-8.
fn percent_decode(s: &str) -> Result<Vec<u8>, CodecError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .ok_or(CodecError::InvalidPercentEncoding)?;
            let hex = std::str::from_utf8(hex).map_err(|_| CodecError::InvalidPercentEncoding)?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| CodecError::InvalidPercentEncoding)?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    std::str::from_utf8(&out).map_err(|_| CodecError::InvalidPercentEncoding)?;
    Ok(out)
}

/// Approximate decoded size of a data URL in bytes, without decoding it. `dataUrlSize`.
pub fn data_url_size(url: &str) -> usize {
    let Some(comma) = url.find(',') else {
        return js::u16_len(url);
    };
    let payload = js::u16_len(&url[comma + 1..]);
    if url[..comma].ends_with(";base64") {
        payload * 3 / 4
    } else {
        payload
    }
}

/// `sha256-<first 32 hex digits>` of the content: stable blob keys, so equal images dedupe.
/// `contentRef`.
pub fn content_ref(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256-{hex}")
}

// ------------------------------------------------------------------------------------------
// JSON text as the studio writes it
// ------------------------------------------------------------------------------------------

/// Wraps a serde_json formatter so numbers print like JavaScript (`24`, not `24.0`).
struct JsFormatter<F>(F);

macro_rules! delegate {
    ($($name:ident),*) => {$(
        fn $name<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
            self.0.$name(w)
        }
    )*};
}

impl<F: Formatter> Formatter for JsFormatter<F> {
    delegate!(
        begin_array,
        end_array,
        begin_object,
        end_object,
        end_array_value,
        end_object_value,
        begin_object_value
    );

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(w, first)
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(w, first)
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f64) -> io::Result<()> {
        w.write_all(js::num(v).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f32) -> io::Result<()> {
        w.write_all(js::num(f64::from(v)).as_bytes())
    }
}

/// `JSON.stringify(value)` (compact) or `JSON.stringify(value, null, 2)` (`pretty`), with
/// JavaScript number formatting. Struct fields keep their declaration order.
pub fn to_js_json<T: Serialize + ?Sized>(value: &T, pretty: bool) -> String {
    let mut out = Vec::new();
    let ok = if pretty {
        let mut ser =
            serde_json::Serializer::with_formatter(&mut out, JsFormatter(PrettyFormatter::new()));
        value.serialize(&mut ser).is_ok()
    } else {
        let mut ser =
            serde_json::Serializer::with_formatter(&mut out, JsFormatter(CompactFormatter));
        value.serialize(&mut ser).is_ok()
    };
    if ok {
        String::from_utf8(out).unwrap_or_default()
    } else {
        String::new()
    }
}

// ------------------------------------------------------------------------------------------
// Label files (persist-files.ts)
// ------------------------------------------------------------------------------------------

/// The file envelope as written (`LabelFile`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LabelFileOut<'a> {
    format: &'static str,
    version: u32,
    exported_at: String,
    generator: &'static str,
    doc: &'a LabelDoc,
}

/// File name for a label: "Cable tags" → "cable-tags.ptlabel.json". `labelFileName`.
pub fn label_file_name(name: &str) -> String {
    let stripped: String = name
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect();
    let lower = stripped.to_lowercase().replace('ß', "ss");
    let mut slug = String::new();
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !slug.is_empty() {
                slug.push('-');
            }
            dash = false;
            slug.push(c);
        } else {
            dash = true;
        }
    }
    slug.truncate(60);
    let slug = slug.trim_end_matches('-');
    format!(
        "{}{FILE_EXTENSION}",
        if slug.is_empty() { "label" } else { slug }
    )
}

/// The self-contained JSON text of a label and the notices to show. `serializeLabelFile`.
///
/// Wi-Fi passwords are left out unless `include_wifi_passwords`; font files are never included.
/// Images without a `dataUrl` are inlined from `resolve_blob(blobRef) -> (bytes, mime)`; a
/// missing blob leaves the image without one and adds a notice. The text is 2-space indented
/// with a trailing newline.
pub fn serialize_label_file(
    doc: &LabelDoc,
    resolve_blob: impl Fn(&str) -> Option<(Vec<u8>, String)>,
    include_wifi_passwords: bool,
) -> (String, Vec<String>) {
    let (mut doc, mut notices) = shareable_doc(doc, include_wifi_passwords);
    for item in &mut doc.items {
        let Item::Image(img) = item else { continue };
        if img.data_url.as_ref().is_some_and(|d| !d.is_empty()) {
            continue;
        }
        let blob = if img.blob_ref.is_empty() {
            None
        } else {
            resolve_blob(&img.blob_ref)
        };
        match blob {
            Some((bytes, mime)) => img.data_url = Some(bytes_to_data_url(&bytes, &mime)),
            None => notices.push(NOTICE_IMAGE_MISSING.into()),
        }
    }
    let file = LabelFileOut {
        format: FILE_FORMAT,
        version: FILE_VERSION,
        exported_at: now_iso(),
        generator: GENERATOR,
        doc: &doc,
    };
    (format!("{}\n", to_js_json(&file, true)), notices)
}

/// An inlined image (`dataUrl`) decoded so storage can keep it by content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    /// The image item it belongs to.
    pub item_id: String,
    pub bytes: Vec<u8>,
    pub mime: String,
    /// [`content_ref`] of `bytes`: the blob key and the item's new `blobRef`.
    pub content_ref: String,
}

/// Decodes every image `dataUrl` of `doc`. The studio's `internalizeImages` fails the whole
/// load on a damaged one; so does this.
pub fn decode_images(doc: &LabelDoc) -> Result<Vec<DecodedImage>, CodecError> {
    doc.items
        .iter()
        .filter_map(|i| match i {
            Item::Image(img) => img
                .data_url
                .as_ref()
                .filter(|d| !d.is_empty())
                .map(|d| (&img.id, d)),
            _ => None,
        })
        .map(|(id, url)| {
            let (bytes, mime) = data_url_to_bytes(url)?;
            Ok(DecodedImage {
                item_id: id.clone(),
                content_ref: content_ref(&bytes),
                bytes,
                mime,
            })
        })
        .collect()
}

/// Points each image item at its stored blob (`blobRef` = content ref) and drops its `dataUrl`,
/// as `internalizeImages` does once the bytes are stored.
pub fn internalize_images(doc: &mut LabelDoc, images: &[DecodedImage]) {
    for item in &mut doc.items {
        let Item::Image(img) = item else { continue };
        if let Some(d) = images.iter().find(|d| d.item_id == img.id) {
            img.blob_ref = d.content_ref.clone();
            img.data_url = None;
        }
    }
}

/// Turns an import into a copy (new id, `createdAt = updatedAt = now`) when the library already
/// has its id or that entry cannot be read. Returns [`NOTICE_IMPORTED_AS_COPY`] for the caller's
/// notices.
pub fn import_as_copy(doc: &mut LabelDoc) -> &'static str {
    let now = now_iso();
    doc.id = new_id();
    doc.created_at = now.clone();
    doc.updated_at = now;
    NOTICE_IMPORTED_AS_COPY
}

/// A parsed, migrated and validated label file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFile {
    /// Image items still carry their `dataUrl`; see [`internalize_images`].
    pub doc: LabelDoc,
    /// Made by a newer version: open read-only (do not autosave).
    pub read_only: bool,
    pub migrated_from: Option<u32>,
    /// User-facing notices (read-only, upgraded).
    pub notices: Vec<String>,
    /// Repairs made while validating (not shown by the studio).
    pub repairs: Vec<String>,
    /// The decoded inlined images, in item order.
    pub images: Vec<DecodedImage>,
}

/// Parses, migrates and validates a label file's text (an envelope or a bare `LabelDoc`).
/// Port of `parseLabelFile` without the store: the caller checks for an id collision
/// ([`import_as_copy`]) and stores [`ParsedFile::images`] ([`internalize_images`]).
///
/// The error is a single user-facing message.
pub fn parse_label_file(text: &str) -> Result<ParsedFile, Vec<String>> {
    let json: Value = serde_json::from_str(text)
        .map_err(|_| vec!["This file is not a ptouch label (it is not valid JSON).".to_string()])?;
    let envelope = json
        .as_object()
        .filter(|o| o.get("format").and_then(Value::as_str) == Some(FILE_FORMAT));
    let raw = match envelope {
        Some(env) => {
            if env
                .get("version")
                .and_then(Value::as_f64)
                .is_some_and(|v| v > f64::from(FILE_VERSION))
            {
                return Err(vec![
                    "This label file was made by a newer version of ptouch studio. Please update the app.".into(),
                ]);
            }
            env.get("doc").unwrap_or(&Value::Null)
        }
        None => &json,
    };
    let m = migrate(raw).map_err(|p| {
        vec![format!(
            "This file is not a valid ptouch label: {}",
            p.join("; ")
        )]
    })?;

    let mut notices = Vec::new();
    if m.read_only {
        notices.push(NOTICE_READ_ONLY.to_string());
    }
    if m.migrated_from.is_some() && !m.read_only {
        notices.push(NOTICE_UPGRADED.to_string());
    }
    let images = decode_images(&m.doc)
        .map_err(|e| vec![format!("An image in this file could not be read: {e}")])?;
    Ok(ParsedFile {
        doc: m.doc,
        read_only: m.read_only,
        migrated_from: m.migrated_from,
        notices,
        repairs: m.notices,
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls() {
        assert_eq!(
            data_url_to_bytes("data:image/PNG;base64,QUJD").unwrap(),
            (b"ABC".to_vec(), "image/png".into())
        );
        assert_eq!(
            data_url_to_bytes("data:,a%20b").unwrap(),
            (b"a b".to_vec(), "application/octet-stream".into())
        );
        assert_eq!(
            data_url_to_bytes("data:text/plain;charset=utf-8;base64,QQ")
                .unwrap()
                .0,
            b"A"
        );
        assert_eq!(
            data_url_to_bytes("data:image/png;base64,QU\nJD").unwrap().0,
            b"ABC"
        );
        assert_eq!(
            data_url_to_bytes("data:image/png;foo;base64,QQ"),
            Err(CodecError::NotDataUrl)
        );
        assert_eq!(
            data_url_to_bytes("data:image/png;base64"),
            Err(CodecError::NotDataUrl)
        );
        assert_eq!(
            data_url_to_bytes("data:,%E0%A4"),
            Err(CodecError::InvalidPercentEncoding)
        );
        assert_eq!(
            data_url_to_bytes("data:image/png;base64,***"),
            Err(CodecError::InvalidBase64)
        );
        assert_eq!(data_url_size("data:image/png;base64,QUJD"), 3);
        assert_eq!(data_url_size("data:,abc"), 3);
        assert_eq!(
            bytes_to_data_url(b"x", ""),
            "data:application/octet-stream;base64,eA=="
        );
    }

    #[test]
    fn base64url_rules() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let s = bytes_to_base64url(&bytes);
        assert!(!s.contains(['=', '+', '/']));
        assert_eq!(base64url_to_bytes(&s).unwrap(), bytes);
        assert_eq!(
            base64url_to_bytes("QQ=="),
            Err(CodecError::InvalidBase64Url)
        );
        assert!(base64url_to_bytes("Q").is_err());
        assert_eq!(base64url_to_bytes("QR").unwrap(), b"A"); // trailing bits ignored, as atob
    }

    #[test]
    fn content_refs() {
        let r = content_ref(b"abc");
        assert_eq!(r, "sha256-ba7816bf8f01cfea414140de5dae2223");
        assert!(super::super::validate::is_content_ref(&r));
    }

    #[test]
    fn js_json_numbers() {
        #[derive(Serialize)]
        struct S {
            a: f64,
            b: f64,
            c: Vec<u8>,
        }
        assert_eq!(
            to_js_json(
                &S {
                    a: 24.0,
                    b: 1.1,
                    c: vec![]
                },
                false
            ),
            r#"{"a":24,"b":1.1,"c":[]}"#
        );
        assert_eq!(
            to_js_json(
                &S {
                    a: 2.0,
                    b: 0.5,
                    c: vec![1]
                },
                true
            ),
            "{\n  \"a\": 2,\n  \"b\": 0.5,\n  \"c\": [\n    1\n  ]\n}"
        );
    }
}
