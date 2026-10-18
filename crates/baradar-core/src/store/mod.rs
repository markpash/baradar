//! Local storage (port of `web/src/doc/persist*.ts`): the label library, image blobs by content
//! hash, thumbnails, print history, user fonts, preferences and the tape counter.
//!
//! The studio keeps these in IndexedDB and localStorage. Here they are plain files under one
//! directory (by default `~/Library/Application Support/baradar` on macOS):
//!
//! ```text
//! labels/<doc id>.json         {"doc": <raw doc, images internalised>, "savedAt": ISO}
//! thumbs/<doc id>.png          1-bit PNG thumbnail
//! blobs/<ref>                  image bytes, ref = "sha256-<32 hex>"; blobs/<ref>.json {type, createdAt}
//! history/<record id>.json     print-history record; history/<record id>.png thumbnail
//! fonts/<ref>                  user font file; fonts/index.json [UserFontInfo]
//! prefs.json                   Prefs
//! usage.json                   tape counter
//! ```
//!
//! Documents are stored as raw JSON and migrated by the caller on load, exactly like the
//! studio, so files written by a newer version are never silently rewritten.

mod blobs;
mod fonts;
mod history;
mod labels;
mod prefs;
mod usage;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

pub use blobs::BLOB_GRACE_MS;
pub use crate::doc::files::content_ref;
pub use fonts::{FontFormat, MAX_USER_FONT_BYTES, MAX_USER_FONTS, UserFontInfo, sniff_font_format};
pub use history::{HISTORY_LIMIT, HistoryRecord, new_record_id};
pub use labels::LabelSummary;
pub use prefs::{DefaultTextSize, FontKey, PreviewMode, Prefs, Theme};
pub use usage::{Usage, WidthUsage};

/// The storage root and its operations. Cheap to clone; every call goes to disk.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens (creating if needed) a store at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        for dir in ["labels", "thumbs", "blobs", "history", "fonts"] {
            fs::create_dir_all(root.join(dir))
                .with_context(|| format!("creating {}", root.join(dir).display()))?;
        }
        Ok(Self { root })
    }

    /// The default location: `<data dir>/baradar`.
    pub fn default_root() -> Result<PathBuf> {
        Ok(dirs::data_dir()
            .context("no data directory for this user")?
            .join("baradar"))
    }

    /// The storage root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, parts: &[&str]) -> PathBuf {
        parts.iter().fold(self.root.clone(), |p, s| p.join(s))
    }
}

/// Writes `bytes` to `path` atomically (temp file + rename), so a crash never leaves a
/// half-written label.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let mut tmp = tempfile_in(dir)?;
    tmp.1.write_all(bytes)?;
    tmp.1.sync_all()?;
    drop(tmp.1);
    fs::rename(&tmp.0, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn tempfile_in(dir: &Path) -> Result<(PathBuf, fs::File)> {
    for n in 0..100u32 {
        let p = dir.join(format!(".tmp-{}-{n}", std::process::id()));
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("could not create a temporary file in {}", dir.display())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// File names are ids we generated or validated, but never trust them as paths.
fn safe_name(s: &str) -> Option<&str> {
    (!s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(s)
}

fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}
