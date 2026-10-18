//! Undo/redo as a ring of document snapshots (port of `web/src/doc/history.ts`): 200 entries by
//! default; consecutive edits with the same coalesce key within 1 s merge (typing, sliders).
//!
//! The caller passes the time of each edit (milliseconds on any monotonic clock), which keeps
//! the coalescing rule testable.

use super::schema::LabelDoc;

/// Edits with the same coalesce key closer together than this merge into one undo step.
/// `COALESCE_MS`.
pub const COALESCE_MS: u64 = 1000;

/// Default number of snapshots kept (undo steps + the present).
pub const DEFAULT_CAPACITY: usize = 200;

/// Undo/redo history (`createHistory`). Generic so callers may keep snapshots behind an `Arc`.
#[derive(Debug, Clone)]
pub struct History<T = LabelDoc> {
    cap: usize,
    past: Vec<T>,
    future: Vec<T>,
    present: T,
    last_key: Option<String>,
    last_at: Option<u64>,
}

impl<T: PartialEq> History<T> {
    /// A history of at most `capacity` snapshots (at least 1); the oldest are dropped.
    pub fn new(initial: T, capacity: usize) -> Self {
        Self {
            cap: capacity.max(1),
            past: Vec::new(),
            future: Vec::new(),
            present: initial,
            last_key: None,
            last_at: None,
        }
    }

    /// [`History::new`] with [`DEFAULT_CAPACITY`].
    pub fn with_default_capacity(initial: T) -> Self {
        Self::new(initial, DEFAULT_CAPACITY)
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// The current document.
    pub fn present(&self) -> &T {
        &self.present
    }

    /// Records `doc` as the new present at time `now_ms`. Pushing a document equal to the
    /// present is a no-op (the studio compares identity; docs here compare by value).
    pub fn push(&mut self, doc: T, coalesce_key: Option<&str>, now_ms: u64) {
        if doc == self.present {
            return;
        }
        let merge = coalesce_key.is_some()
            && coalesce_key == self.last_key.as_deref()
            && self.last_at.is_some_and(|last| {
                i128::from(now_ms) - i128::from(last) < i128::from(COALESCE_MS)
            })
            && !self.past.is_empty()
            && self.future.is_empty();
        let prev = std::mem::replace(&mut self.present, doc);
        if !merge {
            self.past.push(prev);
            if self.past.len() > self.cap - 1 {
                let excess = self.past.len() - (self.cap - 1);
                self.past.drain(..excess);
            }
        }
        self.future.clear();
        self.last_key = coalesce_key.map(str::to_string);
        self.last_at = Some(now_ms);
    }

    /// Steps back; the restored present, or `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<&T> {
        let prev = self.past.pop()?;
        let cur = std::mem::replace(&mut self.present, prev);
        self.future.push(cur);
        self.last_key = None;
        Some(&self.present)
    }

    /// Steps forward; the restored present, or `None` when there is nothing to redo.
    pub fn redo(&mut self) -> Option<&T> {
        let next = self.future.pop()?;
        let cur = std::mem::replace(&mut self.present, next);
        self.past.push(cur);
        self.last_key = None;
        Some(&self.present)
    }

    /// Forgets everything and starts from `doc` (open / new label).
    pub fn reset(&mut self, doc: T) {
        self.past.clear();
        self.future.clear();
        self.present = doc;
        self.last_key = None;
        self.last_at = None;
    }
}
