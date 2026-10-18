//! Tape counter (port of `web/src/doc/persist-usage.ts`): tape used per width since a reset.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Store, read_json, write_atomic};
use crate::doc::now_iso;

/// Use per tape width.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct WidthUsage {
    pub mm: f64,
    pub labels: u64,
    pub jobs: u64,
}

/// The counter: `{since, byWidth: {"12": {mm, labels, jobs}}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub since: String,
    pub by_width: BTreeMap<String, WidthUsage>,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            since: now_iso(),
            by_width: BTreeMap::new(),
        }
    }
}

/// `String(round(w·10)/10)`: `"3.5"`, `"12"`.
pub fn width_key(width_mm: f64) -> Option<String> {
    if !(width_mm > 0.0 && width_mm <= 100.0) {
        return None;
    }
    let tenths = (width_mm * 10.0).round() as i64;
    Some(if tenths % 10 == 0 {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    })
}

impl Store {
    pub fn load_usage(&self) -> Usage {
        read_json(&self.path(&["usage.json"])).unwrap_or_default()
    }

    /// `addUsage`: adds a job's tape (rounded to 0.01 mm) and labels; ignores absurd jobs.
    pub fn add_usage(&self, width_mm: f64, mm: f64, labels: u64) -> Result<Usage> {
        let mut usage = self.load_usage();
        if let Some(key) = width_key(width_mm)
            && mm.is_finite()
            && (0.0..=1_000_000.0).contains(&mm)
        {
            let w = usage.by_width.entry(key).or_default();
            w.mm = ((w.mm + mm) * 100.0).round() / 100.0;
            w.labels += labels;
            w.jobs += 1;
            write_atomic(&self.path(&["usage.json"]), &serde_json::to_vec(&usage)?)?;
        }
        Ok(usage)
    }

    /// Resets the counter (`since` = now).
    pub fn reset_usage(&self) -> Result<Usage> {
        let usage = Usage::default();
        write_atomic(&self.path(&["usage.json"]), &serde_json::to_vec(&usage)?)?;
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_per_width() {
        assert_eq!(width_key(3.5).as_deref(), Some("3.5"));
        assert_eq!(width_key(12.0).as_deref(), Some("12"));
        assert_eq!(width_key(0.0), None);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.add_usage(12.0, 35.123, 1).unwrap();
        let u = store.add_usage(12.0, 10.0, 2).unwrap();
        assert_eq!(u.by_width["12"], WidthUsage { mm: 45.12, labels: 3, jobs: 2 });
        store.add_usage(12.0, 2e6, 1).unwrap();
        assert_eq!(store.load_usage().by_width["12"].jobs, 2);
        assert!(store.reset_usage().unwrap().by_width.is_empty());
    }
}
