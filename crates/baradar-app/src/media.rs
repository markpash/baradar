//! The media bar above the preview: tape width, model, colours, length and margins, plus the
//! unsupported-width and untested-model notices (port of `media/MediaBar.svelte`,
//! `media/ModelPicker.svelte`, `media/UnsupportedWidthNotice.svelte`,
//! `media/UntestedModelNotice.svelte`; the pure helpers in [`models`] port `state/models.ts` and
//! the model copy of `connect/copy.ts`).
//!
//! The tape-mismatch banner ("Tape mismatch. This label is designed for …") is drawn by the
//! centre column in `app.rs`, so it is not repeated here.

use baradar_core::doc::{LengthSettings, Margins, TapeColors};
use baradar_printer::LinkKind;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, NumberInput},
    select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch_transport::TransportKind;

use crate::connect::PrinterLink;
use crate::studio::{Studio, is_connected, tape_widths_of};
use crate::view_model::{DEFAULT_COLORS, TAPE_PRESETS, capitalize, format_mm};

pub use models::*;

/// Pure model helpers (port of `ui/state/models.ts` and the model copy in `connect/copy.ts`).
pub mod models {
    use crate::studio::tape_widths_of;
    use crate::view_model::format_mm;
    use ptouch::ModelProfile;

    /// Models checked on real hardware (`VERIFIED_MODELS`). Any other model gets the "untested
    /// model" notice when a real one is connected.
    /// The PT-E720BT was added after a successful Bluetooth print from this app (2026-10-18).
    pub const VERIFIED_MODELS: [&str; 2] = ["PT-P710BT", "PT-E720BT"];

    /// Where "Report an issue" opens a new issue (`ISSUES_URL`).
    pub const ISSUES_URL: &str = "https://github.com/fjanicki/ptouch/issues/new";

    /// Model used in examples ("your printer, e.g. PT-P710BT").
    pub const EXAMPLE_MODEL: &str = "PT-P710BT";
    /// A paired printer's Bluetooth name, masked (`EXAMPLE_DEVICE`).
    pub const EXAMPLE_DEVICE: &str = "PT-P710BTxxxx";

    /// How this app reaches a model (`ModelReach`, desktop flavour: TCP works natively, so a
    /// network-only model is reachable; only Bluetooth LE is not).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ModelReach {
        /// USB, Bluetooth Classic or the network.
        Connects,
        /// Bluetooth LE only (PT-N25BT).
        BleOnly,
        /// The table lists no link.
        None,
    }

    /// `findModel`: a model by marketing name or alias, case-insensitive.
    pub fn find_model(name: Option<&str>) -> Option<&'static ModelProfile> {
        name.and_then(ptouch::profile_by_name)
    }

    /// `modelReach`.
    pub fn model_reach(m: &ModelProfile) -> ModelReach {
        if m.usb_pid.is_some() || m.transports.bluetooth || m.transports.network {
            ModelReach::Connects
        } else if m.transports.ble {
            ModelReach::BleOnly
        } else {
            ModelReach::None
        }
    }

    /// `reachNote`: one line under the model picker when this app cannot connect to the model.
    pub fn reach_note(m: &ModelProfile) -> Option<&'static str> {
        match model_reach(m) {
            ModelReach::Connects => None,
            ModelReach::BleOnly => {
                Some("Bluetooth LE only: this app can’t connect to it yet. Design and export labels here.")
            }
            ModelReach::None => Some("This app can’t connect to it. Design and export labels here."),
        }
    }

    /// `modelPickerGroups`: reachable models first, then the design-only ones; empty groups are
    /// left out.
    pub fn model_picker_groups(models: &'static [ModelProfile]) -> Vec<(&'static str, Vec<&'static ModelProfile>)> {
        let (reach, design): (Vec<&'static ModelProfile>, Vec<&'static ModelProfile>) = models
            .iter()
            .filter(|m| !tape_widths_of(m).is_empty())
            .partition(|m| model_reach(m) == ModelReach::Connects);
        [("Connects from this computer", reach), ("Design only (this app can’t connect)", design)]
            .into_iter()
            .filter(|(_, ms)| !ms.is_empty())
            .collect()
    }

    pub fn is_verified_model(name: &str) -> bool {
        VERIFIED_MODELS.contains(&name)
    }

    /// `modelOptionText`: "PT-P910BT · up to 36 mm", "PT-P710BT · up to 24 mm · tested".
    pub fn model_option_text(m: &ModelProfile) -> String {
        let max = tape_widths_of(m).last().copied();
        let mut s = m.name.to_owned();
        if let Some(max) = max {
            s.push_str(&format!(" · up to {}", format_mm(max)));
        }
        if is_verified_model(m.name) {
            s.push_str(" · tested");
        }
        s
    }

    /// `connectedModelText`: "PT-P910BT (connected)" / "PT-P710BT (virtual printer)".
    pub fn connected_model_text(model: &str, is_virtual: bool) -> String {
        format!("{model} ({})", if is_virtual { "virtual printer" } else { "connected" })
    }

    /// `untestedModelText`.
    pub fn untested_model_text(model: &str) -> String {
        format!(
            "{model} hasn’t been tested with ptouch studio yet. Printing should work; if anything looks wrong, please report it."
        )
    }

    /// Percent-encodes a query component (RFC 3986 unreserved characters stay).
    fn encode_component(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(b)),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    /// `untestedIssueUrl`: a prefilled "new issue" link with the model and how it was
    /// connected, never the printer's Bluetooth name or address.
    pub fn untested_issue_url(model: &str, connection: Option<&str>) -> String {
        let body = [
            format!("Model: {model}"),
            format!("Connection: {}", connection.filter(|c| !c.is_empty()).unwrap_or("unknown")),
            String::new(),
            "What did you print, and what happened?".into(),
            String::new(),
            String::new(),
            "Please paste “Copy diagnostics” from the Diagnostics page below (device names are masked).".into(),
        ]
        .join("\n");
        format!(
            "{ISSUES_URL}?title={}&body={}",
            encode_component(&format!("Untested model: {model}")),
            encode_component(&body)
        )
    }

    /// `unsupportedWidthText`: "This label is 36 mm; PT-P710BT takes up to 24 mm." /
    /// "…; PT-X doesn’t take 9 mm tape."
    pub fn unsupported_width_text(model: &str, width_mm: f64, widths: &[f64]) -> String {
        let max = widths.iter().copied().fold(f64::MIN, f64::max);
        if width_mm > max {
            format!("This label is {}; {model} takes up to {}.", format_mm(width_mm), format_mm(max))
        } else {
            format!("This label is {}; {model} doesn’t take {} tape.", format_mm(width_mm), format_mm(width_mm))
        }
    }

    /// `nearestTapeWidth`: the nearest of `widths` (the narrower one on a tie).
    pub fn nearest_tape_width(mm: f64, widths: &[f64]) -> f64 {
        let mut best = widths.first().copied().unwrap_or(24.0);
        for &w in widths {
            if (w - mm).abs() < (best - mm).abs() {
                best = w;
            }
        }
        best
    }

    /// `standInModel`: the first of `preferred`, then the table, that takes `width_mm` (TZe).
    pub fn stand_in_model(width_mm: f64, preferred: Option<&str>) -> Option<&'static ModelProfile> {
        let takes = |m: &ModelProfile| tape_widths_of(m).iter().any(|w| (w - width_mm).abs() < 0.01);
        find_model(preferred)
            .filter(|m| takes(m))
            .or_else(|| ptouch::profiles().iter().find(|m| takes(m)))
    }

    /// The doc's tape width is not one the active model takes (`WidthUnsupported`).
    #[derive(Debug, Clone, PartialEq)]
    pub struct WidthUnsupported {
        /// Active model.
        pub model: &'static str,
        pub width_mm: f64,
        /// Widest width the model takes.
        pub max_mm: f64,
        /// What "Fit to …" re-tapes to.
        pub nearest: f64,
        /// A model that takes the width (the doc's own `tape.model` first).
        pub stand_in: Option<&'static str>,
        /// "This label is 36 mm; PT-P710BT takes up to 24 mm."
        pub message: String,
    }

    /// `widthSupport`: `None` when the width is fine (or the model's widths are unknown).
    pub fn width_support(model: &'static ModelProfile, width_mm: f64, doc_model: Option<&str>) -> Option<WidthUnsupported> {
        let widths = tape_widths_of(model);
        if widths.is_empty() || widths.iter().any(|w| (w - width_mm).abs() < 0.01) {
            return None;
        }
        Some(WidthUnsupported {
            model: model.name,
            width_mm,
            max_mm: widths.last().copied().unwrap_or(width_mm),
            nearest: nearest_tape_width(width_mm, &widths),
            stand_in: stand_in_model(width_mm, doc_model).map(|m| m.name),
            message: unsupported_width_text(model.name, width_mm, &widths),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn option_text_and_notes() {
            let p710 = ptouch::profile_by_name("PT-P710BT").expect("PT-P710BT in table");
            assert_eq!(model_option_text(p710), "PT-P710BT · up to 24 mm · tested");
            assert_eq!(reach_note(p710), None);
            assert_eq!(connected_model_text("PT-P910BT", false), "PT-P910BT (connected)");
            assert_eq!(connected_model_text("PT-P710BT", true), "PT-P710BT (virtual printer)");
            let groups = model_picker_groups(ptouch::profiles());
            assert!(!groups.is_empty());
            assert_eq!(groups[0].0, "Connects from this computer");
        }

        #[test]
        fn widths() {
            let p710 = ptouch::profile_by_name("PT-P710BT").expect("PT-P710BT in table");
            assert_eq!(width_support(p710, 24.0, None), None);
            let u = width_support(p710, 36.0, None).expect("36 mm is unsupported");
            assert_eq!(u.message, "This label is 36 mm; PT-P710BT takes up to 24 mm.");
            assert_eq!(u.nearest, 24.0);
            assert_eq!(nearest_tape_width(15.0, &[12.0, 18.0]), 12.0);
            assert_eq!(
                unsupported_width_text("PT-X", 9.0, &[6.0, 12.0]),
                "This label is 9 mm; PT-X doesn’t take 9 mm tape."
            );
        }

        #[test]
        fn issue_url_is_encoded() {
            let url = untested_issue_url("PT-P910BT", Some("USB"));
            assert!(url.starts_with("https://github.com/fjanicki/ptouch/issues/new?title=Untested+model%3A+PT-P910BT&body=Model%3A+PT-P910BT%0AConnection%3A+USB"));
            assert!(!url.contains(' '));
        }
    }
}

/// Parses `#rrggbb` / `#rgb` into a colour (`None` for anything else).
pub fn hex_color(s: &str) -> Option<Hsla> {
    let h = s.strip_prefix('#')?;
    let v = match h.len() {
        6 => u32::from_str_radix(h, 16).ok()?,
        3 => {
            let n = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((n >> 8) & 0xf, (n >> 4) & 0xf, n & 0xf);
            (r * 17) << 16 | (g * 17) << 8 | (b * 17)
        }
        _ => return None,
    };
    Some(rgb(v).into())
}

/// Preview colours (`previewColors`): the printer's colours win when its tape matches the
/// design and both colours are hex and different.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewColors {
    pub tape: String,
    pub ink: String,
    /// "Black on white", when the colours come from the printer.
    pub from_printer: Option<String>,
}

pub fn preview_colors(s: &Studio) -> PreviewColors {
    let doc_w = s.doc.tape.width_mm;
    if is_connected(s.conn)
        && let Some(info) = &s.printer_info
        && let Some(t) = info.tape
        && (f64::from(t.width_mm_x10) / 10.0 - doc_w).abs() < 0.01
    {
        let (tape, ink) = (info.tape_color(), info.text_color());
        if hex_color(tape.css()).is_some() && hex_color(ink.css()).is_some() && tape.css() != ink.css() {
            return PreviewColors {
                tape: tape.css().to_owned(),
                ink: ink.css().to_owned(),
                from_printer: Some(capitalize(&format!("{} on {}", ink.name(), tape.name()))),
            };
        }
    }
    let (tape, ink) = s
        .doc
        .tape
        .colors
        .as_ref()
        .map_or((DEFAULT_COLORS.0.to_owned(), DEFAULT_COLORS.1.to_owned()), |c| (c.tape.clone(), c.ink.clone()));
    PreviewColors { tape, ink, from_printer: None }
}

/// The connected printer's link, when this session opened one.
fn current_link(cx: &App) -> Option<baradar_printer::OpenedLink> {
    cx.try_global::<PrinterLink>().and_then(|l| l.link.clone())
}

/// Whether the open link is the in-process virtual printer.
pub fn link_is_virtual(cx: &App) -> bool {
    current_link(cx).is_some_and(|l| l.transport.kind == TransportKind::Virtual || l.link == LinkKind::Virtual)
}

/// "USB", "Bluetooth", "Serial port", "Virtual printer" (`transportKindLabel`).
pub fn transport_kind_label(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Rfcomm => "Bluetooth",
        TransportKind::Serial => "Serial port",
        TransportKind::Usb => "USB",
        TransportKind::Tcp => "Network",
        TransportKind::Virtual => "Virtual printer",
        _ => "Printer",
    }
}

impl Studio {
    /// `untestedModel`: a connected real (not virtual) printer that is not hardware-verified
    /// and whose notice was not dismissed.
    pub fn untested_model(&self, is_virtual: bool) -> Option<&'static str> {
        let m = self.printer_model()?;
        (!is_virtual && !is_verified_model(m.name) && !self.prefs.dismissed_untested.iter().any(|d| d == m.name))
            .then_some(m.name)
    }

    /// `dismissUntestedModel`: remembered per model, up to 40.
    pub fn dismiss_untested_model(&mut self, model: &str, cx: &mut Context<Self>) {
        if !self.prefs.dismissed_untested.iter().any(|d| d == model) {
            self.prefs.dismissed_untested.push(model.to_owned());
            let n = self.prefs.dismissed_untested.len();
            if n > 40 {
                self.prefs.dismissed_untested.drain(..n - 40);
            }
            let _ = self.store.save_prefs(&self.prefs);
        }
        cx.notify();
    }

    /// `fitToModel`: re-tapes to the nearest width the model takes.
    pub fn fit_to_model(&mut self, cx: &mut Context<Self>) {
        let widths = tape_widths_of(self.model());
        if widths.is_empty() {
            return;
        }
        let w = nearest_tape_width(self.doc.tape.width_mm, &widths);
        self.set_tape_width(w, cx);
    }
}

// ------------------------------------------------------------------------------------------
// Model picker
// ------------------------------------------------------------------------------------------

/// One option of the model picker.
#[derive(Clone)]
pub struct ModelOption {
    name: &'static str,
    title: SharedString,
}

impl SelectItem for ModelOption {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.name
    }
}

type ModelList = SearchableVec<SelectGroup<ModelOption>>;

/// "Printer model": the model designed for while no printer is connected
/// (port of `media/ModelPicker.svelte`).
pub struct ModelPicker {
    studio: Entity<Studio>,
    select: Entity<SelectState<ModelList>>,
    shown: String,
    _subs: Vec<Subscription>,
}

impl ModelPicker {
    pub fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let groups: Vec<SelectGroup<ModelOption>> = model_picker_groups(ptouch::profiles())
            .into_iter()
            .map(|(label, models)| {
                SelectGroup::new(label).items(models.into_iter().map(|m| ModelOption {
                    name: m.name,
                    title: model_option_text(m).into(),
                }))
            })
            .collect();
        let current = studio.read(cx).prefs.design_model.clone();
        let canonical = find_model(Some(&current)).map_or("", |m| m.name);
        let select = cx.new(|cx| {
            let mut s = SelectState::new(SearchableVec::new(groups), None::<IndexPath>, window, cx).searchable(true);
            s.set_selected_value(&canonical, window, cx);
            s
        });
        let subs = vec![
            cx.subscribe_in(&select, window, |this, _, ev: &SelectEvent<ModelList>, _, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                let name: &'static str = name;
                this.shown = name.to_owned();
                this.studio.update(cx, |s, cx| s.set_design_model(name, cx));
            }),
            cx.observe_in(&studio, window, |this, studio, window, cx| {
                let want = studio.read(cx).prefs.design_model.clone();
                if want != this.shown {
                    this.shown = want.clone();
                    let canonical = find_model(Some(&want)).map_or("", |m| m.name);
                    this.select.update(cx, |s, cx| s.set_selected_value(&canonical, window, cx));
                }
                cx.notify();
            }),
        ];
        Self {
            studio,
            select,
            shown: current,
            _subs: subs,
        }
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let note = find_model(Some(&self.studio.read(cx).prefs.design_model)).and_then(reach_note);
        v_flex()
            .gap_1()
            .min_w(px(220.))
            .child(field_label("Printer model", &theme))
            .child(Select::new(&self.select).small().menu_width(px(300.)))
            .when_some(note, |d, n| d.child(div().text_xs().text_color(theme.muted_foreground).child(n)))
    }
}

fn field_label(text: impl Into<SharedString>, theme: &gpui_kit::component::Theme) -> Div {
    div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme.muted_foreground).child(text.into())
}

// ------------------------------------------------------------------------------------------
// Media bar
// ------------------------------------------------------------------------------------------

/// A colour preset option (`TAPE_PRESETS`, plus "Custom" when the doc's colours match none).
#[derive(Clone)]
struct ColourOption {
    id: &'static str,
    label: SharedString,
}

impl SelectItem for ColourOption {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// Which number field an input belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumField {
    Length,
    MarginStart,
    MarginEnd,
}

pub struct MediaBar {
    studio: Entity<Studio>,
    picker: Entity<ModelPicker>,
    colours: Entity<SelectState<Vec<ColourOption>>>,
    colours_custom: bool,
    length: Entity<InputState>,
    margin_start: Entity<InputState>,
    margin_end: Entity<InputState>,
    /// Last values written into the inputs (to reload them when the doc changes elsewhere).
    shown: (Option<f64>, f64, f64),
    _subs: Vec<Subscription>,
}

/// The preset id matching the doc's colours, or "custom" (`presetId`).
fn preset_id(colors: Option<&TapeColors>) -> &'static str {
    let (tape, ink) = colors.map_or(DEFAULT_COLORS, |c| (c.tape.as_str(), c.ink.as_str()));
    TAPE_PRESETS
        .iter()
        .find(|p| p.tape.eq_ignore_ascii_case(tape) && p.ink.eq_ignore_ascii_case(ink))
        .map_or("custom", |p| p.id)
}

fn colour_options(custom: bool) -> Vec<ColourOption> {
    let mut v: Vec<ColourOption> = Vec::with_capacity(TAPE_PRESETS.len() + 1);
    if custom {
        v.push(ColourOption { id: "custom", label: "Custom".into() });
    }
    v.extend(TAPE_PRESETS.iter().map(|p| ColourOption { id: p.id, label: p.label.into() }));
    v
}

fn num_text(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

impl MediaBar {
    pub fn new(studio: Entity<Studio>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| ModelPicker::new(studio.clone(), window, cx));
        let (preset, length_mm, margins) = {
            let s = studio.read(cx);
            let len = match s.doc.length {
                LengthSettings::Fixed { mm, .. } => Some(mm),
                LengthSettings::Auto => None,
            };
            (preset_id(s.doc.tape.colors.as_ref()), len, s.doc.margins_mm)
        };
        let custom = preset == "custom";
        let colours = cx.new(|cx| {
            let mut s = SelectState::new(colour_options(custom), None::<IndexPath>, window, cx);
            s.set_selected_value(&preset, window, cx);
            s
        });
        let length = cx.new(|cx| {
            InputState::new(window, cx)
                .step(1.0)
                .min(5.0)
                .default_value(num_text(length_mm.unwrap_or(50.0)))
        });
        let margin = |v: f64, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).step(0.5).min(0.0).max(50.0).default_value(num_text(v)))
        };
        let margin_start = margin(margins.start, window, cx);
        let margin_end = margin(margins.end, window, cx);

        let mut subs = vec![
            cx.observe_in(&studio, window, |this, _, window, cx| {
                this.sync_from_doc(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&colours, window, |this, _, ev: &SelectEvent<Vec<ColourOption>>, _, cx| {
                let SelectEvent::Confirm(Some(id)) = ev else { return };
                let Some(p) = TAPE_PRESETS.iter().find(|p| p.id == *id) else { return };
                this.studio.update(cx, |s, cx| {
                    s.update_doc(None, |d| d.tape.colors = Some(TapeColors { tape: p.tape.into(), ink: p.ink.into() }), cx)
                });
            }),
            // Keep the "virtual printer" flag and the connected model line current.
            crate::connect::track_printer_link(&studio, cx),
        ];
        for (input, field) in [
            (length.clone(), NumField::Length),
            (margin_start.clone(), NumField::MarginStart),
            (margin_end.clone(), NumField::MarginEnd),
        ] {
            subs.push(cx.subscribe_in(&input, window, move |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => this.apply_number(field, state, false, window, cx),
                InputEvent::Blur | InputEvent::PressEnter { .. } => this.apply_number(field, state, true, window, cx),
                InputEvent::Focus => {}
            }));
        }
        Self {
            studio,
            picker,
            colours,
            colours_custom: custom,
            length,
            margin_start,
            margin_end,
            shown: (length_mm, margins.start, margins.end),
            _subs: subs,
        }
    }

    /// Longest fixed length the print area allows, in whole mm (`maxLen`).
    fn max_length(s: &Studio) -> f64 {
        s.target()
            .ok()
            .and_then(|t| {
                let a = t.area();
                a.max_length_dots.map(|d| (d as f64 * 25.4 / f64::from(a.dpi)).floor())
            })
            .unwrap_or(999.0)
    }

    /// Range and coalesce key of a number field.
    fn field_spec(field: NumField, s: &Studio) -> (f64, f64, &'static str) {
        match field {
            NumField::Length => (5.0, Self::max_length(s), "doc:length"),
            NumField::MarginStart => (0.0, 50.0, "doc:margin-start"),
            NumField::MarginEnd => (0.0, 50.0, "doc:margin-end"),
        }
    }

    /// Applies a typed number. While typing only in-range values are applied; on blur or
    /// Enter the value is clamped and written back (`NumberField`).
    fn apply_number(&mut self, field: NumField, state: &Entity<InputState>, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = state.read(cx).value().to_string();
        let parsed = text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite());
        let (min, max, key) = Self::field_spec(field, self.studio.read(cx));
        let value = match parsed {
            Some(v) if (min..=max).contains(&v) => v,
            Some(v) if commit => v.clamp(min, max),
            _ if commit => {
                // Unparseable: put the stored value back.
                self.shown = (None, f64::NAN, f64::NAN);
                self.sync_from_doc(window, cx);
                return;
            }
            _ => return,
        };
        let value = if field == NumField::Length { value.round() } else { (value * 2.0).round() / 2.0 };
        if commit && num_text(value) != text.trim() {
            state.update(cx, |s, cx| s.set_value(num_text(value), window, cx));
        }
        match field {
            NumField::Length => self.shown.0 = Some(value),
            NumField::MarginStart => self.shown.1 = value,
            NumField::MarginEnd => self.shown.2 = value,
        }
        self.studio.update(cx, |s, cx| {
            let current = s.doc.clone();
            match field {
                NumField::Length => {
                    let shrink = matches!(current.length, LengthSettings::Fixed { shrink: true, .. });
                    if current.length != (LengthSettings::Fixed { mm: value, shrink }) {
                        s.update_doc(Some(key), |d| d.length = LengthSettings::Fixed { mm: value, shrink }, cx);
                    }
                }
                NumField::MarginStart if current.margins_mm.start != value => {
                    s.update_doc(Some(key), |d| d.margins_mm = Margins { start: value, ..d.margins_mm }, cx);
                }
                NumField::MarginEnd if current.margins_mm.end != value => {
                    s.update_doc(Some(key), |d| d.margins_mm = Margins { end: value, ..d.margins_mm }, cx);
                }
                _ => {}
            }
        });
    }

    /// Reloads inputs and the colour select when the doc changed elsewhere (undo, open…).
    fn sync_from_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (len, margins, preset, max_len) = {
            let s = self.studio.read(cx);
            let len = match s.doc.length {
                LengthSettings::Fixed { mm, .. } => Some(mm),
                LengthSettings::Auto => None,
            };
            (len, s.doc.margins_mm, preset_id(s.doc.tape.colors.as_ref()), Self::max_length(s))
        };
        if let Some(mm) = len
            && self.shown.0 != Some(mm)
        {
            self.length.update(cx, |s, cx| s.set_value(num_text(mm), window, cx));
        }
        self.length.update(cx, |s, cx| s.set_max(Some(max_len), window, cx));
        if self.shown.1 != margins.start {
            self.margin_start.update(cx, |s, cx| s.set_value(num_text(margins.start), window, cx));
        }
        if self.shown.2 != margins.end {
            self.margin_end.update(cx, |s, cx| s.set_value(num_text(margins.end), window, cx));
        }
        self.shown = (len, margins.start, margins.end);
        let custom = preset == "custom";
        let selected = self.colours.read(cx).selected_value().copied();
        if custom != self.colours_custom || selected != Some(preset) {
            self.colours_custom = custom;
            self.colours.update(cx, |s, cx| {
                s.set_items(colour_options(custom), window, cx);
                s.set_selected_value(&preset, window, cx);
            });
        }
    }

    fn tape_widths_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let current = s.doc.tape.width_mm;
        let loaded = s
            .printer_info
            .as_ref()
            .filter(|_| is_connected(s.conn))
            .and_then(|i| i.tape)
            .map(|t| f64::from(t.width_mm_x10) / 10.0);
        let widths = s.tape_widths();
        v_flex()
            .gap_1()
            .child(field_label("Tape width (mm)", &theme))
            .child(h_flex().gap_1().flex_wrap().children(widths.into_iter().map(|w| {
                let is_loaded = loaded.is_some_and(|l| (l - w).abs() < 0.01);
                let text = if is_loaded { format!("{} •", num_text(w)) } else { num_text(w) };
                Button::new(SharedString::from(format!("tape-{w}")))
                    .label(text)
                    .small()
                    .map(|b| if (w - current).abs() < 0.01 { b.primary() } else { b.outline() })
                    .on_click(cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.set_tape_width(w, cx))))
            })))
            .when_some(loaded, |d, l| {
                d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("• loaded in the printer: {}", format_mm(l))))
            })
            .into_any_element()
    }

    fn model_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        match s.printer_model() {
            None => div().child(self.picker.clone()).into_any_element(),
            Some(m) => v_flex()
                .gap_1()
                .min_w(px(200.))
                .child(field_label("Designing for", &theme))
                .child(
                    div()
                        .min_h(px(28.))
                        .flex()
                        .items_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(connected_model_text(m.name, link_is_virtual(cx))),
                )
                .into_any_element(),
        }
    }

    fn colours_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let colors = preview_colors(self.studio.read(cx));
        let swatch = div()
            .w(px(28.))
            .h(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .bg(hex_color(&colors.tape).unwrap_or(theme.background))
            .text_color(hex_color(&colors.ink).unwrap_or(theme.foreground))
            .child("Aa");
        let body = match colors.from_printer {
            Some(name) => h_flex()
                .id("colours-from-printer")
                .gap_2()
                .min_h(px(28.))
                .child(swatch)
                .child(div().text_sm().child(name))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Reported by the printer").build(window, cx))
                .into_any_element(),
            None => h_flex()
                .gap_2()
                .child(swatch)
                .child(div().min_w(px(150.)).child(Select::new(&self.colours).small()))
                .into_any_element(),
        };
        v_flex().gap_1().child(field_label("Colours", &theme)).child(body).into_any_element()
    }

    fn length_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let fixed = match s.doc.length {
            LengthSettings::Fixed { mm, .. } => Some(mm),
            LengthSettings::Auto => None,
        };
        let feed = s.render.as_ref().map(|r| r.feed_margin_mm);
        let auto_len = s.render.as_ref().map(|r| r.length_mm);
        v_flex()
            .gap_1()
            .child(field_label("Length", &theme))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("length-auto")
                            .label("Auto")
                            .small()
                            .map(|b| if fixed.is_none() { b.primary() } else { b.outline() })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.studio.update(cx, |s, cx| {
                                    if s.doc.length != LengthSettings::Auto {
                                        s.set_length(LengthSettings::Auto, cx);
                                    }
                                })
                            })),
                    )
                    .child(
                        Button::new("length-fixed")
                            .label("Fixed")
                            .small()
                            .map(|b| if fixed.is_some() { b.primary() } else { b.outline() })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if fixed.is_some() {
                                    return;
                                }
                                let mm = auto_len.unwrap_or(50.0).round().max(10.0);
                                this.shown.0 = Some(mm);
                                this.length.update(cx, |s, cx| s.set_value(num_text(mm), window, cx));
                                this.studio.update(cx, |s, cx| s.set_length(LengthSettings::Fixed { mm, shrink: false }, cx));
                            })),
                    ),
            )
            .when_some(fixed, |d, mm| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(field_label("Printed length", &theme))
                        .child(div().w(px(130.)).child(NumberInput::new(&self.length).small().suffix(div().text_xs().child("mm")))),
                )
                .when_some(feed, |d, feed| {
                    // The printer feeds ~2 mm before and after the printed part: say so, so
                    // "50 mm" isn't a surprise when the cut label measures 54 mm.
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("Cut label ≈ {} with the printer’s feed", format_mm(mm + 2.0 * feed))),
                    )
                })
            })
            .into_any_element()
    }

    fn margins_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let field = |label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(field_label(label, &theme))
                .child(div().w(px(110.)).child(NumberInput::new(state).small().suffix(div().text_xs().child("mm"))))
        };
        h_flex()
            .gap_2()
            .items_start()
            .child(field("Start margin", &self.margin_start))
            .child(field("End margin", &self.margin_end))
            .into_any_element()
    }

    /// `UnsupportedWidthNotice`.
    fn unsupported_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let s = self.studio.read(cx);
        let support = width_support(s.model(), s.doc.tape.width_mm, s.doc.tape.model.as_deref())?;
        // The model the label was made for (tape.model), when it takes the width and only the
        // "Design for" pick stands in the way (a connected printer always wins).
        let made_for = if s.printer_model().is_none() {
            find_model(s.doc.tape.model.as_deref())
                .filter(|m| Some(m.name) == support.stand_in && m.name != support.model)
                .map(|m| m.name)
        } else {
            None
        };
        Some(
            h_flex()
                .gap_3()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.warning)
                .bg(theme.warning.opacity(0.08))
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(support.message.clone()))
                        .child(format!(
                            "The label keeps its width until you change it; it can’t be printed on the {}.",
                            support.model
                        )),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .flex_none()
                        .when_some(made_for, |d, m| {
                            d.child(Button::new("design-for").label(format!("Design for {m}")).small().outline().on_click(
                                cx.listener(move |this, _, _, cx| this.studio.update(cx, |s, cx| s.set_design_model(m, cx))),
                            ))
                        })
                        .child(
                            Button::new("fit-to-model")
                                .label(format!("Fit to {}", format_mm(support.nearest)))
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.studio.update(cx, |s, cx| s.fit_to_model(cx)))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// `UntestedModelNotice`.
    fn untested_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let is_virtual = link_is_virtual(cx);
        let model = self.studio.read(cx).untested_model(is_virtual)?;
        let connection = current_link(cx).map(|l| transport_kind_label(l.transport.kind));
        let url = untested_issue_url(model, connection);
        Some(
            h_flex()
                .gap_3()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.info)
                .child(div().flex_1().text_sm().child(untested_model_text(model)))
                .child(
                    h_flex()
                        .gap_1()
                        .flex_none()
                        .child(Button::new("untested-diagnostics").icon(crate::icons::icon("activity")).label("Open Diagnostics").small().outline().on_click(
                            cx.listener(|this, _, window, cx| crate::diagnostics::open_diagnostics(this.studio.clone(), window, cx)),
                        ))
                        .child(
                            Button::new("untested-report")
                                .icon(crate::icons::icon("send")).label("Report an issue")
                                .small()
                                .outline()
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                        .child(Button::new("untested-dismiss").icon(crate::icons::icon("x")).label("Dismiss").small().ghost().on_click(cx.listener(
                            move |this, _, _, cx| this.studio.update(cx, |s, cx| s.dismiss_untested_model(model, cx)),
                        ))),
                )
                .into_any_element(),
        )
    }
}

impl Render for MediaBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let widths = self.tape_widths_row(cx);
        let model = self.model_group(cx);
        let colours = self.colours_group(cx);
        let length = self.length_group(cx);
        let margins = self.margins_group(cx);
        let unsupported = self.unsupported_notice(cx);
        let untested = self.untested_notice(cx);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .gap_x_5()
                    .gap_y_3()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(div().max_w(px(340.)).child(widths))
                    .child(model)
                    .child(colours)
                    .child(length)
                    .child(margins),
            )
            .children(unsupported)
            .children(untested)
    }
}
