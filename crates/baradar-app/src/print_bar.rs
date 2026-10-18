//! The print bar: copies, cut/chain/mirror, the tape estimate, the block reason and Print
//! (port of `ui/print/PrintBar.svelte`).

use baradar_core::vars::batch::single_tape_note;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    switch::Switch,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::Disableable as _;

use crate::app::StudioView;
use crate::icons::icon;
use crate::view_model::{PagePhase, PrintProgress, print_button_text, progress_fraction, progress_text};

impl StudioView {
    pub fn print_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let copies = s.doc.print.copies;
        let print = s.doc.print.clone();
        let reason = s.print_block_reason();
        let printing = s.printing;
        let labels = s.label_count();
        let note = s.render.as_ref().map(|r| single_tape_note(r.length_mm, r.feed_margin_mm, copies, !print.chain));
        let progress = s.progress.map(|p| PrintProgress {
            page: u32::from(p.page),
            of: u32::from(p.of),
            phase: match p.phase {
                baradar_printer::PrintPhase::Sending => PagePhase::Sending,
                baradar_printer::PrintPhase::Printing => PagePhase::Printing,
                baradar_printer::PrintPhase::Done => PagePhase::Done,
            },
        });

        let left = h_flex()
            .gap_3()
            .items_center()
            .child(div().text_sm().child("Copies"))
            .child(
                Button::new("copies-dec")
                    .icon(icon("minus"))
                    .small()
                    .disabled(copies <= 1)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.studio.update(cx, |s, cx| s.set_copies(copies.saturating_sub(1), cx))
                    })),
            )
            .child(div().w(px(24.)).text_center().child(copies.to_string()))
            .child(
                Button::new("copies-inc")
                    .icon(icon("plus"))
                    .small()
                    .disabled(copies >= 99)
                    .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.set_copies(copies + 1, cx)))),
            )
            .child(
                Switch::new("cut")
                    .label("Cut each")
                    .checked(print.auto_cut)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        this.studio.update(cx, |s, cx| s.update_doc(None, |d| d.print.auto_cut = v, cx))
                    })),
            )
            .child(
                Switch::new("chain")
                    .label("Chain")
                    .checked(print.chain)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        this.studio.update(cx, |s, cx| s.update_doc(None, |d| d.print.chain = v, cx))
                    })),
            )
            .child(
                Switch::new("mirror")
                    .label("Mirror")
                    .checked(print.mirror)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        this.studio.update(cx, |s, cx| s.update_doc(None, |d| d.print.mirror = v, cx))
                    })),
            )
            .when_some(note, |d, n| d.child(div().text_sm().text_color(theme.muted_foreground).child(n)));

        let right = if printing {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_sm().child(progress_text(progress)))
                .child(div().w(px(160.)).child(Progress::new("print-progress").value(progress_fraction(progress) * 100.0)))
                .child(
                    Button::new("cancel-print")
                        .icon(icon("stop"))
                        .label("Cancel")
                        .small()
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, _| s.cancel()))),
                )
        } else {
            h_flex()
                .gap_3()
                .items_center()
                .when_some(reason.clone(), |d, r| {
                    d.child(div().max_w(px(420.)).text_sm().text_color(theme.muted_foreground).child(r))
                })
                .child(
                    Button::new("print")
                        .icon(icon("printer"))
                        .label(print_button_text(labels))
                        .primary()
                        .disabled(reason.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.print(cx)))),
                )
        };

        h_flex()
            .id("print-bar")
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.title_bar)
            .child(left)
            .child(right).into_any_element()
    }
}
