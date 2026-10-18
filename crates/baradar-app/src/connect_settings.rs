//! "Printer settings" inside the connect dialog: read every setting on open, stage a change,
//! confirm, write, verify the read-back (port of `printer-settings/PrinterSettings.svelte` and
//! `printer-settings/settings-view.ts`).
//!
//! Replies arrive as [`StudioEvent::Printer`] events (`SettingsRead`, `SettingsWritten`).

use baradar_printer::problems::{Problem, ProblemId};
use baradar_printer::{ClientState, Command, Event, LinkKind};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    select::{Select, SelectEvent, SelectState},
    spinner::Spinner,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ptouch::settings::{LabelSource, SettingKind};
use ptouch::{ModelProfile, SettingId, SettingReading, SettingReport, SettingSpec, SettingValue};
use ptouch_transport::TransportKind;

use crate::studio::{Studio, StudioEvent};

// ------------------------------------------------------------------------------------------
// Pure helpers (settings-view.ts)
// ------------------------------------------------------------------------------------------

/// Models whose settings reads and writes were checked on a real printer (empty until a write
/// hardware run).
const SETTINGS_VERIFIED_MODELS: [&str; 0] = [];
/// Models whose settings reads (not writes) were checked on a real printer.
const SETTINGS_READ_VERIFIED_MODELS: [&str; 1] = ["PT-P710BT"];

/// `ROW_LABELS`.
pub fn row_label(id: SettingId) -> &'static str {
    match id {
        SettingId::AutoPowerOn => "Auto power on",
        SettingId::AutoPowerOff => "Auto power off: on external power",
        SettingId::AutoPowerOffBattery => "Auto power off: on battery",
        _ => "Setting",
    }
}

/// `SHORT_NAMES`: for sentences.
pub fn short_name(id: SettingId) -> &'static str {
    match id {
        SettingId::AutoPowerOn => "auto power on",
        SettingId::AutoPowerOff => "auto power off (external power)",
        SettingId::AutoPowerOffBattery => "auto power off (battery)",
        _ => "the setting",
    }
}

/// `ROW_HINTS`.
pub fn row_hint(id: SettingId) -> &'static str {
    match id {
        SettingId::AutoPowerOn => "Turns the printer on when power is connected.",
        SettingId::AutoPowerOff => {
            "Turns the printer off after this long without use while it runs on external power."
        }
        SettingId::AutoPowerOffBattery => {
            "Turns the printer off after this long without use while it runs on its battery."
        }
        _ => "",
    }
}

/// `brotherNote`: Brother's name for the setting.
pub fn brother_note(spec: &SettingSpec) -> String {
    match spec.label_source {
        LabelSource::InfoReport => format!("Shown as “{}” in the printer’s information report.", spec.brother_label),
        _ => format!("Same as “{}” in Brother’s Printer Setting Tool.", spec.brother_label),
    }
}

/// `displayText`: "never" → "Never".
pub fn display_text(text: &str) -> String {
    crate::view_model::capitalize(text)
}

/// `savedText`.
pub fn saved_text(id: SettingId, label: &str) -> String {
    format!("Saved and verified: {} is {label}.", short_name(id))
}

/// `confirmText`: "Change auto power off (external power) from 30 min to 10 min?"
pub fn confirm_text(id: SettingId, from: Option<&str>, to: &str) -> String {
    let was = from.map(|f| format!(" from {f}")).unwrap_or_default();
    format!("Change {}{was} to {to}?", short_name(id))
}

/// `settingsNotes`: virtual printer, untested model, untested Bluetooth path.
pub fn settings_notes(model: &str, link: Option<(LinkKind, TransportKind)>) -> Vec<String> {
    let mut notes = Vec::new();
    let is_virtual = link.is_some_and(|(l, t)| l == LinkKind::Virtual || t == TransportKind::Virtual);
    if is_virtual {
        notes.push("Virtual printer: the values are simulated and nothing reaches a real printer.".into());
    } else if SETTINGS_READ_VERIFIED_MODELS.contains(&model) && !SETTINGS_VERIFIED_MODELS.contains(&model) {
        notes.push(format!(
            "Reading is checked on the {model}; changing a setting has not been tested on a real printer yet. Each change is read back to check that the printer stored it."
        ));
    } else if !SETTINGS_VERIFIED_MODELS.contains(&model) {
        notes.push(format!(
            "Untested on the {model}: printer settings have not been checked on a real printer yet. Each change is read back to check that the printer stored it."
        ));
    }
    if link.is_some_and(|(l, t)| matches!(l, LinkKind::Bluetooth | LinkKind::Serial) && t != TransportKind::Virtual) {
        notes.push("Bluetooth: untested. If the printer doesn’t answer, try a USB cable.".into());
    }
    notes
}

/// The value a report holds, if it is one of the model's.
fn report_value(r: Option<&SettingReport>) -> Option<SettingValue> {
    match r?.reading {
        SettingReading::Value(v) => Some(v),
        _ => None,
    }
}

/// `choiceIndex`: index of `value` in the setting's choices.
fn choice_index(spec: &SettingSpec, value: Option<SettingValue>) -> Option<usize> {
    let v = value?;
    spec.choices.iter().position(|c| c.value == v)
}

// ------------------------------------------------------------------------------------------
// The panel
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Reading,
    Writing(SettingId),
}

/// A change waiting for confirmation (or retried after a mismatch).
#[derive(Debug, Clone)]
struct Pending {
    spec: &'static SettingSpec,
    value: SettingValue,
    label: String,
    from: Option<String>,
}

#[derive(Debug, Clone)]
enum Outcome {
    Ok(String),
    Mismatch { text: String, retry: Pending },
    Error { problem: Problem, after_write: bool },
}

type ChoiceList = Vec<SharedString>;

pub struct SettingsPanel {
    studio: Entity<Studio>,
    model: &'static ModelProfile,
    specs: &'static [SettingSpec],
    phase: Phase,
    readings: Vec<SettingReport>,
    /// Durations chosen but not applied yet (choice index per setting).
    drafts: Vec<(SettingId, usize)>,
    outcome: Option<Outcome>,
    /// The change being written (for the outcome sentence).
    pending_write: Option<Pending>,
    /// The read on open has not run yet (it waits while a print is in progress).
    initial_read: bool,
    selects: Vec<(SettingId, Entity<SelectState<ChoiceList>>)>,
    _subs: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(studio: Entity<Studio>, model: &'static ModelProfile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let specs = ptouch::settings_for(model);
        let mut subs = vec![
            cx.observe(&studio, |this, _, cx| {
                if this.initial_read {
                    this.read(cx);
                }
                cx.notify();
            }),
            cx.subscribe_in(&studio, window, |this, _, ev: &StudioEvent, window, cx| {
                if let StudioEvent::Printer(e) = ev {
                    this.on_printer_event(e, window, cx);
                }
            }),
        ];
        let mut selects = Vec::new();
        for spec in specs.iter().filter(|s| s.id.kind() == SettingKind::Duration) {
            let items: ChoiceList = spec.choices.iter().map(|c| display_text(&spec.display_value(c.value)).into()).collect();
            let select = cx.new(|cx| SelectState::new(items, None::<IndexPath>, window, cx));
            let id = spec.id;
            subs.push(cx.subscribe_in(&select, window, move |this, state, ev: &SelectEvent<ChoiceList>, _, cx| {
                if let SelectEvent::Confirm(_) = ev
                    && let Some(ix) = state.read(cx).selected_index(cx)
                {
                    this.set_draft(id, ix.row, cx);
                }
            }));
            selects.push((spec.id, select));
        }
        let mut this = Self {
            studio,
            model,
            specs,
            phase: Phase::Idle,
            readings: Vec::new(),
            drafts: Vec::new(),
            outcome: None,
            pending_write: None,
            initial_read: true,
            selects,
            _subs: subs,
        };
        this.read(cx);
        this
    }

    fn ready(&self, cx: &App) -> bool {
        self.studio.read(cx).conn == ClientState::Connected
    }

    /// A print the user already asked for comes first.
    fn can_run(&self, cx: &App) -> bool {
        self.ready(cx) && !self.studio.read(cx).printing
    }

    fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    fn report_of(&self, id: SettingId) -> Option<&SettingReport> {
        self.readings.iter().find(|r| r.id == id)
    }

    fn reading_text(&self, spec: &SettingSpec) -> Option<String> {
        self.report_of(spec.id).map(|r| display_text(&spec.display_reading(r.reading)))
    }

    /// Reads every setting (also the read on open, once the printer is idle).
    fn read(&mut self, cx: &mut Context<Self>) {
        if self.busy() || !self.can_run(cx) || self.specs.is_empty() {
            return;
        }
        self.initial_read = false;
        self.phase = Phase::Reading;
        self.outcome = None;
        self.studio.read(cx).send_printer(Command::ReadSettings(None));
        cx.notify();
    }

    fn set_draft(&mut self, id: SettingId, index: usize, cx: &mut Context<Self>) {
        let stored = self
            .specs
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| choice_index(s, report_value(self.report_of(id))));
        self.drafts.retain(|(d, _)| *d != id);
        if Some(index) != stored {
            self.drafts.push((id, index));
        }
        cx.notify();
    }

    fn drop_draft(&mut self, id: SettingId) {
        self.drafts.retain(|(d, _)| *d != id);
    }

    /// Puts the selects back on the stored values (except rows with a draft).
    fn sync_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (id, select) in &self.selects {
            if self.drafts.iter().any(|(d, _)| d == id) {
                continue;
            }
            let Some(spec) = self.specs.iter().find(|s| s.id == *id) else { continue };
            let idx = choice_index(spec, report_value(self.report_of(*id)));
            select.update(cx, |s, cx| s.set_selected_index(idx.map(IndexPath::new), window, cx));
        }
    }

    /// `ask`: opens the confirmation for a change.
    fn ask(&mut self, spec: &'static SettingSpec, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = spec.choices.get(index) else { return };
        if self.busy() || !self.can_run(cx) {
            return;
        }
        if report_value(self.report_of(spec.id)) == Some(choice.value) {
            self.drop_draft(spec.id);
            self.sync_selects(window, cx);
            cx.notify();
            return;
        }
        let pending = Pending {
            spec,
            value: choice.value,
            label: spec.display_value(choice.value),
            from: self.report_of(spec.id).map(|r| spec.display_reading(r.reading)),
        };
        let title = confirm_text(spec.id, pending.from.as_deref(), &pending.label);
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (ok_panel, cancel_panel) = (panel.clone(), panel.clone());
            let (ok_pending, cancel_id) = (pending.clone(), pending.spec.id);
            alert
                .title(title.clone())
                .description("The printer stores this setting. It is read back afterwards to check it.")
                .show_cancel(true)
                .cancel_text("Cancel")
                .ok_text("Change")
                .on_ok(move |_, _, cx| {
                    let p = ok_pending.clone();
                    let _ = ok_panel.update(cx, |this, cx| this.write(p, cx));
                    true
                })
                .on_cancel(move |_, window, cx| {
                    // The select shows the stored value again (Apply disappears).
                    let _ = cancel_panel.update(cx, |this, cx| {
                        this.drop_draft(cancel_id);
                        this.sync_selects(window, cx);
                        cx.notify();
                    });
                    true
                })
        });
    }

    fn write(&mut self, p: Pending, cx: &mut Context<Self>) {
        if self.busy() || !self.can_run(cx) {
            return;
        }
        self.phase = Phase::Writing(p.spec.id);
        self.outcome = None;
        self.studio.read(cx).send_printer(Command::WriteSettings(vec![(p.spec.id, p.value)]));
        // Kept for the outcome sentence.
        self.pending_write = Some(p);
        cx.notify();
    }

    fn on_printer_event(&mut self, ev: &Event, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            Event::SettingsRead(Ok(reports)) => {
                for r in reports {
                    self.readings.retain(|x| x.id != r.id);
                    self.readings.push(r.clone());
                }
                if self.phase == Phase::Reading {
                    self.phase = Phase::Idle;
                }
                self.sync_selects(window, cx);
            }
            Event::SettingsRead(Err(f)) => {
                if self.phase == Phase::Reading {
                    self.phase = Phase::Idle;
                    if f.problem.id != ProblemId::Cancelled {
                        self.outcome = Some(Outcome::Error { problem: f.problem.clone(), after_write: false });
                    }
                }
            }
            Event::SettingsWritten(result) => {
                let Phase::Writing(id) = self.phase else { return };
                self.phase = Phase::Idle;
                self.drop_draft(id);
                let pending = self.pending_write.take();
                match result {
                    Ok(r) => {
                        for x in &r.report {
                            self.readings.retain(|y| y.id != x.id);
                            self.readings.push(x.clone());
                        }
                        let got = r.report.iter().find(|x| x.id == id);
                        self.outcome = pending.map(|p| {
                            if r.verified {
                                Outcome::Ok(saved_text(id, &p.label))
                            } else {
                                let now = got.map_or_else(|| "the old value".to_owned(), |g| p.spec.display_reading(g.reading));
                                Outcome::Mismatch { text: format!("The printer still reports {now}."), retry: p }
                            }
                        });
                    }
                    // A cancelled write may have reached the printer too: always offer to read
                    // again.
                    Err(f) => self.outcome = Some(Outcome::Error { problem: f.problem.clone(), after_write: true }),
                }
                self.sync_selects(window, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    fn busy_text(&self, cx: &App) -> Option<&'static str> {
        let s = self.studio.read(cx);
        if s.printing || matches!(s.conn, ClientState::Printing | ClientState::Cancelling) {
            Some("A label is printing. Settings are available when the printer is idle.")
        } else if !self.ready(cx) && self.phase == Phase::Idle {
            Some("The printer isn’t ready.")
        } else {
            None
        }
    }

    fn render_row(&self, spec: &'static SettingSpec, disabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let report = self.report_of(spec.id);
        let value = report_value(report);
        let idx = choice_index(spec, value);
        let spinning = self.phase == Phase::Reading || self.phase == Phase::Writing(spec.id);
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(div().font_weight(FontWeight::MEDIUM).child(row_label(spec.id)))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} {}", row_hint(spec.id), brother_note(spec))),
            );
        let control = match spec.id.kind() {
            SettingKind::Toggle => {
                let on = value == Some(SettingValue::Enabled(true));
                let shown = self.reading_text(spec).unwrap_or_else(|| if spinning { String::new() } else { "—".into() });
                h_flex()
                    .gap_2()
                    .child(
                        Switch::new(SharedString::from(format!("setting-{}", spec.id.as_str())))
                            .checked(on)
                            .disabled(disabled || report.is_none())
                            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                if let Some(i) = choice_index(spec, Some(SettingValue::Enabled(*checked))) {
                                    this.ask(spec, i, window, cx);
                                }
                            })),
                    )
                    .child(div().text_sm().min_w(px(40.)).child(shown))
                    .into_any_element()
            }
            _ => {
                let draft = self.drafts.iter().find(|(d, _)| *d == spec.id).map(|(_, i)| *i);
                let placeholder = match (idx, self.reading_text(spec)) {
                    (None, Some(t)) => t,
                    (None, None) if spinning => "Reading…".into(),
                    _ => "—".into(),
                };
                let select = self.selects.iter().find(|(id, _)| *id == spec.id).map(|(_, s)| s.clone());
                h_flex()
                    .gap_2()
                    .when_some(select, |d, s| {
                        d.child(
                            div()
                                .w(px(150.))
                                .child(Select::new(&s).small().placeholder(placeholder).disabled(disabled || report.is_none())),
                        )
                    })
                    .when_some(draft.filter(|d| Some(*d) != idx), |d, draft| {
                        d.child(
                            Button::new(SharedString::from(format!("apply-{}", spec.id.as_str())))
                                .icon(crate::icons::icon("check")).label("Apply")
                                .small()
                                .primary()
                                .disabled(disabled)
                                .tooltip(format!("Apply {}", short_name(spec.id)))
                                .on_click(cx.listener(move |this, _, window, cx| this.ask(spec, draft, window, cx))),
                        )
                    })
                    .into_any_element()
            }
        };
        h_flex()
            .gap_3()
            .py_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(text)
            .when(spinning, |d| d.child(Spinner::new().small()))
            .child(control)
            .into_any_element()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.specs.is_empty() {
            return v_flex()
                .child(div().text_sm().text_color(theme.muted_foreground).child("This printer model has no settings this app can change."))
                .into_any_element();
        }
        let disabled = self.busy() || !self.can_run(cx);
        let rows: Vec<AnyElement> = self.specs.iter().map(|spec| self.render_row(spec, disabled, cx)).collect();
        let busy_text = self.busy_text(cx);
        let link = cx
            .try_global::<crate::connect::PrinterLink>()
            .and_then(|l| l.link.as_ref())
            .map(|l| (l.link, l.transport.kind));
        let notes = settings_notes(self.model.name, link);
        let outcome = self.outcome.clone();
        v_flex()
            .gap_3()
            .child(v_flex().children(rows))
            .map(|d| match &outcome {
                Some(Outcome::Ok(text)) => d.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.success)
                        .child(gpui_kit::component::Icon::new(IconName::CircleCheck).small())
                        .child(text.clone()),
                ),
                Some(Outcome::Mismatch { text, retry }) => {
                    let retry = retry.clone();
                    d.child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .text_color(theme.warning)
                            .child(gpui_kit::component::Icon::new(IconName::TriangleAlert).small())
                            .child(div().flex_1().child(text.clone()))
                            .child(
                                Button::new("settings-try-again")
                                    .icon(crate::icons::icon("refresh")).label("Try again")
                                    .small()
                                    .outline()
                                    .disabled(disabled)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(i) = choice_index(retry.spec, Some(retry.value)) {
                                            this.ask(retry.spec, i, window, cx);
                                        }
                                    })),
                            ),
                    )
                }
                Some(Outcome::Error { problem, after_write }) => d.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.danger)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(gpui_kit::component::Icon::new(IconName::CircleX).small().text_color(theme.danger))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(problem.title.clone())),
                        )
                        .child(div().text_sm().child(format!(
                            "{}{}",
                            problem.detail,
                            if *after_write { " The printer may have stored the new value. Read again." } else { "" }
                        ))),
                ),
                None => d.when_some(busy_text, |d, t| d.child(div().text_sm().text_color(theme.muted_foreground).child(t))),
            })
            .child(
                h_flex().child(
                    Button::new("settings-read-again")
                        .icon(IconName::RefreshCw)
                        .label("Read again")
                        .small()
                        .outline()
                        .disabled(disabled)
                        .on_click(cx.listener(|this, _, _, cx| this.read(cx))),
                ),
            )
            .children(notes.into_iter().map(|n| div().text_xs().text_color(theme.muted_foreground).child(n)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{confirm_text, saved_text, settings_notes};
    use baradar_printer::LinkKind;
    use ptouch::SettingId;
    use ptouch_transport::TransportKind;

    #[test]
    fn sentences() {
        assert_eq!(
            confirm_text(SettingId::AutoPowerOff, Some("30 min"), "10 min"),
            "Change auto power off (external power) from 30 min to 10 min?"
        );
        assert_eq!(saved_text(SettingId::AutoPowerOn, "on"), "Saved and verified: auto power on is on.");
        let v = settings_notes("PT-P710BT", Some((LinkKind::Custom, TransportKind::Virtual)));
        assert_eq!(v, vec!["Virtual printer: the values are simulated and nothing reaches a real printer.".to_owned()]);
        let bt = settings_notes("PT-P910BT", Some((LinkKind::Bluetooth, TransportKind::Rfcomm)));
        assert_eq!(bt.len(), 2);
        assert!(bt[0].starts_with("Untested on the PT-P910BT"));
    }
}

#[cfg(test)]
mod ui_tests {
    use super::{Outcome, Pending, Phase, SettingsPanel};
    use crate::connect::ui_tests::{new_studio, open_test_window, wait_until};
    use crate::studio::is_connected;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext};
    use ptouch::SettingId;

    #[gpui_kit::test]
    async fn read_write_verify_on_virtual_printer(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "settings");
        studio.update(cx, |s, cx| {
            s.set_design_model("PT-P710BT", cx);
            s.connect_virtual_printer();
        });
        wait_until(cx, "the virtual printer to connect", |cx| cx.update(|cx| is_connected(studio.read(cx).conn)));
        let model = cx.update(|cx| studio.read(cx).printer_model()).expect("model");
        assert_eq!(model.name, "PT-P710BT");
        let s2 = studio.clone();
        let (handle, panel) = open_test_window(cx, move |window, cx| SettingsPanel::new(s2, model, window, cx));
        wait_until(cx, "the settings read", |cx| {
            cx.update(|cx| {
                let p = panel.read(cx);
                p.phase == Phase::Idle && p.readings.len() == p.specs.len()
            })
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx)).expect("window");

        // Change auto power off (external power) to another listed value.
        let pending = cx.update(|cx| {
            let p = panel.read(cx);
            let spec = p.specs.iter().find(|s| s.id == SettingId::AutoPowerOff).expect("auto power off");
            let current = super::report_value(p.report_of(spec.id));
            let choice = spec.choices.iter().find(|c| Some(c.value) != current).expect("another value");
            Pending { spec, value: choice.value, label: spec.display_value(choice.value), from: None }
        });
        let label = pending.label.clone();
        panel.update(cx, |p, cx| p.write(pending, cx));
        wait_until(cx, "the settings write", |cx| cx.update(|cx| panel.read(cx).phase == Phase::Idle));
        cx.update(|cx| match &panel.read(cx).outcome {
            Some(Outcome::Ok(text)) => assert_eq!(text, &format!("Saved and verified: auto power off (external power) is {label}.")),
            other => panic!("unexpected outcome {other:?}"),
        });
        cx.update_window(handle, |_, window, cx| window.render_frame(cx)).expect("window");

        // The connect dialog opens straight on its settings view for a model with settings.
        let s3 = studio.clone();
        cx.update_window(handle, move |_, window, cx| crate::connect::open_printer_settings(s3, window, cx))
            .expect("window");
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("settings-back").is_some(), "settings view with a Back button");
        })
        .expect("window");
        studio.update(cx, |s, cx| s.disconnect(cx));
        cx.run_until_parked();
    }
}
