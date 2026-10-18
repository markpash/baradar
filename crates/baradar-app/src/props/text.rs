//! Properties of a text block (port of `editor/props/TextProps.svelte` and
//! `editor/props/TextSizeQuick.svelte`): text, font (picker) and weight, the quick sizes with
//! their length readouts, the detailed size (Fit tape / cap height in mm / points), the size of
//! new text, alignment, line spacing, italic and inverted.

use std::sync::Arc;

use baradar_core::doc::{Align, FONT_WEIGHTS, FontSource, Item, LabelDoc, LayoutSettings, LengthSettings, TextItem, TextSize};
use baradar_core::render::fonts::{font_def, resolve_weight};
use baradar_core::render::renderer::RenderResult;
use baradar_core::render::text::{
    QuickTextSize, ReadoutInput, default_size_of, match_quick_size, measure_text_at, quick_readouts, quick_text_sizes,
};
use baradar_core::store::DefaultTextSize;
use baradar_core::vars::variables::{VariableContext, resolve_doc};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::{
    Fields, NumSpec, field, field_label, fmt_num, hint, item_key, item_num_get, item_num_set, item_str_get,
    item_str_set, num_view, parse_num, segmented, segmented_field, select, slider_view, switch_view, variable_hint,
};
use super::{Band, PropertiesView, font_picker};
use crate::studio::{Studio, local_today};

/// `WEIGHT_NAMES`.
pub fn weight_name(w: u16) -> &'static str {
    match w {
        400 => "Regular",
        500 => "Medium",
        600 => "Semibold",
        700 => "Bold",
        800 => "Extra bold",
        _ => "Regular",
    }
}

/// The options of "Size of new text".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewSize {
    Auto,
    Fit,
    Half,
    Third,
    Pt,
}

/// What the quick size row shows: the buttons, their readouts and which one is current.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickSizes {
    pub sizes: Vec<QuickTextSize>,
    pub readouts: Vec<String>,
    pub current: Option<baradar_core::render::text::QuickSizeId>,
    pub grows: bool,
}

/// `hint` of TextSizeQuick.
pub fn quick_hint(q: &QuickSizes) -> String {
    let mut s = format!(
        "Each size shows the {}.",
        if q.grows { "label length it gives" } else { "length of this text" }
    );
    for t in q.sizes.iter().filter(|t| t.too_small) {
        s.push_str(&format!(" {} is under 2 mm on this tape and may be hard to read.", t.label));
    }
    s
}

/// The readout cache key: the render, the preview row, the block and its band.
type ReadoutKey = (usize, usize, TextItem, i64, u16);

/// Computes the quick sizes of `item` (`TextSizeQuick.svelte` `sizes`, `current`, `readouts`).
fn quick_sizes(studio: &Studio, item: &TextItem, band: Band, cache: &mut Option<(ReadoutKey, Vec<String>)>) -> QuickSizes {
    let sizes = quick_text_sizes(band.dots, band.dpi);
    let current = match_quick_size(item.size, band.dots, band.dpi);
    let doc = &studio.doc;
    let grows = matches!(doc.length, LengthSettings::Auto) && !(matches!(doc.layout, LayoutSettings::Free) && item.frame.is_some());
    let readouts = match &studio.render {
        Some(render) => {
            let key: ReadoutKey = (Arc::as_ptr(render) as usize, studio.preview_row, item.clone(), band.dots, band.dpi);
            match cache {
                Some((k, r)) if *k == key => r.clone(),
                _ => {
                    let r = readouts_for(studio, render, item, &sizes, current, band, grows);
                    *cache = Some((key, r.clone()));
                    r
                }
            }
        }
        None => Vec::new(),
    };
    QuickSizes { sizes, readouts, current, grows }
}

fn readouts_for(
    studio: &Studio,
    render: &RenderResult,
    item: &TextItem,
    sizes: &[QuickTextSize],
    current: Option<baradar_core::render::text::QuickSizeId>,
    band: Band,
    grows: bool,
) -> Vec<String> {
    // The preview shows placeholders filled with the preview row: measure the same text.
    let one = LabelDoc {
        items: vec![Item::Text(item.clone())],
        ..studio.doc.clone()
    };
    let resolved = resolve_doc(&one, studio.preview_row, &VariableContext::new(local_today()));
    let Some(Item::Text(shown)) = resolved.doc.items.first() else {
        return Vec::new();
    };
    let Ok(font) = studio.fonts.resolve_item(shown) else {
        return Vec::new();
    };
    quick_readouts(&ReadoutInput {
        shown,
        font: &font,
        size: item.size,
        current,
        sizes,
        band_dots: band.dots,
        dpi: band.dpi,
        min_mm: band.min_mm,
        length_mm: render.length_mm,
        rendered_width_dots: render.texts.iter().find(|t| t.item_id == item.id).map(|t| t.width_dots),
        grows,
    })
}

fn edit_text(studio: &Entity<Studio>, id: &str, key: Option<String>, cx: &mut App, f: impl FnOnce(&mut TextItem) + 'static) {
    let id = id.to_owned();
    studio.update(cx, move |s, cx| {
        s.update_item(
            &id,
            key.as_deref(),
            move |i| {
                if let Item::Text(t) = i {
                    f(t);
                }
            },
            cx,
        )
    });
}

pub struct TextEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
    readouts: Option<(ReadoutKey, Vec<String>)>,
}

impl TextEditor {
    pub fn new(item: &TextItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        font_picker::ensure_bundled(cx);
        let mut fields = Fields::default();
        let id = item.id.as_str();
        fields.add_str(
            "text",
            true,
            "Type the label text",
            studio,
            item_str_get(id, |i| match i {
                Item::Text(t) => Some(t.text.clone()),
                _ => None,
            }),
            item_str_set(id, "text", |i, v| {
                if let Item::Text(t) = i {
                    t.text = v;
                }
            }),
            window,
            cx,
        );
        let tape = studio.read(cx).doc.tape.width_mm;
        fields.add_num(
            "size_mm",
            NumSpec::new("Cap height", "mm", 0.5, tape, 0.5),
            studio,
            item_num_get(id, |i| match i {
                Item::Text(TextItem { size: TextSize::Mm { mm }, .. }) => Some(*mm),
                _ => None,
            }),
            item_num_set(id, "size", |i, mm| {
                if let Item::Text(t) = i {
                    t.size = TextSize::Mm { mm };
                }
            }),
            window,
            cx,
        );
        fields.add_num(
            "size_pt",
            NumSpec::new("Font size", "pt", 4.0, 144.0, 0.5),
            studio,
            item_num_get(id, |i| match i {
                Item::Text(TextItem { size: TextSize::Pt { pt }, .. }) => Some(*pt),
                _ => None,
            }),
            item_num_set(id, "size", |i, pt| {
                if let Item::Text(t) = i {
                    t.size = TextSize::Pt { pt };
                }
            }),
            window,
            cx,
        );
        fields.add_slider(
            "line_height",
            (1.0, 2.0, 0.05),
            studio,
            item_num_get(id, |i| match i {
                Item::Text(t) => Some(t.line_height),
                _ => None,
            }),
            item_num_set(id, "lineHeight", |i, v| {
                if let Item::Text(t) = i {
                    t.line_height = ((v * 100.0).round() / 100.0).clamp(1.0, 2.0);
                }
            }),
            window,
            cx,
        );
        Self {
            fields,
            studio: studio.clone(),
            readouts: None,
        }
    }

    pub fn render(&mut self, item: &TextItem, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = item.id.clone();
        let (band, quick, doc, missing, crisp, new_size) = {
            let s = self.studio.read(cx);
            let band = s.props_band(&Item::Text(item.clone()));
            let quick = quick_sizes(s, item, band, &mut self.readouts);
            let missing = item.custom_font.as_ref().is_some_and(|c| s.fonts.custom_font_missing(c));
            let crisp = if item.custom_font.is_some() {
                None
            } else {
                s.render.as_ref().and_then(|r| r.texts.iter().find(|t| t.item_id == item.id).and_then(|t| t.pixel_scale))
            };
            (band, quick, s.doc.clone(), missing, crisp, s.prefs.default_text_size)
        };
        let def = font_def(&item.font_family);
        let mut col = v_flex().gap_3();

        // Text.
        let text_input = self.fields.str("text").map(|f| f.element()).unwrap_or_else(|| div().into_any_element());
        col = col.child(
            v_flex()
                .gap_1()
                .child(field("Text", text_input, Some("Press Enter for a new line.".into()), cx))
                .children(variable_hint(&item.text, &doc, cx)),
        );

        // Font and weight.
        let (family, weight, italic) = font_picker::item_font(item, cx);
        let face_label = item.custom_font.as_ref().map_or_else(
            || def.label.to_owned(),
            |c| match c {
                baradar_core::doc::FontSource::User { family, .. } | baradar_core::doc::FontSource::Local { family, .. } => family.clone(),
            },
        );
        let trigger = {
            let (studio, id) = (self.studio.clone(), id.clone());
            h_flex()
                .id("text-font")
                .gap_2()
                .h(px(32.))
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(theme.input)
                .bg(theme.background)
                .cursor_pointer()
                .hover(|d| d.bg(theme.muted))
                .on_click(move |_, window, cx| font_picker::open(studio.clone(), id.clone(), window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(weight)
                        .when(italic, |d| d.italic())
                        .when_some(family, |d, f| d.font_family(f))
                        .child(face_label),
                )
                .when_some(crisp, |d, n| {
                    d.child(
                        div()
                            .id("text-font-crisp")
                            .px_1p5()
                            .rounded_full()
                            .bg(theme.success.opacity(0.15))
                            .text_color(theme.success)
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(format!("Drawn at {n} dots per font pixel")).build(window, cx)
                            })
                            .child("Crisp"),
                    )
                })
                .child(div().text_color(theme.muted_foreground).child("▾"))
                .test_support()
        };
        // An installed family offers the weights it has (static files or a variable axis).
        let local_weights: Vec<u16> = match &item.custom_font {
            Some(FontSource::Local { postscript_name, .. }) if !missing => self
                .studio
                .read(cx)
                .fonts
                .local_family_of(postscript_name)
                .map(|f| f.weights.iter().copied().filter(|w| FONT_WEIGHTS.contains(w)).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let weight_ctl = if local_weights.len() > 1 {
            let (studio, id) = (self.studio.clone(), id.clone());
            let current = local_weights
                .iter()
                .copied()
                .min_by_key(|w| (w.abs_diff(item.font_weight), *w))
                .unwrap_or(400);
            field(
                "Weight",
                select(
                    "text-weight",
                    local_weights.iter().map(|w| (*w, SharedString::from(weight_name(*w)))).collect(),
                    Some(current),
                    move |w, _, cx| edit_text(&studio, &id, None, cx, move |t| t.font_weight = w),
                ),
                None,
                cx,
            )
        } else if item.custom_font.is_some() && !missing {
            field("Weight", div().h(px(32.)).flex().items_center().child(hint("From the font file", cx)), None, cx)
        } else {
            let (studio, id) = (self.studio.clone(), id.clone());
            field(
                "Weight",
                select(
                    "text-weight",
                    def.weights.iter().map(|w| (*w, SharedString::from(weight_name(*w)))).collect(),
                    Some(resolve_weight(def, item.font_weight)),
                    move |w, _, cx| edit_text(&studio, &id, None, cx, move |t| t.font_weight = w),
                ),
                None,
                cx,
            )
        };
        col = col.child(
            h_flex()
                .gap_2()
                .items_start()
                .child(div().flex_1().min_w_0().child(field("Font", trigger, None, cx)))
                .child(div().w(px(116.)).child(weight_ctl)),
        );
        if let (Some(c), true) = (&item.custom_font, missing) {
            let family = match c {
                baradar_core::doc::FontSource::User { family, .. } | baradar_core::doc::FontSource::Local { family, .. } => family.clone(),
            };
            let studio = self.studio.clone();
            col = col.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .bg(theme.warning.opacity(0.15))
                    .text_sm()
                    .child(div().text_color(theme.warning).child(crate::icons::icon("alert").small()))
                    .child(div().flex_1().min_w(px(140.)).child(format!(
                        "“{family}” is not on this device — using {}.",
                        def.label
                    )))
                    .child(
                        Button::new("text-choose-font")
                            .icon(crate::icons::icon("search")).label("Choose font…")
                            .small()
                            .outline()
                            .on_click(move |_, window, cx| crate::fonts_ui::open_fonts(studio.clone(), window, cx)),
                    ),
            );
        }

        // Quick sizes.
        col = col.child(self.quick_view(item, &quick, cx));

        // Detailed size (SizeField with points).
        col = col.child(self.size_field(item, doc.tape.width_mm, cx));

        // Size of new text + Use for new text.
        {
            let current = match new_size {
                DefaultTextSize::Auto => NewSize::Auto,
                DefaultTextSize::Fit => NewSize::Fit,
                DefaultTextSize::Half => NewSize::Half,
                DefaultTextSize::Third => NewSize::Third,
                DefaultTextSize::Pt(_) => NewSize::Pt,
            };
            let mut options: Vec<(NewSize, SharedString)> = vec![
                (NewSize::Auto, "Auto: Fit below 12 mm, else M".into()),
                (NewSize::Fit, "Fit tape".into()),
                (NewSize::Half, "Half height (M)".into()),
                (NewSize::Third, "Third (S)".into()),
            ];
            if let DefaultTextSize::Pt(pt) = new_size {
                options.push((NewSize::Pt, format!("{} pt", fmt_num(pt)).into()));
            }
            let studio = self.studio.clone();
            let pick = select("text-new-size", options, Some(current), move |v, _, cx| {
                let pref = match v {
                    NewSize::Auto => DefaultTextSize::Auto,
                    NewSize::Fit => DefaultTextSize::Fit,
                    NewSize::Half => DefaultTextSize::Half,
                    NewSize::Third => DefaultTextSize::Third,
                    NewSize::Pt => return,
                };
                studio.update(cx, |s, cx| s.props_update_prefs(|p| p.default_text_size = pref, cx));
            });
            let (studio, item_c) = (self.studio.clone(), item.clone());
            col = col.child(
                v_flex()
                    .gap_2()
                    .child(field("Size of new text", pick, None, cx))
                    .child(div().child(Button::new("text-use-for-new").icon(crate::icons::icon("check")).label("Use for new text").small().outline().on_click(
                        move |_, window, cx| {
                            let pref = studio.read(cx).fonts.resolve_item(&item_c).ok().map(|font| {
                                let em = measure_text_at(&item_c, &font, item_c.size, band.dots, band.dpi).1;
                                default_size_of(item_c.size, band.dots, band.dpi, em)
                            });
                            if let Some(pref) = pref {
                                studio.update(cx, |s, cx| s.props_update_prefs(|p| p.default_text_size = pref, cx));
                                window.push_notification(Notification::new().message("New text blocks will get this size"), cx);
                            }
                        },
                    ))),
            );
        }

        // Alignment.
        {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(segmented_field(
                "text-align",
                "Alignment",
                &[(Align::Start, "Left"), (Align::Center, "Centre"), (Align::End, "Right")],
                Some(item.align),
                move |a, _, cx| edit_text(&studio, &id, None, cx, move |t| t.align = a),
                cx,
            ));
        }

        // Line spacing.
        col = col.child(slider_view(
            "Line spacing",
            format!("{:.2}×", item.line_height),
            self.fields.slider("line_height"),
            None,
            cx,
        ));

        // Italic, inverted.
        let (s1, id1) = (self.studio.clone(), id.clone());
        let (s2, id2) = (self.studio.clone(), id.clone());
        col.child(
            v_flex()
                .gap_2()
                .child(switch_view(
                    "text-italic",
                    "Italic",
                    item.italic,
                    None,
                    move |on, _, cx| edit_text(&s1, &id1, None, cx, move |t| t.italic = on),
                    cx,
                ))
                .child(switch_view(
                    "text-invert",
                    "Inverted",
                    item.invert,
                    Some("White text on a black block"),
                    move |on, _, cx| edit_text(&s2, &id2, None, cx, move |t| t.invert = on),
                    cx,
                )),
        )
        .into_any_element()
    }

    /// The quick size buttons XS S M L Fit with their readouts and the hint (`TextSizeQuick`,
    /// variant `props`).
    fn quick_view(&self, item: &TextItem, q: &QuickSizes, cx: &mut Context<PropertiesView>) -> AnyElement {
        let theme = cx.theme().clone();
        let buttons = h_flex().gap_1().children(q.sizes.iter().enumerate().map(|(i, s)| {
            let pressed = q.current == Some(s.id);
            let readout = [q.readouts.get(i).cloned(), s.too_small.then(|| "hard to read".to_owned())]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            let (studio, id, size) = (self.studio.clone(), item.id.clone(), s.size);
            v_flex()
                .id(SharedString::from(format!("quick-{}", s.label)))
                .flex_1()
                .min_w_0()
                .items_center()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(if pressed { theme.primary } else { theme.border })
                .when(pressed, |d| d.bg(theme.primary.opacity(0.12)))
                .hover(|d| d.bg(theme.muted))
                .cursor_pointer()
                .tooltip({
                    let name: SharedString = s.name.into();
                    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                })
                .on_click(move |_, _, cx| edit_text(&studio, &id, None, cx, move |t| t.size = size))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .when(s.too_small, |d| d.underline())
                        .child(s.label),
                )
                .child(
                    div()
                        .min_h(px(14.))
                        .text_xs()
                        .text_center()
                        .text_color(if pressed { theme.foreground } else { theme.muted_foreground })
                        .child(readout),
                )
                .test_support()
        }));
        v_flex().gap_1().child(buttons).child(hint(quick_hint(q), cx)).into_any_element()
    }

    /// "Text size": Fit tape / Fixed (cap height, mm) / Points (`SizeField` with `allowPt`).
    fn size_field(&self, item: &TextItem, max_mm: f64, cx: &mut Context<PropertiesView>) -> AnyElement {
        let mm_field = self.fields.num("size_mm");
        let pt_field = self.fields.num("size_pt");
        if let Some(f) = mm_field {
            f.set_range(0.5, max_mm);
        }
        let last_mm = mm_field.and_then(|f| parse_num(&f.state.read(cx).value())).unwrap_or(6.0);
        let last_pt = pt_field.and_then(|f| parse_num(&f.state.read(cx).value())).unwrap_or(12.0);
        let mode = match item.size {
            TextSize::Fit => "fit",
            TextSize::Mm { .. } => "mm",
            TextSize::Pt { .. } => "pt",
        };
        let (studio, id) = (self.studio.clone(), item.id.clone());
        let key = item_key(&item.id, "size");
        v_flex()
            .gap_2()
            .child(field_label("Text size", cx))
            .child(segmented(
                "text-size-mode",
                &[("fit", "Fit tape"), ("mm", "Fixed"), ("pt", "Points")],
                Some(mode),
                move |m, _, cx| {
                    let size = match m {
                        "fit" => TextSize::Fit,
                        "pt" => TextSize::Pt { pt: last_pt },
                        _ => TextSize::Mm { mm: last_mm.min(max_mm) },
                    };
                    edit_text(&studio, &id, Some(key.clone()), cx, move |t| t.size = size);
                },
            ))
            .when(mode == "mm", |d| d.child(num_view(mm_field, None, cx)))
            .when(mode == "pt", |d| d.child(num_view(pt_field, None, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{QuickSizes, quick_hint, weight_name};
    use baradar_core::render::text::quick_text_sizes;

    #[test]
    fn quick_size_hint() {
        let sizes = quick_text_sizes(24, 180);
        let q = QuickSizes { sizes, readouts: Vec::new(), current: None, grows: true };
        let h = quick_hint(&q);
        assert!(h.starts_with("Each size shows the label length it gives."), "{h}");
        assert!(h.contains("XS is under 2 mm on this tape and may be hard to read."), "{h}");
        let q2 = QuickSizes { grows: false, sizes: quick_text_sizes(128, 180), ..q };
        assert_eq!(quick_hint(&q2), "Each size shows the length of this text.");
        assert_eq!(weight_name(800), "Extra bold");
    }
}
