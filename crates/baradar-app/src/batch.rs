//! The "Variables & batch" panel under the preview (port of `web/src/ui/batch/BatchPanel.svelte`
//! and the expanded part, `BatchBody.svelte`, with `BatchTable.svelte`, `BatchCounters.svelte`
//! and `BatchPreviews.svelte`).
//!
//! Collapsed by default (its header still shows the label count and unknown variables); opens by
//! itself for a label that prints as a batch. The data table edits one cell at a time with a
//! shared editor: click a cell (or a column name) to edit it.

#[path = "batch_measure.rs"]
mod measure;
pub(crate) use measure::bitmap_image;

use std::ops::Range;

use baradar_core::doc::{BatchCounter, BatchData, DateFormat};
use baradar_core::vars::batch::{
    CounterPatch, DATE_FORMATS, PAGE_SIZE, TABLE_PAGE_SIZE, add_column, add_counter, add_row, batch_of, batch_tape,
    clamp_row, counter_example, date_examples, date_format_label, empty_used_cells, format_tape, name_problem,
    page_count, page_of, remove_column, remove_counter, remove_row, rename_column, replace_table, row_caption,
    set_cell, update_counter,
};
use baradar_core::vars::csv::parse_table;
use baradar_core::vars::variables::{
    BATCH_COLUMNS, BATCH_COUNTERS, BATCH_ROWS, VariableKind, batch_size, find_variables, is_variable_name,
    missing_variables, used_columns, variable_kind,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::studio::{Studio, local_today};

/// Largest file "Open CSV file…" reads (500 rows × 20 columns of text fit easily).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Width of a data cell.
const CELL_W: f32 = 132.0;
/// Width of the row-number column.
const NUM_W: f32 = 64.0;

impl Studio {
    /// Shows batch label `row` in the preview (`studio.previewRow = row`).
    pub fn set_preview_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.preview_row != row {
            self.preview_row = row;
            self.schedule_render(cx);
            cx.notify();
        }
    }

    /// `updateDoc({batch})`: edits the batch data (created with the defaults when missing). Nothing
    /// is recorded when `edit` returns `false` (nothing changed).
    pub fn update_batch(
        &mut self,
        coalesce: Option<&str>,
        edit: impl FnOnce(&mut BatchData) -> bool,
        cx: &mut Context<Self>,
    ) {
        let mut b = batch_of(&self.doc);
        if !edit(&mut b) || self.doc.batch.as_ref() == Some(&b) {
            return;
        }
        self.update_doc(coalesce, move |d| d.batch = Some(b), cx);
    }
}

/// What the shared editor edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    /// A cell (row, column).
    Cell(usize, usize),
    /// A column name.
    Column(usize),
}

/// A counter field (`BatchCounters.svelte`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CounterField {
    Name,
    Start,
    Step,
    Pad,
}

impl CounterField {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Start => "start",
            Self::Step => "step",
            Self::Pad => "pad",
        }
    }

    fn value(self, c: &BatchCounter) -> String {
        match self {
            Self::Name => c.name.clone(),
            Self::Start => c.start.to_string(),
            Self::Step => c.step.to_string(),
            Self::Pad => c.pad.to_string(),
        }
    }
}

const COUNTER_FIELDS: [CounterField; 4] = [CounterField::Name, CounterField::Start, CounterField::Step, CounterField::Pad];

/// The inputs of one counter.
struct CounterInputs {
    fields: [Entity<InputState>; 4],
    /// Why the drafted name cannot be used.
    problem: Option<String>,
    _subs: Vec<Subscription>,
}

pub struct BatchPanel {
    studio: Entity<Studio>,
    /// `batchPanel.open`.
    open: bool,
    last_doc_id: String,
    synced_epoch: u64,
    editing: Option<Edit>,
    /// The shared cell / column-name editor.
    editor: Entity<InputState>,
    /// Why the drafted column name cannot be used.
    name_problem: Option<String>,
    table_page: usize,
    preview_page: usize,
    /// The preview row the grid's page last followed.
    followed_row: usize,
    /// Notices from the last paste or file (they belong to the label they were made for).
    notices: Vec<String>,
    paste: Entity<TextareaState>,
    paste_open: bool,
    count_input: Entity<InputState>,
    counters: Vec<CounterInputs>,
    counters_synced: Vec<BatchCounter>,
    count_synced: u32,
    measure: measure::BatchMeasure,
    _subs: Vec<Subscription>,
}

impl BatchPanel {
    pub fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| InputState::new(window, cx));
        let paste = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder("name,room\nAda,Lab 1\nGrace,Lab 2")
        });
        let count = batch_of(&studio.read(cx).doc).count;
        let count_input = cx.new(|cx| InputState::new(window, cx).default_value(count.to_string()));
        let editor_focus = Focusable::focus_handle(editor.read(cx), cx);
        let weak = cx.entity().downgrade();
        let subs = vec![
            cx.observe_in(&studio, window, |this, _, window, cx| {
                this.sync_from_doc(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&editor, window, |this, state, ev: &InputEvent, window, cx| {
                let value = state.read(cx).value().to_string();
                this.on_editor(&value, ev, window, cx);
            }),
            // "Use pasted data" follows the paste field.
            cx.observe(&paste, |_, _, cx| cx.notify()),
            cx.subscribe_in(&count_input, window, |this, state, ev: &InputEvent, window, cx| {
                this.on_count(state.clone(), ev, window, cx);
            }),
            // ↑ ↓ move between rows, Escape stops editing: the single-line input swallows the
            // arrow keys, so they are intercepted before it sees them.
            cx.intercept_keystrokes(move |ev, window, cx| {
                if ev.keystroke.modifiers.modified() || !editor_focus.is_focused(window) {
                    return;
                }
                let key = ev.keystroke.key.clone();
                if !matches!(key.as_str(), "up" | "down" | "escape") {
                    return;
                }
                if let Some(panel) = weak.upgrade()
                    && panel.update(cx, |p, cx| p.on_cell_key(&key, window, cx))
                {
                    cx.stop_propagation();
                }
            }),
        ];
        let mut this = Self {
            studio,
            open: false,
            last_doc_id: String::new(),
            synced_epoch: u64::MAX,
            editing: None,
            editor,
            name_problem: None,
            table_page: 0,
            preview_page: 0,
            followed_row: usize::MAX,
            notices: Vec::new(),
            paste,
            paste_open: false,
            count_input,
            counters: Vec::new(),
            counters_synced: Vec::new(),
            count_synced: count,
            measure: measure::BatchMeasure::default(),
            _subs: subs,
        };
        this.sync_from_doc(window, cx);
        this
    }

    fn batch(&self, cx: &App) -> BatchData {
        batch_of(&self.studio.read(cx).doc)
    }

    /// `save(b, key)`: one batch edit through the studio.
    fn save(&self, coalesce: Option<&str>, edit: impl FnOnce(&mut BatchData) -> bool, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.update_batch(coalesce, edit, cx));
    }

    // ----------------------------------------------------------------------------------------
    // Following the document
    // ----------------------------------------------------------------------------------------

    /// The effects of `BatchPanel.svelte` / `BatchBody.svelte`: auto-open, clamped preview row,
    /// notices per label, and inputs reloaded when the doc changed elsewhere (undo, open…).
    fn sync_from_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.studio.read(cx);
        let doc_id = s.doc.id.clone();
        let epoch = s.epoch;
        let b = batch_of(&s.doc);
        let row = clamp_row(&s.doc, i64::try_from(s.preview_row).unwrap_or(i64::MAX));
        let preview_row = s.preview_row;
        let enabled = s.doc.batch.as_ref().is_some_and(|b| b.enabled);

        // Opening a label that prints as a batch shows its data.
        if doc_id != self.last_doc_id {
            self.last_doc_id = doc_id;
            if enabled {
                self.open = true;
            }
            self.notices.clear();
            self.editing = None;
            self.name_problem = None;
            self.table_page = 0;
        }
        // The preview row stays inside the batch (rows removed, batch turned off…).
        if row != preview_row {
            self.studio.update(cx, |s, cx| s.set_preview_row(row, cx));
        }
        // The edited cell or column may be gone.
        match self.editing {
            Some(Edit::Cell(r, c)) if r >= b.rows.len() || c >= b.columns.len() => self.editing = None,
            Some(Edit::Column(c)) if c >= b.columns.len() => self.editing = None,
            Some(Edit::Cell(r, c)) if epoch != self.synced_epoch => {
                let v = b.rows.get(r).and_then(|row| row.get(c)).cloned().unwrap_or_default();
                self.editor.update(cx, |e, cx| e.set_value(v, window, cx));
            }
            _ => {}
        }
        let rebuild = epoch != self.synced_epoch || b.counters.len() != self.counters.len();
        self.synced_epoch = epoch;
        if rebuild {
            self.rebuild_counters(&b.counters, window, cx);
        } else if b.counters != self.counters_synced {
            for (k, c) in b.counters.iter().enumerate() {
                let old = self.counters_synced.get(k);
                for (i, field) in COUNTER_FIELDS.iter().enumerate() {
                    let v = field.value(c);
                    if old.is_some_and(|o| field.value(o) == v) {
                        continue;
                    }
                    let input = self.counters[k].fields[i].clone();
                    if !Focusable::focus_handle(input.read(cx), cx).is_focused(window) {
                        input.update(cx, |e, cx| e.set_value(v, window, cx));
                    }
                }
            }
            self.counters_synced = b.counters.clone();
        }
        if b.count != self.count_synced {
            self.count_synced = b.count;
            let input = self.count_input.clone();
            if !Focusable::focus_handle(input.read(cx), cx).is_focused(window) {
                input.update(cx, |e, cx| e.set_value(b.count.to_string(), window, cx));
            }
        }
        self.sync_measure(cx);
        self.sync_thumb_page(cx);
    }

    fn rebuild_counters(&mut self, counters: &[BatchCounter], window: &mut Window, cx: &mut Context<Self>) {
        self.counters = counters
            .iter()
            .enumerate()
            .map(|(k, c)| {
                let fields = COUNTER_FIELDS.map(|f| cx.new(|cx| InputState::new(window, cx).default_value(f.value(c))));
                let subs = COUNTER_FIELDS
                    .iter()
                    .zip(fields.iter())
                    .map(|(&field, input)| {
                        cx.subscribe_in(input, window, move |this, state, ev: &InputEvent, window, cx| {
                            let value = state.read(cx).value().to_string();
                            this.on_counter_input(k, field, &value, ev, window, cx);
                        })
                    })
                    .collect();
                CounterInputs {
                    fields,
                    problem: None,
                    _subs: subs,
                }
            })
            .collect();
        self.counters_synced = counters.to_vec();
    }

    /// The preview grid's page follows the preview row; its thumbnails are rendered first.
    fn sync_thumb_page(&mut self, cx: &mut Context<Self>) {
        let s = self.studio.read(cx);
        let total = batch_size(&s.doc);
        let row = s.preview_row;
        let pages = page_count(total, PAGE_SIZE);
        let first = self.preview_page.min(pages - 1) * PAGE_SIZE;
        if row != self.followed_row {
            self.followed_row = row;
            if row < first || row >= first + PAGE_SIZE {
                self.preview_page = page_of(row, PAGE_SIZE);
            }
        }
        let range = if self.open && total > 0 {
            let first = self.preview_page.min(pages - 1) * PAGE_SIZE;
            first..(first + PAGE_SIZE).min(total)
        } else {
            0..0
        };
        self.want_thumbs(range, cx);
    }

    // ----------------------------------------------------------------------------------------
    // The table (BatchTable.svelte)
    // ----------------------------------------------------------------------------------------

    /// Starts editing a cell or column name: loads its value into the editor and focuses it.
    fn start_edit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.batch(cx);
        let value = match edit {
            Edit::Cell(r, c) => {
                self.table_page = page_of(r, TABLE_PAGE_SIZE);
                b.rows.get(r).and_then(|row| row.get(c)).cloned().unwrap_or_default()
            }
            Edit::Column(c) => b.columns.get(c).cloned().unwrap_or_default(),
        };
        self.editing = Some(edit);
        self.name_problem = None;
        self.editor.update(cx, |e, cx| {
            e.set_value(value, window, cx);
            e.focus(window, cx);
            e.select_all(window, cx);
        });
        cx.notify();
    }

    fn on_editor(&mut self, value: &str, ev: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match (self.editing, ev) {
            (Some(Edit::Cell(r, c)), InputEvent::Change) => {
                let key = format!("batch:cell:{r},{c}");
                let value = value.to_owned();
                self.save(Some(&key), |b| set_cell(b, r, c, &value), cx);
            }
            (Some(Edit::Cell(r, c)), InputEvent::PressEnter { .. }) => {
                // Enter moves down, or adds a row at the end.
                let rows = self.batch(cx).rows.len();
                if r + 1 < rows {
                    self.start_edit(Edit::Cell(r + 1, c), window, cx);
                } else if rows < BATCH_ROWS {
                    self.save(None, |b| add_row(b, None), cx);
                    self.start_edit(Edit::Cell(r + 1, c), window, cx);
                }
            }
            (Some(Edit::Cell(..)), InputEvent::Blur) => {
                self.editing = None;
                cx.notify();
            }
            (Some(Edit::Column(c)), InputEvent::Change) => {
                let b = self.batch(cx);
                let name = value.trim();
                self.name_problem = if b.columns.get(c).is_some_and(|n| n == name) {
                    None
                } else {
                    name_problem(&b, name, Some(c), None)
                };
                cx.notify();
            }
            (Some(Edit::Column(c)), InputEvent::PressEnter { .. } | InputEvent::Blur) => {
                self.commit_column(c, value, window, cx);
            }
            _ => {}
        }
    }

    /// `commitName`: renames the column, or keeps the draft with its problem.
    fn commit_column(&mut self, col: usize, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.batch(cx);
        let name = value.trim().to_owned();
        if b.columns.get(col).is_some_and(|n| *n == name) {
            self.editing = None;
            self.name_problem = None;
        } else if let Some(p) = name_problem(&b, &name, Some(col), None) {
            self.name_problem = Some(p);
        } else {
            self.editing = None;
            self.name_problem = None;
            self.save(None, |b| rename_column(b, col, &name), cx);
            window.push_notification(Notification::new().message(format!("Column renamed to {name}")), cx);
        }
        cx.notify();
    }

    /// ↑ / ↓ between rows and Escape while a cell is edited. Returns whether it was handled.
    fn on_cell_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(edit) = self.editing else { return false };
        if key == "escape" {
            self.editing = None;
            self.name_problem = None;
            cx.notify();
            return true;
        }
        let Edit::Cell(r, c) = edit else { return false };
        let rows = self.batch(cx).rows.len();
        match key {
            "up" if r > 0 => self.start_edit(Edit::Cell(r - 1, c), window, cx),
            "down" if r + 1 < rows => self.start_edit(Edit::Cell(r + 1, c), window, cx),
            _ => return false,
        }
        true
    }

    fn add_column_and_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let col = self.batch(cx).columns.len();
        self.save(None, |b| add_column(b, None), cx);
        if self.batch(cx).columns.len() > col {
            self.start_edit(Edit::Column(col), window, cx);
        }
    }

    fn remove_column_at(&mut self, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.batch(cx).columns.get(col).cloned().unwrap_or_default();
        self.editing = None;
        self.name_problem = None;
        self.save(None, |b| remove_column(b, col), cx);
        window.push_notification(Notification::new().message(format!("Column {name} removed")), cx);
    }

    fn add_row_and_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.batch(cx).rows.len();
        self.save(None, |b| add_row(b, None), cx);
        if self.batch(cx).rows.len() > row {
            self.start_edit(Edit::Cell(row, 0), window, cx);
        }
    }

    fn remove_row_at(&mut self, row: usize, cx: &mut Context<Self>) {
        self.editing = None;
        self.save(None, |b| remove_row(b, row), cx);
        let rows = self.batch(cx).rows.len();
        self.table_page = self.table_page.min(page_count(rows, TABLE_PAGE_SIZE) - 1);
    }

    // ----------------------------------------------------------------------------------------
    // Data (BatchBody.svelte)
    // ----------------------------------------------------------------------------------------

    /// `load(text)`: replaces the table with pasted or opened CSV/TSV.
    fn load(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let t = parse_table(text);
        self.notices = t.notices;
        cx.notify();
        if t.columns.is_empty() {
            return;
        }
        let msg = format!(
            "Loaded {} {} with {} {}: {}",
            t.rows.len(),
            if t.rows.len() == 1 { "row" } else { "rows" },
            t.columns.len(),
            if t.columns.len() == 1 { "column" } else { "columns" },
            t.columns.join(", ")
        );
        self.editing = None;
        self.table_page = 0;
        self.save(
            None,
            move |b| {
                replace_table(b, t.columns, t.rows);
                b.enabled = true;
                true
            },
            cx,
        );
        self.paste.update(cx, |p, cx| p.set_value("", window, cx));
        self.paste_open = false;
        self.studio.update(cx, |s, cx| s.set_preview_row(0, cx));
        window.push_notification(Notification::new().message(msg), cx);
    }

    fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match cx.read_from_clipboard().and_then(|c| c.text()) {
            Some(text) if !text.trim().is_empty() => self.load(&text, window, cx),
            _ => {
                self.notices = vec!["The clipboard holds no text. Copy cells from a spreadsheet first.".into()];
                cx.notify();
            }
        }
    }

    /// `openFile`: [Open CSV file…].
    fn open_csv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.read_csv(path, window, cx)).ok();
            }
        })
        .detach();
    }

    fn read_csv(&mut self, path: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_FILE_BYTES {
            self.notices = vec![format!(
                "“{name}” is too large (over 2 MB). A batch holds at most {BATCH_ROWS} rows."
            )];
            cx.notify();
            return;
        }
        match std::fs::read(&path) {
            Ok(bytes) => self.load(&String::from_utf8_lossy(&bytes), window, cx),
            Err(_) => {
                self.notices = vec![format!("“{name}” could not be read.")];
                cx.notify();
            }
        }
    }

    /// `newTable`: an empty one-column table to type into; its column name is edited.
    fn new_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(
            None,
            |b| {
                add_column(b, None);
                b.enabled = true;
                true
            },
            cx,
        );
        self.start_edit(Edit::Column(0), window, cx);
    }

    fn clear_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.notices.clear();
        self.save(
            None,
            |b| {
                b.columns.clear();
                b.rows.clear();
                true
            },
            cx,
        );
        window.push_notification(Notification::new().message("Data table cleared"), cx);
    }

    fn on_count(&mut self, state: Entity<InputState>, ev: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            InputEvent::Change => {
                if let Ok(n) = state.read(cx).value().trim().parse::<f64>()
                    && n.is_finite()
                {
                    let count = n.round().clamp(1.0, BATCH_ROWS as f64) as u32;
                    self.save(
                        Some("batch:count"),
                        move |b| {
                            b.count = count;
                            true
                        },
                        cx,
                    );
                }
            }
            InputEvent::Blur | InputEvent::PressEnter { .. } => {
                let count = self.batch(cx).count;
                state.update(cx, |e, cx| e.set_value(count.to_string(), window, cx));
            }
            _ => {}
        }
    }

    /// `step(delta)`: the preview stepper.
    fn step(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            let row = clamp_row(&s.doc, i64::try_from(s.preview_row).unwrap_or(0) + delta);
            s.set_preview_row(row, cx);
        });
    }

    // ----------------------------------------------------------------------------------------
    // Counters (BatchCounters.svelte)
    // ----------------------------------------------------------------------------------------

    fn on_counter_input(
        &mut self,
        k: usize,
        field: CounterField,
        value: &str,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = self.batch(cx);
        let Some(current) = b.counters.get(k).cloned() else { return };
        let key = format!("batch:counter:{k}:{}", field.key());
        match (field, ev) {
            (CounterField::Name, InputEvent::Change) => {
                let name = value.trim();
                let problem = if name == current.name { None } else { name_problem(&b, name, None, Some(k)) };
                if let Some(c) = self.counters.get_mut(k) {
                    c.problem = problem;
                }
                cx.notify();
            }
            (CounterField::Name, InputEvent::PressEnter { .. } | InputEvent::Blur) => {
                // `commitName`.
                let name = value.trim().to_owned();
                let problem = if name == current.name { None } else { name_problem(&b, &name, None, Some(k)) };
                let ok = problem.is_none();
                if let Some(c) = self.counters.get_mut(k) {
                    c.problem = problem;
                }
                if ok && name != current.name {
                    let patch = CounterPatch {
                        name: Some(name),
                        ..CounterPatch::default()
                    };
                    self.save(Some(&key), |b| update_counter(b, k, &patch), cx);
                }
                cx.notify();
            }
            (_, InputEvent::Change) => {
                let Ok(n) = value.trim().parse::<f64>() else { return };
                if !n.is_finite() {
                    return;
                }
                let patch = match field {
                    CounterField::Start => CounterPatch {
                        start: Some(n),
                        ..CounterPatch::default()
                    },
                    CounterField::Step => CounterPatch {
                        step: Some(n),
                        ..CounterPatch::default()
                    },
                    _ => CounterPatch {
                        pad: Some(n),
                        ..CounterPatch::default()
                    },
                };
                self.save(Some(&key), |b| update_counter(b, k, &patch), cx);
            }
            (_, InputEvent::Blur | InputEvent::PressEnter { .. }) => {
                // Show the value as stored (rounded and clamped).
                let v = self.batch(cx).counters.get(k).map(|c| field.value(c)).unwrap_or_default();
                if let Some(input) = self.counters.get(k).and_then(|c| COUNTER_FIELDS.iter().position(|f| *f == field).map(|i| c.fields[i].clone())) {
                    input.update(cx, |e, cx| e.set_value(v, window, cx));
                }
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------------------------------
    // Rendering
    // ----------------------------------------------------------------------------------------

    /// `BatchPanel.svelte`'s header: title, badges and the chevron.
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = &self.studio.read(cx).doc;
        let total = batch_size(doc);
        let used = find_variables(doc);
        let missing = missing_variables(doc);
        let badge = |text: String, bg: Hsla, fg: Hsla| {
            div().px_2().rounded_full().bg(bg).text_color(fg).text_xs().child(text)
        };
        h_flex()
            .id("batch-toggle")
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.secondary))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.open = !this.open;
                this.sync_thumb_page(cx);
                cx.notify();
            }))
            .child(IconName::LayoutDashboard)
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Variables & batch"))
            .when(!missing.is_empty(), |d| {
                d.child(badge(format!("{} unknown", missing.len()), theme.danger.opacity(0.15), theme.danger))
            })
            .map(|d| {
                if total > 0 {
                    d.child(badge(
                        format!("{total} {}", if total == 1 { "label" } else { "labels" }),
                        theme.primary.opacity(0.15),
                        theme.foreground,
                    ))
                } else if !used.is_empty() {
                    d.child(badge(
                        format!("{} {}", used.len(), if used.len() == 1 { "variable" } else { "variables" }),
                        theme.secondary,
                        theme.muted_foreground,
                    ))
                } else {
                    d
                }
            })
            .child(div().flex_1())
            .child(if self.open { IconName::ChevronUp } else { IconName::ChevronDown })
            .test_support()
            .into_any_element()
    }

    fn section(title: &str, theme: &gpui_kit::component::Theme) -> Div {
        v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(theme.border)
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title.to_owned()))
    }

    fn hint(text: impl Into<SharedString>, color: Hsla) -> Div {
        div().text_xs().text_color(color).child(text.into())
    }

    /// `BatchBody.svelte`.
    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let doc = s.doc.clone();
        let preview_row = s.preview_row;
        let fallback_mm = s.render.as_ref().map(|r| (r.length_mm, r.feed_margin_mm));
        let b = batch_of(&doc);
        let total = batch_size(&doc);
        let missing = missing_variables(&doc);
        let used = find_variables(&doc);
        let used_cols = used_columns(&doc);
        let empty_cells = empty_used_cells(&doc).len();
        let shown_total = total.max(b.rows.len());
        let today = local_today();
        let muted = theme.muted_foreground;

        let mut body = v_flex().gap_4().px_3().pb_3().min_w_0();

        body = body.child(Self::hint(
            "Write {{name}} in a text or code block, where name is a column of the data below. Each row prints one label, all in one job, so the ~24 mm leader is fed only once.",
            muted,
        ));

        // Print as a batch.
        let switch_hint = if b.enabled {
            format!(
                "{total} {}{}.",
                if total == 1 { "label" } else { "labels" },
                if b.rows.is_empty() { "" } else { ", one per row" }
            )
        } else {
            "Off: Print makes one label showing the row below.".into()
        };
        body = body.child(
            v_flex()
                .gap_1()
                .child(
                    Switch::new("batch-enabled")
                        .label("Print as a batch")
                        .checked(b.enabled)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            let on = *checked;
                            this.save(
                                None,
                                move |b| {
                                    b.enabled = on;
                                    true
                                },
                                cx,
                            );
                        })),
                )
                .child(Self::hint(switch_hint, muted)),
        );

        // Unknown variables, with one-click columns.
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|m| format!("{{{{{m}}}}}")).collect();
            let text = format!(
                "{}: {}. {} as written. Add a column or counter with that name, or fix the spelling.",
                if missing.len() == 1 { "Unknown variable" } else { "Unknown variables" },
                list.join(", "),
                if missing.len() == 1 { "It prints" } else { "They print" }
            );
            let full = b.columns.len() >= BATCH_COLUMNS;
            let fixes = missing.iter().filter(|m| is_variable_name(m)).cloned().enumerate().map(|(i, m)| {
                Button::new(SharedString::from(format!("fix-col-{i}")))
                    .small()
                    .outline()
                    .icon(IconName::Plus)
                    .label(format!("Column {m}"))
                    .disabled(full)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let name = m.clone();
                        this.save(None, move |b| add_column(b, Some(&name)), cx);
                    }))
            });
            body = body.child(
                v_flex()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.danger)
                    .bg(theme.danger.opacity(0.08))
                    .child(h_flex().gap_2().items_start().text_sm().child(div().text_color(theme.danger).child(IconName::TriangleAlert)).child(div().flex_1().child(text)))
                    .child(h_flex().flex_wrap().gap_2().children(fixes)),
            );
        }

        // Variables on the label and columns it doesn't use.
        if !used.is_empty() || !b.columns.is_empty() {
            let chips = used.iter().map(|name| {
                let (kind, color) = match variable_kind(&doc, name) {
                    VariableKind::Column => ("column", theme.primary),
                    VariableKind::Counter => ("counter", theme.info),
                    VariableKind::Date => ("date", theme.success),
                    VariableKind::Ssid => ("network name", theme.success),
                    VariableKind::Missing => ("unknown", theme.danger),
                };
                h_flex()
                    .gap_1()
                    .px_2()
                    .rounded_full()
                    .border_1()
                    .border_color(color.opacity(0.5))
                    .text_xs()
                    .child(div().font_family("Menlo").child(format!("{{{{{name}}}}}")))
                    .child(div().text_color(muted).child(kind))
            });
            let unused: Vec<String> = b
                .columns
                .iter()
                .filter(|c| !used_cols.contains(c))
                .map(|c| format!("{{{{{c}}}}}"))
                .collect();
            body = body.child(
                v_flex()
                    .gap_1()
                    .when(!used.is_empty(), |d| {
                        d.child(Self::hint("On the label:", muted)).child(h_flex().flex_wrap().gap_1().children(chips))
                    })
                    .when(!unused.is_empty(), |d| {
                        d.child(Self::hint(format!("Not on the label yet: {}", unused.join(", ")), muted))
                    }),
            );
        }

        // Preview stepper.
        if total > 1 || b.rows.len() > 1 {
            body = body.child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        Button::new("prev-label")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .tooltip("Previous label")
                            .disabled(preview_row == 0)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step(-1, cx))),
                    )
                    .child(format!("Preview: label {} of {}", preview_row + 1, shown_total))
                    .child(
                        Button::new("next-label")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .tooltip("Next label")
                            .disabled(preview_row + 1 >= shown_total)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.step(1, cx))),
                    ),
            );
        }

        // Data.
        let mut data = Self::section("Data", &theme);
        if b.columns.is_empty() {
            data = data
                .child(Self::hint(
                    "No data yet. Paste a table or open a CSV file (its first row names the columns), or type one.",
                    muted,
                ))
                .child(
                    h_flex().child(
                        Button::new("new-table")
                            .small()
                            .outline()
                            .icon(IconName::LayoutDashboard)
                            .label("New table")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.new_table(window, cx))),
                    ),
                );
        } else {
            data = data.child(self.render_table(&b, &doc, cx));
            if empty_cells > 0 {
                data = data.child(Self::hint(
                    format!(
                        "{empty_cells} {} empty in columns the label uses.",
                        if empty_cells == 1 { "cell is" } else { "cells are" }
                    ),
                    theme.danger,
                ));
            }
        }
        // Paste.
        let paste_open = self.paste_open || b.columns.is_empty();
        data = data.child(
            div()
                .id("paste-toggle")
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.primary)
                .cursor_pointer()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.paste_open = !this.paste_open;
                    cx.notify();
                }))
                .child(if b.columns.is_empty() { "Paste data" } else { "Replace with pasted data…" }),
        );
        if paste_open {
            let has_text = !self.paste.read(cx).value().trim().is_empty();
            data = data
                .child(Self::hint("Paste from a spreadsheet or CSV (first row = column names)", muted))
                .child(Textarea::new(&self.paste).small())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("use-pasted")
                                .small()
                                .primary()
                                .icon(crate::icons::icon("check")).label("Use pasted data")
                                .disabled(!has_text)
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    let text = this.paste.read(cx).value().to_string();
                                    this.load(&text, window, cx);
                                })),
                        )
                        .child(
                            Button::new("paste-clipboard")
                                .small()
                                .outline()
                                .icon(IconName::Copy)
                                .label("Paste from clipboard")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.paste_clipboard(window, cx))),
                        ),
                );
        }
        data = data.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("open-csv")
                        .small()
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label("Open CSV file…")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_csv(window, cx))),
                )
                .when(!b.columns.is_empty(), |d| {
                    d.child(
                        Button::new("clear-data")
                            .small()
                            .ghost()
                            .danger()
                            .icon(IconName::Delete)
                            .label("Clear data")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.clear_data(window, cx))),
                    )
                }),
        );
        if !self.notices.is_empty() {
            data = data.child(v_flex().gap_1().children(
                self.notices.iter().map(|n| Self::hint(format!("• {n}"), theme.warning)),
            ));
        }
        if b.rows.is_empty() {
            data = data.child(
                v_flex()
                    .gap_1()
                    .max_w(px(260.))
                    .child(div().text_sm().child("Labels to print (without data)"))
                    .child(Input::new(&self.count_input).small())
                    .child(Self::hint("With counters only, e.g. asset tags 0001–0050.", muted)),
            );
        }
        body = body.child(data);

        // Counters.
        body = body.child(Self::section("Counters", &theme).child(self.render_counters(&b, cx)));

        // Dates.
        let current = b.date_format;
        let formats = DATE_FORMATS.iter().map(|(f, _)| {
            let f = *f;
            let id = SharedString::from(format!("date-{}", date_format_id(f)));
            Button::new(id)
                .small()
                .label(date_format_label(f, today, None))
                .map(|btn| if f == current { btn.primary() } else { btn.outline() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.save(
                        None,
                        move |b| {
                            b.date_format = f;
                            true
                        },
                        cx,
                    );
                }))
        });
        let examples: Vec<String> = date_examples(&b, today, None)
            .into_iter()
            .map(|ex| format!("{{{{{}}}}} → {}", ex.name, ex.value))
            .collect();
        body = body.child(
            Self::section("Dates", &theme)
                .child(div().text_sm().child("Date format"))
                .child(h_flex().flex_wrap().gap_2().children(formats))
                .child(Self::hint(
                    format!("{}. Also {{{{today-7d}}}}. Local time, the day you print.", examples.join("; ")),
                    muted,
                )),
        );

        // Labels.
        if total > 0 {
            let copies = doc.print.copies.clamp(1, 99) as usize;
            let (fallback_len, render_feed) = fallback_mm.unwrap_or((0.0, 0.0));
            let feed = if self.measure.feed_margin_mm > 0.0 { self.measure.feed_margin_mm } else { render_feed };
            let leader = !doc.print.chain;
            let tape = batch_tape(&self.measure.lengths(), total, copies, feed, leader, fallback_len);
            let mut summary = format!("Labels: {total}");
            if copies > 1 {
                summary.push_str(&format!(" × {copies} copies"));
            }
            summary.push_str(&format!(
                " · {}{} of tape{}{}",
                if tape.exact { "" } else { "≈ " },
                format_tape(tape.total_mm),
                if leader { " incl. the leader" } else { "" },
                if tape.exact { "" } else { " (estimated)" }
            ));
            let problems = self.measure.problems();
            let notices = self.measure.notices();
            let mut labels = Self::section("Labels", &theme).child(div().text_sm().child(summary));
            if let Some((row, msg)) = problems.first() {
                labels = labels.child(Self::hint(
                    format!(
                        "{} be printed. Label {}: {msg}",
                        if problems.len() == 1 { "1 label can’t".to_owned() } else { format!("{} labels can’t", problems.len()) },
                        row + 1
                    ),
                    theme.danger,
                ));
            }
            if let Some((row, msg)) = notices.first() {
                labels = labels.child(Self::hint(
                    format!(
                        "{} print as designed. Label {}: {msg}",
                        if notices.len() == 1 { "1 label doesn’t".to_owned() } else { format!("{} labels don’t", notices.len()) },
                        row + 1
                    ),
                    theme.warning,
                ));
            }
            labels = labels.child(self.render_previews(total, preview_row, &doc, cx));
            body = body.child(labels);
        }

        body.into_any_element()
    }

    /// `BatchTable.svelte`.
    fn render_table(&self, b: &BatchData, doc: &baradar_core::doc::LabelDoc, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let empty = empty_used_cells(doc);
        let pages = page_count(b.rows.len(), TABLE_PAGE_SIZE);
        let first = self.table_page.min(pages - 1) * TABLE_PAGE_SIZE;
        let last = (first + TABLE_PAGE_SIZE).min(b.rows.len());

        let cell_box = || div().w(px(CELL_W)).flex_shrink_0().px_1();
        // Header: column names.
        let mut header = h_flex()
            .gap_0()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.table_head)
            .child(div().w(px(NUM_W)).flex_shrink_0().px_1().text_xs().text_color(theme.muted_foreground).child("#"));
        for (col, name) in b.columns.iter().enumerate() {
            let editing = self.editing == Some(Edit::Column(col));
            let content = if editing {
                v_flex()
                    .gap_1()
                    .child(Input::new(&self.editor).small())
                    .when_some(self.name_problem.clone(), |d, p| d.child(Self::hint(p, theme.danger)))
                    .into_any_element()
            } else {
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!("col-{col}")))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_sm()
                            .font_family("Menlo")
                            .cursor_text()
                            .py_1()
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.start_edit(Edit::Column(col), window, cx)
                            }))
                            .child(name.clone())
                            .test_support(),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rm-col-{col}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip(format!("Remove column {name}"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.remove_column_at(col, window, cx)
                            })),
                    )
                    .into_any_element()
            };
            header = header.child(cell_box().py_1().child(content));
        }

        // Rows of this page.
        let mut rows = v_flex();
        for r in first..last {
            let cells = b.rows.get(r).cloned().unwrap_or_default();
            let mut line = h_flex().border_b_1().border_color(theme.table_row_border).child(
                h_flex()
                    .w(px(NUM_W))
                    .flex_shrink_0()
                    .px_1()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child((r + 1).to_string()))
                    .child(
                        Button::new(SharedString::from(format!("rm-row-{r}")))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Delete)
                            .tooltip(format!("Remove row {}", r + 1))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_row_at(r, cx))),
                    ),
            );
            for c in 0..b.columns.len() {
                let value = cells.get(c).cloned().unwrap_or_default();
                let is_empty = empty.contains(&(r, c));
                let content = if self.editing == Some(Edit::Cell(r, c)) {
                    div().child(Input::new(&self.editor).small()).into_any_element()
                } else {
                    div()
                        .id(SharedString::from(format!("cell-{r}-{c}")))
                        .h(px(26.))
                        .px_1()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_sm()
                        .rounded_sm()
                        .border_1()
                        .border_color(if is_empty { theme.danger } else { gpui_kit::transparent_black() })
                        .cursor_text()
                        .hover(|s| s.bg(theme.secondary))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.start_edit(Edit::Cell(r, c), window, cx)
                        }))
                        .map(|d| {
                            if value.is_empty() && is_empty {
                                d.text_color(theme.muted_foreground).child("empty")
                            } else {
                                d.child(value)
                            }
                        })
                        .test_support()
                        .into_any_element()
                };
                line = line.child(cell_box().py(px(2.)).child(content));
            }
            rows = rows.child(line);
        }

        let table = div()
            .id("batch-table")
            .w_full()
            .overflow_x_scroll()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(v_flex().min_w(px(NUM_W + CELL_W * b.columns.len() as f32)).child(header).child(rows));

        let pager = (pages > 1).then(|| {
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    Button::new("rows-prev")
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .tooltip("Previous rows")
                        .disabled(first == 0)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.table_page = this.table_page.saturating_sub(1);
                            cx.notify();
                        })),
                )
                .child(format!("Rows {}–{} of {}", first + 1, last, b.rows.len()))
                .child(
                    Button::new("rows-next")
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronRight)
                        .tooltip("Next rows")
                        .disabled(last >= b.rows.len())
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.table_page = (first / TABLE_PAGE_SIZE) + 1;
                            cx.notify();
                        })),
                )
        });

        v_flex()
            .gap_2()
            .child(table)
            .child(Self::hint(
                "Click a cell to edit it. ↑ ↓ move between rows; Enter goes to the next row (and adds one at the end).",
                theme.muted_foreground,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("add-row")
                            .small()
                            .outline()
                            .icon(IconName::Plus)
                            .label("Row")
                            .disabled(b.rows.len() >= BATCH_ROWS)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.add_row_and_edit(window, cx))),
                    )
                    .child(
                        Button::new("add-col")
                            .small()
                            .outline()
                            .icon(IconName::Plus)
                            .label("Column")
                            .disabled(b.columns.len() >= BATCH_COLUMNS)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.add_column_and_edit(window, cx))),
                    )
                    .children(pager),
            )
            .when(b.rows.len() >= BATCH_ROWS, |d| {
                d.child(Self::hint(format!("{BATCH_ROWS} rows is the maximum for one batch."), theme.warning))
            })
            .into_any_element()
    }

    /// `BatchCounters.svelte`.
    fn render_counters(&self, b: &BatchData, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mut list = v_flex().gap_3();
        for (k, c) in b.counters.iter().enumerate() {
            let Some(inputs) = self.counters.get(k) else { continue };
            let field = |label: &'static str, input: &Entity<InputState>, w: f32| {
                v_flex().gap_1().w(px(w)).child(div().text_xs().text_color(muted).child(label)).child(Input::new(input).small())
            };
            list = list.child(
                v_flex()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(div().font_family("Menlo").child(format!("{{{{{}}}}}", c.name)))
                            .child(div().text_color(muted).child(format!("= {}", counter_example(c, 3))))
                            .child(div().flex_1())
                            .child(
                                Button::new(SharedString::from(format!("rm-counter-{k}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Delete)
                                    .tooltip(format!("Remove counter {}", c.name))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.save(None, |b| remove_counter(b, k), cx)
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_start()
                            .child(field("Name", &inputs.fields[0], 120.))
                            .child(field("Start", &inputs.fields[1], 90.))
                            .child(field("Step", &inputs.fields[2], 80.))
                            .child(field("Digits", &inputs.fields[3], 70.)),
                    )
                    .when_some(inputs.problem.clone(), |d, p| d.child(Self::hint(p, theme.danger))),
            );
        }
        if b.counters.len() < BATCH_COUNTERS {
            list = list.child(
                h_flex().child(
                    Button::new("add-counter")
                        .small()
                        .outline()
                        .icon(IconName::Plus)
                        .label("Counter")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(None, add_counter, cx))),
                ),
            );
        }
        list.child(Self::hint(
            "Digits pads with zeros: 3 digits print 001, 002, … Write A-{{n}} on the label for A-001.",
            muted,
        ))
        .into_any_element()
    }

    /// `BatchPreviews.svelte`: a page of label tiles; clicking one previews it.
    fn render_previews(&self, total: usize, preview_row: usize, doc: &baradar_core::doc::LabelDoc, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pages = page_count(total, PAGE_SIZE);
        let first = self.preview_page.min(pages - 1) * PAGE_SIZE;
        let rows: Range<usize> = first..(first + PAGE_SIZE).min(total);
        let shown = rows.len();
        let tiles = rows.map(|row| {
            let current = row == preview_row;
            let info = self.measure.rows.get(&row).cloned();
            let thumb = self.measure.thumbs.get(&row).cloned().flatten();
            v_flex()
                .id(SharedString::from(format!("tile-{row}")))
                .w(px(156.))
                .gap_1()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(if current { theme.primary } else { theme.border })
                .when(current, |d| d.bg(theme.primary.opacity(0.08)))
                .cursor_pointer()
                .hover(|s| s.bg(theme.secondary))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.studio.update(cx, |s, cx| s.set_preview_row(row, cx));
                }))
                .child(
                    div()
                        .h(px(48.))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.secondary)
                        .rounded_sm()
                        .overflow_hidden()
                        .map(|d| match thumb {
                            Some(img_src) => d.child(img(img_src).max_h(px(44.)).max_w_full().object_fit(ObjectFit::Contain)),
                            None => d.child(div().text_xs().text_color(theme.muted_foreground).child("…")),
                        }),
                )
                .child(div().text_xs().overflow_hidden().whitespace_nowrap().child(row_caption(doc, row)))
                .when_some(info.as_ref().and_then(|i| i.problem.clone()), |d, p| {
                    d.child(Self::hint(format!("Can’t print: {p}"), theme.danger))
                })
                .when_some(info.and_then(|i| i.notice), |d, n| d.child(Self::hint(n, theme.warning)))
                .test_support()
        });
        let pager = (pages > 1).then(|| {
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    Button::new("labels-prev")
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .tooltip("Previous labels")
                        .disabled(first == 0)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.preview_page = (first / PAGE_SIZE).saturating_sub(1);
                            this.sync_thumb_page_keep(cx);
                        })),
                )
                .child(format!("Labels {}–{} of {}", first + 1, first + shown, total))
                .child(
                    Button::new("labels-next")
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronRight)
                        .tooltip("Next labels")
                        .disabled(first + PAGE_SIZE >= total)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.preview_page = first / PAGE_SIZE + 1;
                            this.sync_thumb_page_keep(cx);
                        })),
                )
        });
        v_flex()
            .gap_2()
            .child(h_flex().flex_wrap().gap_2().children(tiles))
            .children(pager)
            .into_any_element()
    }

    /// After paging the grid by hand: asks for that page's thumbnails without following the
    /// preview row.
    fn sync_thumb_page_keep(&mut self, cx: &mut Context<Self>) {
        let total = batch_size(&self.studio.read(cx).doc);
        let pages = page_count(total, PAGE_SIZE);
        let first = self.preview_page.min(pages - 1) * PAGE_SIZE;
        self.want_thumbs(first..(first + PAGE_SIZE).min(total), cx);
        cx.notify();
    }
}

/// A stable id for a date format button.
fn date_format_id(f: DateFormat) -> &'static str {
    match f {
        DateFormat::Iso => "iso",
        DateFormat::Dmy => "dmy",
        DateFormat::Mdy => "mdy",
        DateFormat::Long => "long",
    }
}

impl Render for BatchPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let header = self.render_header(cx);
        v_flex()
            .w_full()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(header)
            .when(self.open, |d| d.child(self.render_body(cx)))
    }
}

/// Headless UI tests (the virtual printer only; never Bluetooth).
#[cfg(test)]
pub(crate) mod ui_tests {
    // NOT `use super::*`: with test-support, `gpui_kit::*` exports GPUI's `test` macro, which
    // shadows Rust's #[test] and makes #[gpui_kit::test] recurse forever.
    use super::{BatchPanel, CounterField, Edit};
    use crate::studio::Studio;
    use baradar_core::doc::Item;
    use baradar_core::store::Store;
    use gpui_kit::component::input::InputEvent;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Context, Entity, Render, TestAppContext, Window, WindowBounds,
        WindowOptions, point, px, size,
    };
    use std::time::Duration;

    pub(crate) fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("baradar-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::open(dir).expect("temp store")
    }

    /// Runs the app until `f` holds (rendering and the printer client use real threads).
    pub(crate) fn wait_until(cx: &mut TestAppContext, what: &str, mut f: impl FnMut(&mut TestAppContext) -> bool) {
        for _ in 0..800 {
            cx.run_until_parked();
            if f(cx) {
                return;
            }
            cx.executor().advance_clock(Duration::from_millis(20));
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    pub(crate) fn open_test_window<V: Render + 'static>(
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    ) -> (AnyWindowHandle, Entity<V>) {
        cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1400.), px(4000.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| build(window, cx)),
            )
            .expect("open test window")
        })
    }

    pub(crate) fn new_studio(cx: &mut TestAppContext, name: &str) -> Entity<Studio> {
        cx.update(gpui_kit::init);
        cx.executor().allow_parking();
        let store = temp_store(name);
        cx.update(|cx| cx.new(|cx| Studio::new(store, cx)))
    }

    fn enter() -> InputEvent {
        InputEvent::PressEnter {
            secondary: false,
            shift: false,
        }
    }

    #[gpui_kit::test]
    async fn batch_table_counters_and_previews(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "batch");
        let s2 = studio.clone();
        let (handle, panel) = open_test_window(cx, move |window, cx| BatchPanel::new(s2, window, cx));
        let batch = |cx: &mut TestAppContext| cx.update(|cx| studio.read(cx).doc.batch.clone().expect("batch data"));

        // Open the panel, start a table: one column whose name is edited.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("batch-toggle", cx);
            window.render_frame(cx);
            window.click("new-table", cx);
            window.render_frame(cx);
        })
        .expect("window");
        cx.run_until_parked();
        let b = batch(cx);
        assert!(b.enabled);
        assert_eq!(b.columns, vec!["col1".to_owned()]);
        assert_eq!(b.rows.len(), 1);
        cx.update(|cx| assert_eq!(panel.read(cx).editing, Some(Edit::Column(0))));

        // An invalid name keeps the draft with its problem; a valid one renames.
        let p = panel.clone();
        cx.update_window(handle, move |_, window, cx| {
            p.update(cx, |p, cx| p.on_editor("2bad", &enter(), window, cx));
            assert!(p.read(cx).name_problem.is_some());
            p.update(cx, |p, cx| p.on_editor("name", &enter(), window, cx));
            assert_eq!(p.read(cx).editing, None);
        })
        .expect("window");
        assert_eq!(batch(cx).columns, vec!["name".to_owned()]);

        // Pasted CSV replaces the table and turns the batch on.
        let p = panel.clone();
        cx.update_window(handle, move |_, window, cx| {
            p.update(cx, |p, cx| p.load("name,room\nAda,Lab 1\nGrace,Lab 2\n", window, cx));
        })
        .expect("window");
        let b = batch(cx);
        assert_eq!(b.columns, vec!["name".to_owned(), "room".to_owned()]);
        assert_eq!(b.rows.len(), 2);

        // The label uses two columns, the counter and an unknown name: one click adds it.
        studio.update(cx, |s, cx| {
            let id = s.doc.items[0].id().to_owned();
            s.update_item(
                &id,
                None,
                |i| {
                    if let Item::Text(t) = i {
                        t.text = "{{name}} {{room}} {{n}} {{oops}}".into();
                    }
                },
                cx,
            );
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("fix-col-0", cx);
        })
        .expect("window");
        cx.run_until_parked();
        assert_eq!(batch(cx).columns.last().map(String::as_str), Some("oops"));

        // Cells: edit, ↓, Enter on the last row adds one.
        let p = panel.clone();
        cx.update_window(handle, move |_, window, cx| {
            window.render_frame(cx);
            window.click("cell-0-0", cx);
            assert_eq!(p.read(cx).editing, Some(Edit::Cell(0, 0)));
            p.update(cx, |p, cx| p.on_editor("Bob", &InputEvent::Change, window, cx));
            assert!(p.update(cx, |p, cx| p.on_cell_key("down", window, cx)));
            assert_eq!(p.read(cx).editing, Some(Edit::Cell(1, 0)));
            p.update(cx, |p, cx| p.on_editor("Grace", &enter(), window, cx));
            assert_eq!(p.read(cx).editing, Some(Edit::Cell(2, 0)));
            window.render_frame(cx);
        })
        .expect("window");
        let b = batch(cx);
        assert_eq!(b.rows[0][0], "Bob");
        assert_eq!(b.rows.len(), 3);

        // Counters: start 5, a taken name is refused.
        let p = panel.clone();
        cx.update_window(handle, move |_, window, cx| {
            p.update(cx, |p, cx| p.on_counter_input(0, CounterField::Start, "5", &InputEvent::Change, window, cx));
            p.update(cx, |p, cx| p.on_counter_input(0, CounterField::Name, "room", &InputEvent::Blur, window, cx));
            assert!(p.read(cx).counters[0].problem.is_some());
        })
        .expect("window");
        let b = batch(cx);
        assert_eq!(b.counters[0].start, 5);
        assert_eq!(b.counters[0].name, "n");

        // Every label is measured in the background; the page's thumbnails come first.
        wait_until(cx, "the batch to be measured", |cx| {
            cx.update(|cx| {
                let p = panel.read(cx);
                p.measure.rows.len() >= 3 && p.measure.thumbs.values().filter(|t| t.is_some()).count() >= 3
            })
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("tile-1", cx);
        })
        .expect("window");
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(studio.read(cx).preview_row, 1));

        // Turning the batch off keeps the data; the preview row stays inside it.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("batch-enabled", cx);
            window.render_frame(cx);
        })
        .expect("window");
        cx.run_until_parked();
        assert!(!batch(cx).enabled);
        assert_eq!(batch(cx).rows.len(), 3);
    }
}
