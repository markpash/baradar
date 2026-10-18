//! Share links (port of `web/src/doc/persist-share.ts`): the URL fragment
//! `#d=<base64url(deflate-raw(JSON))>`.
//!
//! Images are inlined as data URLs when small (≤ 32 KB); larger ones are left out with a notice.
//! Links are capped in size; on load the payload is size-limited, decompressed with a cap,
//! migrated and validated. Wi-Fi passwords are blanked unless the user opts in, and font files are
//! never included ([`shareable_doc`]).

use std::io::Write as _;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use serde::Serialize;
use serde_json::Value;

use super::files::{
    DecodedImage, NOTICE_READ_ONLY, base64url_to_bytes, bytes_to_base64url, bytes_to_data_url,
    data_url_size, decode_images, to_js_json,
};
use super::migrate::migrate;
use super::schema::{Item, LabelDoc, new_id, now_iso};
use super::validate::js;

pub use super::secrets::{
    NOTICE_CUSTOM_FONTS, NOTICE_WIFI_PASSWORD_OMITTED, NOTICE_WIFI_PASSWORDS_OMITTED,
    notice_wifi_password_columns, shareable_doc,
};

/// `SHARE_PREFIX`.
pub const SHARE_PREFIX: &str = "#d=";
/// Images up to this many bytes are inlined. `SHARE_IMAGE_LIMIT_BYTES`.
pub const SHARE_IMAGE_LIMIT_BYTES: usize = 32 * 1024;
/// Largest fragment created (characters after `#d=`). `SHARE_MAX_FRAGMENT_CHARS`.
pub const SHARE_MAX_FRAGMENT_CHARS: usize = 64 * 1024;
/// Largest fragment accepted (reading is a little more lenient than writing).
/// `SHARE_READ_MAX_FRAGMENT_CHARS`.
pub const SHARE_READ_MAX_FRAGMENT_CHARS: usize = 256 * 1024;
/// Decompression cap (guards against deflate bombs). `SHARE_MAX_JSON_BYTES`.
pub const SHARE_MAX_JSON_BYTES: usize = 1024 * 1024;

/// The deployed studio's base URL (`location.origin + BASE_URL`).
pub const STUDIO_BASE_URL: &str = "https://fjanicki.github.io/ptouch/";

const MSG_EMPTY: &str = "This share link is empty.";
const MSG_TOO_LARGE_TO_OPEN: &str = "This share link is too large to open.";
const MSG_DAMAGED: &str =
    "This share link is damaged (it may have been cut off when it was copied).";
const MSG_TOO_LARGE_FOR_LINK: &str =
    "This label is too large for a share link. Use “Export file” instead.";

/// A damaged or invalid share link, with a user-facing message (`ShareLinkError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ShareLinkError(pub String);

/// A label that cannot fit in a link, with a user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ShareTooLarge(pub String);

/// A created link (`ShareResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLink {
    pub url: String,
    /// Human-readable notices, e.g. "1 image was too large for a link and was left out. …".
    pub notices: Vec<String>,
}

/// Why [`inflate_raw`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// More output than the cap allows.
    TooLarge,
    /// Not a complete raw DEFLATE stream (or data after its end).
    Damaged,
}

/// Raw DEFLATE (RFC 1951, no zlib or gzip header) at the default level, as
/// `CompressionStream('deflate-raw')`. `deflateRaw`.
pub fn deflate_raw(bytes: &[u8]) -> Vec<u8> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    // Writing to a Vec cannot fail.
    let _ = enc.write_all(bytes);
    enc.finish().unwrap_or_default()
}

/// Inflates raw DEFLATE with an output cap. `inflateRaw`.
pub fn inflate_raw(input: &[u8], max_bytes: usize) -> Result<Vec<u8>, InflateError> {
    let mut d = Decompress::new(false);
    let mut out: Vec<u8> = Vec::new();
    loop {
        if out.len() == out.capacity() {
            out.reserve(32 * 1024);
        }
        let (in_before, out_before) = (d.total_in(), d.total_out());
        let rest = input.get(in_before as usize..).unwrap_or(&[]);
        let status = d
            .decompress_vec(rest, &mut out, FlushDecompress::None)
            .map_err(|_| InflateError::Damaged)?;
        if out.len() > max_bytes {
            return Err(InflateError::TooLarge);
        }
        match status {
            Status::StreamEnd if (d.total_in() as usize) < input.len() => {
                return Err(InflateError::Damaged);
            }
            Status::StreamEnd => return Ok(out),
            _ if d.total_in() == in_before && d.total_out() == out_before => {
                return Err(InflateError::Damaged);
            }
            _ => {}
        }
    }
}

/// Encodes a (share-ready) doc into the fragment payload, without `#d=`. `encodeSharePayload`.
pub fn encode_share_payload<T: Serialize + ?Sized>(doc: &T) -> String {
    bytes_to_base64url(&deflate_raw(to_js_json(doc, false).as_bytes()))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Builds `<base_url>#d=…`. Port of `createShareLink`.
///
/// `resolve_blob(blobRef) -> (bytes, mime)` supplies stored images so they can be inlined.
/// Wi-Fi passwords stay in the link only with `include_wifi_passwords`. Fails with a user-facing
/// message if the label cannot fit in a link even without images.
pub fn create_share_link(
    source: &LabelDoc,
    base_url: &str,
    resolve_blob: impl Fn(&str) -> Option<(Vec<u8>, String)>,
    include_wifi_passwords: bool,
) -> Result<ShareLink, ShareTooLarge> {
    let (doc, mut notices) = shareable_doc(source, include_wifi_passwords);
    let mut too_large = 0;
    let mut missing = 0;
    let mut items: Vec<Item> = Vec::new();
    for item in doc.items.iter() {
        let Item::Image(img) = item else {
            items.push(item.clone());
            continue;
        };
        let mut data_url = img.data_url.clone().filter(|d| !d.is_empty());
        if data_url.is_none() && !img.blob_ref.is_empty() {
            match resolve_blob(&img.blob_ref) {
                Some((bytes, mime)) if bytes.len() <= SHARE_IMAGE_LIMIT_BYTES => {
                    data_url = Some(bytes_to_data_url(&bytes, &mime));
                }
                Some(_) => {
                    too_large += 1;
                    continue;
                }
                None => {
                    missing += 1;
                    continue;
                }
            }
        }
        let Some(data_url) = data_url else {
            missing += 1;
            continue;
        };
        if data_url_size(&data_url) > SHARE_IMAGE_LIMIT_BYTES {
            too_large += 1;
            continue;
        }
        let mut img = img.clone();
        img.blob_ref = String::new();
        img.data_url = Some(data_url);
        items.push(Item::Image(img));
    }

    // The recipient gets a copy; the id is replaced on load anyway.
    let mut shared = LabelDoc { items, ..doc };
    let mut payload = encode_share_payload(&shared);

    // Still too long: drop the largest images first.
    while payload.len() > SHARE_MAX_FRAGMENT_CHARS {
        let size = |i: &Item| match i {
            Item::Image(img) => Some(data_url_size(img.data_url.as_deref().unwrap_or(""))),
            _ => None,
        };
        let mut largest: Option<(usize, usize)> = None;
        for (idx, item) in shared.items.iter().enumerate() {
            if let Some(s) = size(item)
                && largest.is_none_or(|(_, best)| s > best)
            {
                largest = Some((idx, s));
            }
        }
        let Some((idx, _)) = largest else { break };
        shared.items.remove(idx);
        too_large += 1;
        payload = encode_share_payload(&shared);
    }
    if payload.len() > SHARE_MAX_FRAGMENT_CHARS {
        return Err(ShareTooLarge(MSG_TOO_LARGE_FOR_LINK.into()));
    }

    if too_large > 0 {
        let one = too_large == 1;
        notices.push(format!(
            "{} too large for a link and {} left out. Use “Export file” to share {}.",
            plural(too_large, "image was", "images were"),
            if one { "was" } else { "were" },
            if one { "it" } else { "them" }
        ));
    }
    if missing > 0 {
        notices.push(format!(
            "{} could not be found and {} left out.",
            plural(missing, "image", "images"),
            if missing == 1 { "was" } else { "were" }
        ));
    }

    let base = base_url.split('#').next().unwrap_or(base_url);
    Ok(ShareLink {
        url: format!("{base}{SHARE_PREFIX}{payload}"),
        notices,
    })
}

/// A share link's label (`ParsedShare`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedShare {
    /// A copy: fresh id, `createdAt = updatedAt = now`. Image items still carry their `dataUrl`
    /// (see [`super::files::internalize_images`]).
    pub doc: LabelDoc,
    /// Made by a newer app version: show it, but do not autosave over anything.
    pub read_only: bool,
    pub notices: Vec<String>,
    /// Repairs made while validating (not shown by the studio).
    pub repairs: Vec<String>,
    /// The decoded inlined images, in item order.
    pub images: Vec<DecodedImage>,
}

/// Decodes a fragment payload (after `#d=`, before any `&`) into JSON. Errors are user-facing.
pub fn decode_share_payload(payload: &str) -> Result<Value, ShareLinkError> {
    let damaged = || ShareLinkError(MSG_DAMAGED.into());
    let bytes = base64url_to_bytes(payload).map_err(|_| damaged())?;
    let json = inflate_raw(&bytes, SHARE_MAX_JSON_BYTES).map_err(|e| match e {
        InflateError::TooLarge => ShareLinkError(MSG_TOO_LARGE_TO_OPEN.into()),
        InflateError::Damaged => damaged(),
    })?;
    // TextDecoder('utf-8', {fatal: true}) drops a leading BOM.
    let json = json.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(&json);
    let text = std::str::from_utf8(json).map_err(|_| damaged())?;
    serde_json::from_str(text).map_err(|_| damaged())
}

/// Parses a URL fragment (`location.hash`, with or without the leading `#`). `Ok(None)` if it is
/// not a share fragment. Port of `parseShareFragment` without the store: the caller stores
/// [`ParsedShare::images`].
pub fn parse_share_fragment(hash: &str) -> Result<Option<ParsedShare>, ShareLinkError> {
    let h = hash.strip_prefix('#').unwrap_or(hash);
    let Some(rest) = h.strip_prefix(&SHARE_PREFIX[1..]) else {
        return Ok(None);
    };
    let payload = rest.split('&').next().unwrap_or("");
    if payload.is_empty() {
        return Err(ShareLinkError(MSG_EMPTY.into()));
    }
    if js::u16_len(payload) > SHARE_READ_MAX_FRAGMENT_CHARS {
        return Err(ShareLinkError(MSG_TOO_LARGE_TO_OPEN.into()));
    }
    let raw = decode_share_payload(payload)?;
    let m = migrate(&raw).map_err(|p| {
        ShareLinkError(format!(
            "This share link does not contain a valid label: {}",
            p.join("; ")
        ))
    })?;
    let now = now_iso();
    let doc = LabelDoc {
        id: new_id(),
        created_at: now.clone(),
        updated_at: now,
        ..m.doc
    };
    let images = decode_images(&doc).map_err(|_| ShareLinkError(MSG_DAMAGED.into()))?;
    let notices = if m.read_only {
        vec![NOTICE_READ_ONLY.to_string()]
    } else {
        Vec::new()
    };
    Ok(Some(ParsedShare {
        doc,
        read_only: m.read_only,
        notices,
        repairs: m.notices,
        images,
    }))
}

/// `readShareFragment`: just the doc of [`parse_share_fragment`].
pub fn read_share_fragment(hash: &str) -> Result<Option<LabelDoc>, ShareLinkError> {
    Ok(parse_share_fragment(hash)?.map(|p| p.doc))
}
