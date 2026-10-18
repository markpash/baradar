//! Print history (port of `web/src/doc/persist-history.ts`): the last [`HISTORY_LIMIT`] printed
//! labels, ready to reprint or reopen.

use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Store, now_ms, read_json, safe_name, write_atomic};

/// Records kept (`HISTORY_LIMIT`).
pub const HISTORY_LIMIT: usize = 50;

/// One printed job. `doc` is the raw document as printed (with Wi-Fi passwords and unresolved
/// placeholders), migrated by the caller when reopened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    /// ISO 8601.
    pub printed_at: String,
    pub doc: Value,
    pub name: String,
    pub tape_width_mm: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Labels in the job (rows × copies for a batch).
    pub labels: u32,
    pub copies: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_rows: Option<u32>,
    /// Tape used, mm.
    pub tape_mm: f64,
    pub seq: u64,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

/// A record id like the studio's: base-36 milliseconds, `-`, then two base-36 digits of a
/// sequence number and two random ones.
pub fn new_record_id() -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) % 1296;
    let rand = (uuid::Uuid::new_v4().as_u128() % 1296) as u32;
    format!("{}-{}{}", base36(now_ms().max(0) as u64), pad2(seq), pad2(rand))
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn pad2(n: u32) -> String {
    format!("{:0>2}", base36(u64::from(n)))
}

impl Store {
    /// History records, newest first (by `printedAt`, then `seq`).
    pub fn list_history(&self) -> Result<Vec<HistoryRecord>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(self.path(&["history"]))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(r) = read_json::<HistoryRecord>(&path)
            {
                out.push(r);
            }
        }
        out.sort_by(|a, b| b.printed_at.cmp(&a.printed_at).then(b.seq.cmp(&a.seq)));
        Ok(out)
    }

    /// Adds a record (and its thumbnail), then trims the history to [`HISTORY_LIMIT`].
    pub fn add_history(&self, record: &HistoryRecord, thumbnail_png: Option<&[u8]>) -> Result<()> {
        let id = safe_name(&record.id).context("invalid history record id")?;
        write_atomic(
            &self.path(&["history", &format!("{id}.json")]),
            &serde_json::to_vec(record)?,
        )?;
        if let Some(png) = thumbnail_png {
            let _ = write_atomic(&self.path(&["history", &format!("{id}.png")]), png);
        }
        for old in self.list_history()?.into_iter().skip(HISTORY_LIMIT) {
            self.remove_history(&old.id)?;
        }
        Ok(())
    }

    /// The record's thumbnail PNG.
    pub fn history_thumbnail(&self, id: &str) -> Option<Vec<u8>> {
        fs::read(self.path(&["history", &format!("{}.png", safe_name(id)?)])).ok()
    }

    /// Deletes one record.
    pub fn remove_history(&self, id: &str) -> Result<()> {
        let id = safe_name(id).context("invalid history record id")?;
        let _ = fs::remove_file(self.path(&["history", &format!("{id}.json")]));
        let _ = fs::remove_file(self.path(&["history", &format!("{id}.png")]));
        Ok(())
    }

    /// Deletes every record, then collects unreferenced blobs.
    pub fn clear_history(&self) -> Result<()> {
        for r in self.list_history()? {
            self.remove_history(&r.id)?;
        }
        self.collect_garbage()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(n: u64) -> HistoryRecord {
        HistoryRecord {
            id: new_record_id(),
            printed_at: format!("2026-10-18T00:00:{:02}.000Z", n % 60),
            doc: serde_json::json!({"name": "x"}),
            name: format!("Label {n}"),
            tape_width_mm: 12.0,
            media_id: None,
            model: Some("PT-E720BT".into()),
            labels: 1,
            copies: 1,
            batch_rows: None,
            tape_mm: 40.0,
            seq: n,
        }
    }

    #[test]
    fn keeps_the_newest_fifty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for n in 0..55 {
            store.add_history(&record(n), None).unwrap();
        }
        let list = store.list_history().unwrap();
        assert_eq!(list.len(), HISTORY_LIMIT);
        assert_eq!(list[0].name, "Label 54");
        store.clear_history().unwrap();
        assert!(store.list_history().unwrap().is_empty());
    }

    #[test]
    fn record_ids_are_safe_and_unique() {
        let a = new_record_id();
        assert!(safe_name(&a).is_some());
        assert_ne!(a, new_record_id());
    }
}
