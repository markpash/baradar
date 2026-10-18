//! The label library (port of the `ptouch-labels` / `ptouch-thumbs` stores in
//! `web/src/doc/persist.ts`).

use std::collections::HashSet;
use std::fs;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Store, read_json, safe_name, write_atomic};
use crate::doc::files::data_url_to_bytes;
use crate::doc::{Item, LabelDoc, now_iso};

/// One entry of [`Store::list_labels`].
#[derive(Debug, Clone, PartialEq)]
pub struct LabelSummary {
    pub id: String,
    pub name: String,
    pub tape_width_mm: f64,
    pub saved_at: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredLabel {
    doc: Value,
    saved_at: String,
}

impl Store {
    /// Saved labels, newest first.
    pub fn list_labels(&self) -> Result<Vec<LabelSummary>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.path(&["labels"]))? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(stored) = read_json::<StoredLabel>(&path) else {
                continue;
            };
            let Some(id) = stored.doc.get("id").and_then(Value::as_str) else {
                continue;
            };
            out.push(LabelSummary {
                id: id.to_owned(),
                name: stored
                    .doc
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Untitled label")
                    .to_owned(),
                tape_width_mm: stored
                    .doc
                    .pointer("/tape/widthMm")
                    .and_then(Value::as_f64)
                    .unwrap_or(24.0),
                saved_at: stored.saved_at,
            });
        }
        out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        Ok(out)
    }

    /// The raw stored document (the caller migrates and validates it).
    pub fn load_label(&self, id: &str) -> Option<Value> {
        let id = safe_name(id)?;
        read_json::<StoredLabel>(&self.path(&["labels", &format!("{id}.json")])).map(|s| s.doc)
    }

    /// Whether a label with `id` is stored.
    pub fn has_label(&self, id: &str) -> bool {
        safe_name(id).is_some_and(|id| self.path(&["labels", &format!("{id}.json")]).is_file())
    }

    /// Saves a label: inline images (`dataUrl`) move into the blob store first, then the doc
    /// and its thumbnail are written. Returns the doc as stored (images internalised).
    pub fn save_label(&self, doc: &LabelDoc, thumbnail_png: Option<&[u8]>) -> Result<LabelDoc> {
        let id = safe_name(&doc.id).context("invalid label id")?;
        let doc = self.internalize_images(doc)?;
        let stored = StoredLabel {
            doc: serde_json::to_value(&doc)?,
            saved_at: now_iso(),
        };
        write_atomic(
            &self.path(&["labels", &format!("{id}.json")]),
            &serde_json::to_vec(&stored)?,
        )?;
        if let Some(png) = thumbnail_png {
            // A failed thumbnail write is not worth failing the save for.
            let _ = write_atomic(&self.path(&["thumbs", &format!("{id}.png")]), png);
        }
        Ok(doc)
    }

    /// The label's thumbnail PNG.
    pub fn label_thumbnail(&self, id: &str) -> Option<Vec<u8>> {
        fs::read(self.path(&["thumbs", &format!("{}.png", safe_name(id)?)])).ok()
    }

    /// Deletes a label and its thumbnail, then collects unreferenced blobs.
    pub fn remove_label(&self, id: &str) -> Result<()> {
        let id = safe_name(id).context("invalid label id")?;
        let _ = fs::remove_file(self.path(&["labels", &format!("{id}.json")]));
        let _ = fs::remove_file(self.path(&["thumbs", &format!("{id}.png")]));
        self.collect_garbage()?;
        Ok(())
    }

    /// Moves every image's `dataUrl` into the blob store (`internalizeImages`). An image whose
    /// `blobRef` already exists keeps it; otherwise the bytes are hashed and stored.
    pub fn internalize_images(&self, doc: &LabelDoc) -> Result<LabelDoc> {
        let mut doc = doc.clone();
        for item in &mut doc.items {
            let Item::Image(img) = item else { continue };
            let Some(url) = img.data_url.take() else {
                continue;
            };
            if self.has_blob(&img.blob_ref) {
                continue;
            }
            match data_url_to_bytes(&url) {
                Ok((bytes, mime)) => img.blob_ref = self.put_blob(&bytes, &mime)?,
                // Keep the inline image rather than lose it.
                Err(_) => img.data_url = Some(url),
            }
        }
        Ok(doc)
    }

    /// Blob refs used by stored labels and print-history records.
    pub fn referenced_blobs(&self) -> Result<HashSet<String>> {
        let mut refs = HashSet::new();
        let mut collect = |doc: &Value| {
            for item in doc.get("items").and_then(Value::as_array).into_iter().flatten() {
                if let Some(r) = item.get("blobRef").and_then(Value::as_str).filter(|r| !r.is_empty()) {
                    refs.insert(r.to_owned());
                }
            }
        };
        for entry in fs::read_dir(self.path(&["labels"]))? {
            if let Some(s) = read_json::<StoredLabel>(&entry?.path()) {
                collect(&s.doc);
            }
        }
        for record in self.list_history()? {
            collect(&record.doc);
        }
        Ok(refs)
    }

    /// Garbage-collects blobs nothing references any more (run after every save and remove).
    pub fn collect_garbage(&self) -> Result<usize> {
        let refs = self.referenced_blobs()?;
        self.gc_blobs(&refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ItemKind, LabelDoc};

    #[test]
    fn saves_lists_loads_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut doc = LabelDoc::new();
        doc.name = "Cables".into();
        let mut img = Item::new(ItemKind::Image);
        if let Item::Image(i) = &mut img {
            i.data_url = Some("data:image/png;base64,aGk=".into());
        }
        doc.items.push(img);
        let saved = store.save_label(&doc, Some(b"png")).unwrap();
        let Item::Image(i) = &saved.items[1] else { panic!() };
        assert!(i.data_url.is_none() && store.has_blob(&i.blob_ref));

        let list = store.list_labels().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].tape_width_mm), ("Cables", 24.0));
        let raw = store.load_label(&doc.id).unwrap();
        assert_eq!(raw["name"], "Cables");
        assert_eq!(store.label_thumbnail(&doc.id).unwrap(), b"png");
        assert!(store.referenced_blobs().unwrap().contains(&i.blob_ref));

        store.remove_label(&doc.id).unwrap();
        assert!(store.list_labels().unwrap().is_empty() && !store.has_label(&doc.id));
        assert!(store.load_label("../../x").is_none());
    }
}
