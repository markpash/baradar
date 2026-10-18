//! Controls shared by the property editors, ports of the studio's `ui/common/*.svelte`:
//! `NumberField` (commits valid values while typing, clamps on blur), `Slider` (label, readout,
//! hint), `Switch` (label + hint), `Segmented`, a `<select>` stand-in, and text inputs bound to a
//! field of the document.
//!
//! Bound inputs keep their own text while focused. [`Fields::sync`] reloads them from the studio:
//! unfocused ones on every studio change (another view may edit the same value), all of them when
//! the document was replaced (`Studio::epoch`), so typing never makes the cursor jump.

use std::rc::Rc;

use baradar_core::doc::{Item, ItemSize, LabelDoc};
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup},
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::studio::Studio;

/// Reads a number from the studio (`None`: the field does not apply right now).
pub type NumGet = Rc<dyn Fn(&Studio) -> Option<f64>>;
/// Writes a number to the studio.
pub type NumSet = Rc<dyn Fn(&mut Studio, f64, &mut Context<Studio>)>;
/// Reads a string from the studio.
pub type StrGet = Rc<dyn Fn(&Studio) -> Option<String>>;
/// Writes a string to the studio.
pub type StrSet = Rc<dyn Fn(&mut Studio, String, &mut Context<Studio>)>;

/// `${item.id}:<field>`: the coalesce key of a continuous item edit.
pub fn item_key(id: &str, field: &str) -> String {
    format!("{id}:{field}")
}

/// A number setter that edits item `id` with coalesce key `${id}:<field>`.
pub fn item_num_set(id: &str, field: &str, edit: impl Fn(&mut Item, f64) + 'static) -> NumSet {
    let (id, key) = (id.to_owned(), item_key(id, field));
    let edit = Rc::new(edit);
    Rc::new(move |s: &mut Studio, v: f64, cx: &mut Context<Studio>| {
        let edit = edit.clone();
        s.update_item(&id, Some(&key), move |i| edit(i, v), cx);
    })
}

/// A number getter reading item `id`.
pub fn item_num_get(id: &str, read: impl Fn(&Item) -> Option<f64> + 'static) -> NumGet {
    let id = id.to_owned();
    Rc::new(move |s: &Studio| s.doc.item(&id).and_then(&read))
}

/// A string setter that edits item `id` with coalesce key `${id}:<field>`.
pub fn item_str_set(id: &str, field: &str, edit: impl Fn(&mut Item, String) + 'static) -> StrSet {
    let (id, key) = (id.to_owned(), item_key(id, field));
    let edit = Rc::new(edit);
    Rc::new(move |s: &mut Studio, v: String, cx: &mut Context<Studio>| {
        let edit = edit.clone();
        s.update_item(&id, Some(&key), move |i| edit(i, v), cx);
    })
}

/// A string getter reading item `id`.
pub fn item_str_get(id: &str, read: impl Fn(&Item) -> Option<String> + 'static) -> StrGet {
    let id = id.to_owned();
    Rc::new(move |s: &Studio| s.doc.item(&id).and_then(&read))
}

/// A number setter that edits the document with coalesce key `key` (`'doc:gap'`…).
pub fn doc_num_set(key: &'static str, edit: impl Fn(&mut LabelDoc, f64) + 'static) -> NumSet {
    let edit = Rc::new(edit);
    Rc::new(move |s: &mut Studio, v: f64, cx: &mut Context<Studio>| {
        let edit = edit.clone();
        s.update_doc(Some(key), move |d| edit(d, v), cx);
    })
}

/// `Math.round(v * 100) / 100` as the input shows it ("12", "0.5", "1.25").
pub fn fmt_num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// `Number(draft.replace(',', '.'))` for a non-empty draft.
pub fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Range and look of a number field (`NumberField` props).
#[derive(Debug, Clone, Copy)]
pub struct NumSpec {
    pub label: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl NumSpec {
    pub const fn new(label: &'static str, unit: &'static str, min: f64, max: f64, step: f64) -> Self {
        Self { label, unit, min, max, step }
    }
    fn clamp(&self, v: f64) -> f64 {
        v.max(self.min).min(self.max)
    }
}

/// A labelled number input bound to a value (`NumberField.svelte`).
pub struct NumField {
    pub state: Entity<InputState>,
    pub spec: Rc<std::cell::Cell<NumSpec>>,
    get: NumGet,
}

impl NumField {
    /// Changes the range (e.g. a maximum that follows the tape width).
    pub fn set_range(&self, min: f64, max: f64) {
        let mut s = self.spec.get();
        s.min = min;
        s.max = max;
        self.spec.set(s);
    }
}

/// The input of a bound text field: one line or a textarea.
#[derive(Clone)]
pub enum StrState {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

impl StrState {
    fn value(&self, cx: &App) -> SharedString {
        match self {
            Self::Line(s) => s.read(cx).value(),
            Self::Area(s) => s.read(cx).value(),
        }
    }
    fn focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            Self::Line(s) => s.read(cx).focus_handle(cx).is_focused(window),
            Self::Area(s) => s.read(cx).focus_handle(cx).is_focused(window),
        }
    }
    fn set(&self, v: String, window: &mut Window, cx: &mut App) {
        match self {
            Self::Line(s) => s.update(cx, |s, cx| s.set_value(v, window, cx)),
            Self::Area(s) => s.update(cx, |s, cx| s.set_value(v, window, cx)),
        }
    }
}

/// A text input bound to a string of the document.
pub struct StrField {
    pub state: StrState,
    get: StrGet,
}

impl StrField {
    pub fn element(&self) -> AnyElement {
        match &self.state {
            StrState::Line(s) => Input::new(s).into_any_element(),
            StrState::Area(s) => Textarea::new(s).h(px(78.)).into_any_element(),
        }
    }
}

/// A slider bound to a number of the document.
pub struct SliderField {
    pub state: Entity<SliderState>,
    get: NumGet,
}

/// The bound inputs of one editor, by key.
#[derive(Default)]
pub struct Fields {
    nums: Vec<(&'static str, NumField)>,
    strs: Vec<(&'static str, StrField)>,
    sliders: Vec<(&'static str, SliderField)>,
    subs: Vec<Subscription>,
}

impl Fields {
    pub fn num(&self, key: &str) -> Option<&NumField> {
        self.nums.iter().find(|(k, _)| *k == key).map(|(_, f)| f)
    }
    pub fn str(&self, key: &str) -> Option<&StrField> {
        self.strs.iter().find(|(k, _)| *k == key).map(|(_, f)| f)
    }
    pub fn slider(&self, key: &str) -> Option<&SliderField> {
        self.sliders.iter().find(|(k, _)| *k == key).map(|(_, f)| f)
    }

    /// Adds a number field. Typing commits values inside the range; Enter, blur and the stepper
    /// buttons clamp (`NumberField.svelte` `oninput` / `onblur`).
    #[allow(clippy::too_many_arguments)]
    pub fn add_num<V: 'static>(
        &mut self,
        key: &'static str,
        spec: NumSpec,
        studio: &Entity<Studio>,
        get: NumGet,
        set: NumSet,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        let initial = get(studio.read(cx)).map(fmt_num).unwrap_or_default();
        let state = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let spec = Rc::new(std::cell::Cell::new(spec));
        let commit = {
            let (studio, get, set, spec) = (studio.clone(), get.clone(), set.clone(), spec.clone());
            move |state: &Entity<InputState>, clamp: bool, step: Option<StepAction>, window: &mut Window, cx: &mut App| {
                let sp = spec.get();
                let text = state.read(cx).value();
                let current = get(studio.read(cx));
                let typed = parse_num(&text);
                let v = match step {
                    Some(a) => {
                        let base = typed.or(current).unwrap_or(sp.min);
                        let n = match a {
                            StepAction::Increment => base + sp.step,
                            StepAction::Decrement => base - sp.step,
                        };
                        Some(sp.clamp((n / sp.step).round() * sp.step))
                    }
                    None if clamp => typed.map(|v| sp.clamp(v)),
                    None => typed.filter(|v| sp.clamp(*v) == *v),
                };
                if let Some(v) = v
                    && current.is_none_or(|c| (c - v).abs() > 1e-9)
                {
                    let set = set.clone();
                    studio.update(cx, |s, cx| set(s, v, cx));
                }
                if clamp || step.is_some() {
                    let shown = get(studio.read(cx)).map(fmt_num).unwrap_or_default();
                    if shown != text.as_ref() {
                        state.update(cx, |s, cx| s.set_value(shown, window, cx));
                    }
                }
            }
        };
        let c1 = commit.clone();
        self.subs.push(cx.subscribe_in(&state, window, move |_, state, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => c1(state, false, None, window, cx),
            InputEvent::Blur | InputEvent::PressEnter { .. } => c1(state, true, None, window, cx),
            _ => {}
        }));
        self.subs.push(cx.subscribe_in(&state, window, move |_, state, ev: &NumberInputEvent, window, cx| {
            let NumberInputEvent::Step(a) = ev;
            commit(state, false, Some(*a), window, cx);
        }));
        self.nums.push((key, NumField { state, spec, get }));
    }

    /// Adds a text field (one line, or a textarea when `multi_line`) committing every change.
    #[allow(clippy::too_many_arguments)]
    pub fn add_str<V: 'static>(
        &mut self,
        key: &'static str,
        multi_line: bool,
        placeholder: &'static str,
        studio: &Entity<Studio>,
        get: StrGet,
        set: StrSet,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        let initial = get(studio.read(cx)).unwrap_or_default();
        let state = if multi_line {
            StrState::Area(cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder).default_value(initial)))
        } else {
            StrState::Line(cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(initial)))
        };
        let on_event = {
            let (studio, state, get) = (studio.clone(), state.clone(), get.clone());
            move |ev: &InputEvent, cx: &mut App| {
                if !matches!(ev, InputEvent::Change) {
                    return;
                }
                let v = state.value(cx).to_string();
                if get(studio.read(cx)).as_deref() == Some(v.as_str()) {
                    return;
                }
                let set = set.clone();
                studio.update(cx, |s, cx| set(s, v, cx));
            }
        };
        match &state {
            StrState::Line(s) => self.subs.push(cx.subscribe(s, move |_, _, ev: &InputEvent, cx| on_event(ev, cx))),
            StrState::Area(s) => self.subs.push(cx.subscribe(s, move |_, _, ev: &InputEvent, cx| on_event(ev, cx))),
        }
        self.strs.push((key, StrField { state, get }));
    }

    /// Adds a slider; moving it commits (coalesced by the setter's key).
    #[allow(clippy::too_many_arguments)]
    pub fn add_slider<V: 'static>(
        &mut self,
        key: &'static str,
        (min, max, step): (f64, f64, f64),
        studio: &Entity<Studio>,
        get: NumGet,
        set: NumSet,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        let initial = get(studio.read(cx)).unwrap_or(min);
        let state = cx.new(|_| {
            SliderState::new()
                .min(min as f32)
                .max(max as f32)
                .step(step as f32)
                .default_value(initial as f32)
        });
        let (studio_c, get_c) = (studio.clone(), get.clone());
        self.subs.push(cx.subscribe_in(&state, window, move |_, _, ev: &SliderEvent, _, cx| {
            let SliderEvent::Change(v) = ev else { return };
            let v = f64::from(v.start());
            if get_c(studio_c.read(cx)).is_some_and(|c| (c - v).abs() < 1e-6) {
                return;
            }
            let set = set.clone();
            studio_c.update(cx, |s, cx| set(s, v, cx));
        }));
        self.sliders.push((key, SliderField { state, get }));
    }

    /// Reloads inputs from the studio: all of them when `force`, else only unfocused ones.
    pub fn sync(&self, studio: &Entity<Studio>, force: bool, window: &mut Window, cx: &mut App) {
        let (nums, strs, sliders) = {
            let s = studio.read(cx);
            let nums: Vec<Option<f64>> = self.nums.iter().map(|(_, f)| (f.get)(s)).collect();
            let strs: Vec<Option<String>> = self.strs.iter().map(|(_, f)| (f.get)(s)).collect();
            let sliders: Vec<Option<f64>> = self.sliders.iter().map(|(_, f)| (f.get)(s)).collect();
            (nums, strs, sliders)
        };
        for ((_, f), want) in self.nums.iter().zip(nums) {
            let Some(want) = want else { continue };
            if !force && f.state.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let text = f.state.read(cx).value();
            if parse_num(&text).is_none_or(|v| (v - want).abs() > 0.005) {
                f.state.update(cx, |s, cx| s.set_value(fmt_num(want), window, cx));
            }
        }
        for ((_, f), want) in self.strs.iter().zip(strs) {
            let Some(want) = want else { continue };
            if !force && f.state.focused(window, cx) {
                continue;
            }
            if f.state.value(cx).as_ref() != want {
                f.state.set(want, window, cx);
            }
        }
        for ((_, f), want) in self.sliders.iter().zip(sliders) {
            let Some(want) = want else { continue };
            let have = f64::from(f.state.read(cx).value().start());
            if (have - want).abs() > 1e-6 {
                f.state.update(cx, |s, cx| s.set_value(want as f32, window, cx));
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Rendering helpers
// ------------------------------------------------------------------------------------------

/// The small bold caption above a control (`.field-label`).
pub fn field_label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// How a hint reads: plain, a warning or an error (`.hint`, `.hint.warn`, `.hint.error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Warn,
    Error,
}

/// A hint line under a control (`<p class="hint">`).
pub fn hint(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    hint_tone(text, Tone::Plain, cx)
}

pub fn hint_tone(text: impl Into<SharedString>, tone: Tone, cx: &App) -> AnyElement {
    let t = cx.theme();
    let color = match tone {
        Tone::Plain => t.muted_foreground,
        Tone::Warn => t.warning,
        Tone::Error => t.danger,
    };
    div().text_xs().text_color(color).child(text.into()).into_any_element()
}

/// A labelled field: caption, control and an optional hint (`.field`).
pub fn field(label: impl Into<SharedString>, control: impl IntoElement, hint_text: Option<SharedString>, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(field_label(label, cx))
        .child(control)
        .when_some(hint_text, |d, h| d.child(hint(h, cx)))
        .into_any_element()
}

/// A [`NumField`] with its caption, unit and stepper (`NumberField.svelte`).
pub fn num_view(f: Option<&NumField>, hint_text: Option<SharedString>, cx: &App) -> AnyElement {
    let Some(f) = f else { return div().into_any_element() };
    let spec = f.spec.get();
    let input = NumberInput::new(&f.state)
        .small()
        .when(!spec.unit.is_empty(), |n| n.suffix(div().text_xs().text_color(cx.theme().muted_foreground).child(spec.unit)));
    field(spec.label, input, hint_text, cx)
}

/// A slider with its caption, value readout and optional hint (`Slider.svelte`).
pub fn slider_view(label: &'static str, readout: String, f: Option<&SliderField>, hint_text: Option<SharedString>, cx: &App) -> AnyElement {
    let Some(f) = f else { return div().into_any_element() };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(field_label(label, cx))
                .child(div().text_xs().text_color(cx.theme().foreground).child(readout)),
        )
        .child(Slider::new(&f.state))
        .when_some(hint_text, |d, h| d.child(hint(h, cx)))
        .into_any_element()
}

/// A switch with a label and an optional hint (`Switch.svelte`).
pub fn switch_view(
    id: impl Into<ElementId>,
    label: &'static str,
    checked: bool,
    hint_text: Option<&'static str>,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_0p5()
        .child(Switch::new(id).label(label).checked(checked).on_click(move |v: &bool, w, cx| on_change(*v, w, cx)))
        .when_some(hint_text, |d, h| d.child(div().pl(px(36.)).child(hint(h, cx))))
        .into_any_element()
}

/// A segmented control (`Segmented.svelte`): one button per option, the current one pressed.
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: impl Into<ElementId>,
    options: &[(T, &'static str)],
    current: Option<T>,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let values: Vec<T> = options.iter().map(|(v, _)| *v).collect();
    ButtonGroup::new(id)
        .small()
        .outline()
        .children(options.iter().enumerate().map(|(i, (v, label))| {
            Button::new(i).label(*label).selected(Some(*v) == current)
        }))
        .on_click(move |clicked: &Vec<usize>, w, cx| {
            if let Some(v) = clicked.first().and_then(|i| values.get(*i)) {
                on_pick(*v, w, cx);
            }
        })
        .into_any_element()
}

/// A labelled segmented control.
pub fn segmented_field<T: Copy + PartialEq + 'static>(
    id: impl Into<ElementId>,
    label: &'static str,
    options: &[(T, &'static str)],
    current: Option<T>,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    field(label, segmented(id, options, current, on_pick), None, cx)
}

/// A `<select>`: a button showing the current option that opens a checked menu.
pub fn select<T: Copy + PartialEq + 'static>(
    id: impl Into<ElementId>,
    options: Vec<(T, SharedString)>,
    current: Option<T>,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label = options
        .iter()
        .find(|(v, _)| Some(*v) == current)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| "Choose…".into());
    let on_pick = Rc::new(on_pick);
    Button::new(id)
        .label(label.clone())
        .dropdown_caret(true)
        .small()
        .outline()
        .w_full()
        .dropdown_menu(move |menu, _, _| {
            options.iter().fold(menu, |m, (v, l)| {
                let (v, f) = (*v, on_pick.clone());
                m.item(PopupMenuItem::new(l.clone()).checked(Some(v) == current).on_click(move |_, w, cx| f(v, w, cx)))
            })
        })
        .into_any_element()
}

// ------------------------------------------------------------------------------------------
// SizeField (`common/SizeField.svelte`) for icon, image and shape blocks
// ------------------------------------------------------------------------------------------

/// The `size` of an icon, image or shape block.
pub fn item_size(item: &Item) -> Option<ItemSize> {
    match item {
        Item::Icon(i) => Some(i.size),
        Item::Image(i) => Some(i.size),
        Item::Shape(s) => Some(s.size),
        _ => None,
    }
}

/// The `size` of an icon, image or shape block, mutably.
pub fn item_size_mut(item: &mut Item) -> Option<&mut ItemSize> {
    match item {
        Item::Icon(i) => Some(&mut i.size),
        Item::Image(i) => Some(&mut i.size),
        Item::Shape(s) => Some(&mut s.size),
        _ => None,
    }
}

/// Adds the "Height" number field of a SizeField under key `size_mm` (0.5 mm to the tape
/// width, step 0.5).
pub fn add_item_size<V: 'static>(
    fields: &mut Fields,
    id: &str,
    fixed_label: &'static str,
    studio: &Entity<Studio>,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    let max = studio.read(cx).doc.tape.width_mm;
    fields.add_num(
        "size_mm",
        NumSpec::new(fixed_label, "mm", 0.5, max, 0.5),
        studio,
        item_num_get(id, |i| match item_size(i) {
            Some(ItemSize::Mm { mm }) => Some(mm),
            _ => None,
        }),
        item_num_set(id, "size", |i, mm| {
            if let Some(s) = item_size_mut(i) {
                *s = ItemSize::Mm { mm };
            }
        }),
        window,
        cx,
    );
}

/// A SizeField: "Fit tape" / "Fixed", and the height while fixed.
pub fn item_size_view(
    id: &str,
    label: &'static str,
    size: ItemSize,
    max_mm: f64,
    fields: &Fields,
    studio: &Entity<Studio>,
    cx: &App,
) -> AnyElement {
    let field_state = fields.num("size_mm");
    if let Some(f) = field_state {
        f.set_range(0.5, max_mm);
    }
    let last_mm = field_state.and_then(|f| parse_num(&f.state.read(cx).value())).unwrap_or(6.0);
    let (studio, item_id, key) = (studio.clone(), id.to_owned(), item_key(id, "size"));
    let current = match size {
        ItemSize::Fit => "fit",
        ItemSize::Mm { .. } => "mm",
    };
    v_flex()
        .gap_2()
        .child(segmented_field(
            SharedString::from(format!("{id}-size-mode")),
            label,
            &[("fit", "Fit tape"), ("mm", "Fixed")],
            Some(current),
            move |mode, _, cx| {
                let size = if mode == "fit" { ItemSize::Fit } else { ItemSize::Mm { mm: last_mm.min(max_mm) } };
                studio.update(cx, |s, cx| {
                    s.update_item(
                        &item_id,
                        Some(&key),
                        |i| {
                            if let Some(sz) = item_size_mut(i) {
                                *sz = size;
                            }
                        },
                        cx,
                    )
                });
            },
            cx,
        ))
        .when(matches!(size, ItemSize::Mm { .. }), |d| d.child(num_view(field_state, None, cx)))
        .into_any_element()
}

// ------------------------------------------------------------------------------------------
// VariableHint (`batch/VariableHint.svelte`)
// ------------------------------------------------------------------------------------------

/// Lists the `{{variables}}` used in `text` with their kind and flags unknown ones; nothing when
/// the text has no placeholders. (The studio's "Edit data…" button belongs to the batch panel.)
pub fn variable_hint(text: &str, doc: &LabelDoc, cx: &App) -> Option<AnyElement> {
    use baradar_core::vars::variables::{VariableKind, placeholder_names, variable_kind};
    let names = placeholder_names(text);
    if names.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let vars: Vec<(String, VariableKind)> = names.into_iter().map(|n| {
        let k = variable_kind(doc, &n);
        (n, k)
    }).collect();
    let unknown = vars.iter().filter(|(_, k)| *k == VariableKind::Missing).count();
    let chips = h_flex().flex_wrap().gap_1().children(vars.into_iter().map(|(name, kind)| {
        let missing = kind == VariableKind::Missing;
        let kind_text = match kind {
            VariableKind::Column => "column",
            VariableKind::Counter => "counter",
            VariableKind::Date => "date",
            VariableKind::Ssid => "network name",
            VariableKind::Missing => "unknown",
        };
        h_flex()
            .gap_1()
            .px_1p5()
            .rounded_md()
            .border_1()
            .border_color(if missing { theme.danger } else { theme.border })
            .text_xs()
            .when(missing, |d| d.child(div().text_color(theme.danger).child(crate::icons::icon("alert").small())))
            .child(div().font_family("Menlo").child(format!("{{{{{name}}}}}")))
            .child(div().text_color(theme.muted_foreground).child(kind_text))
    }));
    Some(
        v_flex()
            .gap_1()
            .child(chips)
            .when(unknown > 0, |d| {
                let what = if unknown == 1 {
                    "This variable is not defined".to_owned()
                } else {
                    format!("{unknown} variables are not defined")
                };
                d.child(hint_tone(
                    format!("{what}: add a column or counter with that name, or fix the spelling."),
                    Tone::Error,
                    cx,
                ))
            })
            .into_any_element(),
    )
}
