//! The label preview (port of `web/src/ui/preview/Preview.svelte` and `render/preview.ts`).
//!
//! The preview *is* the print bitmap: the render's 1-bit dots are painted as merged runs of
//! quads (GPUI samples images linearly, which would blur the dots), tinted tape/ink, on a
//! to-scale tape with feed margins and cut lines. Clicking selects the block under the pointer.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use baradar_core::render::renderer::{PrintArea, RenderResult};
use baradar_core::render::units::{css_to_rgb, dots_to_mm, mm_to_dots};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use crate::app::StudioView;
use crate::studio::{Studio, Zoom, is_connected};
use crate::view_model::{DEFAULT_COLORS, format_mm, real_scale, ruler_steps};

/// Left/right padding of the stage, logical px (the studio subtracts 48 for the fit scale).
const PAD: f32 = 24.0;
/// Height the fit scale aims for, logical px.
const FIT_HEIGHT: f32 = 300.0;
/// Space under the tape for the ruler.
const RULER_H: f32 = 30.0;

/// What the last paint used, for hit-testing clicks and zoom steps.
#[derive(Debug, Clone, Copy, Default)]
struct Geom {
    origin_x: f32,
    origin_y: f32,
    /// Logical px per dot.
    scale: f32,
    /// Feed margin (dots) before the label.
    feed: f32,
    /// Offset of the printable band inside the tape (dots).
    band_offset: f32,
    /// `scale / real scale` of the last fit.
    fit_ratio: f32,
}

#[derive(Clone, Default)]
pub struct PreviewState {
    geom: Rc<Cell<Geom>>,
    /// Exact-dots view (dot grid) instead of the design view.
    pub exact: Rc<Cell<bool>>,
}

impl PreviewState {
    /// The fit scale as a multiple of real size (for zoom steps from Fit).
    pub fn fit_ratio(&self) -> f32 {
        let r = self.geom.get().fit_ratio;
        if r > 0.0 { r } else { 1.0 }
    }
}

/// `previewColors`: the printer's colours when its tape is the design's, else the doc's.
fn preview_colors(s: &Studio) -> (Hsla, Hsla) {
    let printer = s
        .printer_info
        .as_ref()
        .filter(|_| is_connected(s.conn) && s.mismatch().is_none())
        .map(|i| (i.tape_color().css(), i.text_color().css()))
        .filter(|(t, k)| t.starts_with('#') && k.starts_with('#') && t != k);
    let (tape, ink) = match (printer, &s.doc.tape.colors) {
        (Some((t, k)), _) => (t.to_owned(), k.to_owned()),
        (None, Some(c)) => (c.tape.clone(), c.ink.clone()),
        (None, None) => (DEFAULT_COLORS.0.to_owned(), DEFAULT_COLORS.1.to_owned()),
    };
    (rgb(css_to_rgb(&tape)).into(), rgb(css_to_rgb(&ink)).into())
}

struct PaintInput {
    render: Arc<RenderResult>,
    area: PrintArea,
    tape: Hsla,
    ink: Hsla,
    line: Hsla,
    accent: Hsla,
    muted: Hsla,
    selected: Option<(i64, i64, i64, i64)>,
    margins: (i64, i64),
    zoom: Zoom,
    exact: bool,
    design: bool,
}

impl StudioView {
    pub fn preview_element(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = self.preview_toolbar(cx);
        let s = self.studio.read(cx);
        let theme = cx.theme().clone();
        let target = s.target();
        let Some(render) = s.render.clone() else {
            let msg = match &s.render_error {
                Some(e) => format!("Preview unavailable: {e}"),
                None => "Rendering…".into(),
            };
            return v_flex()
                .gap_2()
                .child(toolbar)
                .child(div().h(px(160.)).flex().items_center().justify_center().text_color(theme.muted_foreground).child(msg))
                .into_any_element();
        };
        let Ok(target) = target else {
            return v_flex().child(toolbar).into_any_element();
        };
        let area = target.area();
        let (tape, ink) = preview_colors(s);
        let selected = s
            .selected
            .as_deref()
            .and_then(|id| render.boxes.iter().find(|b| b.item_id == id))
            .map(|b| (b.x, b.y, b.w, b.h));
        let input = PaintInput {
            render: render.clone(),
            area: area.clone(),
            tape,
            ink,
            line: theme.muted_foreground,
            accent: theme.primary,
            muted: theme.muted_foreground.opacity(0.18),
            selected,
            margins: (
                mm_to_dots(s.doc.margins_mm.start, area.dpi),
                mm_to_dots(s.doc.margins_mm.end, area.dpi),
            ),
            zoom: s.zoom,
            exact: self.preview.exact.get(),
            design: !self.preview.exact.get(),
        };
        let total_dots = render.length_dots + 2 * area.default_feed_dots;
        let tape_dots = area.tape_width_dots.max(render.height_dots);
        let (canvas_w, canvas_h) = match s.zoom {
            Zoom::Fit => (None, FIT_HEIGHT + RULER_H + 16.0),
            Zoom::Real(z) => {
                let sc = z * real_scale(area.dpi);
                (
                    Some(total_dots as f32 * sc + 2.0 * PAD),
                    tape_dots as f32 * sc + RULER_H + 16.0,
                )
            }
        };
        let geom = self.preview.geom.clone();
        let paint_geom = geom.clone();
        let canvas_el = canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, window, cx| paint_preview(&input, bounds, &paint_geom, window, cx),
        )
        .h(px(canvas_h));
        let canvas_el = match canvas_w {
            Some(w) => canvas_el.w(px(w)),
            None => canvas_el.w_full(),
        };
        let boxes = render.boxes.clone();
        let stage = div()
            .id("preview-stage")
            .w_full()
            .overflow_x_scroll()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.4))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let g = geom.get();
                    if g.scale <= 0.0 {
                        return;
                    }
                    let x = (f32::from(ev.position.x) - g.origin_x) / g.scale - g.feed;
                    let y = (f32::from(ev.position.y) - g.origin_y) / g.scale - g.band_offset;
                    let hit = boxes
                        .iter()
                        .rev()
                        .find(|b| x >= b.x as f32 && x < (b.x + b.w) as f32 && y >= b.y as f32 && y < (b.y + b.h) as f32)
                        .map(|b| b.item_id.clone());
                    if hit.is_some() {
                        this.studio.update(cx, |s, cx| s.select(hit, cx));
                    }
                }),
            )
            .child(canvas_el);

        let feed_mm = render.feed_margin_mm;
        let total_mm = render.length_mm + 2.0 * feed_mm;
        let band_mm = dots_to_mm(render.height_dots as f64, area.dpi);
        let tape_mm = s.doc.tape.width_mm;
        let readout = h_flex()
            .gap_2()
            .flex_wrap()
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} label", format_mm(total_mm))))
            .child(div().text_color(theme.muted_foreground).child(format!(
                "{} printed + 2 × {} feed · {} tape · printable {} ({} dots)",
                format_mm(render.length_mm),
                format_mm(feed_mm),
                format_mm(tape_mm),
                format_mm(band_mm),
                render.height_dots
            )))
            .when(s.doc.print.mirror, |d| d.child(div().text_xs().text_color(theme.warning).child("Mirrored when printed")));
        let item_ids: Vec<String> = s.doc.items.iter().map(|i| i.id().to_owned()).collect();
        let warnings = v_flex().gap_1().children(
            render
                .warnings
                .iter()
                .filter(|w| w.item_id.as_ref().is_none_or(|id| !item_ids.contains(id)) || w.blocking)
                .map(|w| {
                    h_flex()
                        .gap_1()
                        .text_sm()
                        .text_color(if w.blocking { theme.danger } else { theme.warning })
                        .child(crate::icons::icon("alert").small())
                        .child(w.message.clone())
                }),
        );
        v_flex().gap_2().child(toolbar).child(stage).child(readout).child(warnings).into_any_element()
    }

    fn preview_toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.studio.read(cx);
        let theme = cx.theme().clone();
        let exact = self.preview.exact.get();
        let zoom_text = match s.zoom {
            Zoom::Fit => format!("Fit · {} %", (self.preview.fit_ratio() * 100.0).round()),
            Zoom::Real(z) => format!("{} %", (z * 100.0).round()),
        };
        let real = matches!(s.zoom, Zoom::Real(z) if (z - 1.0).abs() < 1e-3);
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().text_color(theme.muted_foreground).child("PREVIEW"))
            .child(
                Button::new("view-design")
                    .label("Design")
                    .small()
                    .when(!exact, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.exact.set(false);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("view-dots")
                    .label("Exact dots")
                    .small()
                    .when(exact, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preview.exact.set(true);
                        cx.notify();
                    })),
            )
            .when(s.rendering, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child("⟳ Updating")))
            .child(div().flex_1())
            .child(
                Button::new("zoom-out")
                    .icon(crate::icons::icon("minus"))
                    .small()
                    .ghost()
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::ZoomOut), cx)),
            )
            .child(
                Button::new("zoom-fit")
                    .label(zoom_text)
                    .small()
                    .ghost()
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::ZoomFit), cx)),
            )
            .child(
                Button::new("zoom-in")
                    .icon(crate::icons::icon("plus"))
                    .small()
                    .ghost()
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::ZoomIn), cx)),
            )
            .child(
                Button::new("zoom-real")
                    .label("1:1")
                    .small()
                    .when(real, |b| b.primary())
                    .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::ZoomReal), cx)),
            ).into_any_element()
    }
}

fn paint_preview(input: &PaintInput, bounds: Bounds<Pixels>, geom: &Rc<Cell<Geom>>, window: &mut Window, cx: &mut App) {
    let r = &input.render;
    let area = &input.area;
    let feed = area.default_feed_dots as f32;
    let total = r.length_dots as f32 + 2.0 * feed;
    let band = r.height_dots as f32;
    let tape_dots = (area.tape_width_dots as f32).max(band);
    let band_offset = (tape_dots - band) / 2.0;
    let real = real_scale(area.dpi);
    let bw = f32::from(bounds.size.width);
    let fit = ((bw - 2.0 * PAD) / total).min(FIT_HEIGHT / tape_dots).clamp(0.25 * real, 8.0 * real);
    let s = match input.zoom {
        Zoom::Fit => fit,
        Zoom::Real(z) => z * real,
    };
    let content_w = total * s;
    let ox = f32::from(bounds.origin.x)
        + match input.zoom {
            Zoom::Fit => ((bw - content_w) / 2.0).max(PAD),
            Zoom::Real(_) => PAD,
        };
    let oy = f32::from(bounds.origin.y) + 8.0;
    geom.set(Geom {
        origin_x: ox,
        origin_y: oy,
        scale: s,
        feed,
        band_offset,
        fit_ratio: fit / real,
    });
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(point(px(x), px(y)), size(px(w.max(0.0)), px(h.max(0.0))));

    // Tape and the unprintable edges.
    window.paint_quad(fill(rect(ox, oy, content_w, tape_dots * s), input.tape));
    let band_y = oy + band_offset * s;
    let label_x = ox + feed * s;
    if input.design {
        let edge = input.muted;
        window.paint_quad(fill(rect(ox, oy, content_w, band_offset * s), edge));
        window.paint_quad(fill(rect(ox, band_y + band * s, content_w, band_offset * s), edge));
        // Start/end margins.
        let (ms, me) = input.margins;
        window.paint_quad(fill(rect(label_x, band_y, ms as f32 * s, band * s), edge));
        window.paint_quad(fill(
            rect(label_x + (r.length_dots - me) as f32 * s, band_y, me as f32 * s, band * s),
            edge,
        ));
    }

    // The dots: merged horizontal runs per row.
    let bm = &r.bitmap;
    let (len, height) = (bm.length(), bm.height());
    for y in 0..height {
        let mut x = 0;
        while x < len {
            if bm.get(x, y) {
                let start = x;
                while x < len && bm.get(x, y) {
                    x += 1;
                }
                window.paint_quad(fill(
                    rect(label_x + start as f32 * s, band_y + f32::from(y) * s, (x - start) as f32 * s, s.max(0.5)),
                    input.ink,
                ));
            } else {
                x += 1;
            }
        }
    }

    // Dot grid in exact mode when dots are big enough.
    if input.exact && s >= 4.0 {
        let grid = input.line.opacity(0.25);
        for x in 0..=len {
            window.paint_quad(fill(rect(label_x + x as f32 * s, band_y, 0.5, band * s), grid));
        }
        for y in 0..=height {
            window.paint_quad(fill(rect(label_x, band_y + f32::from(y) * s, len as f32 * s, 0.5), grid));
        }
    }

    // Cut lines at both tape ends (dashed).
    for cut_x in [ox, ox + content_w] {
        let mut y = oy - 4.0;
        while y < oy + tape_dots * s + 4.0 {
            window.paint_quad(fill(rect(cut_x - 0.5, y, 1.0, 4.0), input.line));
            y += 7.0;
        }
    }

    // Selection outline.
    if input.design
        && let Some((x, y, w, h)) = input.selected
    {
        let (x, y, w, h) = (label_x + x as f32 * s, band_y + y as f32 * s, w as f32 * s, h as f32 * s);
        let t = 1.5;
        for b in [rect(x, y, w, t), rect(x, y + h - t, w, t), rect(x, y, t, h), rect(x + w - t, y, t, h)] {
            window.paint_quad(fill(b, input.accent));
        }
    }

    // Ruler in mm from the left tape end.
    let px_per_mm = s * area.dpi as f32 / 25.4;
    let (minor, label) = ruler_steps(px_per_mm);
    let ruler_y = oy + tape_dots * s + 6.0;
    let total_mm = (total / area.dpi as f32 * 25.4).floor() as u32;
    let font_size = px(10.);
    let mut mm = 0;
    while mm <= total_mm {
        let x = ox + mm as f32 * px_per_mm;
        let major = mm % label == 0;
        window.paint_quad(fill(rect(x, ruler_y, 1.0, if major { 7.0 } else { 4.0 }), input.line));
        if major {
            let text: SharedString = format!("{mm}").into();
            let run = TextRun {
                len: text.len(),
                font: window.text_style().font(),
                color: input.line,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(text, font_size, &[run], None);
            let _ = line.paint(point(px(x + 2.0), px(ruler_y + 6.0)), px(12.), TextAlign::Left, None, window, cx);
        }
        mm += minor;
    }
}
