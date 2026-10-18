//! Label library dialog "Your labels" (port of ui/labels/LibraryDialog.svelte): search, New label,
//! and a grid of the saved labels (thumbnail, name, "<w> mm · <date>", newest first) with the
//! open label highlighted. Clicking a card opens it; the trash button deletes after a confirm.

use std::collections::HashMap;
use std::sync::Arc;

use baradar_core::store::LabelSummary;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::common::{confirm, hint, png_image, thumb, when};
use crate::studio::Studio;
use crate::view_model::format_mm;

pub fn open_library(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| LibraryDialog::new(studio, window, cx));
    let search = view.read(cx).search.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Your labels").w(px(680.)).child(view.clone())
    });
    search.update(cx, |s, cx| s.focus(window, cx));
}

/// The labels whose name contains `filter` (trimmed, case-insensitive). `shown`.
fn matching<'a>(labels: &'a [LabelSummary], filter: &str) -> Vec<&'a LabelSummary> {
    let f = filter.trim().to_lowercase();
    labels.iter().filter(|l| l.name.to_lowercase().contains(&f)).collect()
}

struct LibraryDialog {
    studio: Entity<Studio>,
    search: Entity<InputState>,
    labels: Vec<LabelSummary>,
    thumbs: HashMap<String, Option<Arc<Image>>>,
    _subs: Vec<Subscription>,
}

impl LibraryDialog {
    fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search labels"));
        let subs = vec![
            cx.subscribe(&search, |_, _, ev: &InputEvent, cx| {
                if let InputEvent::Change = ev {
                    cx.notify();
                }
            }),
            cx.observe(&studio, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            studio,
            search,
            labels: Vec::new(),
            thumbs: HashMap::new(),
            _subs: subs,
        };
        this.refresh(cx);
        this
    }

    /// Writes any pending autosave, then reloads the list and thumbnails. `refresh`.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, _| s.flush_autosave());
        let studio = self.studio.read(cx);
        self.labels = studio.library();
        self.thumbs = self
            .labels
            .iter()
            .map(|l| (l.id.clone(), studio.store.label_thumbnail(&l.id).map(png_image)))
            .collect();
        cx.notify();
    }

    fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.open_from_library(id, cx));
        window.close_dialog(cx);
    }

    /// `remove(l)`: asks, deletes, and starts a new label when the open one went.
    fn remove(&mut self, label: &LabelSummary, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let id = label.id.clone();
        confirm(
            window,
            cx,
            format!("Delete “{}”? This cannot be undone.", label.name),
            "Delete",
            move |_, cx| {
                this.update(cx, |this, cx| {
                    this.studio.update(cx, |s, cx| s.delete_label(&id, cx));
                    this.refresh(cx);
                })
                .ok();
            },
        );
    }

    fn card(&self, ix: usize, label: &LabelSummary, current: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let image = self.thumbs.get(&label.id).cloned().flatten();
        let open_id = label.id.clone();
        let del = label.clone();
        h_flex()
            .id(("label", ix))
            .w(px(306.))
            .gap_2()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(if current { theme.primary } else { theme.border })
            .when(current, |d| d.bg(theme.accent))
            .child(
                h_flex()
                    .id(("open", ix))
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open(&open_id, window, cx)))
                    .child(thumb(image, px(96.), px(48.), cx))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(label.name.clone()))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} · {}", format_mm(label.tape_width_mm), when(&label.saved_at))),
                            ),
                    ),
            )
            .child(
                Button::new(("delete", ix))
                    .ghost()
                    .small()
                    .icon(gpui_kit::assets::IconName::Trash)
                    .tooltip(format!("Delete {}", label.name))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.remove(&del, window, cx))),
            )
            .into_any_element()
    }
}

impl Render for LibraryDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.search.read(cx).value().to_string();
        let current = self.studio.read(cx).doc.id.clone();
        let shown: Vec<LabelSummary> = matching(&self.labels, &filter).into_iter().cloned().collect();
        let body = if shown.is_empty() {
            hint(
                if self.labels.is_empty() {
                    "Labels you make are saved on this computer automatically. They will show up here."
                } else {
                    "No label matches."
                },
                cx,
            )
        } else {
            h_flex()
                .flex_wrap()
                .gap_2()
                .children(shown.iter().enumerate().map(|(ix, l)| self.card(ix, l, l.id == current, cx)))
                .into_any_element()
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.search).cleanable(true)))
                    .child(
                        Button::new("new-label")
                            .primary()
                            .icon(gpui_kit::assets::IconName::FilePlus)
                            .label("New label")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.studio.update(cx, |s, cx| {
                                    s.flush_autosave();
                                    s.new_label(cx);
                                });
                                window.close_dialog(cx);
                            })),
                    ),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::matching;
    use baradar_core::store::LabelSummary;

    fn label(name: &str) -> LabelSummary {
        LabelSummary {
            id: name.into(),
            name: name.into(),
            tape_width_mm: 12.0,
            saved_at: String::new(),
        }
    }

    #[test]
    fn search_is_a_trimmed_case_insensitive_substring() {
        let labels = [label("Cable tags"), label("Wi-Fi"), label("Spices")];
        let names = |f: &str| matching(&labels, f).iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("  TAG "), ["Cable tags"]);
        assert_eq!(names(""), ["Cable tags", "Wi-Fi", "Spices"]);
        assert!(names("zzz").is_empty());
    }
}
