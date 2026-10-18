//! Image blobs stored by content hash (port of the `ptouch-blobs` store in
//! `web/src/doc/persist.ts`).

use std::collections::HashSet;
use std::fs;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use crate::doc::files::content_ref;
use crate::doc::validate::is_content_ref;
use super::{Store, now_ms, read_json, write_atomic};

/// Unreferenced blobs younger than this survive garbage collection (`blobGraceMs`, one day).
pub const BLOB_GRACE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlobMeta {
    #[serde(rename = "type")]
    mime: String,
    created_at: i64,
}

impl Store {
    /// Stores `bytes` (deduplicated by content) and returns its reference.
    pub fn put_blob(&self, bytes: &[u8], mime: &str) -> Result<String> {
        let r = content_ref(bytes);
        let path = self.path(&["blobs", &r]);
        if !path.exists() {
            write_atomic(&path, bytes)?;
            let meta = BlobMeta {
                mime: mime.to_owned(),
                created_at: now_ms(),
            };
            write_atomic(&self.path(&["blobs", &format!("{r}.json")]), &serde_json::to_vec(&meta)?)?;
        }
        Ok(r)
    }

    /// The bytes and MIME type of a stored blob.
    pub fn get_blob(&self, reference: &str) -> Option<(Vec<u8>, String)> {
        if !is_content_ref(reference) {
            return None;
        }
        let bytes = fs::read(self.path(&["blobs", reference])).ok()?;
        let mime = read_json::<BlobMeta>(&self.path(&["blobs", &format!("{reference}.json")]))
            .map_or_else(|| "application/octet-stream".to_owned(), |m| m.mime);
        Some((bytes, mime))
    }

    /// Whether a blob exists.
    pub fn has_blob(&self, reference: &str) -> bool {
        is_content_ref(reference) && self.path(&["blobs", reference]).is_file()
    }

    /// Deletes blobs older than [`BLOB_GRACE_MS`] that `referenced` does not contain.
    pub fn gc_blobs(&self, referenced: &HashSet<String>) -> Result<usize> {
        let now = now_ms();
        let mut removed = 0;
        for entry in fs::read_dir(self.path(&["blobs"]))? {
            let name = entry?.file_name();
            let Some(name) = name.to_str().filter(|n| is_content_ref(n)) else {
                continue;
            };
            if referenced.contains(name) {
                continue;
            }
            let meta_path = self.path(&["blobs", &format!("{name}.json")]);
            let created = read_json::<BlobMeta>(&meta_path).map_or(0, |m| m.created_at);
            if now - created >= BLOB_GRACE_MS {
                let _ = fs::remove_file(self.path(&["blobs", name]));
                let _ = fs::remove_file(meta_path);
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_refs_match_the_studio_format() {
        // SHA-256("") = e3b0c442 98fc1c14 9afbf4c8 996fb924 …
        assert_eq!(content_ref(b""), "sha256-e3b0c44298fc1c149afbf4c8996fb924");
        assert!(is_content_ref(&content_ref(b"x")));
    }

    #[test]
    fn stores_dedupes_and_collects() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let a = store.put_blob(b"png bytes", "image/png").unwrap();
        assert_eq!(store.put_blob(b"png bytes", "image/png").unwrap(), a);
        assert_eq!(store.get_blob(&a).unwrap(), (b"png bytes".to_vec(), "image/png".into()));
        assert!(store.get_blob("../etc/passwd").is_none());
        // Young unreferenced blobs survive; referenced ones always do.
        assert_eq!(store.gc_blobs(&HashSet::new()).unwrap(), 0);
        assert!(store.has_blob(&a));
    }
}
