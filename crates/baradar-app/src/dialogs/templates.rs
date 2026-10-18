//! "New from template" gallery (port of ui/templates/TemplateGallery.svelte and
//! TemplateThumb.svelte): the cards grouped by use, each with a live thumbnail drawn with the
//! sample network, the tape width, the label length as the editor shows it (measured by that
//! render, `Template::gallery_length_mm`) and "Fits the loaded tape". Templates for widths the
//! active model lacks sit behind "Show templates for other tape widths". Choosing one opens a new
//! label (`Studio::open_new`) with its main block selected.

use std::sync::Arc;

use baradar_core::export::png::export_png;
use baradar_core::render::renderer::{Cancelled, RenderOptions, RenderTarget};
use baradar_core::templates::{
    ModelWidths, TEMPLATE_CATEGORIES, Template, TemplateBand, fits_loaded_tape, needs_width_text,
    primary_item_id, template_fits, template_model, templates, templates_in,
};
use baradar_core::vars::variables::VariableContext;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch::TapeKind;

use super::common::{heading, hint, png_image};
use crate::studio::{Studio, is_connected, local_today, tape_widths_of, width_byte};
use crate::view_model::format_mm;

pub fn open_templates(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    open_gallery(studio, window, cx);
}

fn open_gallery(studio: Entity<Studio>, window: &mut Window, cx: &mut App) -> Entity<TemplateGallery> {
    let view = cx.new(|cx| TemplateGallery::new(studio, window, cx));
    let child = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("New from template").w(px(800.)).child(child.clone())
    });
    view
}

/// CSS px per mm of tape in a thumbnail (24 mm tape → 48 px tall). `PX_PER_MM`.
const PX_PER_MM: f32 = 2.0;
/// Tallest tape drawn at [`PX_PER_MM`]; 36 mm tape is drawn smaller to the same height.
const MAX_TAPE_MM: f32 = 24.0;
/// Width of a card.
const CARD_W: f32 = 240.0;

/// The model a template opens with and its band of the template's tape (`opening(t)`):
/// the active model when it takes the width, else the first model that does.
fn opening(t: &Template, active: &str) -> Option<(&'static ptouch::ModelProfile, TemplateBand, RenderTarget)> {
    let widths: Vec<(&'static ptouch::ModelProfile, Vec<f64>)> =
        ptouch::profiles().iter().map(|p| (p, tape_widths_of(p))).collect();
    let models: Vec<ModelWidths<'_>> = widths
        .iter()
        .map(|(p, w)| ModelWidths { name: p.name, aliases: p.aliases, tape_widths_mm: w })
        .collect();
    let name = template_model(t, active, &models)?;
    let profile = ptouch::profile_by_name(&name)?;
    let tape = profile.tape(width_byte(t.tape_width_mm), TapeKind::Tze)?;
    let band = TemplateBand { band_dots: u32::from(tape.print_pins), dpi: profile.dpi };
    Some((profile, band, RenderTarget { profile, tape }))
}

/// One card's thumbnail state.
#[derive(Clone, Default)]
struct Thumb {
    image: Option<Arc<Image>>,
    /// Printed length + 2 × feed (mm), once rendered.
    length_mm: Option<f64>,
    failed: bool,
}

struct TemplateGallery {
    studio: Entity<Studio>,
    /// Per template (catalogue order), for `thumbs_model`.
    thumbs: Vec<Thumb>,
    thumbs_model: &'static str,
    show_others: bool,
    model_select: Entity<SelectState<SearchableVec<&'static str>>>,
    _render: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl TemplateGallery {
    fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let names: Vec<&'static str> = ptouch::profiles()
            .iter()
            .filter(|p| !tape_widths_of(p).is_empty())
            .map(|p| p.name)
            .collect();
        let active = studio.read(cx).model().name;
        let at = names.iter().position(|n| *n == active).map(IndexPath::new);
        let model_select = cx.new(|cx| SelectState::new(SearchableVec::new(names), at, window, cx));
        let subs = vec![
            cx.observe(&studio, |this: &mut Self, _, cx| {
                this.ensure_thumbs(cx);
                cx.notify();
            }),
            cx.subscribe(&model_select, |this, _, ev: &SelectEvent<SearchableVec<&'static str>>, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                let name = *name;
                this.studio.update(cx, |s, cx| s.set_design_model(name, cx));
            }),
        ];
        let mut this = Self {
            studio,
            thumbs: Vec::new(),
            thumbs_model: "",
            show_others: false,
            model_select,
            _render: Vec::new(),
            _subs: subs,
        };
        this.ensure_thumbs(cx);
        this
    }

    /// (Re)draws every thumbnail when the active model changed, two at a time.
    fn ensure_thumbs(&mut self, cx: &mut Context<Self>) {
        let studio = self.studio.read(cx);
        let active = studio.model().name;
        if self.thumbs_model == active {
            return;
        }
        self.thumbs_model = active;
        self.thumbs = vec![Thumb::default(); templates().len()];
        let renderer = studio.renderer.clone();
        self._render = (0..2)
            .map(|worker| {
                let renderer = renderer.clone();
                cx.spawn(async move |this, cx| {
                    for (ix, t) in templates().iter().enumerate().skip(worker).step_by(2) {
                        let renderer = renderer.clone();
                        let thumb = cx
                            .background_executor()
                            .spawn(async move { draw_thumb(t, active, &renderer) })
                            .await;
                        let alive = this.update(cx, |this, cx| {
                            if this.thumbs_model == active
                                && let Some(slot) = this.thumbs.get_mut(ix)
                            {
                                *slot = thumb;
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                })
            })
            .collect();
    }

    /// `choose(t)`: builds on the opening model's band, records that model, opens it as a new
    /// label and selects its primary block.
    fn choose(&mut self, t: &'static Template, window: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            let opened = opening(t, s.model().name);
            let mut doc = t.build(opened.map(|(_, band, _)| band));
            if let Some((profile, _, _)) = opened {
                doc.tape.model = Some(profile.name.to_owned());
            }
            s.flush_autosave();
            s.open_new(doc, cx);
            let primary = primary_item_id(&s.doc).map(str::to_owned);
            s.select(primary, cx);
        });
        window.close_dialog(cx);
    }

    fn card(&self, t: &'static Template, needs: Option<String>, loaded: Option<f64>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let ix = templates().iter().position(|x| x.id == t.id).unwrap_or(0);
        let thumb = self.thumbs.get(ix).cloned().unwrap_or_default();
        let fits = fits_loaded_tape(t, loaded);
        let ppm = PX_PER_MM.min(MAX_TAPE_MM * PX_PER_MM / t.tape_width_mm as f32);
        let length = thumb.length_mm.unwrap_or_else(|| t.guess_length_mm());
        let tape_w = px((length as f32 * ppm).clamp(24.0, CARD_W - 24.0));
        let tape_h = px(t.tape_width_mm as f32 * ppm);
        let tape = div()
            .flex()
            .items_center()
            .justify_center()
            .w(tape_w)
            .h(tape_h)
            .bg(gpui_kit::white())
            .border_1()
            .border_color(theme.border)
            .rounded_sm()
            .overflow_hidden()
            .map(|d| match (&thumb.image, thumb.failed) {
                (Some(i), _) => d.child(img(i.clone()).max_w(tape_w).max_h(tape_h).object_fit(ObjectFit::Contain)),
                (None, true) => d.child(div().text_xs().text_color(theme.muted_foreground).child("—")),
                (None, false) => d,
            });
        v_flex()
            .id(SharedString::from(t.id))
            .w(px(CARD_W))
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if fits { theme.success } else { theme.border })
            .cursor_pointer()
            .hover(|s| s.bg(theme.accent))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.choose(t, window, cx)))
            .child(h_flex().h(px(56.)).justify_center().child(tape))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(t.name))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(match needs {
                        Some(n) => Tag::warning().small().child(n),
                        None => Tag::secondary().small().child(format!("{} tape", format_mm(t.tape_width_mm))),
                    })
                    .when_some(thumb.length_mm, |d, mm| d.child(Tag::secondary().small().child(t.length_chip(mm))))
                    .when(fits, |d| {
                        d.child(
                            Tag::success()
                                .small()
                                .child(h_flex().gap_1().child(Icon::new(gpui_kit::component::IconName::Check).xsmall()).child("Fits the loaded tape")),
                        )
                    }),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(t.description))
            .into_any_element()
    }
}

/// `TemplateThumb.draw`: renders `t` with the sample network on its opening model.
fn draw_thumb(t: &Template, active: &str, renderer: &baradar_core::render::renderer::Renderer) -> Thumb {
    let Some((profile, band, target)) = opening(t, active) else {
        return Thumb { failed: true, ..Thumb::default() };
    };
    let ctx = VariableContext::new(local_today());
    let mut png = None;
    let length = t.gallery_length_mm(Some(band), &ctx, |doc| {
        let r = renderer.render(doc, &target, &RenderOptions::default())?;
        png = export_png(&r.bitmap, profile.dpi).ok();
        Ok::<_, Cancelled>((r.length_mm, r.feed_margin_mm))
    });
    match (length, png) {
        (Ok(mm), Some(png)) => Thumb { image: Some(png_image(png)), length_mm: Some(mm), failed: false },
        (Ok(mm), None) => Thumb { image: None, length_mm: Some(mm), failed: true },
        _ => Thumb { failed: true, ..Thumb::default() },
    }
}

/// The loaded-tape status line. `p.loaded`.
fn loaded_text(loaded: Option<f64>, fitting: usize) -> String {
    match loaded {
        None => "Connect your printer to see which templates fit the loaded tape.".into(),
        Some(w) if fitting > 0 => format!(
            "{} tape is loaded: templates marked “Fits the loaded tape” print on it as they are.",
            format_mm(w)
        ),
        Some(w) => format!(
            "{} tape is loaded. No template is made for it; you can change the tape width after choosing one.",
            format_mm(w)
        ),
    }
}

impl Render for TemplateGallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.studio.read(cx);
        let model = s.model().name;
        let widths = s.tape_widths();
        let connected = is_connected(s.conn);
        let loaded = s
            .printer_info
            .as_ref()
            .filter(|_| connected)
            .and_then(|i| i.tape)
            .map(|t| f64::from(t.width_mm_x10) / 10.0);
        let fitting = templates().iter().filter(|t| fits_loaded_tape(t, loaded)).count();
        let others: Vec<&'static Template> = templates().iter().filter(|t| !template_fits(t, &widths)).collect();
        let fits_icon = if fitting > 0 { gpui_kit::component::IconName::Check } else { gpui_kit::component::IconName::Info };

        let mut body = v_flex().gap_4();
        if !connected {
            body = body.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("Printer model"))
                    .child(div().w(px(220.)).child(Select::new(&self.model_select).small())),
            );
        }
        body = body.child(
            h_flex()
                .gap_2()
                .text_sm()
                .when(fitting > 0, |d| d.text_color(cx.theme().success))
                .child(Icon::new(fits_icon).small())
                .child(loaded_text(loaded, fitting)),
        );
        for cat in TEMPLATE_CATEGORIES {
            let list: Vec<&'static Template> = templates_in(cat).into_iter().filter(|t| template_fits(t, &widths)).collect();
            if list.is_empty() {
                continue;
            }
            body = body.child(
                v_flex()
                    .gap_2()
                    .child(heading(cat.label()))
                    .child(h_flex().flex_wrap().gap_3().children(list.into_iter().map(|t| self.card(t, None, loaded, cx)))),
            );
        }
        if !others.is_empty() {
            let open = self.show_others;
            body = body.child(
                v_flex()
                    .gap_2()
                    .child(
                        Button::new("others")
                            .ghost()
                            .icon(if open {
                                gpui_kit::component::IconName::ChevronDown
                            } else {
                                gpui_kit::component::IconName::ChevronRight
                            })
                            .label(format!("Show templates for other tape widths ({})", others.len()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.show_others = !this.show_others;
                                cx.notify();
                            })),
                    )
                    .when(open, |d| {
                        d.child(hint(
                            format!(
                                "{model} doesn’t take these tape widths. You can still open one: the editor offers to fit it to a width {model} takes."
                            ),
                            cx,
                        ))
                        .child(h_flex().flex_wrap().gap_3().children(others.iter().map(|t| {
                            let needs = needs_width_text(t.tape_width_mm, model, &widths);
                            self.card(t, needs, loaded, cx)
                        })))
                    }),
            );
        }
        v_flex()
            .gap_3()
            .child(hint("Start a new label from a ready-made design. Your current label stays in Your labels.", cx))
            .child(body)
    }
}

/// Test access to the gallery (cards are plain clickable divs, which headless tests can't find).
#[cfg(test)]
pub(super) fn test_open_and_choose(studio: Entity<Studio>, id: &str, window: &mut Window, cx: &mut App) -> usize {
    let view = open_gallery(studio, window, cx);
    let rendered = view.read(cx).thumbs.len();
    if let Some(t) = templates().iter().find(|t| t.id == id) {
        view.update(cx, |v, cx| v.choose(t, window, cx));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{loaded_text, opening};
    use baradar_core::templates::templates;

    #[test]
    fn loaded_tape_lines() {
        assert_eq!(loaded_text(None, 0), "Connect your printer to see which templates fit the loaded tape.");
        assert!(loaded_text(Some(12.0), 3).starts_with("12 mm tape is loaded: templates marked"));
        assert!(loaded_text(Some(9.0), 0).contains("No template is made for it"));
    }

    #[test]
    fn every_template_opens_on_some_model() {
        for t in templates() {
            let (profile, band, target) = opening(t, "PT-P710BT").expect(t.id);
            assert_eq!(band.dpi, profile.dpi);
            assert_eq!(band.band_dots, u32::from(target.tape.print_pins));
        }
    }
}
