//! Fonts dialog: user fonts and installed fonts (port of `web/src/ui/fonts/FontManager.svelte`).
//!
//! "Your fonts" lists the uploaded fonts with a sample drawn by the label renderer (so it shows
//! how the font prints), adds font files and removes them. "On this computer" lists the
//! installed fonts (the desktop stand-in for Local Font Access); either kind can be applied to
//! the selected text block.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use baradar_core::doc::{FontSource, Item, LabelDoc, LengthSettings, Margins, TextSize};
use baradar_core::render::fonts::{LocalFontInfo, MAX_FONT_BYTES, MAX_FONTS, check_font_count, inspect_font_file};
use baradar_core::render::renderer::{RenderOptions, RenderTarget};
use baradar_core::store::{FontFormat, UserFontInfo};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch::TapeKind;

use crate::batch::bitmap_image;
use crate::studio::{Studio, width_byte};

/// The sample line of every font row (`FontManager.svelte`).
const SAMPLE: &str = "Shelf A-12 0123";
/// Height of the installed-fonts list.
const LOCAL_LIST_H: f32 = 240.0;
/// Row height of the installed-fonts list.
const LOCAL_ROW_H: f32 = 26.0;

impl Studio {
    /// `studio.fontsChanged()`: re-renders the label (a font it uses may have come or gone).
    pub fn fonts_changed(&mut self, cx: &mut Context<Self>) {
        self.render = None;
        self.rendering = false;
        self.schedule_render(cx);
    }

    /// Applies a custom font to the selected text block (`FontPicker` pick: `{customFont}`).
    /// Returns `false` when no text block is selected.
    pub fn apply_custom_font(&mut self, src: FontSource, cx: &mut Context<Self>) -> bool {
        let Some(id) = self
            .selected_item()
            .filter(|i| matches!(i, Item::Text(_)))
            .map(|i| i.id().to_owned())
        else {
            return false;
        };
        self.update_item(
            &id,
            None,
            move |item| {
                if let Item::Text(t) = item {
                    t.custom_font = Some(src);
                }
            },
            cx,
        );
        true
    }
}

/// Opens the "Fonts" dialog.
pub fn open_fonts(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| FontsDialog::new(studio, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Fonts")
            .w(px(640.))
            .child(view.clone())
    });
}

/// Installed fonts, once asked for (`localFonts.status`).
enum LocalFonts {
    NotLoaded,
    Loading,
    Ready(Arc<Vec<LocalFontInfo>>),
}

struct FontsDialog {
    studio: Entity<Studio>,
    fonts: Vec<UserFontInfo>,
    /// Rendered sample per font key (`u:<ref>` / `l:<postscript name>`); `None` = failed.
    samples: HashMap<String, Option<Arc<RenderImage>>>,
    busy: bool,
    error: Option<String>,
    status: Option<String>,
    local: LocalFonts,
    local_filter: Entity<InputState>,
    /// Indices into the local list that match the filter.
    local_shown: Arc<Vec<usize>>,
    local_selected: Option<String>,
    _subs: Vec<Subscription>,
}

/// `FORMAT`.
fn format_name(f: FontFormat) -> &'static str {
    match f {
        FontFormat::Ttf => "TrueType",
        FontFormat::Otf => "OpenType",
        FontFormat::Woff => "WOFF",
        FontFormat::Woff2 => "WOFF2",
    }
}

/// `kb(n)`: "48 KB" / "1.2 MB".
fn kb(n: usize) -> String {
    if n < 1024 * 1024 {
        format!("{} KB", (n as f64 / 1024.0).round().max(1.0))
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

fn source_key(src: &FontSource) -> String {
    match src {
        FontSource::User { reference, .. } => format!("u:{reference}"),
        FontSource::Local { postscript_name, .. } => format!("l:{postscript_name}"),
    }
}

/// A one-line label in `src`, for the sample image.
fn sample_doc(src: FontSource, width_mm: f64) -> LabelDoc {
    let mut doc = LabelDoc::new();
    doc.tape.width_mm = width_mm;
    doc.length = LengthSettings::Auto;
    doc.margins_mm = Margins { start: 1.0, end: 1.0 };
    if let Some(Item::Text(t)) = doc.items.first_mut() {
        t.text = SAMPLE.into();
        t.custom_font = Some(src);
        t.font_weight = 400;
        t.size = TextSize::Fit;
    }
    doc
}

impl FontsDialog {
    fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let local_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search installed fonts"));
        let subs = vec![cx.subscribe(&local_filter, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::Change = ev {
                this.refilter(cx);
            }
        })];
        let mut this = Self {
            studio,
            fonts: Vec::new(),
            samples: HashMap::new(),
            busy: false,
            error: None,
            status: None,
            local: LocalFonts::NotLoaded,
            local_filter,
            local_shown: Arc::new(Vec::new()),
            local_selected: None,
            _subs: subs,
        };
        this.refresh(cx);
        this
    }

    /// `refresh()`: reloads the list and renders missing samples.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fonts = self.studio.read(cx).store.list_fonts();
        let wanted: Vec<FontSource> = self
            .fonts
            .iter()
            .map(|f| FontSource::User {
                reference: f.reference.clone(),
                family: f.family.clone(),
            })
            .filter(|s| !self.samples.contains_key(&source_key(s)))
            .collect();
        for src in wanted {
            self.render_sample(src, cx);
        }
        cx.notify();
    }

    /// The model and a tape for samples: the label's target, else 12 mm (or the narrowest) TZe.
    fn sample_target(&self, cx: &App) -> Option<RenderTarget> {
        let s = self.studio.read(cx);
        if let Ok(t) = s.target() {
            return Some(t);
        }
        let profile = s.model();
        let tape = profile
            .tape(width_byte(12.0), TapeKind::Tze)
            .or_else(|| profile.media.iter().copied().find(|t| t.kind == TapeKind::Tze))?;
        Some(RenderTarget { profile, tape })
    }

    /// Renders the sample line in `src` in the background.
    fn render_sample(&mut self, src: FontSource, cx: &mut Context<Self>) {
        let key = source_key(&src);
        let Some(target) = self.sample_target(cx) else {
            self.samples.insert(key, None);
            return;
        };
        let renderer = self.studio.read(cx).renderer.clone();
        let width_mm = f64::from(target.tape.width_mm_x10) / 10.0;
        let doc = sample_doc(src, width_mm);
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    let r = renderer.render(&doc, &target, &RenderOptions::default()).ok()?;
                    // A font that did not load falls back to the built-in one: no sample then.
                    if r.warnings.iter().any(|w| w.code.contains("font")) {
                        return None;
                    }
                    bitmap_image(&r.bitmap, (0xFF_FF_FF, 0x00_00_00))
                })
                .await;
            this.update(cx, |this, cx| {
                this.samples.insert(key, image);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// [Add font file…].
    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update_in(cx, |this, window, cx| this.add(paths, window, cx)).ok();
            }
        })
        .detach();
    }

    /// `add(files)`: validates, names and stores each font file, then registers it.
    fn add(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let store = self.studio.read(cx).store.clone();
        let fonts = self.studio.read(cx).fonts.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (added, failed) = cx
                .background_executor()
                .spawn(async move {
                    let mut added: Vec<String> = Vec::new();
                    let mut failed: Vec<String> = Vec::new();
                    for path in paths {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let result = (|| -> Result<String, String> {
                            let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                            if size > MAX_FONT_BYTES as u64 {
                                return Err(format!(
                                    "This font is too large ({:.1} MB). Fonts up to {} MB can be added.",
                                    size as f64 / 1024.0 / 1024.0,
                                    MAX_FONT_BYTES / 1024 / 1024
                                ));
                            }
                            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                            let info = inspect_font_file(&bytes, &file_name).map_err(|e| e.to_string())?;
                            check_font_count(store.list_fonts().len()).map_err(|e| e.to_string())?;
                            let stored = store
                                .add_font(&bytes, &info.family, &file_name)
                                .map_err(|e| e.to_string())?;
                            fonts
                                .add_user_font(&stored.reference, &bytes)
                                .map_err(|e| e.to_string())?;
                            Ok(stored.family)
                        })();
                        match result {
                            Ok(family) => added.push(family),
                            Err(e) => failed.push(format!("{file_name}: {e}")),
                        }
                    }
                    (added, failed)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if !failed.is_empty() {
                    this.error = Some(failed.join(" "));
                }
                if !added.is_empty() {
                    let status = if added.len() == 1 {
                        format!("Added “{}”.", added[0])
                    } else {
                        format!("Added {} fonts.", added.len())
                    };
                    window.push_notification(Notification::new().message(status.clone()), cx);
                    this.status = Some(status);
                    this.studio.update(cx, |s, cx| s.fonts_changed(cx));
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// `remove(f)`, after `confirm(…)`.
    fn confirm_remove(&mut self, font: UserFontInfo, window: &mut Window, cx: &mut Context<Self>) {
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Remove “{}”?", font.family),
            Some("Labels that use it will show the built-in font instead."),
            &["Remove", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                this.update_in(cx, |this, window, cx| this.remove(font, window, cx)).ok();
            }
        })
        .detach();
    }

    fn remove(&mut self, font: UserFontInfo, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.studio.read(cx);
        match s.store.remove_font(&font.reference) {
            Ok(()) => {
                s.fonts.forget_custom_font(&FontSource::User {
                    reference: font.reference.clone(),
                    family: font.family.clone(),
                });
                self.samples.remove(&format!("u:{}", font.reference));
                let status = format!("Removed “{}”.", font.family);
                window.push_notification(Notification::new().message(status.clone()), cx);
                self.status = Some(status);
                self.studio.update(cx, |s, cx| s.fonts_changed(cx));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.refresh(cx);
    }

    /// Sets `src` on the selected text block.
    fn apply(&mut self, src: FontSource, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let ok = self.studio.update(cx, |s, cx| s.apply_custom_font(src, cx));
        let msg = if ok {
            format!("Font: {name}")
        } else {
            "Select a text block first.".to_owned()
        };
        window.push_notification(Notification::new().message(msg), cx);
    }

    /// [Use fonts from this computer…] (`localFonts.query()`).
    fn query_local(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.local, LocalFonts::NotLoaded) {
            return;
        }
        self.local = LocalFonts::Loading;
        cx.notify();
        let fonts = self.studio.read(cx).fonts.clone();
        cx.spawn(async move |this, cx| {
            let list = cx.background_executor().spawn(async move { fonts.local_fonts() }).await;
            this.update(cx, |this, cx| {
                this.local = LocalFonts::Ready(Arc::new(list));
                this.refilter(cx);
            })
            .ok();
        })
        .detach();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let LocalFonts::Ready(list) = &self.local else { return };
        let query = self.local_filter.read(cx).value().to_lowercase();
        let tokens: Vec<&str> = query.split_whitespace().collect();
        let shown: Vec<usize> = list
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let name = f.name.to_lowercase();
                tokens.iter().all(|t| name.contains(t))
            })
            .map(|(i, _)| i)
            .collect();
        self.local_shown = Arc::new(shown);
        cx.notify();
    }

    fn select_local(&mut self, font: LocalFontInfo, cx: &mut Context<Self>) {
        self.local_selected = Some(font.postscript_name.clone());
        let src = FontSource::Local {
            postscript_name: font.postscript_name,
            family: font.name,
        };
        if !self.samples.contains_key(&source_key(&src)) {
            self.render_sample(src, cx);
        }
        cx.notify();
    }

    fn sample_view(&self, key: &str, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match self.samples.get(key) {
            Some(Some(image)) => div()
                .h(px(36.))
                .flex()
                .items_center()
                .child(img(image.clone()).max_h(px(36.)).max_w(px(360.)).object_fit(ObjectFit::Contain))
                .into_any_element(),
            Some(None) => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No preview: the font could not be drawn.")
                .into_any_element(),
            None => div().text_xs().text_color(theme.muted_foreground).child("Loading…").into_any_element(),
        }
    }

    fn render_yours(&self, can_apply: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mut list = v_flex().gap_2();
        if self.fonts.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No fonts added yet. Added fonts appear in the Font list of every text block."),
            );
        }
        for (i, f) in self.fonts.iter().enumerate() {
            let font = f.clone();
            let apply_src = FontSource::User {
                reference: f.reference.clone(),
                family: f.family.clone(),
            };
            let apply_name = f.family.clone();
            list = list.child(
                h_flex()
                    .gap_3()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(f.family.clone()))
                            .child(div().text_xs().text_color(muted).child(format!(
                                "{} · {} · {}",
                                format_name(f.format),
                                kb(f.size),
                                f.file_name
                            )))
                            .child(self.sample_view(&format!("u:{}", f.reference), cx)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("apply-user-{i}")))
                            .small()
                            .outline()
                            .icon(crate::icons::icon("type")).label("Use for selected text")
                            .disabled(!can_apply)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.apply(apply_src.clone(), apply_name.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("remove-user-{i}")))
                            .small()
                            .ghost()
                            .icon(IconName::Delete)
                            .tooltip(format!("Remove {}", f.family))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.confirm_remove(font.clone(), window, cx)
                            })),
                    ),
            );
        }
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_base().font_weight(FontWeight::SEMIBOLD).child("Your fonts"))
                    .child(
                        Button::new("add-font")
                            .primary()
                            .icon(IconName::FolderOpen)
                            .label(if self.busy { "Adding…" } else { "Add font file…" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.pick_files(window, cx))),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(format!(
                "TTF, OTF, WOFF or WOFF2, up to {} MB each and {MAX_FONTS} fonts. Only add fonts you are allowed to use.",
                MAX_FONT_BYTES / 1024 / 1024
            )))
            .when_some(self.error.clone(), |d, e| d.child(div().text_xs().text_color(theme.danger).child(e)))
            .when_some(self.status.clone(), |d, s| d.child(div().text_xs().text_color(theme.success).child(s)))
            .child(list)
            .into_any_element()
    }

    fn render_local(&self, can_apply: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mut section = v_flex()
            .gap_2()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("On this computer"))
            .child(div().text_xs().text_color(muted).child(
                "Fonts installed on this computer can be used too. They vary by machine: a label that uses one prints as designed only on a computer that has the font.",
            ));
        match &self.local {
            LocalFonts::NotLoaded | LocalFonts::Loading => {
                let loading = matches!(self.local, LocalFonts::Loading);
                section = section.child(
                    h_flex().child(
                        Button::new("query-local")
                            .outline()
                            .icon(IconName::HardDrive)
                            .label(if loading { "Asking…" } else { "Use fonts from this computer…" })
                            .disabled(loading)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.query_local(cx))),
                    ),
                );
            }
            LocalFonts::Ready(list) => {
                let n = list.len();
                section = section.child(
                    h_flex()
                        .gap_1()
                        .text_sm()
                        .text_color(theme.success)
                        .child(IconName::Check)
                        .child(format!(
                            "{} available. Pick them in the Font list of a text block, under “This computer”.",
                            if n == 1 { "1 font".to_owned() } else { format!("{n} fonts") }
                        )),
                );
                section = section.child(Input::new(&self.local_filter).small().cleanable(true));
                let shown = self.local_shown.clone();
                let list_for_rows = list.clone();
                let selected = self.local_selected.clone();
                let hover = theme.secondary;
                let active = theme.primary.opacity(0.12);
                section = section.child(
                    div()
                        .h(px(LOCAL_LIST_H))
                        .border_1()
                        .border_color(theme.border)
                        .rounded_md()
                        .child(
                            uniform_list(
                                "local-fonts",
                                shown.len(),
                                cx.processor(move |_this, range: Range<usize>, _window, cx| {
                                    range
                                        .filter_map(|i| {
                                            let font = list_for_rows.get(*shown.get(i)?)?.clone();
                                            let is_sel = selected.as_deref() == Some(font.postscript_name.as_str());
                                            Some(
                                                div()
                                                    .id(i)
                                                    .h(px(LOCAL_ROW_H))
                                                    .px_2()
                                                    .flex()
                                                    .items_center()
                                                    .text_sm()
                                                    .cursor_pointer()
                                                    .when(is_sel, |d| d.bg(active))
                                                    .hover(|s| s.bg(hover))
                                                    .child(font.name.clone())
                                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                        this.select_local(font.clone(), cx)
                                                    })),
                                            )
                                        })
                                        .collect()
                                }),
                            )
                            .size_full(),
                        ),
                );
                if let Some(ps) = &self.local_selected
                    && let Some(font) = list.iter().find(|f| &f.postscript_name == ps)
                {
                    let src = FontSource::Local {
                        postscript_name: font.postscript_name.clone(),
                        family: font.name.clone(),
                    };
                    let name = font.name.clone();
                    section = section.child(
                        h_flex()
                            .gap_3()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(font.name.clone()))
                                    .child(self.sample_view(&format!("l:{}", font.postscript_name), cx)),
                            )
                            .child(
                                Button::new("apply-local")
                                    .small()
                                    .outline()
                                    .icon(crate::icons::icon("type")).label("Use for selected text")
                                    .disabled(!can_apply)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        this.apply(src.clone(), name.clone(), window, cx)
                                    })),
                            ),
                    );
                }
            }
        }
        section.into_any_element()
    }
}

impl Render for FontsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let can_apply = matches!(self.studio.read(cx).selected_item(), Some(Item::Text(_)));
        let yours = self.render_yours(can_apply, cx);
        let local = self.render_local(can_apply, cx);
        v_flex()
            .gap_4()
            .pb_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Use your own fonts in text blocks. They stay on this computer."),
            )
            .child(yours)
            .child(div().h(px(1.)).bg(theme.border))
            .child(local)
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(IconName::Info)
                    .child(div().flex_1().child(
                        "Share links and exported labels do not include fonts. Someone without the font sees the built-in font instead.",
                    )),
            )
    }
}

/// Headless UI tests (fonts are read from the core's bundled assets).
#[cfg(test)]
mod ui_tests {
    // NOT `use super::*` (see `batch::ui_tests`).
    use super::FontsDialog;
    use crate::batch::ui_tests::{new_studio, open_test_window, wait_until};
    use baradar_core::doc::{FontSource, Item};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext};
    use std::path::PathBuf;

    #[gpui_kit::test]
    async fn add_apply_and_remove_a_font(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "fonts");
        let s2 = studio.clone();
        let (handle, view) = open_test_window(cx, move |window, cx| FontsDialog::new(s2, window, cx));
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../baradar-core/assets/fonts/Anton-Regular.woff2");
        let v = view.clone();
        cx.update_window(handle, move |_, window, cx| {
            window.render_frame(cx);
            v.update(cx, |d, cx| d.add(vec![file, PathBuf::from("/nonexistent/x.ttf")], window, cx));
        })
        .expect("window");
        wait_until(cx, "the font to be added", |cx| cx.update(|cx| !view.read(cx).busy));
        cx.update(|cx| {
            let d = view.read(cx);
            assert_eq!(d.fonts.len(), 1, "{:?}", d.error);
            assert!(d.error.as_deref().is_some_and(|e| e.contains("x.ttf")), "the missing file is reported");
        });
        let reference = cx.update(|cx| view.read(cx).fonts[0].reference.clone());
        wait_until(cx, "the sample", |cx| {
            cx.update(|cx| view.read(cx).samples.contains_key(&format!("u:{reference}")))
        });
        cx.update(|cx| {
            assert!(
                view.read(cx).samples.get(&format!("u:{reference}")).is_some_and(Option::is_some),
                "the sample is drawn in the font"
            )
        });

        // Apply it to the selected text block.
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("apply-user-0", cx);
        })
        .expect("window");
        cx.update(|cx| match studio.read(cx).selected_item() {
            Some(Item::Text(t)) => {
                assert!(matches!(&t.custom_font, Some(FontSource::User { reference: r, .. }) if *r == reference))
            }
            other => panic!("expected the text block, got {other:?}"),
        });

        // Remove (the confirmation is the native prompt; call the action directly).
        let font = cx.update(|cx| view.read(cx).fonts[0].clone());
        let v = view.clone();
        cx.update_window(handle, move |_, window, cx| v.update(cx, |d, cx| d.remove(font, window, cx)))
            .expect("window");
        cx.update(|cx| {
            assert!(view.read(cx).fonts.is_empty());
            assert!(studio.read(cx).store.list_fonts().is_empty());
        });
    }

    #[gpui_kit::test]
    async fn lists_installed_fonts(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "local-fonts");
        let (handle, view) = open_test_window(cx, move |window, cx| FontsDialog::new(studio, window, cx));
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("query-local", cx);
        })
        .expect("window");
        wait_until(cx, "the installed fonts", |cx| {
            cx.update(|cx| matches!(view.read(cx).local, super::LocalFonts::Ready(_)))
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx)).expect("window");
    }
}
