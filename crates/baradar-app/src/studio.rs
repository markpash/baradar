//! The studio's state and actions (port of `web/src/ui/state/studio.svelte.ts`).
//!
//! One `Studio` entity owns the open document, its undo history, the selection, the latest
//! render and the preferences. Views read it and call its actions; it emits [`StudioEvent`]s for
//! things a view must show (toasts).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use baradar_core::doc::files::{label_file_name, parse_label_file, serialize_label_file};
use baradar_core::doc::history::History;
use baradar_core::doc::migrate::migrate;
use baradar_core::doc::{self, Item, ItemKind, LabelDoc, LengthSettings, ops};
use baradar_core::render::fonts::FontRegistry;
use baradar_core::render::renderer::{RenderOptions, RenderResult, RenderTarget, Renderer};
use baradar_core::render::text::{new_text_defaults, retape_text_size};
use baradar_core::store::{LabelSummary, Prefs, Store};
use baradar_core::store::{HistoryRecord, new_record_id};
use baradar_core::vars::batch::batch_pages;
use baradar_core::vars::variables::{
    VariableContext, batch_size, columns_without_data, missing_variables, resolve_doc,
};
use baradar_printer::{
    ClientChannels, ClientConfig, ClientState, Command, ConnectTarget, Event, PacketEntry,
    PrintProgress, PrinterClient, PrinterInfo, Problem,
};
use gpui_kit::{Context, EventEmitter, Task};
use ptouch::encode::{CutMode, JobOptions};
use ptouch::{Bitmap, ModelProfile, TapeKind, TapeSpec};

/// Debounce before the open label is written to disk (`createAutosave`, 500 ms).
const AUTOSAVE_DELAY: Duration = Duration::from_millis(500);
/// Debounce between renders once a first render exists.
const RENDER_DELAY: Duration = Duration::from_millis(50);

/// How a toast looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Ok,
    Warn,
    Error,
}

/// Something a view must show.
#[derive(Debug, Clone)]
pub enum StudioEvent {
    Toast {
        tone: Tone,
        title: String,
        detail: Option<String>,
    },
    /// Every printer-client event, forwarded for views that need results Studio doesn't keep
    /// (scan, pairing, settings, probes).
    Printer(baradar_printer::Event),
}

/// Preview zoom: fit the stage, or a multiple of real size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    Fit,
    Real(f32),
}

pub struct Studio {
    pub store: Store,
    pub prefs: Prefs,
    pub renderer: Arc<Renderer>,
    pub fonts: Arc<FontRegistry>,
    pub doc: LabelDoc,
    /// From a newer schema: the first edit saves a copy.
    pub read_only: bool,
    history: History<LabelDoc>,
    pub selected: Option<String>,
    pub render: Option<Arc<RenderResult>>,
    pub render_error: Option<String>,
    pub rendering: bool,
    /// Bumped whenever the document is replaced by something other than a property edit
    /// (undo, open, template…), so property inputs know to reload their values.
    pub epoch: u64,
    pub zoom: Zoom,
    /// Batch row shown in the preview.
    pub preview_row: usize,
    render_cancel: Option<Arc<AtomicBool>>,
    render_task: Option<Task<()>>,
    rendered_key: Option<(LabelDoc, &'static str, &'static str, usize, String)>,
    autosave_task: Option<Task<()>>,
    started: Instant,
    // Printer
    pub printer: PrinterClient,
    pub conn: ClientState,
    pub printer_info: Option<PrinterInfo>,
    pub problem: Option<Problem>,
    pub progress: Option<PrintProgress>,
    pub open_progress: Option<(u8, u8)>,
    /// A print from this window is not yet settled.
    pub printing: bool,
    /// Raw packets for the diagnostics view.
    pub packets: async_channel::Receiver<PacketEntry>,
    pending_print: Option<PendingPrint>,
    _printer_events: Task<()>,
}

/// What was sent to the printer, kept for the print history once it finishes.
struct PendingPrint {
    doc: LabelDoc,
    labels: u32,
    copies: u32,
    batch_rows: Option<u32>,
    tape_mm: f64,
    width_mm: f64,
    media_id: String,
    model: String,
    thumb: Option<Vec<u8>>,
}

/// Tape fed before the first label of a job (`LEADER_MM`).
const LEADER_MM: f64 = 24.0;

impl EventEmitter<StudioEvent> for Studio {}

/// The tape-width byte the printer reports for a width (3.5 mm tape reports 4).
pub fn width_byte(width_mm: f64) -> u8 {
    if (width_mm - 3.5).abs() < 0.01 {
        4
    } else {
        width_mm.round().clamp(0.0, 255.0) as u8
    }
}

/// TZe continuous widths a model takes, in mm (`tapeWidthsOf`).
pub fn tape_widths_of(profile: &ModelProfile) -> Vec<f64> {
    let mut out: Vec<f64> = profile
        .media
        .iter()
        .filter(|t| t.kind == TapeKind::Tze)
        .map(|t| f64::from(t.width_mm_x10) / 10.0)
        .collect();
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// The local calendar day (for `{{today}}`).
pub fn local_today() -> time::Date {
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date()
}

impl Studio {
    pub fn new(store: Store, cx: &mut Context<Self>) -> Self {
        let prefs = store.load_prefs();
        let fonts = Arc::new(FontRegistry::new());
        {
            let store = store.clone();
            fonts.set_user_font_loader(Arc::new(move |r: &str| store.font_bytes(r)));
        }
        let blob_store = store.clone();
        let renderer = Arc::new(
            Renderer::new(fonts.clone())
                .with_blob_loader(Arc::new(move |r: &str| blob_store.get_blob(r).map(|(b, _)| b))),
        );
        let doc = LabelDoc::new();
        let ClientChannels { client, events, packets } = PrinterClient::spawn(ClientConfig::default());
        let printer_events = cx.spawn(async move |this, cx| {
            while let Ok(ev) = events.recv().await {
                if this.update(cx, |this, cx| this.on_printer_event(ev, cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            store,
            prefs,
            renderer,
            fonts,
            history: History::with_default_capacity(doc.clone()),
            selected: doc.items.first().map(|i| i.id().to_owned()),
            doc,
            read_only: false,
            render: None,
            render_error: None,
            rendering: false,
            epoch: 0,
            zoom: Zoom::Fit,
            preview_row: 0,
            render_cancel: None,
            render_task: None,
            rendered_key: None,
            autosave_task: None,
            started: Instant::now(),
            printer: client,
            conn: ClientState::Idle,
            printer_info: None,
            problem: None,
            progress: None,
            open_progress: None,
            printing: false,
            packets,
            pending_print: None,
            _printer_events: printer_events,
        };
        this.restore(cx);
        if this.prefs.auto_reconnect
            && let Some(endpoint) = this.prefs.last_endpoint.as_deref().and_then(|e| e.parse().ok())
        {
            let _ = this.printer.send(Command::Connect(ConnectTarget::Endpoint(endpoint)));
        }
        this
    }

    fn toast(&self, tone: Tone, title: impl Into<String>, detail: Option<String>, cx: &mut Context<Self>) {
        cx.emit(StudioEvent::Toast {
            tone,
            title: title.into(),
            detail,
        });
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    // --------------------------------------------------------------------------------------
    // Targets
    // --------------------------------------------------------------------------------------

    /// The model designed for (`designModel`); a connected printer will take precedence.
    pub fn model(&self) -> &'static ModelProfile {
        if let Some(m) = self.printer_model() {
            return m;
        }
        ptouch::profile_by_name(&self.prefs.design_model)
            .or_else(|| ptouch::profile(baradar_printer::MODEL))
            .unwrap_or(&ptouch::profiles()[0])
    }

    /// Tape widths the current model takes.
    pub fn tape_widths(&self) -> Vec<f64> {
        tape_widths_of(self.model())
    }

    /// The model + tape the label renders for (`target`), or why there is none.
    pub fn target(&self) -> Result<RenderTarget, String> {
        let profile = self.model();
        let width = self.doc.tape.width_mm;
        let byte = width_byte(width);
        let by_id = self.doc.tape.media_id.as_deref().and_then(|id| {
            profile
                .media
                .iter()
                .copied()
                .find(|t| t.id == id && t.media_width_byte == byte)
        });
        let loaded = self
            .printer_model()
            .and(self.printer_info.as_ref())
            .and_then(|i| i.tape)
            .filter(|t| t.media_width_byte == byte);
        let tape: Option<&'static TapeSpec> = loaded
            .or(by_id)
            .or_else(|| profile.tape(byte, TapeKind::Tze));
        match tape {
            Some(tape) => Ok(RenderTarget { profile, tape }),
            None => {
                let widths = tape_widths_of(profile);
                let max = widths.last().copied().unwrap_or(0.0);
                Err(if width > max {
                    format!(
                        "This label is {}; {} takes up to {}.",
                        crate::view_model::format_mm(width),
                        profile.name,
                        crate::view_model::format_mm(max)
                    )
                } else {
                    format!(
                        "This label is {}; {} doesn’t take {} tape.",
                        crate::view_model::format_mm(width),
                        profile.name,
                        crate::view_model::format_mm(width)
                    )
                })
            }
        }
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected.as_deref().and_then(|id| self.doc.item(id))
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Labels a batch prints (0 = a plain label).
    pub fn batch_count(&self) -> usize {
        batch_size(&self.doc)
    }

    // --------------------------------------------------------------------------------------
    // Documents
    // --------------------------------------------------------------------------------------

    /// `#restoreDoc`: reopens the last label, or remembers the fresh one.
    fn restore(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.prefs.last_label_id.clone()
            && let Some(raw) = self.store.load_label(&id)
            && let Ok(m) = migrate(&raw)
        {
            self.open_doc(m.doc, m.read_only, false, cx);
            return;
        }
        let doc = self.fresh_doc();
        self.open_doc(doc, false, true, cx);
    }

    /// A new label with the user's default font and size (`#freshDoc`).
    fn fresh_doc(&self) -> LabelDoc {
        let mut doc = LabelDoc::new();
        let profile = self.model();
        let widths = tape_widths_of(profile);
        if !widths.contains(&doc.tape.width_mm)
            && let Some(w) = nearest(&widths, doc.tape.width_mm)
        {
            doc.tape.width_mm = w;
        }
        let base = doc.clone();
        if let Some(item) = doc.items.first_mut() {
            self.apply_text_defaults(item, &base);
        }
        doc
    }

    fn apply_text_defaults(&self, item: &mut Item, doc: &LabelDoc) {
        let Item::Text(t) = item else { return };
        let (band, dpi) = self.band_for(doc);
        let d = new_text_defaults(
            &self.prefs.default_font.family,
            self.prefs.default_font.weight,
            self.prefs.default_text_size,
            doc.tape.width_mm,
            band,
            dpi,
        );
        t.font_family = d.font_family.into();
        t.font_weight = d.font_weight;
        t.size = d.size;
    }

    /// The band a doc's tape prints and its dpi (with the fallback when the model lacks it).
    fn band_for(&self, doc: &LabelDoc) -> (i64, u16) {
        let profile = self.model();
        match profile.tape(width_byte(doc.tape.width_mm), TapeKind::Tze) {
            Some(t) => (i64::from(t.print_pins), profile.dpi),
            None => baradar_core::render::text::fallback_band(doc.tape.width_mm).unwrap_or((128, 180)),
        }
    }

    /// `openDoc`: shows `doc`, resetting undo and selection.
    pub fn open_doc(&mut self, doc: LabelDoc, read_only: bool, save: bool, cx: &mut Context<Self>) {
        self.history.reset(doc.clone());
        self.selected = doc.items.first().map(|i| i.id().to_owned());
        self.preview_row = 0;
        self.read_only = read_only;
        self.prefs.last_label_id = Some(doc.id.clone());
        let _ = self.store.save_prefs(&self.prefs);
        self.doc = doc;
        self.epoch += 1;
        if save && !read_only {
            self.schedule_autosave(cx);
        }
        self.schedule_render(cx);
        cx.notify();
    }

    /// `setDoc`: an edit. Read-only docs fork into a copy first.
    pub fn set_doc(&mut self, mut doc: LabelDoc, coalesce: Option<&str>, cx: &mut Context<Self>) {
        if self.read_only {
            doc.id = doc::new_id();
            doc.name = format!("{} (copy)", doc.name);
            self.read_only = false;
            self.prefs.last_label_id = Some(doc.id.clone());
            let _ = self.store.save_prefs(&self.prefs);
            self.toast(
                Tone::Info,
                "Saved as a copy",
                Some("The original was made with a newer version and stays unchanged.".into()),
                cx,
            );
        }
        let now = self.now_ms();
        self.history.push(doc.clone(), coalesce, now);
        self.doc = doc;
        self.schedule_autosave(cx);
        self.schedule_render(cx);
        cx.notify();
    }

    /// A doc edit that also reloads property inputs (structural changes).
    fn replace_doc(&mut self, doc: LabelDoc, cx: &mut Context<Self>) {
        self.epoch += 1;
        self.set_doc(doc, None, cx);
    }

    pub fn new_label(&mut self, cx: &mut Context<Self>) {
        let doc = self.fresh_doc();
        self.open_doc(doc, false, true, cx);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = self.history.undo().cloned() {
            self.restore_from_history(doc, cx);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = self.history.redo().cloned() {
            self.restore_from_history(doc, cx);
        }
    }

    fn restore_from_history(&mut self, doc: LabelDoc, cx: &mut Context<Self>) {
        if self.selected.as_deref().is_none_or(|id| doc.item(id).is_none()) {
            self.selected = doc.items.first().map(|i| i.id().to_owned());
        }
        self.doc = doc;
        self.epoch += 1;
        self.schedule_autosave(cx);
        self.schedule_render(cx);
        cx.notify();
    }

    pub fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected != id {
            self.selected = id;
            cx.notify();
        }
    }

    /// `selectRelative(±1)`.
    pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.doc.items.len();
        if n == 0 {
            return;
        }
        let at = self
            .selected
            .as_deref()
            .and_then(|id| self.doc.items.iter().position(|i| i.id() == id));
        let next = match at {
            Some(i) => (i as isize + delta).clamp(0, n as isize - 1) as usize,
            None => 0,
        };
        self.select(Some(self.doc.items[next].id().to_owned()), cx);
    }

    /// `insert(kind)`: adds a block after the selection and selects it.
    pub fn insert(&mut self, kind: ItemKind, edit: impl FnOnce(&mut Item), cx: &mut Context<Self>) {
        let (mut doc, id) = ops::add_item(&self.doc, kind, self.selected.as_deref());
        let base = doc.clone();
        if let Some(item) = doc.item_mut(&id) {
            self.apply_text_defaults(item, &base);
            edit(item);
        }
        self.selected = Some(id);
        self.replace_doc(doc, cx);
    }

    /// `updateItem`: edits one block; `coalesce` merges rapid edits into one undo step.
    pub fn update_item(&mut self, id: &str, coalesce: Option<&str>, edit: impl FnOnce(&mut Item), cx: &mut Context<Self>) {
        let doc = ops::update_item(&self.doc, id, edit);
        self.set_doc(doc, coalesce, cx);
    }

    /// `updateDoc`.
    pub fn update_doc(&mut self, coalesce: Option<&str>, edit: impl FnOnce(&mut LabelDoc), cx: &mut Context<Self>) {
        let doc = ops::update_doc(&self.doc, edit);
        self.set_doc(doc, coalesce, cx);
    }

    /// `removeItem`: selects the item now at the same index, else the previous one.
    pub fn remove_item(&mut self, id: &str, cx: &mut Context<Self>) {
        let at = self.doc.items.iter().position(|i| i.id() == id);
        let doc = ops::remove_item(&self.doc, id);
        self.selected = at
            .and_then(|i| doc.items.get(i).or_else(|| doc.items.get(i.saturating_sub(1))))
            .map(|i| i.id().to_owned());
        self.replace_doc(doc, cx);
    }

    pub fn duplicate(&mut self, id: &str, cx: &mut Context<Self>) {
        let (doc, new_id) = ops::duplicate_item(&self.doc, id);
        self.selected = Some(new_id);
        self.replace_doc(doc, cx);
    }

    /// `moveTo(id, to)`: moves a block to position `to` (drag and drop in the block list).
    pub fn move_to(&mut self, id: &str, to: usize, cx: &mut Context<Self>) {
        let Some(at) = self.doc.items.iter().position(|i| i.id() == id) else {
            return;
        };
        let to = to.min(self.doc.items.len().saturating_sub(1));
        if to != at {
            let doc = ops::move_item(&self.doc, id, to);
            self.replace_doc(doc, cx);
        }
    }

    /// `move(id, delta)`.
    pub fn move_by(&mut self, id: &str, delta: isize, cx: &mut Context<Self>) {
        let Some(at) = self.doc.items.iter().position(|i| i.id() == id) else {
            return;
        };
        let to = (at as isize + delta).clamp(0, self.doc.items.len() as isize - 1) as usize;
        if to != at {
            let doc = ops::move_item(&self.doc, id, to);
            self.replace_doc(doc, cx);
        }
    }

    /// `setTapeWidth`: re-tapes the label; quick text sizes map so M stays M.
    pub fn set_tape_width(&mut self, width_mm: f64, cx: &mut Context<Self>) {
        let (old_band, dpi) = self.band_for(&self.doc);
        let model = self.model().name.to_owned();
        let mut doc = ops::update_doc(&self.doc, |d| {
            d.tape.width_mm = width_mm;
            d.tape.media_id = None;
            d.tape.model = Some(model);
        });
        let (new_band, new_dpi) = self.band_for(&doc);
        for item in &mut doc.items {
            if let Item::Text(t) = item
                && t.frame.is_none()
            {
                t.size = retape_text_size(t.size, old_band, new_band, dpi, None, new_dpi);
            }
        }
        self.replace_doc(doc, cx);
    }

    /// `setDesignModel`: the model to design for while no printer is connected.
    pub fn set_design_model(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(profile) = ptouch::profile_by_name(name) else {
            return;
        };
        self.prefs.design_model = profile.name.to_owned();
        let _ = self.store.save_prefs(&self.prefs);
        let widths = tape_widths_of(profile);
        if !widths.contains(&self.doc.tape.width_mm)
            && let Some(w) = nearest(&widths, self.doc.tape.width_mm)
        {
            self.set_tape_width(w, cx);
        } else {
            self.schedule_render(cx);
            cx.notify();
        }
    }

    /// `setCopies`.
    pub fn set_copies(&mut self, n: u32, cx: &mut Context<Self>) {
        let n = n.clamp(1, 99);
        self.update_doc(Some("print:copies"), |d| d.print.copies = n, cx);
    }

    pub fn set_length(&mut self, length: LengthSettings, cx: &mut Context<Self>) {
        self.update_doc(Some("doc:length"), |d| d.length = length, cx);
    }

    /// `newFromTemplate`: opens a copy of `doc` with a new id and timestamps.
    pub fn open_new(&mut self, mut doc: LabelDoc, cx: &mut Context<Self>) {
        let now = doc::now_iso();
        doc.id = doc::new_id();
        doc.created_at = now.clone();
        doc.updated_at = now;
        self.open_doc(doc, false, true, cx);
    }

    // --------------------------------------------------------------------------------------
    // Library and files
    // --------------------------------------------------------------------------------------

    pub fn library(&self) -> Vec<LabelSummary> {
        self.store.list_labels().unwrap_or_default()
    }

    /// `openFromLibrary`.
    pub fn open_from_library(&mut self, id: &str, cx: &mut Context<Self>) {
        self.flush_autosave();
        match self.store.load_label(id).map(|raw| migrate(&raw)) {
            Some(Ok(m)) => self.open_doc(m.doc, m.read_only, false, cx),
            Some(Err(p)) => self.toast(Tone::Error, "Could not open the label", Some(p.join(" ")), cx),
            None => self.toast(Tone::Error, "That label no longer exists", None, cx),
        }
    }

    pub fn delete_label(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(e) = self.store.remove_label(id) {
            self.toast(Tone::Error, "Could not delete the label", Some(e.to_string()), cx);
        }
        if self.doc.id == id {
            self.new_label(cx);
        }
        cx.notify();
    }

    /// `importFile`.
    pub fn import_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.flush_autosave();
        match self.read_label_file(&path) {
            Ok((doc, read_only, notices)) => {
                let name = doc.name.clone();
                self.open_doc(doc, read_only, true, cx);
                let detail = (!notices.is_empty()).then(|| notices.join(" "));
                self.toast(Tone::Ok, format!("Imported “{name}”"), detail, cx);
            }
            Err(e) => self.toast(Tone::Error, "Could not import the file", Some(e.to_string()), cx),
        }
    }

    fn read_label_file(&self, path: &PathBuf) -> Result<(LabelDoc, bool, Vec<String>)> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut parsed = parse_label_file(&text).map_err(|p| anyhow::anyhow!(p.join(" ")))?;
        let mut notices = parsed.notices.clone();
        if self.store.has_label(&parsed.doc.id) {
            notices.push(baradar_core::doc::files::import_as_copy(&mut parsed.doc).to_owned());
        }
        for img in &parsed.images {
            self.store.put_blob(&img.bytes, &img.mime)?;
        }
        baradar_core::doc::files::internalize_images(&mut parsed.doc, &parsed.images);
        Ok((parsed.doc, parsed.read_only, notices))
    }

    /// A suggested file name for exporting the open label.
    pub fn export_file_name(&self) -> String {
        label_file_name(&self.doc.name)
    }

    /// `exportFile`: writes the label as `.ptlabel.json`.
    pub fn export_file(&mut self, path: PathBuf, include_wifi_passwords: bool, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let (text, notices) = serialize_label_file(&self.doc, |r| store.get_blob(r), include_wifi_passwords);
        match std::fs::write(&path, text) {
            Ok(()) => {
                if !notices.is_empty() {
                    self.toast(Tone::Info, "Label exported", Some(notices.join(" ")), cx);
                }
            }
            Err(e) => self.toast(Tone::Error, "Could not export the label", Some(e.to_string()), cx),
        }
    }

    pub fn set_name(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = if name.trim().is_empty() { "Untitled label" } else { name };
        let name: String = name.chars().take(80).collect();
        if name != self.doc.name {
            self.update_doc(Some("doc:name"), |d| d.name = name, cx);
        }
    }

    // --------------------------------------------------------------------------------------
    // Autosave
    // --------------------------------------------------------------------------------------

    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.autosave_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            let job = this
                .update(cx, |this, _| this.save_job())
                .ok()
                .flatten();
            let Some((store, doc, thumb)) = job else { return };
            let result = cx
                .background_executor()
                .spawn(async move { store.save_label(&doc, thumb.as_deref()).map(|_| ()) })
                .await;
            if let Err(e) = result {
                this.update(cx, |this, cx| {
                    this.toast(Tone::Error, "Could not save the label", Some(e.to_string()), cx)
                })
                .ok();
            }
        }));
    }

    /// The store, doc and a thumbnail for one save.
    fn save_job(&self) -> Option<(Store, LabelDoc, Option<Vec<u8>>)> {
        if self.read_only {
            return None;
        }
        let thumb = self
            .render
            .as_ref()
            .and_then(|r| baradar_core::export::png::export_png(&r.bitmap, self.model().dpi).ok());
        Some((self.store.clone(), self.doc.clone(), thumb))
    }

    /// Writes any pending autosave now (before opening another label).
    pub fn flush_autosave(&mut self) {
        if self.autosave_task.take().is_some()
            && let Some((store, doc, thumb)) = self.save_job()
        {
            let _ = store.save_label(&doc, thumb.as_deref());
        }
    }

    // --------------------------------------------------------------------------------------
    // Rendering
    // --------------------------------------------------------------------------------------

    /// Re-renders when the doc, target or preview row changed (50 ms debounce, stale renders
    /// cancelled).
    pub fn schedule_render(&mut self, cx: &mut Context<Self>) {
        let target = match self.target() {
            Ok(t) => t,
            Err(e) => {
                self.render = None;
                self.render_error = Some(e);
                self.rendering = false;
                self.rendered_key = None;
                cx.notify();
                return;
            }
        };
        let today = local_today();
        let key = (
            self.doc.clone(),
            target.profile.name,
            target.tape.id,
            self.preview_row,
            today.to_string(),
        );
        if self.rendered_key.as_ref() == Some(&key) && (self.render.is_some() || self.rendering) {
            return;
        }
        self.rendered_key = Some(key);
        if let Some(c) = self.render_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.render_cancel = Some(cancel.clone());
        self.rendering = true;
        let delay = if self.render.is_some() { RENDER_DELAY } else { Duration::ZERO };
        let resolved = resolve_doc(&self.doc, self.preview_row, &VariableContext::new(today))
            .doc
            .into_owned();
        let renderer = self.renderer.clone();
        self.render_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let flag = cancel.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    renderer.render(&resolved, &target, &RenderOptions { factor: 0, cancel: Some(&flag) })
                })
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            this.update(cx, |this, cx| {
                this.rendering = false;
                if let Ok(r) = result {
                    this.render = Some(Arc::new(r));
                    this.render_error = None;
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}


/// The nearest of `widths` to `w`.
fn nearest(widths: &[f64], w: f64) -> Option<f64> {
    widths
        .iter()
        .copied()
        .min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs()))
}

// ------------------------------------------------------------------------------------------
// Printer
// ------------------------------------------------------------------------------------------

/// `isConnected`: ready, printing, cancelling or busy with settings.
pub fn is_connected(state: ClientState) -> bool {
    matches!(
        state,
        ClientState::Connected | ClientState::Printing | ClientState::Cancelling | ClientState::Settings
    )
}

impl Studio {
    /// The connected printer's model (`printerModel`; none while lost, no-reply or in error).
    pub fn printer_model(&self) -> Option<&'static ModelProfile> {
        if !is_connected(self.conn) {
            return None;
        }
        self.printer_info.as_ref().and_then(|i| i.model)
    }

    fn on_printer_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        cx.emit(StudioEvent::Printer(ev.clone()));
        match ev {
            Event::State(s) => {
                let was = self.conn;
                self.conn = s;
                if !matches!(s, ClientState::Opening | ClientState::Waking) {
                    self.open_progress = None;
                }
                if is_connected(was) != is_connected(s) {
                    self.schedule_render(cx);
                }
            }
            Event::OpenProgress { attempt, of } => self.open_progress = Some((attempt, of)),
            Event::Opened(link) => {
                if let Some(ep) = link.endpoint {
                    self.prefs.last_endpoint = Some(ep.to_string());
                    self.prefs.auto_reconnect = true;
                    let _ = self.store.save_prefs(&self.prefs);
                }
            }
            Event::Connected(info) => {
                let model = info.model.map(|m| m.name).unwrap_or("printer");
                let tape = info.tape_width_text().unwrap_or_else(|| "no".into());
                self.toast(Tone::Ok, format!("Connected to {model}, {tape} tape"), None, cx);
                self.set_printer_info(info, cx);
            }
            Event::Status(info) | Event::StatusRefreshed(Ok(info)) => self.set_printer_info(info, cx),
            Event::StatusRefreshed(Err(f)) | Event::Error(f) | Event::ConnectFailed(f) => {
                self.problem = Some(f.problem);
            }
            Event::Problem(p) => self.problem = p,
            Event::Progress(p) => self.progress = Some(p),
            Event::PrintFinished(result) => {
                self.printing = false;
                self.progress = None;
                let pending = self.pending_print.take();
                match result {
                    Ok(()) => {
                        if let Some(p) = pending {
                            self.record_print(p);
                        }
                    }
                    Err(f) => {
                        self.toast(Tone::Error, f.problem.title.clone(), Some(f.problem.detail.clone()), cx);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn set_printer_info(&mut self, info: PrinterInfo, cx: &mut Context<Self>) {
        let changed = self.printer_info.as_ref().map(|i| (i.model.map(|m| m.name), i.tape.map(|t| t.id)))
            != Some((info.model.map(|m| m.name), info.tape.map(|t| t.id)));
        self.printer_info = Some(info);
        if changed {
            self.schedule_render(cx);
        }
    }

    /// Connects to the remembered printer, or discovers the paired one.
    pub fn connect(&mut self) {
        let target = self
            .prefs
            .last_endpoint
            .as_deref()
            .and_then(|e| e.parse().ok())
            .map_or(ConnectTarget::Auto, ConnectTarget::Endpoint);
        let _ = self.printer.send(Command::Connect(target));
    }

    pub fn connect_to(&mut self, target: ConnectTarget) {
        let _ = self.printer.send(Command::Connect(target));
    }

    pub fn reconnect(&mut self) {
        let _ = self.printer.send(Command::Reconnect);
    }

    pub fn cancel(&mut self) {
        let _ = self.printer.send(Command::Cancel);
    }

    /// `disconnect`: a deliberate disconnect also stops auto-reconnect.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let _ = self.printer.send(Command::Disconnect);
        self.prefs.auto_reconnect = false;
        let _ = self.store.save_prefs(&self.prefs);
        cx.notify();
    }

    pub fn dismiss_problem(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        let _ = self.printer.send(Command::DismissProblem);
        cx.notify();
    }

    /// `useLoadedTape`: re-tapes the label to the printer's cassette.
    pub fn use_loaded_tape(&mut self, cx: &mut Context<Self>) {
        let Some(tape) = self.printer_info.as_ref().and_then(|i| i.tape) else {
            return;
        };
        if tape.kind != TapeKind::Tze {
            self.toast(
                Tone::Warn,
                format!("The loaded {} media can’t be designed for yet", crate::view_model::format_mm(f64::from(tape.width_mm_x10) / 10.0)),
                Some("Load a TZe tape cassette (3.5–24 mm).".into()),
                cx,
            );
            return;
        }
        let w = f64::from(tape.width_mm_x10) / 10.0;
        self.set_tape_width(w, cx);
        self.toast(Tone::Ok, format!("Label switched to {} tape", crate::view_model::format_mm(w)), None, cx);
    }

    /// `mediaMismatch`: the loaded tape width differs from the design.
    pub fn mismatch(&self) -> Option<(f64, f64)> {
        let info = self.printer_info.as_ref().filter(|_| is_connected(self.conn))?;
        let loaded = f64::from(info.tape?.width_mm_x10) / 10.0;
        (width_byte(loaded) != width_byte(self.doc.tape.width_mm)).then_some((loaded, self.doc.tape.width_mm))
    }

    /// Labels the next print makes (batch rows × copies, or copies).
    pub fn label_count(&self) -> u32 {
        let copies = self.doc.print.copies.max(1);
        match self.batch_count() {
            0 => copies,
            n => (n as u32).saturating_mul(copies),
        }
    }

    /// `printBlockReason`: why Print is disabled, or `None`.
    pub fn print_block_reason(&self) -> Option<String> {
        if self.doc.items.is_empty() {
            return Some("Add something to the label first.".into());
        }
        if let Some(e) = &self.render_error {
            return Some(format!("{e} Fit it to a tape the printer takes to print it."));
        }
        let Some(render) = &self.render else {
            return Some("Preparing the preview…".into());
        };
        if render.blocking {
            let w = render
                .warnings
                .iter()
                .find(|w| w.blocking && w.code == "code-invalid")
                .or_else(|| render.warnings.iter().find(|w| w.blocking));
            return Some(w.map_or_else(|| "The label cannot be printed as designed.".into(), |w| w.message.clone()));
        }
        let missing = missing_variables(&self.doc);
        if !missing.is_empty() {
            let list: Vec<String> = missing.iter().map(|m| format!("{{{{{m}}}}}")).collect();
            return Some(format!(
                "Unknown variable{} {}: add a column or counter with that name, or fix the spelling.",
                if list.len() == 1 { "" } else { "s" },
                list.join(", ")
            ));
        }
        let empty_cols = columns_without_data(&self.doc);
        if let Some(c) = empty_cols.first() {
            return Some(format!(
                "The data table has no rows, so {{{{{c}}}}} would print blank: paste data or add a row."
            ));
        }
        match self.conn {
            ClientState::Printing | ClientState::Cancelling => return Some("A label is printing.".into()),
            ClientState::Settings => return Some("Busy: printer settings.".into()),
            ClientState::Opening | ClientState::Waking | ClientState::Handshaking => {
                return Some("Connecting to the printer…".into());
            }
            ClientState::NoReply => return Some("The printer is not answering. Wake it and reconnect.".into()),
            ClientState::Lost => return Some("The connection was lost. Reconnect first.".into()),
            ClientState::Error | ClientState::Idle => return Some("Connect a printer to print.".into()),
            ClientState::Connected => {}
        }
        let info = self.printer_info.as_ref()?;
        if info.tape.is_none() || !info.status.has_media() {
            return Some("No tape cassette is loaded.".into());
        }
        let errors = info.errors();
        if !errors.is_empty() {
            let list: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            return Some(format!("Printer reports: {}.", list.join(", ")));
        }
        if let Some((loaded, design)) = self.mismatch() {
            return Some(format!(
                "The printer has {} tape; this label is {}.",
                crate::view_model::format_mm(loaded),
                crate::view_model::format_mm(design)
            ));
        }
        None
    }

    /// `print()`: encodes the label (a batch as one job) and sends it to the printer.
    pub fn print(&mut self, cx: &mut Context<Self>) {
        if let Some(reason) = self.print_block_reason() {
            self.toast(Tone::Warn, reason, None, cx);
            return;
        }
        let Ok(target) = self.target() else { return };
        let Some(render) = self.render.clone() else { return };
        let copies = self.doc.print.copies.clamp(1, 99);
        let batch = self.batch_count();
        let pages: Vec<Bitmap> = if batch > 0 {
            let ctx = VariableContext::new(local_today());
            let mut cache: Vec<Option<Bitmap>> = vec![None; batch];
            let mut out = Vec::new();
            for i in batch_pages(&self.doc) {
                if cache[i].is_none() {
                    let resolved = resolve_doc(&self.doc, i, &ctx).doc.into_owned();
                    match self.renderer.render(&resolved, &target, &RenderOptions::default()) {
                        Ok(r) if !r.blocking => cache[i] = Some(r.bitmap),
                        Ok(r) => {
                            let msg = r.warnings.first().map(|w| w.message.clone()).unwrap_or_default();
                            self.toast(Tone::Error, format!("Label {} cannot be printed", i + 1), Some(msg), cx);
                            return;
                        }
                        Err(_) => return,
                    }
                }
                if let Some(b) = &cache[i] {
                    out.push(b.clone());
                }
            }
            out
        } else {
            vec![render.bitmap.clone(); copies as usize]
        };
        let opts = JobOptions {
            cut: if self.doc.print.auto_cut { CutMode::EveryLabel } else { CutMode::None },
            chain: self.doc.print.chain,
            mirror: self.doc.print.mirror,
            ..JobOptions::default()
        };
        let job = ptouch::prepare_pages(target.profile, target.tape, &pages, &opts).and_then(|(pre, o)| {
            let refs: Vec<&Bitmap> = pre.iter().collect();
            ptouch::encode_job(target.profile, target.tape, &refs, &o)
        });
        let job = match job {
            Ok(j) => j,
            Err(e) => {
                self.toast(Tone::Error, "The label could not be prepared for printing", Some(e.to_string()), cx);
                return;
            }
        };
        let dpi = target.profile.dpi;
        let feed_mm = render.feed_margin_mm;
        let labels_mm: f64 = pages
            .iter()
            .map(|p| baradar_core::render::units::dots_to_mm(f64::from(p.length()), dpi) + 2.0 * feed_mm)
            .sum();
        self.pending_print = Some(PendingPrint {
            doc: self.doc.clone(),
            labels: pages.len() as u32,
            copies,
            batch_rows: (batch > 0).then_some(batch as u32),
            tape_mm: labels_mm + if self.doc.print.chain { 0.0 } else { LEADER_MM },
            width_mm: self.doc.tape.width_mm,
            media_id: target.tape.id.to_owned(),
            model: target.profile.name.to_owned(),
            thumb: baradar_core::export::png::export_png(&render.bitmap, dpi).ok(),
        });
        self.printing = true;
        self.progress = None;
        if self.printer.send(Command::Print(job)).is_err() {
            self.printing = false;
            self.pending_print = None;
        }
        cx.notify();
    }

    /// Adds a finished job to the print history and the tape counter.
    fn record_print(&mut self, p: PendingPrint) {
        let record = HistoryRecord {
            id: new_record_id(),
            printed_at: doc::now_iso(),
            doc: serde_json::to_value(&p.doc).unwrap_or_default(),
            name: p.doc.name.clone(),
            tape_width_mm: p.width_mm,
            media_id: Some(p.media_id),
            model: Some(p.model),
            labels: p.labels,
            copies: p.copies,
            batch_rows: p.batch_rows,
            tape_mm: (p.tape_mm * 10.0).round() / 10.0,
            seq: self.now_ms(),
        };
        let _ = self.store.add_history(&record, p.thumb.as_deref());
        let _ = self.store.add_usage(p.width_mm, p.tape_mm, u64::from(p.labels));
    }
}
