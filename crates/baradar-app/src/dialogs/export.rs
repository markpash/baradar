//! Export image dialog: the exact print bitmap as a 1-bit PNG at the print dpi, or a true-size
//! PDF with the tape width included (port of ui/export/ExportDialog.svelte). For a batch, the
//! export is the label shown in the preview; the dialog lets you pick which one.

use std::path::PathBuf;

use baradar_core::doc::files::{FILE_EXTENSION, label_file_name};
use baradar_core::export::pdf::{PdfExportOptions, export_pdf, pdf_page_mm};
use baradar_core::export::png::{export_base_name, export_png};
use baradar_core::render::units::mm_to_dots;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::Button,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::common::{hint, save_dir, toast};
use crate::studio::{Studio, Tone};

pub fn open_export_image(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ExportDialog::new(studio, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Export image").w(px(520.)).child(view.clone())
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Png,
    Pdf,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

/// `formatMm(mm, digits)`: "52.3 mm", "0.141 mm", "24 mm" (a trailing ".0" dropped).
fn fmt_mm(mm: f64, digits: usize) -> String {
    format!("{} mm", num(mm, digits))
}

/// `num`: [`fmt_mm`] without the unit.
fn num(mm: f64, digits: usize) -> String {
    let s = format!("{mm:.digits$}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

/// `baseName()` + extension: "cable-tags.png", "cable-tags-3.pdf" for batch label 3.
fn export_file_name(doc_name: &str, batch_count: usize, preview_row: usize, kind: Kind) -> String {
    let file = label_file_name(doc_name);
    let slug = file.strip_suffix(FILE_EXTENSION).unwrap_or(&file);
    format!("{}.{}", export_base_name(slug, batch_count, preview_row), kind.ext())
}

struct ExportDialog {
    studio: Entity<Studio>,
    busy: Option<Kind>,
    status: String,
    _subs: Vec<Subscription>,
}

impl ExportDialog {
    fn new(studio: Entity<Studio>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&studio, |_, _, cx| cx.notify())];
        Self { studio, busy: None, status: String::new(), _subs: subs }
    }

    /// Shows batch label `row` in the preview (and so in the export).
    fn set_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            let n = s.batch_count();
            if n == 0 {
                return;
            }
            s.preview_row = row.min(n - 1);
            s.schedule_render(cx);
            cx.notify();
        });
    }

    /// `save(kind)`: encodes the current bitmap, asks where to save it, writes it.
    fn save(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let s = self.studio.read(cx);
        let (Some(render), Ok(target)) = (s.render.clone(), s.target()) else { return };
        let area = target.area();
        let bytes = match kind {
            Kind::Png => export_png(&render.bitmap, area.dpi),
            Kind::Pdf => export_pdf(
                &render.bitmap,
                &PdfExportOptions {
                    dpi: area.dpi,
                    tape_width_dots: u32::try_from(area.tape_width_dots).unwrap_or(0),
                    margin_dots: u32::try_from(mm_to_dots(render.feed_margin_mm, area.dpi)).unwrap_or(0),
                    title: Some(s.doc.name.clone()),
                },
            ),
        };
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                let what = kind.ext().to_uppercase();
                toast(window, cx, Tone::Error, format!("Could not export the {what}"), Some(e.to_string()));
                return;
            }
        };
        let name = export_file_name(&s.doc.name, s.batch_count(), s.preview_row, kind);
        self.busy = Some(kind);
        cx.notify();
        let rx = cx.prompt_for_new_path(&save_dir(), Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let written: Option<Result<PathBuf, String>> = match rx.await {
                Ok(Ok(Some(path))) => Some(std::fs::write(&path, &bytes).map(|()| path).map_err(|e| e.to_string())),
                Ok(Err(e)) => Some(Err(e.to_string())),
                _ => None,
            };
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match written {
                    Some(Ok(path)) => {
                        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(name);
                        this.status = format!("Saved {file}");
                    }
                    Some(Err(e)) => {
                        let what = kind.ext().to_uppercase();
                        toast(window, cx, Tone::Error, format!("Could not export the {what}"), Some(e));
                    }
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choice(&self, kind: Kind, title: &'static str, detail: String, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let busy = self.busy.is_some();
        let label = if self.busy == Some(kind) {
            format!("Saving {}…", kind.ext().to_uppercase())
        } else {
            title.to_owned()
        };
        let icon = match kind {
            Kind::Png => gpui_kit::assets::IconName::Image,
            Kind::Pdf => gpui_kit::assets::IconName::Download,
        };
        h_flex()
            .id(kind.ext())
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme.accent))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.save(kind, window, cx)))
            })
            .when(busy, |d| d.opacity(0.6))
            .child(Icon::new(icon).large())
            .child(
                v_flex()
                    .flex_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(detail)),
            )
            .into_any_element()
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.studio.read(cx);
        let render = s.render.clone();
        let target = s.target().ok();
        let ready = render.as_ref().is_some_and(|r| r.length_dots > 0) && target.is_some() && s.render_error.is_none();
        let intro = hint("The exact dots that are printed, as an image file.", cx);
        let (Some(render), Some(target), true) = (render, target, ready) else {
            let why = match &s.render_error {
                Some(e) => format!("The label cannot be drawn: {e}"),
                None => "The preview is still being prepared…".to_owned(),
            };
            return v_flex().gap_3().child(intro).child(hint(why, cx));
        };
        let area = target.area();
        let dpi = f64::from(area.dpi);
        let margin = u32::try_from(mm_to_dots(render.feed_margin_mm, area.dpi)).unwrap_or(0);
        let page = pdf_page_mm(
            u32::try_from(render.length_dots).unwrap_or(0),
            area.dpi,
            u32::try_from(area.tape_width_dots).unwrap_or(0),
            margin,
        );
        let batch = s.batch_count();
        let row = s.preview_row;
        let fact = |term: &'static str, value: String| {
            h_flex()
                .gap_3()
                .text_sm()
                .child(div().w(px(110.)).text_color(cx.theme().muted_foreground).child(term))
                .child(div().flex_1().child(value))
        };
        let facts = v_flex()
            .gap_1()
            .child(fact(
                "Printed area",
                format!(
                    "{} × {} ({} × {} dots)",
                    num(render.length_dots as f64 * 25.4 / dpi, 1),
                    fmt_mm(render.height_dots as f64 * 25.4 / dpi, 1),
                    render.length_dots,
                    render.height_dots
                ),
            ))
            .child(fact(
                "Resolution",
                format!("{} dpi (1 dot = {}) · {}", area.dpi, fmt_mm(25.4 / dpi, 3), area.model),
            ))
            .child(fact(
                "Cut label",
                format!("{} × {} on {} tape", num(page.0, 1), fmt_mm(page.1, 1), fmt_mm(s.doc.tape.width_mm, 1)),
            ));
        let dpi_text = area.dpi;
        v_flex()
            .gap_3()
            .child(intro)
            .child(facts)
            .when(batch > 0, |d| {
                d.child(
                    v_flex()
                        .gap_2()
                        .child(hint(
                            format!(
                                "This label has a batch: the export shows label {} of {batch}, the one in the preview.",
                                row + 1
                            ),
                            cx,
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("prev-row")
                                        .small()
                                        .icon(gpui_kit::component::IconName::ChevronLeft)
                                        .disabled(row == 0)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.set_row(row.saturating_sub(1), cx)
                                        })),
                                )
                                .child(div().text_sm().child(format!("Label {} of {batch}", row + 1)))
                                .child(
                                    Button::new("next-row")
                                        .small()
                                        .icon(gpui_kit::component::IconName::ChevronRight)
                                        .disabled(row + 1 >= batch)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_row(row + 1, cx))),
                                ),
                        ),
                )
            })
            .child(self.choice(
                Kind::Png,
                "Save PNG…",
                format!("Black and white image of the printed area. Opens at real size ({dpi_text} dpi)."),
                cx,
            ))
            .child(self.choice(
                Kind::Pdf,
                "Save PDF…",
                "The whole label at true size, tape width included. Print it at 100 % to check the size.".into(),
                cx,
            ))
            .when(!self.status.is_empty(), |d| d.child(hint(self.status.clone(), cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, export_file_name, fmt_mm, num};

    #[test]
    fn file_names_follow_the_label_name() {
        assert_eq!(export_file_name("Cable tags", 0, 0, Kind::Png), "cable-tags.png");
        assert_eq!(export_file_name("Cable tags", 25, 2, Kind::Pdf), "cable-tags-3.pdf");
        assert_eq!(export_file_name("", 0, 0, Kind::Pdf), "label.pdf");
    }

    #[test]
    fn millimetres_like_format_mm() {
        assert_eq!(fmt_mm(24.0, 1), "24 mm");
        assert_eq!(fmt_mm(25.4 / 180.0, 3), "0.141 mm");
        assert_eq!(num(52.34, 1), "52.3");
    }
}
