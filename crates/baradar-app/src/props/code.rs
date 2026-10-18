//! Properties of a code block (port of `editor/props/CodeProps.svelte`): QR / DataMatrix /
//! Code 128 / EAN-13, a Wi-Fi network builder for QR codes, quiet-zone modes and the module size
//! with the resulting physical size and a readability rating.

use baradar_core::doc::{
    CodeContent, CodeItem, Item, ItemKind, ModuleSize, QrEcc, QuietZone, Symbology, WifiSecurity, WifiSettings,
};
use baradar_core::render::codes::{
    ModuleOptions, Readability, choose_module_dots, code_matrix, dots_at, is_linear, js_to_fixed, quiet_modules,
    readability,
};
use baradar_core::render::units::dots_to_mm;
use baradar_core::render::wifi::{wifi_payload, wifi_warnings};
use baradar_core::vars::variables::{VariableContext, resolve_doc};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::Button,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::{
    Fields, NumSpec, Tone, field, hint, hint_tone, item_num_get, item_num_set, item_str_get, item_str_set, num_view,
    segmented_field, select, switch_view, variable_hint,
};
use super::{Band, PropertiesView};
use crate::studio::{Studio, local_today};

/// `SYMBOLOGY_HINT`.
pub fn symbology_hint(s: Symbology) -> &'static str {
    match s {
        Symbology::Qr => "QR: best for phone cameras (the iPhone Camera app reads it).",
        Symbology::Datamatrix => "DataMatrix: smaller, but needs a scanner app. Phone cameras usually ignore it.",
        Symbology::Code128 => "Code 128: a barcode for handheld scanners and inventory apps.",
        Symbology::Ean13 => "EAN-13: retail product numbers (12 or 13 digits).",
    }
}

/// `ECC`.
const ECC: [(QrEcc, &str); 4] = [
    (QrEcc::L, "Low (7 %)"),
    (QrEcc::M, "Medium (15 %)"),
    (QrEcc::Q, "Quartile (25 %)"),
    (QrEcc::H, "High (30 %)"),
];

/// `SECURITY`.
const SECURITY: [(WifiSecurity, &str); 3] = [
    (WifiSecurity::Wpa, "WPA / WPA2 / WPA3"),
    (WifiSecurity::Wep, "WEP (old)"),
    (WifiSecurity::Open, "None (open network)"),
];

/// `quietHint`.
pub fn quiet_hint(symbology: Symbology, quiet: QuietZone) -> &'static str {
    let linear = symbology.is_linear();
    let dm = symbology == Symbology::Datamatrix;
    match quiet {
        QuietZone::Standard if linear => "10 modules of blank space left and right (the standard).",
        QuietZone::Standard if dm => "1 module of blank space all round (the DataMatrix standard).",
        QuietZone::Standard => {
            "4 modules of blank space all round (the QR standard). The unprinted tape edge counts above and below."
        }
        QuietZone::Compact if linear => "5 modules left and right. Most scanners still read it.",
        QuietZone::Compact if dm => {
            "The code may fill the tape height; the unprinted tape edge is its margin. 1 module left and right."
        }
        QuietZone::Compact => {
            "The code may fill the tape height; the unprinted tape edge is its margin. 2 modules left and right, half the QR standard of 4: keep other ink at least that far away. Most phones still read it."
        }
        QuietZone::None => "No blank margin. Keep other ink away from the code, or it may not scan.",
    }
}

/// `dataHint`: the hint under the data field and whether it is an error. `encode_error` is the
/// encoder's message for QR / DataMatrix data.
pub fn data_hint(symbology: Symbology, data: &str, encode_error: Option<&str>) -> (String, bool) {
    match symbology {
        Symbology::Ean13 => {
            if !data.bytes().all(|b| b.is_ascii_digit()) {
                return ("EAN-13 takes digits only.".into(), true);
            }
            let n = data.len();
            if n != 12 && n != 13 {
                return (format!("Enter 12 digits (the check digit is added) or 13. Now: {n}."), true);
            }
            let text = if n == 12 {
                "The check digit is added automatically."
            } else {
                "13 digits including the check digit."
            };
            (text.into(), false)
        }
        Symbology::Code128 => {
            if !data.is_ascii() {
                return ("Code 128 supports plain ASCII characters only.".into(), true);
            }
            ("Letters, digits and symbols. Short is better on narrow tape.".into(), false)
        }
        _ => {
            if let Some(e) = encode_error {
                return (e.to_owned(), true);
            }
            let bytes = data.len();
            if symbology == Symbology::Datamatrix {
                (format!("{bytes} bytes. Plain letters and digits scan most reliably."), false)
            } else {
                (format!("{bytes} bytes. URLs and plain text both work."), false)
            }
        }
    }
}

/// The encoder message without a leading `CODE:` prefix (`/^[A-Z_]+:\s*/`).
fn strip_code_prefix(m: &str) -> String {
    if let Some((head, rest)) = m.split_once(':')
        && !head.is_empty()
        && head.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
    {
        return rest.trim_start().to_owned();
    }
    m.to_owned()
}

/// What the readout under the module size says.
#[derive(Debug, Clone, PartialEq)]
pub struct Sizing {
    pub md: i64,
    pub max: i64,
    pub linear: bool,
    pub size_text: String,
}

/// `sizing` + `sizeText`: the module size the renderer uses (read back from the rendered box when
/// it matches, else `chooseModuleDots`) and the sentence describing it.
pub fn code_sizing(item: &CodeItem, printed: &CodeItem, band: Band, rendered_w: Option<i64>) -> Option<Result<Sizing, String>> {
    if printed.content != CodeContent::Wifi && printed.data.is_empty() {
        return None;
    }
    let m = match code_matrix(printed) {
        Ok(m) => m,
        Err(e) => return Some(Err(strip_code_prefix(e.message()))),
    };
    let (qx, _) = quiet_modules(&m, item.quiet_zone, Some(item.symbology));
    let choice = choose_module_dots(
        &m,
        &ModuleOptions {
            module_dots: item.module_dots,
            quiet_zone: item.quiet_zone,
            symbology: Some(item.symbology),
            band_dots: band.dots,
            margin_dots: band.margin_dots,
            limit_w: None,
            dpi: band.dpi,
        },
    );
    let mut md = i64::from(choice.md);
    let unit = i64::from(m.width) + 2 * qx;
    if let Some(w) = rendered_w
        && !band.framed
        && unit > 0
        && w % unit == 0
        && w / unit >= 1
    {
        md = w / unit;
    }
    let linear = is_linear(&m);
    let mm = |dots: i64| js_to_fixed(dots_to_mm(dots as f64, band.dpi), 1);
    let s = if md == 1 { "" } else { "s" };
    let size_text = if !linear && choice.max < 1 {
        "This code does not fit this tape. Use wider tape or less data.".to_owned()
    } else if linear {
        format!("{md} dot{s} per bar module · about {} mm long with its quiet zone.", mm(unit * md))
    } else {
        format!(
            "{md} dot{s} per module · {} × {} mm ({} × {} modules).",
            mm(i64::from(m.width) * md),
            mm(i64::from(m.height) * md),
            m.width,
            m.height
        )
    };
    Some(Ok(Sizing { md, max: choice.max, linear, size_text }))
}

/// Edits the code item `id` (no coalescing).
fn edit_code(studio: &Entity<Studio>, id: &str, cx: &mut App, f: impl FnOnce(&mut CodeItem) + 'static) {
    let id = id.to_owned();
    studio.update(cx, move |s, cx| {
        s.update_item(
            &id,
            None,
            move |i| {
                if let Item::Code(c) = i {
                    f(c);
                }
            },
            cx,
        )
    });
}

fn wifi_get(id: &str, read: fn(&WifiSettings) -> String) -> super::widgets::StrGet {
    item_str_get(id, move |i| match i {
        Item::Code(c) => Some(c.wifi.as_ref().map_or_else(String::new, read)),
        _ => None,
    })
}

fn wifi_set(id: &str, key: &'static str, write: fn(&mut WifiSettings, String)) -> super::widgets::StrSet {
    item_str_set(id, key, move |i, v| {
        if let Item::Code(c) = i {
            write(c.wifi.get_or_insert_with(WifiSettings::default), v.chars().take(64).collect());
        }
    })
}

pub struct CodeEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
    show_password: bool,
}

impl CodeEditor {
    pub fn new(item: &CodeItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        let id = item.id.as_str();
        let data_get = || {
            item_str_get(id, |i| match i {
                Item::Code(c) => Some(c.data.clone()),
                _ => None,
            })
        };
        let data_set = || {
            item_str_set(id, "data", |i, v| {
                if let Item::Code(c) = i {
                    c.data = v;
                }
            })
        };
        // QR and DataMatrix take a textarea, the bar codes one line; both edit `data`.
        fields.add_str("data_area", true, "", studio, data_get(), data_set(), window, cx);
        fields.add_str("data_line", false, "", studio, data_get(), data_set(), window, cx);
        fields.add_str("ssid", false, "", studio, wifi_get(id, |w| w.ssid.clone()), wifi_set(id, "ssid", |w, v| w.ssid = v), window, cx);
        fields.add_str(
            "password",
            false,
            "",
            studio,
            wifi_get(id, |w| w.password.clone()),
            wifi_set(id, "password", |w, v| w.password = v),
            window,
            cx,
        );
        if let Some(super::widgets::StrState::Line(p)) = fields.str("password").map(|f| f.state.clone()) {
            p.update(cx, |s, cx| s.set_masked(true, window, cx));
        }
        fields.add_num(
            "module",
            NumSpec::new("Dots per module", "dots", 1.0, 20.0, 1.0),
            studio,
            item_num_get(id, |i| match i {
                Item::Code(c) => match c.module_dots {
                    ModuleSize::Dots(n) => Some(f64::from(n)),
                    ModuleSize::Auto => None,
                },
                _ => None,
            }),
            item_num_set(id, "module", |i, v| {
                if let Item::Code(c) = i {
                    c.module_dots = ModuleSize::Dots(v.round().clamp(1.0, 20.0) as u8);
                }
            }),
            window,
            cx,
        );
        Self {
            fields,
            studio: studio.clone(),
            show_password: false,
        }
    }

    pub fn render(&mut self, item: &CodeItem, window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let theme = cx.theme().clone();
        let (band, printed, rendered_w, doc) = {
            let s = self.studio.read(cx);
            let band = s.props_band(&Item::Code(item.clone()));
            let resolved = resolve_doc(&s.doc, s.preview_row, &VariableContext::new(local_today()));
            let printed = match resolved.doc.item(&item.id) {
                Some(Item::Code(c)) => c.clone(),
                _ => item.clone(),
            };
            let rendered_w = s
                .render
                .as_ref()
                .and_then(|r| r.boxes.iter().find(|b| b.item_id == item.id).map(|b| b.w));
            (band, printed, rendered_w, s.doc.clone())
        };
        let dpi = band.dpi;
        let sizing = code_sizing(item, &printed, band, rendered_w);
        let encode_error = match &sizing {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        };
        let sizing = sizing.and_then(Result::ok);
        let is_wifi = item.symbology == Symbology::Qr && item.content == CodeContent::Wifi;
        let wifi = item.wifi.clone().unwrap_or_default();
        let id = item.id.clone();
        let mut col = v_flex().gap_3();

        // Code type.
        {
            let (studio, id, cur) = (self.studio.clone(), id.clone(), item.clone());
            col = col.child(
                v_flex()
                    .gap_1()
                    .child(segmented_field(
                        "code-type",
                        "Code type",
                        &[
                            (Symbology::Qr, "QR"),
                            (Symbology::Datamatrix, "DataMatrix"),
                            (Symbology::Code128, "Code 128"),
                            (Symbology::Ean13, "EAN-13"),
                        ],
                        Some(item.symbology),
                        move |symbology, _, cx| {
                            if symbology == cur.symbology {
                                return;
                            }
                            let cur = cur.clone();
                            edit_code(&studio, &id, cx, move |c| {
                                let linear = symbology.is_linear();
                                c.symbology = symbology;
                                c.show_text = linear;
                                let digits = cur.data.bytes().all(|b| b.is_ascii_digit());
                                if symbology == Symbology::Ean13 && !(digits && matches!(cur.data.len(), 12 | 13)) {
                                    c.data = "400638133393".into();
                                }
                                if symbology != Symbology::Qr && cur.content == CodeContent::Wifi {
                                    c.content = CodeContent::Text;
                                }
                                // A fixed module wider than 3 dots (at 180 dpi) makes a very long barcode: back to 2.
                                if linear
                                    && let ModuleSize::Dots(n) = cur.module_dots
                                    && i64::from(n) > dots_at(3, dpi)
                                {
                                    c.module_dots = ModuleSize::Dots(dots_at(2, dpi).clamp(1, 255) as u8);
                                }
                            });
                        },
                        cx,
                    ))
                    .child(hint(symbology_hint(item.symbology), cx)),
            );
        }

        // What to encode (QR only).
        if item.symbology == Symbology::Qr {
            let (studio, id, has_wifi) = (self.studio.clone(), id.clone(), item.wifi.is_some());
            let current = item.content;
            col = col.child(segmented_field(
                "code-content",
                "What to encode",
                &[(CodeContent::Text, "Text or link"), (CodeContent::Wifi, "Wi-Fi network")],
                Some(item.content),
                move |content, _, cx| {
                    if content == current {
                        return;
                    }
                    edit_code(&studio, &id, cx, move |c| {
                        c.content = content;
                        if content == CodeContent::Wifi && !has_wifi {
                            c.wifi = Some(WifiSettings::default());
                        }
                    });
                },
                cx,
            ));
        }

        if is_wifi {
            col = col.child(self.wifi_fields(item, &wifi, &printed, &doc, window, cx));
        } else {
            let two_d = matches!(item.symbology, Symbology::Qr | Symbology::Datamatrix);
            let (text, error) = data_hint(item.symbology, &item.data, encode_error.as_deref());
            let input = self
                .fields
                .str(if two_d { "data_area" } else { "data_line" })
                .map(|f| f.element())
                .unwrap_or_else(|| div().into_any_element());
            col = col.child(
                v_flex()
                    .gap_1()
                    .child(field(if two_d { "Content" } else { "Data" }, input, None, cx))
                    .child(hint_tone(text, if error { Tone::Error } else { Tone::Plain }, cx))
                    .children(variable_hint(&item.data, &doc, cx)),
            );
        }

        // Quiet zone.
        {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(
                v_flex()
                    .gap_1()
                    .child(segmented_field(
                        "code-quiet",
                        "Quiet zone",
                        &[(QuietZone::Standard, "Standard"), (QuietZone::Compact, "Compact"), (QuietZone::None, "None")],
                        Some(item.quiet_zone),
                        move |q, _, cx| edit_code(&studio, &id, cx, move |c| c.quiet_zone = q),
                        cx,
                    ))
                    .child(hint(quiet_hint(item.symbology, item.quiet_zone), cx)),
            );
        }

        // Module size.
        {
            let fixed = matches!(item.module_dots, ModuleSize::Dots(_));
            let (studio, id) = (self.studio.clone(), id.clone());
            let md_now = sizing.as_ref().map_or(2, |s| s.md);
            let mut m = v_flex().gap_1().child(segmented_field(
                "code-module",
                "Module size",
                &[(false, "Auto"), (true, "Fixed")],
                Some(fixed),
                move |want_fixed, _, cx| {
                    if !want_fixed {
                        edit_code(&studio, &id, cx, |c| c.module_dots = ModuleSize::Auto);
                    } else if !fixed {
                        let n = md_now.clamp(1, 20) as u8;
                        edit_code(&studio, &id, cx, move |c| c.module_dots = ModuleSize::Dots(n));
                    }
                },
                cx,
            ));
            if fixed {
                let fixed_dots = match item.module_dots {
                    ModuleSize::Dots(n) => i64::from(n),
                    ModuleSize::Auto => md_now,
                };
                let fixed_hint = match &sizing {
                    Some(s) if !s.linear => {
                        if s.max >= 1 && fixed_dots > s.max {
                            format!(
                                "Only {} fit{} this tape, so it prints at {}.",
                                s.max,
                                if s.max == 1 { "s" } else { "" },
                                s.max
                            )
                        } else {
                            format!("Up to {} fit this tape.", s.max.max(0))
                        }
                    }
                    _ => "Wider modules scan more reliably.".to_owned(),
                };
                m = m.child(num_view(self.fields.num("module"), Some(fixed_hint.into()), cx));
            } else {
                let linear = sizing.as_ref().is_some_and(|s| s.linear);
                m = m.child(hint(
                    if linear {
                        "Fills a fixed label length (up to 4 dots), else 2 dots."
                    } else {
                        "The largest size that fits the tape."
                    },
                    cx,
                ));
            }
            col = col.child(m);
        }

        // Readout.
        if let Some(s) = &sizing {
            let read = (!(s.max < 1 && !s.linear)).then(|| readability(s.md, dpi));
            col = col.child(
                v_flex()
                    .gap_0p5()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted.opacity(0.4))
                    .text_xs()
                    .child(s.size_text.clone())
                    .when_some(read, |d, r| {
                        let (title, color) = match r.level {
                            Readability::Good => ("Readability: good.", theme.foreground),
                            Readability::Ok => ("Readability: OK.", theme.warning),
                            Readability::Poor => ("Readability: poor.", theme.danger),
                        };
                        d.child(
                            div()
                                .text_color(color)
                                .child(div().font_weight(FontWeight::BOLD).child(title))
                                .child(r.text),
                        )
                    }),
            );
        }

        // Error correction (QR).
        if item.symbology == Symbology::Qr {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(field(
                "Error correction",
                select(
                    "code-ecc",
                    ECC.iter().map(|(v, l)| (*v, SharedString::from(*l))).collect(),
                    Some(item.ecc),
                    move |ecc, _, cx| edit_code(&studio, &id, cx, move |c| c.ecc = ecc),
                ),
                None,
                cx,
            ));
        }

        // Show text (bar codes).
        if item.symbology.is_linear() {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(switch_view(
                "code-show-text",
                "Show text",
                item.show_text,
                Some("Print the data under the bars"),
                move |on, _, cx| edit_code(&studio, &id, cx, move |c| c.show_text = on),
                cx,
            ));
        }
        col.into_any_element()
    }

    /// The Wi-Fi network fields: SSID, security, password (with show/hide), hidden network, the
    /// live hint and "Add network name label".
    fn wifi_fields(
        &mut self,
        item: &CodeItem,
        wifi: &WifiSettings,
        printed: &CodeItem,
        doc: &baradar_core::doc::LabelDoc,
        _window: &mut Window,
        cx: &mut Context<PropertiesView>,
    ) -> AnyElement {
        let id = item.id.clone();
        let w = printed.wifi.clone().unwrap_or_default();
        let (problem, tone) = match wifi_payload(&w) {
            Err(e) => (e, Tone::Error),
            Ok(_) => match wifi_warnings(&w).into_iter().next() {
                Some(warn) => (warn, Tone::Warn),
                None => ("iPhone and Android cameras offer to join this network.".to_owned(), Tone::Plain),
            },
        };
        let ssid = self.fields.str("ssid").map(|f| f.element()).unwrap_or_else(|| div().into_any_element());
        let mut col = v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(field("Network name (SSID)", ssid, None, cx))
                    .children(variable_hint(&wifi.ssid, doc, cx)),
            );
        {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(field(
                "Security",
                select(
                    "wifi-security",
                    SECURITY.iter().map(|(v, l)| (*v, SharedString::from(*l))).collect(),
                    Some(wifi.security),
                    move |sec, _, cx| {
                        edit_code(&studio, &id, cx, move |c| c.wifi.get_or_insert_with(WifiSettings::default).security = sec)
                    },
                ),
                None,
                cx,
            ));
        }
        if wifi.security != WifiSecurity::Open {
            let password = self.fields.str("password").map(|f| f.state.clone());
            let show = self.show_password;
            let input = self.fields.str("password").map(|f| f.element()).unwrap_or_else(|| div().into_any_element());
            let toggle = Button::new("wifi-show-password")
                .icon(if show { IconName::EyeOff } else { IconName::Eye })
                .small()
                .outline()
                .tooltip(if show { "Hide password" } else { "Show password" })
                .selected(show)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(super::ItemEditor::Code(e)) = &mut this.editor {
                        e.show_password = !e.show_password;
                        let masked = !e.show_password;
                        if let Some(super::widgets::StrState::Line(p)) = &password {
                            p.update(cx, |s, cx| s.set_masked(masked, window, cx));
                        }
                        cx.notify();
                    }
                }));
            col = col.child(
                v_flex()
                    .gap_1()
                    .child(super::widgets::field_label("Password", cx))
                    .child(h_flex().gap_1().child(div().flex_1().min_w_0().child(input)).child(toggle))
                    .child(hint(
                        "The password stays on this device. Share links and exported files leave it out unless you choose to include it.",
                        cx,
                    )),
            );
        }
        {
            let (studio, id) = (self.studio.clone(), id.clone());
            col = col.child(switch_view(
                "wifi-hidden",
                "Hidden network",
                wifi.hidden,
                Some("The network does not broadcast its name"),
                move |on, _, cx| edit_code(&studio, &id, cx, move |c| c.wifi.get_or_insert_with(WifiSettings::default).hidden = on),
                cx,
            ));
        }
        let studio = self.studio.clone();
        col.child(hint_tone(problem, tone, cx))
            .child(
                div().child(
                    Button::new("wifi-add-ssid")
                        .icon(crate::icons::icon("plus")).label("Add network name label")
                        .small()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio.update(cx, |s, cx| {
                                s.insert(
                                    ItemKind::Text,
                                    |i| {
                                        if let Item::Text(t) = i {
                                            t.text = "{{ssid}}".into();
                                        }
                                    },
                                    cx,
                                )
                            })
                        }),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{data_hint, quiet_hint, strip_code_prefix};
    use baradar_core::doc::{QuietZone, Symbology};

    #[test]
    fn hints() {
        assert_eq!(data_hint(Symbology::Ean13, "12a", None), ("EAN-13 takes digits only.".into(), true));
        assert_eq!(
            data_hint(Symbology::Ean13, "123", None),
            ("Enter 12 digits (the check digit is added) or 13. Now: 3.".into(), true)
        );
        assert!(!data_hint(Symbology::Ean13, "400638133393", None).1);
        assert_eq!(data_hint(Symbology::Qr, "héllo", None).0, "6 bytes. URLs and plain text both work.");
        assert!(data_hint(Symbology::Code128, "é", None).1);
        assert_eq!(quiet_hint(Symbology::Code128, QuietZone::Compact), "5 modules left and right. Most scanners still read it.");
        assert_eq!(strip_code_prefix("INVALID_INPUT: too long"), "too long");
        assert_eq!(strip_code_prefix("Too long: really"), "Too long: really");
    }
}
