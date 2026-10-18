//! Port of the ops and history parts of web/tests/unit/doc/schema.test.ts.

use baradar_core::doc::history::{COALESCE_MS, History};
use baradar_core::doc::ops::*;
use baradar_core::doc::*;

fn docs(n: usize) -> Vec<LabelDoc> {
    (0..n)
        .map(|i| {
            let mut d = LabelDoc::new();
            d.name = format!("v{i}");
            d
        })
        .collect()
}

#[test]
fn ops_never_change_the_input() {
    let d = LabelDoc::new();
    let before = d.clone();
    let (d2, id) = add_item(&d, ItemKind::Code, None);
    assert_eq!(d.items.len(), 1);
    assert_eq!(move_item(&d2, &id, 0).items[0].id(), id);
    let text_id = d.items[0].id().to_string();
    let edited = update_item(&d, &text_id, |i| {
        if let Item::Text(t) = i {
            t.text = "Hi".into();
        }
    });
    let Item::Text(t) = &edited.items[0] else {
        panic!()
    };
    assert_eq!(t.text, "Hi");
    assert!(remove_item(&d, &text_id).items.is_empty());
    assert_eq!(update_doc(&d, |x| x.name = "x".into()).name, "x");
    assert_eq!(d, before);
}

#[test]
fn update_item_keeps_id_and_kind() {
    let d = LabelDoc::new();
    let id = d.items[0].id().to_string();
    let out = update_item(&d, &id, |i| *i = Item::new(ItemKind::Icon));
    assert_eq!(out.items[0].kind(), ItemKind::Text);
    let out = update_item(&d, &id, |i| i.set_id("other".into()));
    assert_eq!(out.items[0].id(), id);
}

#[test]
fn update_doc_cannot_change_identity_or_items() {
    let d = LabelDoc::new();
    let out = update_doc(&d, |x| {
        x.id = "zzz".into();
        x.items.clear();
        x.created_at = "never".into();
        x.tape.width_mm = 12.0;
    });
    assert_eq!(
        (out.id.as_str(), out.items.len(), out.created_at.as_str()),
        (d.id.as_str(), 1, d.created_at.as_str())
    );
    assert_eq!(out.tape.width_mm, 12.0);
}

#[test]
fn update_item_drops_clip_tall_when_the_text_size_changes_and_only_then() {
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        unreachable!()
    };
    t.size = TextSize::Mm { mm: 9.0 };
    t.clip_tall = true;
    let id = t.id.clone();
    let d = LabelDoc::with_items(vec![Item::Text(t)]);
    let clip = |doc: &LabelDoc| match &doc.items[0] {
        Item::Text(t) => t.clip_tall,
        _ => panic!(),
    };
    let text = |s: &str| {
        let s = s.to_string();
        move |i: &mut Item| {
            if let Item::Text(t) = i {
                t.text = s;
            }
        }
    };
    assert!(clip(&update_item(&d, &id, text("Hi"))));
    assert!(clip(&update_item(&d, &id, |i| {
        if let Item::Text(t) = i {
            t.font_family = "oswald".into();
        }
    })));
    let resized = update_item(&d, &id, |i| {
        if let Item::Text(t) = i {
            t.size = TextSize::Mm { mm: 4.0 };
        }
    });
    let Item::Text(r) = &resized.items[0] else {
        panic!()
    };
    assert_eq!((r.size, r.clip_tall), (TextSize::Mm { mm: 4.0 }, false));
    assert!(clip(&d));
}

#[test]
fn add_item_inserts_after_the_given_item() {
    let (d1, a) = add_item(&LabelDoc::with_items(vec![]), ItemKind::Text, None);
    let (d2, b) = add_item(&d1, ItemKind::Icon, None);
    let (d3, c) = add_item(&d2, ItemKind::Code, Some(&a));
    assert_eq!(
        d3.items.iter().map(Item::id).collect::<Vec<_>>(),
        [a.as_str(), &c, &b]
    );
    // An unknown id appends.
    let (d4, e) = add_item(&d3, ItemKind::Spacer, Some("missing"));
    assert_eq!(d4.items.last().unwrap().id(), e);
}

#[test]
fn move_item_clamps_and_ignores_unknown_ids() {
    let (d, a) = add_item(&LabelDoc::new(), ItemKind::Icon, None);
    let moved = move_item(&d, &a, 99);
    assert_eq!(moved.items[1].id(), a);
    let moved = move_item(&d, &a, 0);
    assert_eq!(moved.items[0].id(), a);
    assert_eq!(move_item(&d, "missing", 0), d);
}

#[test]
fn duplicate_item_deep_copies_right_after_the_source_with_a_new_id() {
    let mut icon = Item::new(ItemKind::Icon);
    *icon.frame_mut() = Some(ItemFrame {
        x_mm: 1.0,
        y_mm: 1.0,
        w_mm: 5.0,
        h_mm: 5.0,
        rotation: 0,
    });
    let base = LabelDoc::with_items(vec![Item::new(ItemKind::Text), icon]);
    let first = base.items[0].clone();
    let (d, id) = duplicate_item(&base, first.id());
    assert_eq!(d.items.len(), 3);
    assert_eq!(d.items[1].id(), id);
    assert_ne!(id, first.id());
    let mut copy = d.items[1].clone();
    copy.set_id(first.id().into());
    assert_eq!(copy, first);
    let (d2, id2) = duplicate_item(&base, base.items[1].id());
    assert_eq!(
        d2.items[2].frame(),
        Some(&ItemFrame {
            x_mm: 3.0,
            y_mm: 3.0,
            w_mm: 5.0,
            h_mm: 5.0,
            rotation: 0
        })
    );
    assert_eq!(d2.items[2].id(), id2);
    assert_eq!(
        duplicate_item(&base, "missing"),
        (base.clone(), String::new())
    );
}

#[test]
fn undo_redo_walk_the_snapshots_and_a_new_edit_clears_redo() {
    let [a, b, c, d]: [LabelDoc; 4] = docs(4).try_into().unwrap();
    let mut h = History::with_default_capacity(a.clone());
    assert!(!h.can_undo());
    h.push(b.clone(), None, 0);
    h.push(c, None, 0);
    assert_eq!(h.undo(), Some(&b));
    assert_eq!(h.undo(), Some(&a));
    assert_eq!(h.undo(), None);
    assert!(h.can_redo());
    assert_eq!(h.redo(), Some(&b));
    h.push(d.clone(), None, 0);
    assert!(!h.can_redo());
    assert_eq!(h.redo(), None);
    assert_eq!(h.present(), &d);
    assert_eq!(h.undo(), Some(&b));
}

#[test]
fn coalesces_edits_with_the_same_key_within_1s() {
    let [a, b, c, d, e]: [LabelDoc; 5] = docs(5).try_into().unwrap();
    let mut h = History::new(a.clone(), 200);
    let mut t = 0;
    h.push(b, Some("text:1"), t);
    t += 300;
    h.push(c.clone(), Some("text:1"), t); // merged into b's step
    t += 300;
    h.push(d.clone(), Some("text:2"), t); // other key → new step
    t += COALESCE_MS + 1;
    h.push(e, Some("text:2"), t); // too late → new step
    assert_eq!(h.undo(), Some(&d));
    assert_eq!(h.undo(), Some(&c));
    assert_eq!(h.undo(), Some(&a));
    assert!(!h.can_undo());
}

#[test]
fn does_not_coalesce_across_an_undo() {
    let [a, b, c]: [LabelDoc; 3] = docs(3).try_into().unwrap();
    let mut h = History::new(a.clone(), 200);
    h.push(b, Some("k"), 0);
    h.undo();
    h.push(c, Some("k"), 10);
    assert_eq!(h.undo(), Some(&a));
}

#[test]
fn keeps_at_most_capacity_snapshots() {
    let all = docs(10);
    let mut h = History::new(all[0].clone(), 4);
    for d in &all[1..] {
        h.push(d.clone(), None, 0);
    }
    let mut seen = Vec::new();
    while let Some(d) = h.undo() {
        seen.push(d.name.clone());
    }
    assert_eq!(seen, ["v8", "v7", "v6"]);
}

#[test]
fn reset_forgets_everything() {
    let [a, b, c]: [LabelDoc; 3] = docs(3).try_into().unwrap();
    let mut h = History::with_default_capacity(a);
    h.push(b, None, 0);
    h.reset(c);
    assert!(!h.can_undo() && !h.can_redo());
    assert_eq!(h.undo(), None);
}

#[test]
fn pushing_the_present_again_is_a_no_op() {
    let a = LabelDoc::new();
    let mut h = History::with_default_capacity(a.clone());
    h.push(a, None, 0);
    assert!(!h.can_undo());
}
