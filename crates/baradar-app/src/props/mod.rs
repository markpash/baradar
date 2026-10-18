//! The properties column: the selected block's editor and the label settings card (port of
//! `editor/PropertiesPanel.svelte`, `editor/props/*.svelte`, `LabelProps.svelte`).
//!
//! [`PropertiesView`] keeps one editor for the selected block (rebuilt when the selection or its
//! kind changes) and the label card. Inputs are reloaded from the studio as described in
//! [`widgets`]: unfocused ones after every change, all of them when `Studio::epoch` moved.

mod code;
pub mod font_picker;
mod icon;
mod image;
mod label;
mod shape;
mod spacer;
mod text;
pub mod widgets;

use baradar_core::doc::{Item, ItemKind};
use baradar_core::render::shapes::item_sizing_band;
use baradar_core::render::text::fallback_band;
use baradar_core::render::units::dots_to_mm;
use baradar_core::store::Prefs;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;

use crate::studio::Studio;
use crate::view_model::kind_label;
use widgets::Fields;

/// The band an item is sized in, at the target's resolution (`itemSizingBand`, or the fallback
/// band before the print area is known: `fallbackBand` in `ui/state/text-defaults.ts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub dots: i64,
    pub dpi: u16,
    /// The model's minimum label length (0 before the print area is known).
    pub min_mm: f64,
    /// Unprinted tape a code may count as quiet zone.
    pub margin_dots: i64,
    /// The item sits in a free-layout frame.
    pub framed: bool,
    /// The print area is known.
    pub known: bool,
}

impl Studio {
    /// The band `item` is sized in (`TextSizeQuick` / `CodeProps` `band`).
    pub fn props_band(&self, item: &Item) -> Band {
        match self.target() {
            Ok(t) => {
                let area = t.area();
                let b = item_sizing_band(&self.doc, item, area.height_dots, area.tape_width_dots, area.dpi);
                Band {
                    dots: b.band_dots,
                    dpi: area.dpi,
                    min_mm: dots_to_mm(area.min_length_dots as f64, area.dpi),
                    margin_dots: b.margin_dots,
                    framed: b.framed,
                    known: true,
                }
            }
            Err(_) => {
                let (dots, dpi) = fallback_band(self.doc.tape.width_mm).unwrap_or((128, 180));
                Band {
                    dots,
                    dpi,
                    min_mm: 0.0,
                    margin_dots: 0,
                    framed: false,
                    known: false,
                }
            }
        }
    }

    /// `updatePrefs`: edits the preferences and saves them.
    pub fn props_update_prefs(&mut self, edit: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        edit(&mut self.prefs);
        let _ = self.store.save_prefs(&self.prefs);
        cx.notify();
    }
}

/// The editor of the selected block, by kind.
enum ItemEditor {
    Text(text::TextEditor),
    Icon(icon::IconEditor),
    Code(code::CodeEditor),
    Image(image::ImageEditor),
    Shape(shape::ShapeEditor),
    Spacer(spacer::SpacerEditor),
}

impl ItemEditor {
    fn new(item: &Item, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        match item {
            Item::Text(t) => Self::Text(text::TextEditor::new(t, studio, window, cx)),
            Item::Icon(i) => Self::Icon(icon::IconEditor::new(i, studio, window, cx)),
            Item::Code(c) => Self::Code(code::CodeEditor::new(c, studio, window, cx)),
            Item::Image(i) => Self::Image(image::ImageEditor::new(i, studio, window, cx)),
            Item::Shape(s) => Self::Shape(shape::ShapeEditor::new(s, studio, window, cx)),
            Item::Spacer(s) => Self::Spacer(spacer::SpacerEditor::new(s, studio, window, cx)),
        }
    }

    fn fields(&self) -> &Fields {
        match self {
            Self::Text(e) => &e.fields,
            Self::Icon(e) => &e.fields,
            Self::Code(e) => &e.fields,
            Self::Image(e) => &e.fields,
            Self::Shape(e) => &e.fields,
            Self::Spacer(e) => &e.fields,
        }
    }
}

pub struct PropertiesView {
    studio: Entity<Studio>,
    editor: Option<ItemEditor>,
    /// The block the editor was built for.
    editor_for: Option<(String, ItemKind)>,
    label: label::LabelEditor,
    epoch: u64,
    _subs: Vec<Subscription>,
}

impl PropertiesView {
    pub fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe_in(&studio, window, |this, _, window, cx| {
            this.on_studio(window, cx);
            cx.notify();
        })];
        let label = label::LabelEditor::new(&studio, window, cx);
        let epoch = studio.read(cx).epoch;
        let mut this = Self {
            studio,
            editor: None,
            editor_for: None,
            label,
            epoch,
            _subs: subs,
        };
        this.on_studio(window, cx);
        this
    }

    /// Rebuilds the block editor when the selection changed, else reloads the inputs.
    fn on_studio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selected, epoch) = {
            let s = self.studio.read(cx);
            (s.selected_item().cloned(), s.epoch)
        };
        let force = epoch != self.epoch;
        self.epoch = epoch;
        let key = selected.as_ref().map(|i| (i.id().to_owned(), i.kind()));
        if key != self.editor_for {
            let studio = self.studio.clone();
            self.editor = selected.as_ref().map(|i| ItemEditor::new(i, &studio, window, cx));
            self.editor_for = key;
        } else if let Some(e) = &self.editor {
            e.fields().sync(&self.studio, force, window, cx);
        }
        self.label.fields.sync(&self.studio, force, window, cx);
    }

    /// The selected block's card: "<Kind> block i/n", duplicate/delete, warnings and the kind's
    /// editor (`PropertiesPanel.svelte`).
    fn selected_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (item, ix, n, warnings) = {
            let s = self.studio.read(cx);
            let Some(item) = s.selected_item().cloned() else {
                return card(&theme)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Block"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Select a block in the list or on the preview to edit it."),
                    )
                    .into_any_element();
            };
            let id = item.id().to_owned();
            let ix = s.doc.items.iter().position(|i| i.id() == id).unwrap_or(0) + 1;
            let warnings: Vec<String> = s
                .render
                .as_ref()
                .map(|r| {
                    r.warnings
                        .iter()
                        .filter(|w| w.item_id.as_deref() == Some(id.as_str()))
                        .map(|w| w.message.clone())
                        .collect()
                })
                .unwrap_or_default();
            (item, ix, s.doc.items.len(), warnings)
        };
        let id = item.id().to_owned();
        let (dup, del) = (id.clone(), id.clone());
        let body = match (&mut self.editor, &item) {
            (Some(ItemEditor::Text(e)), Item::Text(t)) => e.render(t, window, cx),
            (Some(ItemEditor::Icon(e)), Item::Icon(t)) => e.render(t, window, cx),
            (Some(ItemEditor::Code(e)), Item::Code(t)) => e.render(t, window, cx),
            (Some(ItemEditor::Image(e)), Item::Image(t)) => e.render(t, window, cx),
            (Some(ItemEditor::Shape(e)), Item::Shape(t)) => e.render(t, window, cx),
            (Some(ItemEditor::Spacer(e)), Item::Spacer(t)) => e.render(t, window, cx),
            _ => div().into_any_element(),
        };
        card(&theme)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .gap_1()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} block", kind_label(&item))))
                            .child(div().text_color(theme.muted_foreground).child(format!("{ix}/{n}"))),
                    )
                    .child(
                        Button::new("card-dup")
                            .icon(crate::icons::icon("copy"))
                            .small()
                            .ghost()
                            .tooltip("Duplicate")
                            .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.duplicate(&dup, cx)))),
                    )
                    .child(
                        Button::new("card-del")
                            .icon(crate::icons::icon("trash"))
                            .small()
                            .ghost()
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.remove_item(&del, cx)))),
                    ),
            )
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .gap_0p5()
                        .p_2()
                        .rounded_md()
                        .bg(theme.warning.opacity(0.15))
                        .text_sm()
                        .children(warnings.into_iter().map(|m| {
                            h_flex()
                                .gap_1()
                                .items_start()
                                .child(div().text_color(theme.warning).child(crate::icons::icon("alert").small()))
                                .child(div().flex_1().child(m))
                        })),
                )
            })
            .child(body)
            .into_any_element()
    }
}

/// A properties card (`.panel.card`).
fn card(theme: &gpui_kit::component::Theme) -> Div {
    v_flex()
        .gap_3()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
}

use gpui_kit::prelude::FluentBuilder as _;

impl Render for PropertiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected_card(window, cx);
        let label = self.label.render(window, cx);
        v_flex()
            .id("properties")
            .size_full()
            .p_3()
            .gap_3()
            .bg(theme.sidebar)
            .overflow_y_scroll()
            .child(div().text_sm().text_color(theme.muted_foreground).child("PROPERTIES"))
            .child(selected)
            .child(
                card(&theme)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Label"))
                    .child(label),
            )
    }
}

#[cfg(test)]
mod ui_tests;
