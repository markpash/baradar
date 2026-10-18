//! Print history dialog and tape counter (port of ui/history/HistoryDialog.svelte): "Tape used"
//! per width with Reset counter, and the last 50 printed labels with Open, Reprint and Remove,
//! plus Clear history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use baradar_core::doc::migrate::migrate;
use baradar_core::doc::{self, LabelDoc};
use baradar_core::store::{HistoryRecord, Usage};
use baradar_core::templates::needs_width_text;
use baradar_core::vars::batch::format_tape;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::common::{confirm, day, heading, hint, png_image, thumb, toast, when};
use crate::studio::{Studio, Tone};
use crate::view_model::format_mm;

/// How long Reprint waits for the reopened label's render.
const REPRINT_RENDER_WAIT: Duration = Duration::from_secs(10);

pub fn open_history(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| HistoryDialog::new(studio, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Print history").w(px(720.)).child(view.clone())
    });
}

/// How a print record was reopened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reopened {
    /// As itself (the library holds that version, or no longer holds the label).
    AsItself,
    /// As a new copy, because the label changed after the print.
    AsCopy,
}

/// `sameDesign`: equal apart from the timestamps.
fn same_design(a: &LabelDoc, b: &LabelDoc) -> bool {
    let strip = |d: &LabelDoc| {
        let mut d = d.clone();
        d.created_at.clear();
        d.updated_at.clear();
        d
    };
    strip(a) == strip(b)
}

/// "24 mm tape", plus " · PT-P910BT" when printed on another model than the active one.
/// `tapeText`.
fn tape_text(r: &HistoryRecord, active_model: &str) -> String {
    let model = r
        .model
        .as_deref()
        .filter(|m| *m != active_model)
        .map(|m| format!(" · {m}"))
        .unwrap_or_default();
    format!("{} tape{model}", format_mm(r.tape_width_mm))
}

/// "1 label", "3 labels", "10 batch labels × 2". `labelsText`.
fn labels_text(r: &HistoryRecord) -> String {
    if let Some(rows) = r.batch_rows {
        let copies = if r.copies > 1 { format!(" × {}", r.copies) } else { String::new() };
        return format!("{rows} batch {}{copies}", if rows == 1 { "label" } else { "labels" });
    }
    if r.labels == 1 { "1 label".into() } else { format!("{} labels", r.labels) }
}

/// Counter rows sorted by width: (width mm, key, entry). `usageRows`.
fn usage_rows(usage: &Usage) -> Vec<(f64, &str, baradar_core::store::WidthUsage)> {
    let mut rows: Vec<_> = usage
        .by_width
        .iter()
        .filter_map(|(k, e)| k.parse::<f64>().ok().map(|w| (w, k.as_str(), *e)))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows
}

impl Studio {
    /// `openRecord`: opens the label as printed. As itself when the library still holds that
    /// version (or no longer holds the label), else as a copy named "<name> (printed <date>)",
    /// so later edits are never overwritten.
    pub fn open_print_record(&mut self, r: &HistoryRecord, cx: &mut Context<Self>) -> Result<Reopened, String> {
        self.flush_autosave();
        let printed = migrate(&r.doc).map_err(|p| p.join(" "))?;
        if self.doc.id == printed.doc.id && same_design(&self.doc, &printed.doc) {
            return Ok(Reopened::AsItself);
        }
        let saved = self.store.load_label(&printed.doc.id).and_then(|raw| migrate(&raw).ok());
        match saved {
            None => {
                let save = !printed.read_only;
                self.open_doc(printed.doc, printed.read_only, save, cx);
                Ok(Reopened::AsItself)
            }
            Some(s) if same_design(&s.doc, &printed.doc) => {
                self.open_doc(s.doc, s.read_only, false, cx);
                Ok(Reopened::AsItself)
            }
            Some(_) => {
                let now = doc::now_iso();
                let mut copy = printed.doc;
                copy.id = doc::new_id();
                copy.name = format!("{} (printed {})", copy.name, day(&r.printed_at));
                copy.created_at = now.clone();
                copy.updated_at = now;
                self.open_doc(copy, false, true, cx);
                Ok(Reopened::AsCopy)
            }
        }
    }
}

struct HistoryDialog {
    studio: Entity<Studio>,
    records: Vec<HistoryRecord>,
    thumbs: HashMap<String, Option<Arc<Image>>>,
    usage: Usage,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl HistoryDialog {
    fn new(studio: Entity<Studio>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify())];
        let usage = studio.read(cx).store.load_usage();
        let mut this = Self {
            studio,
            records: Vec::new(),
            thumbs: HashMap::new(),
            usage,
            error: None,
            _subs: subs,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let store = self.studio.read(cx).store.clone();
        match store.list_history() {
            Ok(records) => {
                self.error = None;
                self.records = records;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.records.clear();
            }
        }
        self.thumbs = self
            .records
            .iter()
            .map(|r| (r.id.clone(), store.history_thumbnail(&r.id).map(png_image)))
            .collect();
        self.usage = store.load_usage();
        cx.notify();
    }

    /// Opens the record, toasting how; `false` when nothing could be opened.
    fn open_record(&mut self, r: &HistoryRecord, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.studio.update(cx, |s, cx| s.open_print_record(r, cx)) {
            Ok(Reopened::AsItself) => true,
            Ok(Reopened::AsCopy) => {
                toast(
                    window,
                    cx,
                    Tone::Info,
                    "Opened as a copy",
                    Some("The label was changed after this print, so the printed version opens as a new label.".into()),
                );
                true
            }
            Err(e) => {
                toast(window, cx, Tone::Error, "Could not open the label", Some(e));
                false
            }
        }
    }

    fn reopen(&mut self, r: &HistoryRecord, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_record(r, window, cx) {
            window.close_dialog(cx);
        }
    }

    /// `reprint(r)`: opens the label, closes the dialog, waits for its render (bounded), then
    /// prints it unless an image is gone or the print gate says no.
    fn reprint(&mut self, r: &HistoryRecord, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open_record(r, window, cx) {
            return;
        }
        window.close_dialog(cx);
        let studio = self.studio.clone();
        window
            .spawn(cx, async move |cx| {
                let end = Instant::now() + REPRINT_RENDER_WAIT;
                loop {
                    let rendering = cx.update(|_, cx| studio.read(cx).rendering).unwrap_or(false);
                    if !rendering || Instant::now() >= end {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                }
                cx.update(|window, cx| {
                    let s = studio.read(cx);
                    let image_missing = s
                        .render
                        .as_ref()
                        .is_some_and(|r| r.warnings.iter().any(|w| w.code == "image-missing"));
                    if image_missing {
                        toast(
                            window,
                            cx,
                            Tone::Error,
                            "Not reprinted",
                            Some("An image of this label is no longer available. The label is open: add the image again, then print.".into()),
                        );
                    } else if let Some(reason) = s.print_block_reason() {
                        toast(window, cx, Tone::Info, "Can’t print yet", Some(reason));
                    } else {
                        studio.update(cx, |s, cx| s.print(cx));
                    }
                })
                .ok();
            })
            .detach();
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let store = self.studio.read(cx).store.clone();
        if let Err(e) = store.remove_history(id) {
            self.error = Some(e.to_string());
        }
        self.refresh(cx);
    }

    fn clear_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        confirm(
            window,
            cx,
            "Clear the whole print history? Your saved labels are not affected.",
            "Clear history",
            move |_, cx| {
                this.update(cx, |this, cx| {
                    let store = this.studio.read(cx).store.clone();
                    if let Err(e) = store.clear_history() {
                        this.error = Some(e.to_string());
                    }
                    this.refresh(cx);
                })
                .ok();
            },
        );
    }

    fn reset_counter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        confirm(window, cx, "Reset the tape usage counter to zero?", "Reset counter", move |window, cx| {
            this.update(cx, |this, cx| {
                let store = this.studio.read(cx).store.clone();
                match store.reset_usage() {
                    Ok(u) => this.usage = u,
                    Err(e) => toast(window, cx, Tone::Error, "Could not reset the counter", Some(e.to_string())),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn usage_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let rows = usage_rows(&self.usage);
        let since = day(&self.usage.since);
        let total: f64 = rows.iter().map(|r| r.2.mm).sum();
        let cell = |text: String, w: f32, bold: bool| {
            div()
                .w(px(w))
                .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(text)
        };
        let table = v_flex()
            .text_sm()
            .child(
                h_flex()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .child(cell("Tape".into(), 100., false))
                    .child(cell("Used".into(), 100., false))
                    .child(cell("Labels".into(), 100., false))
                    .child(cell("Jobs".into(), 100., false)),
            )
            .children(rows.iter().map(|(_, key, e)| {
                h_flex()
                    .py_1()
                    .child(cell(format!("{key} mm"), 100., true))
                    .child(cell(format_tape(e.mm), 100., false))
                    .child(cell(e.labels.to_string(), 100., false))
                    .child(cell(e.jobs.to_string(), 100., false))
            }));
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .child(heading("Tape used"))
                    .child(
                        Button::new("reset-counter")
                            .small()
                            .icon(crate::icons::icon("refresh")).label("Reset counter")
                            .disabled(rows.is_empty())
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.reset_counter(window, cx))),
                    ),
            )
            .map(|d| {
                if rows.is_empty() {
                    d.child(hint(format!("Nothing printed since {since}."), cx))
                } else {
                    d.child(table).child(hint(
                        format!(
                            "{} in total since {since}, leaders included (about 24 mm per job).",
                            format_tape(total)
                        ),
                        cx,
                    ))
                }
            })
            .into_any_element()
    }

    fn record_row(&self, ix: usize, r: &HistoryRecord, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let s = self.studio.read(cx);
        let model = s.model().name;
        let needs = needs_width_text(r.tape_width_mm, model, &s.tape_widths());
        let printing = s.printing;
        let image = self.thumbs.get(&r.id).cloned().flatten();
        let (open_r, print_r, remove_id) = (r.clone(), r.clone(), r.id.clone());
        h_flex()
            .id(("record", ix))
            .gap_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(thumb(image, px(96.), px(48.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).truncate().child(r.name.clone()))
                    .child(div().text_color(theme.muted_foreground).child(when(&r.printed_at)))
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "{} · {} · {}",
                        tape_text(r, model),
                        labels_text(r),
                        format_tape(r.tape_mm)
                    )))
                    .when_some(needs, |d, n| d.child(div().text_color(theme.warning).child(n))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new(("open", ix))
                            .small()
                            .icon(crate::icons::icon("folder")).label("Open")
                            .tooltip(format!("Open {}", r.name))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.reopen(&open_r, window, cx))),
                    )
                    .child(
                        Button::new(("reprint", ix))
                            .small()
                            .primary()
                            .icon(gpui_kit::assets::IconName::Printer)
                            .label("Reprint")
                            .tooltip(format!("Reprint {}", r.name))
                            .disabled(printing)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.reprint(&print_r, window, cx))),
                    )
                    .child(
                        Button::new(("remove", ix))
                            .small()
                            .ghost()
                            .icon(gpui_kit::component::IconName::Close)
                            .tooltip(format!("Remove {} from the history", r.name))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove(&remove_id, cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for HistoryDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let records = self.records.clone();
        let list = if records.is_empty() {
            hint(
                "Labels you print show up here, so you can print them again in one click. The last 50 are kept.",
                cx,
            )
        } else {
            v_flex()
                .children(records.iter().enumerate().map(|(ix, r)| self.record_row(ix, r, cx)))
                .into_any_element()
        };
        v_flex()
            .gap_4()
            .child(hint("Labels printed from this computer, newest first.", cx))
            .child(self.usage_section(cx))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(heading("Printed labels"))
                            .child(
                                Button::new("clear-history")
                                    .small()
                                    .icon(gpui_kit::assets::IconName::Trash)
                                    .label("Clear history")
                                    .disabled(records.is_empty())
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.clear_all(window, cx))),
                            ),
                    )
                    .when_some(self.error.clone(), |d, e| d.child(div().text_sm().text_color(cx.theme().danger).child(e)))
                    .child(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{labels_text, same_design, tape_text, usage_rows};
    use baradar_core::doc::LabelDoc;
    use baradar_core::store::{HistoryRecord, Usage, WidthUsage};

    fn record() -> HistoryRecord {
        HistoryRecord {
            id: "r".into(),
            printed_at: "2026-10-18T10:00:00.000Z".into(),
            doc: serde_json::json!({}),
            name: "Spices".into(),
            tape_width_mm: 12.0,
            media_id: None,
            model: Some("PT-P910BT".into()),
            labels: 1,
            copies: 1,
            batch_rows: None,
            tape_mm: 60.0,
            seq: 0,
        }
    }

    #[test]
    fn record_texts() {
        let mut r = record();
        assert_eq!(tape_text(&r, "PT-P710BT"), "12 mm tape · PT-P910BT");
        assert_eq!(tape_text(&r, "PT-P910BT"), "12 mm tape");
        assert_eq!(labels_text(&r), "1 label");
        r.labels = 3;
        assert_eq!(labels_text(&r), "3 labels");
        r.batch_rows = Some(10);
        r.copies = 2;
        assert_eq!(labels_text(&r), "10 batch labels × 2");
        r.batch_rows = Some(1);
        r.copies = 1;
        assert_eq!(labels_text(&r), "1 batch label");
    }

    #[test]
    fn designs_compare_without_timestamps() {
        let a = LabelDoc::new();
        let mut b = a.clone();
        b.updated_at = "2030-01-01T00:00:00.000Z".into();
        assert!(same_design(&a, &b));
        b.name = "Other".into();
        assert!(!same_design(&a, &b));
    }

    #[test]
    fn usage_rows_sort_numerically() {
        let mut usage = Usage::default();
        for k in ["24", "3.5", "12"] {
            usage.by_width.insert(k.into(), WidthUsage { mm: 1.0, labels: 1, jobs: 1 });
        }
        let keys: Vec<&str> = usage_rows(&usage).iter().map(|r| r.1).collect();
        assert_eq!(keys, ["3.5", "12", "24"]);
    }
}
