//! User font files (port of the storage half of `web/src/doc/persist-fonts.ts`). Parsing font
//! names and loading faces lives in `render::fonts`; this module only stores files.

use std::fs;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::doc::validate::is_content_ref;
use super::{Store, content_ref, read_json, write_atomic};
use crate::doc::now_iso;

/// Largest user font file (10 MiB).
pub const MAX_USER_FONT_BYTES: usize = 10 * 1024 * 1024;
/// Most user fonts kept.
pub const MAX_USER_FONTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontFormat {
    Ttf,
    Otf,
    Woff,
    Woff2,
}

/// Font format from the file's magic bytes (needs at least 12 bytes).
pub fn sniff_font_format(bytes: &[u8]) -> Option<FontFormat> {
    if bytes.len() < 12 {
        return None;
    }
    match &bytes[..4] {
        [0, 1, 0, 0] | b"true" => Some(FontFormat::Ttf),
        b"OTTO" => Some(FontFormat::Otf),
        b"wOFF" => Some(FontFormat::Woff),
        b"wOF2" => Some(FontFormat::Woff2),
        _ => None,
    }
}

/// `UserFontInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFontInfo {
    /// Content reference (`sha256-…`), also the `customFont.ref` in documents.
    #[serde(rename = "ref")]
    pub reference: String,
    pub family: String,
    pub file_name: String,
    pub format: FontFormat,
    pub size: usize,
    pub added_at: String,
}

impl Store {
    /// Stored user fonts, in the order they were added.
    pub fn list_fonts(&self) -> Vec<UserFontInfo> {
        read_json(&self.path(&["fonts", "index.json"])).unwrap_or_default()
    }

    /// Stores a font file. `family` is the display name the caller read from the font. The same
    /// file is never stored twice; the existing entry is returned instead.
    pub fn add_font(&self, bytes: &[u8], family: &str, file_name: &str) -> Result<UserFontInfo> {
        if bytes.len() > MAX_USER_FONT_BYTES {
            bail!("This font file is larger than 10 MB.");
        }
        let Some(format) = sniff_font_format(bytes) else {
            bail!("This file is not a TTF, OTF, WOFF or WOFF2 font.");
        };
        let reference = content_ref(bytes);
        let mut index = self.list_fonts();
        if let Some(existing) = index.iter().find(|f| f.reference == reference) {
            return Ok(existing.clone());
        }
        if index.len() >= MAX_USER_FONTS {
            bail!("You can add up to 50 fonts. Remove one first.");
        }
        write_atomic(&self.path(&["fonts", &reference]), bytes)?;
        let info = UserFontInfo {
            reference,
            family: family.chars().take(100).collect(),
            file_name: file_name.chars().take(200).collect(),
            format,
            size: bytes.len(),
            added_at: now_iso(),
        };
        index.push(info.clone());
        write_atomic(&self.path(&["fonts", "index.json"]), &serde_json::to_vec_pretty(&index)?)?;
        Ok(info)
    }

    /// The bytes of a stored user font.
    pub fn font_bytes(&self, reference: &str) -> Option<Vec<u8>> {
        is_content_ref(reference)
            .then(|| fs::read(self.path(&["fonts", reference])).ok())
            .flatten()
    }

    /// Removes a user font.
    pub fn remove_font(&self, reference: &str) -> Result<()> {
        let mut index = self.list_fonts();
        index.retain(|f| f.reference != reference);
        write_atomic(&self.path(&["fonts", "index.json"]), &serde_json::to_vec_pretty(&index)?)?;
        if is_content_ref(reference) {
            let _ = fs::remove_file(self.path(&["fonts", reference]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_fonts_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut ttf = vec![0, 1, 0, 0];
        ttf.extend_from_slice(&[0; 20]);
        assert!(store.add_font(b"not a font..", "X", "x.bin").is_err());
        let a = store.add_font(&ttf, "My Font", "my.ttf").unwrap();
        assert_eq!(a.format, FontFormat::Ttf);
        assert_eq!(store.add_font(&ttf, "Again", "again.ttf").unwrap(), a);
        assert_eq!(store.list_fonts().len(), 1);
        assert_eq!(store.font_bytes(&a.reference).unwrap(), ttf);
        store.remove_font(&a.reference).unwrap();
        assert!(store.list_fonts().is_empty() && store.font_bytes(&a.reference).is_none());
    }
}
