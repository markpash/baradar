//! Pure, immutable document operations used by the editor (port of `web/src/doc/ops.ts`).
//!
//! Each returns a new doc with `updatedAt` bumped and never changes its input (undo snapshots
//! rely on that).

use super::schema::{Item, ItemKind, LabelDoc, new_id, now_iso};

fn touch(mut doc: LabelDoc) -> LabelDoc {
    doc.updated_at = now_iso();
    doc
}

/// Inserts a new item of `kind` after `after_id` (or at the end when it is `None` or not found).
/// Returns the new doc and the new item's id. `addItem`.
pub fn add_item(doc: &LabelDoc, kind: ItemKind, after_id: Option<&str>) -> (LabelDoc, String) {
    let item = Item::new(kind);
    let id = item.id().to_string();
    let at = after_id
        .filter(|a| !a.is_empty())
        .and_then(|a| doc.items.iter().position(|i| i.id() == a))
        .map_or(doc.items.len(), |p| p + 1);
    let mut next = doc.clone();
    next.items.insert(at, item);
    (touch(next), id)
}

/// Edits item `id` in place on a copy (its id and kind cannot change). `updateItem`.
///
/// A changed text size also drops `clipTall` (v1 clipping of an old label's text): from then on
/// the text follows the current sizing rules. (The studio drops it whenever the patch names
/// `size`; here it is dropped when the size actually changes.)
pub fn update_item(doc: &LabelDoc, id: &str, edit: impl FnOnce(&mut Item)) -> LabelDoc {
    let mut next = doc.clone();
    if let Some(item) = next.items.iter_mut().find(|i| i.id() == id) {
        let mut edited = item.clone();
        edit(&mut edited);
        if edited.kind() == item.kind() {
            edited.set_id(id.to_string());
            if let (Item::Text(before), Item::Text(after)) = (&*item, &mut edited)
                && before.size != after.size
            {
                after.clip_tall = false;
            }
            *item = edited;
        }
    }
    touch(next)
}

/// Removes item `id`. `removeItem`.
pub fn remove_item(doc: &LabelDoc, id: &str) -> LabelDoc {
    let mut next = doc.clone();
    next.items.retain(|i| i.id() != id);
    touch(next)
}

/// Moves item `id` to index `to` (clamped). An unknown id returns the doc unchanged. `moveItem`.
pub fn move_item(doc: &LabelDoc, id: &str, to: usize) -> LabelDoc {
    let Some(from) = doc.items.iter().position(|i| i.id() == id) else {
        return doc.clone();
    };
    let mut next = doc.clone();
    let it = next.items.remove(from);
    let at = to.min(next.items.len());
    next.items.insert(at, it);
    touch(next)
}

/// Duplicates item `id` right after itself (deep copy, new id; a free-layout frame is offset by
/// 2 mm so the copy is visible). Returns the doc and the new id; an unknown id returns the doc
/// unchanged and an empty id. `duplicateItem`.
pub fn duplicate_item(doc: &LabelDoc, id: &str) -> (LabelDoc, String) {
    let Some(at) = doc.items.iter().position(|i| i.id() == id) else {
        return (doc.clone(), String::new());
    };
    let mut copy = doc.items[at].clone();
    let new = new_id();
    copy.set_id(new.clone());
    if let Some(f) = copy.frame_mut() {
        f.x_mm += 2.0;
        f.y_mm += 2.0;
    }
    let mut next = doc.clone();
    next.items.insert(at + 1, copy);
    (touch(next), new)
}

/// Edits label-level settings (tape, length, margins, layout, frame, print, name, batch) on a
/// copy. `schema`, `id`, `items` and `createdAt` cannot change. `updateDoc`.
pub fn update_doc(doc: &LabelDoc, edit: impl FnOnce(&mut LabelDoc)) -> LabelDoc {
    let mut next = doc.clone();
    edit(&mut next);
    next.schema = doc.schema;
    next.id = doc.id.clone();
    next.items = doc.items.clone();
    next.created_at = doc.created_at.clone();
    touch(next)
}
