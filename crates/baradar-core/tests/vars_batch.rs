//! Port of web/tests/unit/ui/batch-model.test.ts (batch-model.ts and batch-job.ts checks).

use baradar_core::doc::schema::{BatchCounter, BatchData, DateFormat, Item, ItemKind, LabelDoc};
use baradar_core::vars::batch::*;
use baradar_core::vars::variables::{
    BATCH_CELL_CHARS, BATCH_COUNTERS, BATCH_ROWS, COUNTER_PAD_MAX,
};
use time::macros::date;

fn text(t: &str) -> Item {
    let mut i = Item::new(ItemKind::Text);
    if let Item::Text(x) = &mut i {
        x.text = t.into();
    }
    i
}

fn doc(items: Vec<Item>, batch: Option<BatchData>, copies: u32) -> LabelDoc {
    let mut d = LabelDoc::with_items(items);
    d.batch = batch;
    d.print.copies = copies;
    d
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn table() -> BatchData {
    BatchData {
        enabled: true,
        columns: strs(&["name", "room"]),
        rows: vec![strs(&["Ada", "Lab 1"]), strs(&["Grace", ""])],
        ..BatchData::default()
    }
}

fn counting(count: u32) -> BatchData {
    BatchData {
        enabled: true,
        count,
        ..BatchData::default()
    }
}

/// `estimateTape(pages, feed, {leader}).totalMm`.
fn estimate(pages: &[f64], feed: f64, leader: bool) -> f64 {
    pages.iter().map(|p| p + 2.0 * feed).sum::<f64>()
        + if leader && !pages.is_empty() {
            24.0
        } else {
            0.0
        }
}

#[test]
fn set_cell_replaces_one_cell() {
    let mut b = table();
    assert!(set_cell(&mut b, 1, 1, "Lab 2"));
    assert_eq!(
        b.rows,
        vec![strs(&["Ada", "Lab 1"]), strs(&["Grace", "Lab 2"])]
    );
    assert!(!set_cell(&mut b, 0, 0, "Ada"));
    assert!(!set_cell(&mut b, 5, 0, "x"));
    assert!(!set_cell(&mut b, 0, 5, "x"));
    assert!(set_cell(&mut b, 0, 0, &"z".repeat(600)));
    assert_eq!(b.rows[0][0].len(), BATCH_CELL_CHARS);
}

#[test]
fn adds_and_removes_rows_up_to_the_cap() {
    let mut b = table();
    assert!(add_row(&mut b, None));
    assert_eq!(
        b.rows,
        vec![
            strs(&["Ada", "Lab 1"]),
            strs(&["Grace", ""]),
            strs(&["", ""])
        ]
    );
    let mut b = table();
    add_row(&mut b, Some(0));
    assert_eq!(b.rows[1], strs(&["", ""]));
    let mut b = table();
    assert!(remove_row(&mut b, 0));
    assert_eq!(b.rows, vec![strs(&["Grace", ""])]);
    assert!(!remove_row(&mut b, 3));
    let mut full = table();
    full.rows = vec![strs(&["a", "b"]); BATCH_ROWS];
    assert!(!add_row(&mut full, None));
    let mut empty = BatchData::default();
    assert!(!add_row(&mut empty, None));
    assert_eq!(empty, BatchData::default());
}

#[test]
fn adds_columns_with_free_names_and_removes_them() {
    let mut one = BatchData::default();
    add_column(&mut one, None);
    assert_eq!(one.columns, strs(&["col1"]));
    assert_eq!(one.rows, vec![strs(&[""])]);
    let mut b = table();
    add_column(&mut b, None);
    assert_eq!(
        b.rows,
        vec![strs(&["Ada", "Lab 1", ""]), strs(&["Grace", "", ""])]
    );
    let mut b = table();
    add_column(&mut b, Some("id"));
    assert_eq!(b.columns, strs(&["name", "room", "id"]));
    let mut b = table();
    add_column(&mut b, Some("name"));
    assert_eq!(b.columns, strs(&["name", "room", "name_2"]));
    assert_eq!(free_name(&BatchData::default(), "n"), "n_2");
    let mut b = table();
    remove_column(&mut b, 0);
    assert_eq!(b.columns, strs(&["room"]));
    assert_eq!(b.rows, vec![strs(&["Lab 1"]), strs(&[""])]);
    remove_column(&mut b, 0);
    assert!(b.columns.is_empty() && b.rows.is_empty());
}

#[test]
fn renames_only_to_valid_free_names() {
    let mut b = table();
    assert!(rename_column(&mut b, 0, "who"));
    assert_eq!(b.columns, strs(&["who", "room"]));
    let mut b = table();
    assert!(!rename_column(&mut b, 0, "room"));
    assert!(!rename_column(&mut b, 0, "n"));
    assert!(!rename_column(&mut b, 0, "1x"));
    assert!(
        name_problem(&b, "1x", Some(0), None)
            .unwrap()
            .contains("letters, digits")
    );
    assert!(
        name_problem(&b, "room", Some(0), None)
            .unwrap()
            .contains("column is already")
    );
    assert!(
        name_problem(&b, "n", Some(0), None)
            .unwrap()
            .contains("counter is already")
    );
    assert_eq!(name_problem(&b, "name", Some(0), None), None);
}

#[test]
fn replace_table_renames_counters_that_clash() {
    let mut b = BatchData::default();
    replace_table(&mut b, strs(&["n", "name"]), vec![strs(&["1", "a"])]);
    assert_eq!(b.columns, strs(&["n", "name"]));
    assert_eq!(
        b.counters
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["n_2"]
    );
}

#[test]
fn counters_add_update_remove() {
    let mut b = BatchData::default();
    add_counter(&mut b);
    assert_eq!(
        b.counters
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["n", "n_2"]
    );
    add_counter(&mut b);
    add_counter(&mut b);
    assert!(!add_counter(&mut b));
    assert_eq!(b.counters.len(), BATCH_COUNTERS);

    let mut u = b.clone();
    update_counter(
        &mut u,
        0,
        &CounterPatch {
            start: Some(5.4),
            step: Some(-2.0),
            pad: Some(99.0),
            ..CounterPatch::default()
        },
    );
    assert_eq!(
        u.counters[0],
        BatchCounter {
            name: "n".into(),
            start: 5,
            step: -2,
            pad: COUNTER_PAD_MAX
        }
    );
    let rename = |name: &str| {
        let mut x = b.clone();
        update_counter(
            &mut x,
            0,
            &CounterPatch {
                name: Some(name.into()),
                ..CounterPatch::default()
            },
        );
        x.counters[0].name.clone()
    };
    assert_eq!(rename("n_2"), "n");
    assert_eq!(rename("id"), "id");
    let mut x = b.clone();
    update_counter(
        &mut x,
        0,
        &CounterPatch {
            start: Some(f64::NAN),
            ..CounterPatch::default()
        },
    );
    assert_eq!(x.counters[0].start, 1);
    let mut x = b.clone();
    update_counter(
        &mut x,
        0,
        &CounterPatch {
            start: Some(5e12),
            ..CounterPatch::default()
        },
    );
    assert_eq!(x.counters[0].start, 1_000_000_000);
    let mut x = b.clone();
    assert!(remove_counter(&mut x, 0));
    assert_eq!(x.counters.len(), 3);
}

#[test]
fn counter_example_shows_a_live_example() {
    let c = |start, step, pad| BatchCounter {
        name: "n".into(),
        start,
        step,
        pad,
    };
    assert_eq!(counter_example(&c(1, 1, 3), 3), "001, 002, 003, …");
    assert_eq!(counter_example(&c(10, -5, 0), 3), "10, 5, 0, …");
}

#[test]
fn date_format_labels_and_examples() {
    let now = date!(2026 - 10 - 08);
    assert_eq!(
        date_format_label(DateFormat::Dmy, now, None),
        "Day/month/year (08/10/2026)"
    );
    assert_eq!(
        date_format_label(DateFormat::Long, now, Some("en-US")),
        "Written out (October 8, 2026)"
    );
    let ex = date_examples(&BatchData::default(), now, None);
    assert_eq!(
        ex,
        vec![
            DateExample {
                name: "today",
                value: "2026-10-08".into()
            },
            DateExample {
                name: "today+30d",
                value: "2026-11-07".into()
            },
        ]
    );
}

#[test]
fn flags_empty_cells_only_in_used_columns() {
    let mut b = table();
    b.rows = vec![strs(&["", "Lab 1"]), strs(&["Grace", " "])];
    let d = doc(vec![text("{{name}} {{room}}")], Some(b), 1);
    assert_eq!(
        empty_used_cells(&d).into_iter().collect::<Vec<_>>(),
        [(0, 0), (1, 1)]
    );
    assert!(empty_used_cells(&doc(vec![text("{{name}}")], Some(table()), 1)).is_empty());
    assert!(empty_used_cells(&doc(vec![text("x")], None, 1)).is_empty());
}

#[test]
fn captions_rows_with_the_first_used_value() {
    let d = doc(vec![text("{{room}}")], Some(table()), 1);
    assert_eq!(row_caption(&d, 0), "Label 1: Lab 1");
    assert_eq!(row_caption(&d, 1), "Label 2: Grace");
    assert_eq!(row_caption(&doc(vec![text("x")], None, 1), 3), "Label 4");
    let mut b = table();
    b.rows = vec![vec!["x".repeat(40), String::new()]];
    assert_eq!(
        row_caption(&doc(vec![text("{{name}}")], Some(b), 1), 0),
        format!("Label 1: {}…", "x".repeat(23))
    );
}

#[test]
fn batch_blocker_missing_variables_and_page_cap() {
    assert!(
        batch_blocker(&doc(vec![text("x")], None, 1))
            .unwrap()
            .contains("off")
    );
    assert_eq!(
        batch_blocker(&doc(vec![text("{{name}} {{nmae}}")], Some(table()), 1)).as_deref(),
        Some(
            "Unknown variable {{nmae}}: add a column or counter with that name, or fix the spelling."
        )
    );
    assert_eq!(
        batch_blocker(&doc(vec![text("{{a}} {{b}}")], Some(table()), 1)).as_deref(),
        Some(
            "Unknown variables {{a}}, {{b}}: add a column or counter with that name, or fix the spelling."
        )
    );
    assert_eq!(
        batch_blocker(&doc(vec![text("{{name}}")], Some(table()), 1)),
        None
    );
    let many = doc(vec![text("{{n}}")], Some(counting(100)), 10);
    assert_eq!(
        batch_blocker(&many).as_deref(),
        Some(
            "100 labels × 10 copies is 1000 labels; one print job holds at most 999. Lower the copies or split the data."
        )
    );
    assert_eq!(
        batch_blocker(&doc(vec![text("{{n}}")], Some(counting(99)), 10)),
        None
    );
    assert_eq!(placeholder_list(&["a", "b"]), "{{a}}, {{b}}");
}

#[test]
fn batch_pages_repeat_copies_per_label() {
    assert_eq!(
        batch_pages(&doc(vec![text("{{n}}")], Some(counting(3)), 2)),
        [0, 0, 1, 1, 2, 2]
    );
    assert_eq!(
        batch_pages(&doc(vec![text("{{name}}")], Some(table()), 1)),
        [0, 1]
    );
    assert!(batch_pages(&doc(vec![text("x")], None, 3)).is_empty());
    // Copies are clamped to 1–99 as the print does.
    assert_eq!(
        batch_pages(&doc(vec![text("x")], Some(counting(1)), 0)),
        [0]
    );
}

#[test]
fn clamp_row_keeps_the_preview_row_inside_the_batch() {
    assert_eq!(clamp_row(&doc(vec![text("x")], None, 1), 5), 0);
    assert_eq!(clamp_row(&doc(vec![text("x")], Some(table()), 1), 5), 1);
    assert_eq!(
        clamp_row(&doc(vec![text("x")], Some(counting(10)), 1), 7),
        7
    );
    assert_eq!(
        clamp_row(&doc(vec![text("x")], Some(counting(10)), 1), -3),
        0
    );
    let mut off = table();
    off.enabled = false;
    assert_eq!(clamp_row(&doc(vec![text("x")], Some(off), 1), 1), 1);
}

#[test]
fn pages() {
    assert_eq!(page_count(0, 12), 1);
    assert_eq!(page_count(25, 12), 3);
    assert_eq!(page_of(11, 12), 0);
    assert_eq!(page_of(12, 12), 1);
}

#[test]
fn batch_tape_is_estimate_tape_over_labels_times_copies() {
    let all = batch_tape(&[Some(30.0), Some(40.0), Some(50.0)], 3, 2, 2.0, true, 99.0);
    assert!(all.exact);
    assert_eq!(
        all.total_mm,
        estimate(&[30.0, 30.0, 40.0, 40.0, 50.0, 50.0], 2.0, true)
    );
    assert_eq!(all.total_mm, 2.0 * (30.0 + 40.0 + 50.0) + 6.0 * 4.0 + 24.0);
    let part = batch_tape(&[Some(30.0), None, Some(50.0)], 3, 1, 2.0, false, 99.0);
    assert!(!part.exact);
    assert_eq!(part.total_mm, 30.0 + 40.0 + 50.0 + 3.0 * 4.0);
    assert_eq!(
        batch_tape(&[], 2, 1, 2.0, false, 25.0),
        BatchTape {
            total_mm: 2.0 * (25.0 + 4.0),
            exact: false
        }
    );
}

#[test]
fn single_tape_note_uses_the_print_formula() {
    let with_leader = estimate(&[50.0, 50.0], 2.0, true);
    assert_eq!(with_leader, 2.0 * 54.0 + 24.0);
    assert_eq!(
        single_tape_note(50.0, 2.0, 2, true),
        format!(
            "Uses about {} of tape incl. the ~24 mm leader.",
            format_tape(with_leader)
        )
    );
    assert_eq!(
        single_tape_note(50.0, 2.0, 2, false),
        "Uses about 108 mm of tape."
    );
    assert_eq!(
        single_tape_note(50.0, 2.0, 0, false),
        "Uses about 54 mm of tape."
    );
}

#[test]
fn formats_tape_and_the_print_button() {
    assert_eq!(format_tape(86.4), "86 mm");
    assert_eq!(format_tape(1234.0), "1.23 m");
    assert_eq!(format_tape(1125.0), "1.13 m"); // toFixed rounds the tie up
    assert_eq!(batch_print_text(25, 1), "Print 25 labels");
    assert_eq!(batch_print_text(3, 2), "Print 6 labels");
    assert_eq!(batch_print_text(1, 1), "Print label");
}
