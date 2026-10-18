//! Batch helpers (port of `web/src/ui/batch/batch-model.ts` and the pure parts of
//! `web/src/ui/batch/batch-job.ts`): table and counter edits on [`BatchData`], empty-cell
//! checks, copy for counters and dates, preview-grid paging, the print bar's numbers, and which
//! labels and pages a batch print job holds.
//!
//! The TS edits return a new `BatchData` (the same object when nothing changes); here they edit
//! in place and return `true` when something changed.

use std::collections::{BTreeSet, HashSet};

use time::Date;

use super::variables::{
    BATCH_CELL_CHARS, BATCH_COLUMNS, BATCH_COUNTERS, BATCH_ROWS, COUNTER_PAD_MAX, COUNTER_PAD_MIN,
    COUNTER_VALUE, add_days, batch_size, counter_value, format_date, is_variable_name, js_to_fixed,
    js_trim, missing_variables, used_columns, utf16_len, utf16_prefix,
};
use crate::doc::schema::{BatchCounter, BatchData, DateFormat, LabelDoc};
use crate::render::units::js_round;

/// The doc's batch data, or the defaults (off). `batchOf`.
pub fn batch_of(doc: &LabelDoc) -> BatchData {
    doc.batch.clone().unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Table edits
// ---------------------------------------------------------------------------------------------

/// Sets one cell (cut to [`BATCH_CELL_CHARS`]). `setCell`.
pub fn set_cell(b: &mut BatchData, row: usize, col: usize, value: &str) -> bool {
    if col >= b.columns.len() {
        return false;
    }
    let v = utf16_prefix(value, BATCH_CELL_CHARS);
    let Some(r) = b.rows.get_mut(row) else {
        return false;
    };
    // Rows are normally as long as `columns`; a short one is padded rather than ignored.
    if r.len() <= col {
        r.resize(col + 1, String::new());
    }
    if r[col] == v {
        return false;
    }
    r[col] = v.to_owned();
    true
}

/// Adds an empty row after `after` (`None`: at the end). Unchanged at the row cap. `addRow`.
pub fn add_row(b: &mut BatchData, after: Option<usize>) -> bool {
    if b.rows.len() >= BATCH_ROWS || b.columns.is_empty() {
        return false;
    }
    let at = after.map_or(b.rows.len(), |a| a.saturating_add(1).min(b.rows.len()));
    b.rows.insert(at, vec![String::new(); b.columns.len()]);
    true
}

/// `removeRow`.
pub fn remove_row(b: &mut BatchData, row: usize) -> bool {
    if row >= b.rows.len() {
        return false;
    }
    b.rows.remove(row);
    true
}

/// A column name not yet used by a column or counter (`col1`, `col2`, …, or `base`, `base_2`).
/// `freeName`.
pub fn free_name(b: &BatchData, base: &str) -> String {
    let taken: HashSet<&str> = b
        .columns
        .iter()
        .map(String::as_str)
        .chain(b.counters.iter().map(|c| c.name.as_str()))
        .collect();
    if base != "col" && !taken.contains(base) && is_variable_name(base) {
        return base.to_owned();
    }
    let mut k = if base == "col" { 1 } else { 2 };
    loop {
        let name = if base == "col" {
            format!("col{k}")
        } else {
            format!("{base}_{k}")
        };
        if !taken.contains(name.as_str()) {
            return name;
        }
        k += 1;
    }
}

/// Adds a column (a fresh name from `name`, default `col`; empty cells; a first row when there is
/// none). `addColumn`.
pub fn add_column(b: &mut BatchData, name: Option<&str>) -> bool {
    if b.columns.len() >= BATCH_COLUMNS {
        return false;
    }
    let col = free_name(b, name.unwrap_or("col"));
    if b.rows.is_empty() {
        b.rows.push(vec![String::new(); b.columns.len() + 1]);
    } else {
        for r in &mut b.rows {
            r.push(String::new());
        }
    }
    b.columns.push(col);
    true
}

/// Removes a column; removing the last one clears the rows. `removeColumn`.
pub fn remove_column(b: &mut BatchData, col: usize) -> bool {
    if col >= b.columns.len() {
        return false;
    }
    b.columns.remove(col);
    if b.columns.is_empty() {
        b.rows.clear();
    } else {
        for r in &mut b.rows {
            if col < r.len() {
                r.remove(col);
            }
        }
    }
    true
}

/// Why `name` cannot name column `col` (or the counter at `counter`), or `None` when it can.
/// `nameProblem`.
pub fn name_problem(
    b: &BatchData,
    name: &str,
    col: Option<usize>,
    counter: Option<usize>,
) -> Option<String> {
    if !is_variable_name(name) {
        return Some(
            "Use letters, digits and _ (not starting with a digit), up to 32 characters.".into(),
        );
    }
    if b.columns
        .iter()
        .enumerate()
        .any(|(j, c)| c == name && Some(j) != col)
    {
        return Some(format!("A column is already called {name}."));
    }
    if b.counters
        .iter()
        .enumerate()
        .any(|(k, c)| c.name == name && Some(k) != counter)
    {
        return Some(format!("A counter is already called {name}."));
    }
    None
}

/// Renames column `col` when the name is valid and free; otherwise unchanged. `renameColumn`.
pub fn rename_column(b: &mut BatchData, col: usize, name: &str) -> bool {
    if col >= b.columns.len()
        || b.columns[col] == name
        || name_problem(b, name, Some(col), None).is_some()
    {
        return false;
    }
    b.columns[col] = name.to_owned();
    true
}

/// Replaces the table with a parsed one (counters whose names clash with a column are renamed).
/// `replaceTable`.
pub fn replace_table(b: &mut BatchData, columns: Vec<String>, rows: Vec<Vec<String>>) {
    let mut taken: HashSet<String> = columns.iter().cloned().collect();
    for c in &mut b.counters {
        if !taken.contains(&c.name) {
            taken.insert(c.name.clone());
            continue;
        }
        let mut name = c.name.clone();
        let mut k = 2usize;
        while taken.contains(&name) {
            let keep = 32usize.saturating_sub(k.to_string().len() + 1);
            name = format!("{}_{k}", utf16_prefix(&c.name, keep));
            k += 1;
        }
        taken.insert(name.clone());
        c.name = name;
    }
    b.columns = columns;
    b.rows = rows;
}

// ---------------------------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------------------------

/// Adds a counter `n` (or `n_2`, …) counting 1, 2, 3, … Unchanged at the cap. `addCounter`.
pub fn add_counter(b: &mut BatchData) -> bool {
    if b.counters.len() >= BATCH_COUNTERS {
        return false;
    }
    let name = free_name(b, "n");
    b.counters.push(BatchCounter {
        name,
        start: 1,
        step: 1,
        pad: 0,
    });
    true
}

/// The fields [`update_counter`] changes (`Partial<BatchCounter>`). Numbers are as typed: they are
/// rounded and clamped, and a non-finite one keeps the old value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CounterPatch {
    pub name: Option<String>,
    pub start: Option<f64>,
    pub step: Option<f64>,
    pub pad: Option<f64>,
}

/// Updates counter `k`: integers clamped to ±[`COUNTER_VALUE`] (pad 0–12); an invalid or taken
/// name is ignored. `updateCounter`.
pub fn update_counter(b: &mut BatchData, k: usize, patch: &CounterPatch) -> bool {
    let Some(c) = b.counters.get(k).cloned() else {
        return false;
    };
    let mut next = c.clone();
    if let Some(name) = &patch.name
        && *name != c.name
        && name_problem(b, name, None, Some(k)).is_none()
    {
        next.name = name.clone();
    }
    let lim = COUNTER_VALUE as f64;
    next.start = clamp_int(
        patch.start.unwrap_or(c.start as f64),
        -lim,
        lim,
        c.start as f64,
    ) as i64;
    next.step = clamp_int(
        patch.step.unwrap_or(c.step as f64),
        -lim,
        lim,
        c.step as f64,
    ) as i64;
    next.pad = clamp_int(
        patch.pad.unwrap_or(f64::from(c.pad)),
        f64::from(COUNTER_PAD_MIN),
        f64::from(COUNTER_PAD_MAX),
        f64::from(c.pad),
    ) as u8;
    if next == c {
        return false;
    }
    b.counters[k] = next;
    true
}

/// `removeCounter`.
pub fn remove_counter(b: &mut BatchData, k: usize) -> bool {
    if k >= b.counters.len() {
        return false;
    }
    b.counters.remove(k);
    true
}

fn clamp_int(v: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if !v.is_finite() {
        return fallback;
    }
    (js_round(v) as f64).clamp(min, max)
}

/// "001, 002, 003, …" for a counter's first `n` labels (the TS default is 3). `counterExample`.
pub fn counter_example(c: &BatchCounter, n: usize) -> String {
    let values: Vec<String> = (0..n).map(|i| counter_value(c, i)).collect();
    format!("{}, …", values.join(", "))
}

/// The date format picker's options. `DATE_FORMATS`.
pub const DATE_FORMATS: [(DateFormat, &str); 4] = [
    (DateFormat::Iso, "Year-month-day"),
    (DateFormat::Dmy, "Day/month/year"),
    (DateFormat::Mdy, "Month/day/year"),
    (DateFormat::Long, "Written out"),
];

/// "Day/month/year (08/10/2026)" for the date format picker. `dateFormatLabel`.
pub fn date_format_label(f: DateFormat, today: Date, locale: Option<&str>) -> String {
    let label = DATE_FORMATS
        .iter()
        .find(|(v, _)| *v == f)
        .map_or("", |(_, l)| *l);
    format!("{label} ({})", format_date(today, f, locale))
}

/// An example value of a date built-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateExample {
    pub name: &'static str,
    pub value: String,
}

/// Example values of the date built-ins, for the help text. `dateExamples`.
pub fn date_examples(b: &BatchData, today: Date, locale: Option<&str>) -> Vec<DateExample> {
    vec![
        DateExample {
            name: "today",
            value: format_date(today, b.date_format, locale),
        },
        DateExample {
            name: "today+30d",
            value: format_date(add_days(today, 30), b.date_format, locale),
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------------------------

/// Cells that are empty in a column the label uses, as (row, col), 0-based. `emptyUsedCells`
/// (the TS returns `"row,col"` strings).
pub fn empty_used_cells(doc: &LabelDoc) -> BTreeSet<(usize, usize)> {
    let mut out = BTreeSet::new();
    let Some(b) = &doc.batch else {
        return out;
    };
    let used = used_columns(doc);
    let cols: Vec<usize> = b
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| used.contains(c))
        .map(|(j, _)| j)
        .collect();
    for (i, r) in b.rows.iter().enumerate() {
        for &j in &cols {
            if js_trim(r.get(j).map_or("", String::as_str)).is_empty() {
                out.insert((i, j));
            }
        }
    }
    out
}

/// "Label 3" + the first non-empty used cell (or first cell) as a short caption. `rowCaption`.
pub fn row_caption(doc: &LabelDoc, row: usize) -> String {
    let plain = format!("Label {}", row + 1);
    let Some(b) = &doc.batch else {
        return plain;
    };
    let Some(r) = b.rows.get(row) else {
        return plain;
    };
    let cell = |k: usize| r.get(k).map_or("", String::as_str);
    let used = used_columns(doc);
    let j = used
        .iter()
        .filter_map(|c| b.columns.iter().position(|x| x == c))
        .find(|&k| !js_trim(cell(k)).is_empty());
    let v = match j {
        Some(j) => cell(j),
        None => r
            .iter()
            .find(|c| !js_trim(c).is_empty())
            .map_or("", String::as_str),
    };
    if v.is_empty() {
        return plain;
    }
    if utf16_len(v) > 24 {
        format!("{plain}: {}…", utf16_prefix(v, 23))
    } else {
        format!("{plain}: {v}")
    }
}

// ---------------------------------------------------------------------------------------------
// Preview grid paging, print bar
// ---------------------------------------------------------------------------------------------

/// Thumbnails per page (keeps the renders bounded at 500 rows). `PAGE_SIZE`.
pub const PAGE_SIZE: usize = 12;
/// Table rows per page. `TABLE_PAGE_SIZE`.
pub const TABLE_PAGE_SIZE: usize = 20;

/// `pageCount`.
pub fn page_count(total: usize, size: usize) -> usize {
    total.div_ceil(size.max(1)).max(1)
}

/// Page (0-based) that shows `row`. `pageOf`.
pub fn page_of(row: usize, size: usize) -> usize {
    row / size.max(1)
}

/// The preview row clamped to the batch (0 without a batch). `clampRow`.
pub fn clamp_row(doc: &LabelDoc, row: i64) -> usize {
    let n = match &doc.batch {
        Some(b) => batch_size(doc).max(b.rows.len()).max(1),
        None => 1,
    };
    usize::try_from(row.max(0)).unwrap_or(0).min(n - 1)
}

/// Tape for a batch. `BatchTape`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchTape {
    pub total_mm: f64,
    /// Every label was measured (otherwise extrapolated from the measured ones).
    pub exact: bool,
}

/// `render/job.ts LEADER_MM`: tape fed before the first label of a job.
const LEADER_MM: f64 = 24.0;

/// `render/job.ts estimateTape(...).totalMm`: Σ(page + 2 × feed margin) + the leader once.
fn estimate_tape_total(pages_mm: &[f64], feed_margin_mm: f64, leader: bool) -> f64 {
    let feed = if feed_margin_mm.is_finite() {
        feed_margin_mm.max(0.0)
    } else {
        0.0
    };
    let labels: f64 = pages_mm
        .iter()
        .map(|l| if l.is_finite() { l.max(0.0) } else { 0.0 } + 2.0 * feed)
        .sum();
    let leader_mm = if !leader || pages_mm.is_empty() {
        0.0
    } else {
        LEADER_MM
    };
    labels + leader_mm
}

/// Tape for a batch of `labels` × `copies`. `lengths_mm[i]` is label i's measured length or
/// `None`; unmeasured labels count as the average measured one (or `fallback_mm`, e.g. the
/// preview's length). `batchTape`.
pub fn batch_tape(
    lengths_mm: &[Option<f64>],
    labels: usize,
    copies: usize,
    feed_margin_mm: f64,
    leader: bool,
    fallback_mm: f64,
) -> BatchTape {
    let known: Vec<f64> = lengths_mm.iter().take(labels).filter_map(|l| *l).collect();
    let avg = if known.is_empty() {
        fallback_mm
    } else {
        known.iter().sum::<f64>() / known.len() as f64
    };
    let mut pages = Vec::with_capacity(labels * copies);
    for i in 0..labels {
        let l = lengths_mm.get(i).copied().flatten().unwrap_or(avg);
        pages.extend(std::iter::repeat_n(l, copies));
    }
    BatchTape {
        total_mm: estimate_tape_total(&pages, feed_margin_mm, leader),
        exact: known.len() >= labels,
    }
}

/// The print bar note for a plain label (`copies` pages, at least 1). `singleTapeNote`.
pub fn single_tape_note(length_mm: f64, feed_margin_mm: f64, copies: u32, leader: bool) -> String {
    let pages = vec![length_mm; copies.max(1) as usize];
    let total = estimate_tape_total(&pages, feed_margin_mm, leader);
    let incl = if leader {
        " incl. the ~24 mm leader"
    } else {
        ""
    };
    format!("Uses about {} of tape{incl}.", format_tape(total))
}

/// "1.24 m" / "86 mm" for tape totals. `formatTape`.
pub fn format_tape(mm: f64) -> String {
    if mm >= 1000.0 {
        format!("{} m", js_to_fixed(mm / 1000.0, 2))
    } else {
        format!("{} mm", js_round(mm))
    }
}

/// Print button copy for a batch: "Print 25 labels" (labels × copies). `batchPrintText`.
pub fn batch_print_text(labels: usize, copies: usize) -> String {
    let n = labels * copies;
    if n == 1 {
        "Print label".into()
    } else {
        format!("Print {n} labels")
    }
}

// ---------------------------------------------------------------------------------------------
// The print job (batch-job.ts, without rendering and encoding)
// ---------------------------------------------------------------------------------------------

/// Pages one job may hold (the wasm binding's limit). `MAX_BATCH_PAGES`.
pub const MAX_BATCH_PAGES: usize = 999;

/// `LIMITS.copies.max` (`render/job.ts MAX_COPIES`).
const MAX_COPIES: u32 = 99;

/// Copies clamped to 1–99. `render/job.ts copiesOf`.
fn copies_of(doc: &LabelDoc) -> usize {
    doc.print.copies.clamp(1, MAX_COPIES) as usize
}

/// `{{a}}, {{b}}` for messages. `placeholderList`.
pub fn placeholder_list<S: AsRef<str>>(names: &[S]) -> String {
    names
        .iter()
        .map(|n| format!("{{{{{}}}}}", n.as_ref()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why `doc` cannot print as a batch before anything is rendered, or `None`. `batchBlocker`.
pub fn batch_blocker(doc: &LabelDoc) -> Option<String> {
    let n = batch_size(doc);
    if n == 0 {
        return Some("Batch printing is off.".into());
    }
    let missing = missing_variables(doc);
    if !missing.is_empty() {
        let noun = if missing.len() == 1 {
            "variable"
        } else {
            "variables"
        };
        return Some(format!(
            "Unknown {noun} {}: add a column or counter with that name, or fix the spelling.",
            placeholder_list(&missing)
        ));
    }
    let copies = copies_of(doc);
    let pages = n * copies;
    if pages > MAX_BATCH_PAGES {
        return Some(format!(
            "{n} labels × {copies} copies is {pages} labels; one print job holds at most {MAX_BATCH_PAGES}. Lower the copies or split the data."
        ));
    }
    None
}

/// Pages of the batch job in print order, as label indices for [`super::variables::resolve_doc`]:
/// copies per label (A A B B …), `batch_size × copies` pages. Empty when the batch is off.
/// (`buildBatchJob`'s page loop, without rendering; check [`batch_blocker`] first.)
pub fn batch_pages(doc: &LabelDoc) -> Vec<usize> {
    let copies = copies_of(doc);
    (0..batch_size(doc))
        .flat_map(|i| std::iter::repeat_n(i, copies))
        .collect()
}
