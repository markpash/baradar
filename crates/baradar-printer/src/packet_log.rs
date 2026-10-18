//! Hex packet log for diagnostics, as in the studio's `printer/packetlog.ts`: `>>` host to
//! printer, `<<` printer to host, `--` notes. Writes are truncated (raster data is long).

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

/// Bytes of a write kept in the log (the studio's `logMaxBytes`).
pub const LOG_MAX_BYTES: usize = 96;
/// Entries kept by [`PacketLog`] and by the client's packet channel (the studio's ring size).
pub const LOG_CAPACITY: usize = 2000;

/// Direction of a log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PacketDir {
    /// Host to printer (`>>`).
    Sent,
    /// Printer to host (`<<`).
    Received,
    /// A note (`--`): open, retry, cancel, failure…
    Note,
}

impl PacketDir {
    /// `>>`, `<<` or `--`.
    #[must_use]
    pub fn arrow(self) -> &'static str {
        match self {
            Self::Sent => ">>",
            Self::Received => "<<",
            Self::Note => "--",
        }
    }
}

/// One entry of the packet log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketEntry {
    /// Time since the client started.
    pub at: Duration,
    /// Direction.
    pub dir: PacketDir,
    /// The bytes (at most [`LOG_MAX_BYTES`] for writes); empty for notes.
    pub bytes: Vec<u8>,
    /// Length of the whole buffer (`bytes.len()` unless truncated).
    pub total_len: usize,
    /// Text of a note.
    pub note: Option<String>,
}

impl PacketEntry {
    /// A data entry; `bytes` longer than [`LOG_MAX_BYTES`] are truncated for writes.
    #[must_use]
    pub fn data(at: Duration, dir: PacketDir, bytes: &[u8]) -> Self {
        let keep = if dir == PacketDir::Sent {
            bytes.len().min(LOG_MAX_BYTES)
        } else {
            bytes.len()
        };
        Self {
            at,
            dir,
            bytes: bytes.get(..keep).unwrap_or(bytes).to_vec(),
            total_len: bytes.len(),
            note: None,
        }
    }

    /// A note.
    #[must_use]
    pub fn note(at: Duration, text: impl Into<String>) -> Self {
        Self {
            at,
            dir: PacketDir::Note,
            bytes: Vec::new(),
            total_len: 0,
            note: Some(text.into()),
        }
    }
}

/// `   12.345s >> 1B 69 53 … (+N bytes, T total)`, the studio's diagnostics line format.
impl fmt::Display for PacketEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = format!("{:.3}s", self.at.as_secs_f64());
        write!(f, "{t:>10} {} ", self.dir.arrow())?;
        if let Some(note) = &self.note {
            return f.write_str(note);
        }
        f.write_str(&hex(&self.bytes))?;
        if self.total_len > self.bytes.len() {
            write!(
                f,
                " … (+{} bytes, {} total)",
                self.total_len - self.bytes.len(),
                self.total_len
            )?;
        }
        Ok(())
    }
}

/// `1B 69 53`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// A bounded ring of [`PacketEntry`] (oldest dropped first), e.g. for a diagnostics view fed
/// from the client's packet channel.
#[derive(Debug, Clone)]
pub struct PacketLog {
    entries: VecDeque<PacketEntry>,
    capacity: usize,
}

impl Default for PacketLog {
    fn default() -> Self {
        Self::new(LOG_CAPACITY)
    }
}

impl PacketLog {
    /// An empty log keeping at most `capacity` entries (at least 1).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Adds an entry, dropping the oldest when full.
    pub fn push(&mut self, entry: PacketEntry) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &PacketEntry> + ExactSizeIterator {
        self.entries.iter()
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// One line per entry (the newest `last` entries), for "Copy diagnostics".
    #[must_use]
    pub fn to_text(&self, last: usize) -> String {
        let skip = self.entries.len().saturating_sub(last);
        let lines: Vec<String> = self
            .entries
            .iter()
            .skip(skip)
            .map(ToString::to_string)
            .collect();
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_use_the_studio_format() {
        let e = PacketEntry::data(
            Duration::from_millis(12_345),
            PacketDir::Sent,
            &[0x1B, 0x69, 0x53],
        );
        assert_eq!(e.to_string(), "   12.345s >> 1B 69 53");
        let long = vec![0u8; 200];
        let e = PacketEntry::data(Duration::ZERO, PacketDir::Sent, &long);
        assert_eq!(e.bytes.len(), LOG_MAX_BYTES);
        assert!(e.to_string().ends_with(" … (+104 bytes, 200 total)"), "{e}");
        // Reads are kept whole.
        let e = PacketEntry::data(Duration::ZERO, PacketDir::Received, &long);
        assert_eq!(e.bytes.len(), 200);
        let e = PacketEntry::note(Duration::from_millis(5), "open bt:…");
        assert_eq!(e.to_string(), "    0.005s -- open bt:…");
    }

    #[test]
    fn ring_is_bounded() {
        let mut log = PacketLog::new(3);
        for i in 0..5u8 {
            log.push(PacketEntry::data(Duration::ZERO, PacketDir::Received, &[i]));
        }
        let kept: Vec<u8> = log.entries().map(|e| e.bytes[0]).collect();
        assert_eq!(kept, [2, 3, 4]);
        assert_eq!(log.to_text(1), "    0.000s << 04");
    }
}
