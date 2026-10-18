//! The studio window: top bar, three columns (insert + blocks | media bar + preview | properties)
//! and the print bar (port of `web/src/App.svelte` and `ui/TopBar.svelte`).

use baradar_core::store::Store;
use baradar_printer::ClientState;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::{Notification, NotificationType},
    resizable_panel, h_resizable, v_flex,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::{Disableable as _, menu::DropdownMenu as _};

use crate::actions::*;
use crate::icons::icon;
use crate::preview::PreviewState;
use crate::studio::{Studio, StudioEvent, Tone, Zoom, is_connected};
use crate::view_model::{ZOOM_STEPS, format_mm};

pub struct StudioView {
    pub studio: Entity<Studio>,
    focus: FocusHandle,
    name_input: Entity<InputState>,
    name_epoch: u64,
    pub preview: PreviewState,
    props: Entity<crate::props::PropertiesView>,
    media: Entity<crate::media::MediaBar>,
    batch: Entity<crate::batch::BatchPanel>,
    /// Where a block dragged in the block list would land (insertion index), while dragging.
    pub block_drop: Option<usize>,
    updater: Entity<crate::update_ui::Updater>,
    _subs: Vec<Subscription>,
}

impl StudioView {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let studio = cx.new(|cx| Studio::new(store, cx));
        // Capture the packet log from launch (Diagnostics keeps it in a global).
        crate::diagnostics::packet_log(&studio, cx);
        let name = studio.read(cx).doc.name.clone();
        let name_input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let mut subs = vec![
            cx.observe(&studio, |this, studio, cx| {
                this.sync_inputs(&studio, cx);
                cx.notify();
            }),
            cx.subscribe_in(&studio, window, |_, _, ev: &StudioEvent, window, cx| match ev {
                StudioEvent::Toast { tone, title, detail } => {
                    let kind = match tone {
                        Tone::Info => NotificationType::Info,
                        Tone::Ok => NotificationType::Success,
                        Tone::Warn => NotificationType::Warning,
                        Tone::Error => NotificationType::Error,
                    };
                    let mut n = Notification::new().title(title.clone()).with_type(kind);
                    if let Some(d) = detail {
                        n = n.message(d.clone());
                    }
                    window.push_notification(n, cx);
                }
                StudioEvent::Printer(_) => {}
            }),
            cx.subscribe_in(&name_input, window, |this, input, ev: &InputEvent, _, cx| {
                if let InputEvent::Blur | InputEvent::PressEnter { .. } = ev {
                    let v = input.read(cx).value().to_string();
                    this.studio.update(cx, |s, cx| s.set_name(&v, cx));
                }
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                gpui_kit::component::Theme::sync_system_appearance(Some(window), cx);
            }),
        ];
        let epoch = studio.read(cx).epoch;
        let props = cx.new(|cx| crate::props::PropertiesView::new(studio.clone(), window, cx));
        let media = cx.new(|cx| crate::media::MediaBar::new(studio.clone(), window, cx));
        let batch = cx.new(|cx| crate::batch::BatchPanel::new(studio.clone(), window, cx));
        let check_updates = studio.read(cx).prefs.check_updates;
        let updater = cx.new(|cx| crate::update_ui::Updater::new(check_updates, cx));
        subs.push(cx.observe(&updater, |_, _, cx| cx.notify()));
        Self {
            studio,
            focus: cx.focus_handle(),
            name_input,
            name_epoch: epoch,
            preview: PreviewState::default(),
            props,
            media,
            batch,
            block_drop: None,
            updater,
            _subs: subs,
        }
    }

    /// Reloads inputs whose value came from a replaced document (undo, open…).
    fn sync_inputs(&mut self, studio: &Entity<Studio>, cx: &mut Context<Self>) {
        let s = studio.read(cx);
        if s.epoch != self.name_epoch {
            self.name_epoch = s.epoch;
            let name = s.doc.name.clone();
            let input = self.name_input.clone();
            if let Some(window) = cx.windows().first().copied() {
                let _ = window.update(cx, |_, window, cx| {
                    input.update(cx, |i, cx| i.set_value(name, window, cx));
                });
            }
        }
    }

    // ----- actions -----

    fn on_new(&mut self, _: &NewLabel, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.new_label(cx));
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.undo(cx));
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.redo(cx));
    }

    fn on_print(&mut self, _: &Print, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.print(cx));
    }

    fn on_cancel_print(&mut self, _: &CancelPrint, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, _| s.cancel());
    }

    fn on_delete(&mut self, _: &DeleteBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            if let Some(id) = s.selected.clone() {
                s.remove_item(&id, cx);
            }
        });
    }

    fn on_duplicate(&mut self, _: &DuplicateBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            if let Some(id) = s.selected.clone() {
                s.duplicate(&id, cx);
            }
        });
    }

    fn on_prev(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.select_relative(-1, cx));
    }

    fn on_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.select_relative(1, cx));
    }

    fn on_move_up(&mut self, _: &MoveBlockUp, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            if let Some(id) = s.selected.clone() {
                s.move_by(&id, -1, cx);
            }
        });
    }

    fn on_move_down(&mut self, _: &MoveBlockDown, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            if let Some(id) = s.selected.clone() {
                s.move_by(&id, 1, cx);
            }
        });
    }

    fn zoom_step(&mut self, dir: i32, cx: &mut Context<Self>) {
        let current = match self.studio.read(cx).zoom {
            Zoom::Real(z) => z,
            Zoom::Fit => self.preview.fit_ratio(),
        };
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|z| *z > current + 1e-3)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 1e-3)
        };
        if let Some(z) = next {
            self.studio.update(cx, |s, cx| {
                s.zoom = Zoom::Real(z);
                cx.notify();
            });
        }
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_step(1, cx);
    }

    fn on_zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_step(-1, cx);
    }

    fn on_zoom_fit(&mut self, _: &ZoomFit, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            s.zoom = Zoom::Fit;
            cx.notify();
        });
    }

    fn on_zoom_real(&mut self, _: &ZoomReal, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| {
            s.zoom = Zoom::Real(1.0);
            cx.notify();
        });
    }

    fn on_connect(&mut self, _: &ConnectPrinter, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::connect::open_connect);
    }

    fn on_disconnect(&mut self, _: &DisconnectPrinter, _: &mut Window, cx: &mut Context<Self>) {
        self.studio.update(cx, |s, cx| s.disconnect(cx));
    }

    fn studio_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>, open: fn(Entity<Studio>, &mut Window, &mut App)) {
        open(self.studio.clone(), window, cx);
    }

    fn on_library(&mut self, _: &OpenLibrary, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::library::open_library);
    }

    fn on_templates(&mut self, _: &NewFromTemplate, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::templates::open_templates);
    }

    fn on_history(&mut self, _: &PrintHistory, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::history::open_history);
    }

    fn on_export_image(&mut self, _: &ExportImage, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::export::open_export_image);
    }

    fn on_share(&mut self, _: &CopyShareLink, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::share::copy_share_link);
    }

    fn on_shortcuts(&mut self, _: &ShowShortcuts, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::dialogs::shortcuts::open_shortcuts);
    }

    fn on_fonts(&mut self, _: &ManageFonts, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::fonts_ui::open_fonts);
    }

    fn on_printer_settings(&mut self, _: &PrinterSettings, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::connect::open_printer_settings);
    }

    fn on_check_updates(&mut self, _: &CheckForUpdates, w: &mut Window, cx: &mut Context<Self>) {
        crate::update_ui::open_update_dialog(self.updater.clone(), self.studio.clone(), w, cx);
    }

    fn on_diagnostics(&mut self, _: &ShowDiagnostics, w: &mut Window, cx: &mut Context<Self>) {
        self.studio_dialog(w, cx, crate::diagnostics::open_diagnostics);
    }

    fn on_import(&mut self, _: &ImportFile, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        let studio = self.studio.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                studio.update(cx, |s, cx| s.import_file(path, cx));
            }
        })
        .detach();
    }

    /// `requestShare('file')`: asks about Wi-Fi passwords when the label has any, then saves.
    fn on_export(&mut self, _: &ExportFile, window: &mut Window, cx: &mut Context<Self>) {
        crate::dialogs::share::request_export_file(self.studio.clone(), window, cx);
    }

    // ----- rendering -----

    fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.studio.read(cx);
        let theme = cx.theme();
        let (chip_text, chip_detail) = chip_view(s);
        let busy = matches!(s.conn, ClientState::Opening | ClientState::Waking | ClientState::Handshaking);
        let connected = is_connected(s.conn);
        let lost = matches!(s.conn, ClientState::Lost | ClientState::NoReply | ClientState::Error);
        let chip_icon = if lost { "alert" } else { "printer" };
        let update_label = match &self.updater.read(cx).state {
            crate::update_ui::UpdateState::Available(_) => Some("Update available".to_owned()),
            crate::update_ui::UpdateState::Downloading { done, total, .. } if *total > 0 => {
                Some(format!("Updating… {}%", done * 100 / total))
            }
            crate::update_ui::UpdateState::Installing => Some("Restarting…".to_owned()),
            _ => None,
        };
        // Battery / AC indicator next to the chip (`batteryText`).
        let battery = s.printer_info.as_ref().filter(|_| connected).map(|i| {
            use ptouch::status::{Battery, PowerSource};
            match i.battery {
                Battery::Level { source: PowerSource::Ac, .. } | Battery::NoBattery { source: PowerSource::Ac } => "plug",
                _ if i.weak_battery() => "battery-low",
                _ => "battery",
            }
        });
        h_flex()
            .id("top-bar")
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.title_bar)
            .child(div().font_weight(FontWeight::BOLD).child("Baradar"))
            .child(
                Button::new("labels-menu")
                    .icon(icon("tag"))
                    .label("Labels")
                    .dropdown_caret(true)
                    .small()
                    .ghost()
                    .dropdown_menu(|m, _, _| {
                        m.menu("New label", Box::new(NewLabel))
                            .menu("New from template…", Box::new(NewFromTemplate))
                            .menu("Open…", Box::new(OpenLibrary))
                            .menu("Print history…", Box::new(PrintHistory))
                            .separator()
                            .menu("Import file…", Box::new(ImportFile))
                            .menu("Export file", Box::new(ExportFile))
                            .menu("Export image (PNG, PDF)…", Box::new(ExportImage))
                            .menu("Copy share link", Box::new(CopyShareLink))
                            .separator()
                            .menu("Fonts…", Box::new(ManageFonts))
                    }),
            )
            .child(div().w(px(260.)).child(Input::new(&self.name_input).small()))
            .when(s.read_only, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child("Read-only")))
            .child(
                Button::new("undo")
                    .icon(icon("undo"))
                    .small()
                    .ghost()
                    .tooltip("Undo (⌘Z)")
                    .disabled(!s.can_undo())
                    .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.undo(cx)))),
            )
            .child(
                Button::new("redo")
                    .icon(icon("redo"))
                    .small()
                    .ghost()
                    .tooltip("Redo (⇧⌘Z)")
                    .disabled(!s.can_redo())
                    .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.redo(cx)))),
            )
            .child(div().flex_1())
            .when_some(update_label, |d, label| {
                d.child(
                    Button::new("update-available")
                        .icon(icon("download"))
                        .label(label)
                        .small()
                        .primary()
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(CheckForUpdates), cx)),
                )
            })
            .child(
                Button::new("theme")
                    .icon(icon(if theme.is_dark() { "moon" } else { "sun" }))
                    .small()
                    .ghost()
                    .tooltip(if theme.is_dark() { "Dark theme" } else { "Light theme" })
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleTheme), cx)),
            )
            .child(
                Button::new("shortcuts")
                    .icon(icon("keyboard"))
                    .small()
                    .ghost()
                    .tooltip("Keyboard shortcuts (⌘/)")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowShortcuts), cx)),
            )
            .child(
                Button::new("diagnostics")
                    .icon(icon("activity"))
                    .label("Diagnostics")
                    .small()
                    .ghost()
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowDiagnostics), cx)),
            )
            .child(
                Button::new("chip")
                    .icon(icon(chip_icon))
                    .label(chip_text)
                    .small()
                    .when(connected, |b| b.outline())
                    .when(!connected, |b| b.primary())
                    .loading(busy)
                    .tooltip(chip_detail)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !busy {
                            this.studio_dialog(window, cx, crate::connect::open_connect);
                        }
                    })),
            )
            .when_some(battery, |d, b| {
                d.child(
                    div()
                        .id("battery")
                        .text_color(if b == "battery-low" { theme.warning } else { theme.muted_foreground })
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(match b {
                                "plug" => "On AC power",
                                "battery-low" => "Battery low",
                                _ => "On battery",
                            })
                            .build(window, cx)
                        })
                        .child(icon(b).small()),
                )
            })
            .when(busy, |d| {
                d.child(
                    Button::new("cancel-connect")
                        .icon(icon("x"))
                        .label("Cancel")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, _| s.cancel()))),
                )
            })
            .when(lost, |d| {
                d.child(
                    Button::new("reconnect")
                        .icon(icon("refresh"))
                        .label("Reconnect")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, _| s.reconnect()))),
                )
            })
            .when(connected || lost, |d| {
                d.child(
                    Button::new("disconnect")
                        .icon(icon("eject"))
                        .small()
                        .ghost()
                        .tooltip("Disconnect printer")
                        .disabled(matches!(s.conn, ClientState::Printing))
                        .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.disconnect(cx)))),
                )
            }).into_any_element()
    }
}

/// `chipView`: the connection chip's text and detail.
fn chip_view(s: &Studio) -> (String, String) {
    let model = s
        .printer_info
        .as_ref()
        .and_then(|i| i.model)
        .map_or("Printer", |m| m.name);
    match s.conn {
        ClientState::Connected => {
            let info = s.printer_info.as_ref();
            let tape = info.and_then(|i| i.tape_width_text());
            match tape {
                Some(t) if info.is_some_and(|i| i.errors().is_empty()) => {
                    let battery = if info.is_some_and(|i| i.weak_battery()) { " · Battery low" } else { "" };
                    (format!("{model} · {t}"), format!("Connected{battery}"))
                }
                Some(_) => (format!("{model} · needs attention"), String::new()),
                None => (format!("{model} · no tape"), String::new()),
            }
        }
        ClientState::Opening => ("Connecting…".into(), String::new()),
        ClientState::Waking => (
            "Waking printer…".into(),
            match s.open_progress {
                Some((a, of)) if a > 1 => format!("Attempt {a} of {of}"),
                _ => "This can take up to 10 s".into(),
            },
        ),
        ClientState::Handshaking => ("Talking to printer…".into(), String::new()),
        ClientState::Printing => match s.progress {
            Some(p) if p.of > 1 => (format!("Printing {}/{}", p.page, p.of), String::new()),
            _ => ("Printing…".into(), String::new()),
        },
        ClientState::Cancelling => ("Cancelling…".into(), String::new()),
        ClientState::Settings => (format!("{model} · settings…"), String::new()),
        ClientState::NoReply => ("Printer asleep?".into(), "Press the power button, then Reconnect".into()),
        ClientState::Lost => ("Connection lost".into(), "Reconnect when the printer is on".into()),
        ClientState::Error => match &s.problem {
            Some(p) if p.title.chars().count() <= 24 => (p.title.clone(), p.detail.clone()),
            Some(p) => ("Printer problem".into(), p.title.clone()),
            None => ("Couldn’t connect".into(), String::new()),
        },
        ClientState::Idle => ("Connect printer".into(), "Pair the printer in System Settings → Bluetooth first".into()),
    }
}

impl Focusable for StudioView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StudioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let top = self.top_bar(cx);
        let left = self.left_panel(cx);
        let center = self.center_panel(window, cx);
        let right = self.right_panel(cx);
        let print_bar = self.print_bar(cx);
        v_flex()
            .id("studio")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_print))
            .on_action(cx.listener(Self::on_cancel_print))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_duplicate))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_fit))
            .on_action(cx.listener(Self::on_zoom_real))
            .on_action(cx.listener(Self::on_connect))
            .on_action(cx.listener(Self::on_disconnect))
            .on_action(cx.listener(Self::on_import))
            .on_action(cx.listener(Self::on_export))
            .on_action(cx.listener(Self::on_library))
            .on_action(cx.listener(Self::on_templates))
            .on_action(cx.listener(Self::on_history))
            .on_action(cx.listener(Self::on_export_image))
            .on_action(cx.listener(Self::on_share))
            .on_action(cx.listener(Self::on_shortcuts))
            .on_action(cx.listener(Self::on_fonts))
            .on_action(cx.listener(Self::on_printer_settings))
            .on_action(cx.listener(Self::on_diagnostics))
            .on_action(cx.listener(Self::on_check_updates))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(top)
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("body")
                        .child(resizable_panel().size(px(250.)).size_range(px(200.)..px(360.)).child(left))
                        .child(resizable_panel().child(center))
                        .child(resizable_panel().size(px(320.)).size_range(px(260.)..px(460.)).child(right)),
                ),
            )
            .child(print_bar)
    }
}

impl StudioView {
    /// The centre column: banners, the preview and its readout.
    fn center_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.studio.read(cx);
        let theme = cx.theme().clone();
        let mismatch = s.mismatch();
        let problem = s.problem.clone();
        let studio = self.studio.clone();
        let preview = self.preview_element(window, cx);
        v_flex()
            .id("center")
            .size_full()
            .min_w_0()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .when_some(problem, |d, p| d.child(crate::connect::problem_banner(&studio, &p, cx)))
            .when_some(mismatch, |d, (loaded, design)| {
                d.child(
                    h_flex()
                        .p_2()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.warning)
                        .child(div().text_sm().child(format!(
                            "Tape mismatch. This label is designed for {} tape, but the printer has {} loaded.",
                            format_mm(design),
                            format_mm(loaded)
                        )))
                        .child(
                            Button::new("use-loaded")
                                .label(format!("Use loaded {} tape", format_mm(loaded)))
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.studio.update(cx, |s, cx| s.use_loaded_tape(cx))
                                })),
                        ),
                )
            })
            .child(self.media.clone())
            .child(preview)
            .child(self.batch.clone()).into_any_element()
    }

    /// The right column: the selected block's properties and the label card.
    fn right_panel(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        div().size_full().child(self.props.clone()).into_any_element()
    }
}
