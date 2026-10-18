//! The "Label" card (port of `editor/props/LabelProps.svelte`): flow gap and vertical alignment
//! (or the free-placement notice), "Shrink text to fit length" on a fixed length, the Boldness
//! slider (print threshold) and the label frame. Margins and length live in the media bar, as in
//! the studio.

use baradar_core::doc::{Align, LabelFrame, LayoutSettings, LengthSettings};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PropertiesView;
use super::widgets::{Fields, NumSpec, doc_num_set, hint, num_view, segmented_field, slider_view, switch_view};
use crate::studio::Studio;

pub struct LabelEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
}

/// "Normal", "Bolder +n" or "Lighter −n" for a Boldness value (`boldnessText`).
pub fn boldness_text(v: f64) -> String {
    let step = ((v - 128.0) / 16.0).round() as i64;
    match step {
        0 => "Normal".into(),
        s if s > 0 => format!("Bolder +{s}"),
        s => format!("Lighter {s}"),
    }
}

impl LabelEditor {
    pub fn new(studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        fields.add_num(
            "gap",
            NumSpec::new("Gap between blocks", "mm", 0.0, 50.0, 0.5),
            studio,
            std::rc::Rc::new(|s: &Studio| match s.doc.layout {
                LayoutSettings::Flow { gap_mm, .. } => Some(gap_mm),
                LayoutSettings::Free => None,
            }),
            doc_num_set("doc:gap", |d, v| {
                if let LayoutSettings::Flow { gap_mm, .. } = &mut d.layout {
                    *gap_mm = v;
                }
            }),
            window,
            cx,
        );
        // Boldness = 256 − threshold, 32–224 in steps of 16.
        fields.add_slider(
            "boldness",
            (32.0, 224.0, 16.0),
            studio,
            std::rc::Rc::new(|s: &Studio| Some(256.0 - f64::from(s.doc.print.threshold))),
            doc_num_set("doc:threshold", |d, v| {
                d.print.threshold = (256.0 - v).round().clamp(0.0, 255.0) as u8;
            }),
            window,
            cx,
        );
        let frame_get = |read: fn(&LabelFrame) -> f64| {
            std::rc::Rc::new(move |s: &Studio| s.doc.frame.as_ref().map(read)) as super::widgets::NumGet
        };
        fields.add_num(
            "frame_t",
            NumSpec::new("Line", "mm", 0.1, 3.0, 0.1),
            studio,
            frame_get(|f| f.thickness_mm),
            doc_num_set("doc:frame-t", |d, v| {
                if let Some(f) = &mut d.frame {
                    f.thickness_mm = v;
                }
            }),
            window,
            cx,
        );
        fields.add_num(
            "frame_r",
            NumSpec::new("Corners", "mm", 0.0, 10.0, 0.5),
            studio,
            frame_get(|f| f.radius_mm),
            doc_num_set("doc:frame-r", |d, v| {
                if let Some(f) = &mut d.frame {
                    f.radius_mm = v;
                }
            }),
            window,
            cx,
        );
        fields.add_num(
            "frame_i",
            NumSpec::new("Inset", "mm", 0.0, 5.0, 0.1),
            studio,
            frame_get(|f| f.inset_mm),
            doc_num_set("doc:frame-i", |d, v| {
                if let Some(f) = &mut d.frame {
                    f.inset_mm = v;
                }
            }),
            window,
            cx,
        );
        Self { fields, studio: studio.clone() }
    }

    pub fn render(&mut self, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let (layout, length, threshold, frame, dpi) = {
            let s = self.studio.read(cx);
            let dpi = s.target().map_or(180, |t| t.profile.dpi);
            (s.doc.layout.clone(), s.doc.length.clone(), s.doc.print.threshold, s.doc.frame, dpi)
        };
        let boldness = 256.0 - f64::from(threshold);
        let mut col = v_flex().gap_3();

        col = match layout {
            LayoutSettings::Flow { align, .. } => {
                let studio = self.studio.clone();
                col.child(num_view(self.fields.num("gap"), None, cx)).child(segmented_field(
                    "label-valign",
                    "Vertical alignment",
                    &[(Align::Start, "Top"), (Align::Center, "Middle"), (Align::End, "Bottom")],
                    Some(align),
                    move |a, _, cx| {
                        studio.update(cx, |s, cx| {
                            s.update_doc(
                                None,
                                |d| {
                                    if let LayoutSettings::Flow { align, .. } = &mut d.layout {
                                        *align = a;
                                    }
                                },
                                cx,
                            )
                        })
                    },
                    cx,
                ))
            }
            LayoutSettings::Free => col.child(hint(
                "This label uses free placement (made with a newer editor). Blocks keep their saved positions.",
                cx,
            )),
        };

        if let LengthSettings::Fixed { mm, shrink } = length {
            let studio = self.studio.clone();
            col = col.child(
                v_flex()
                    .gap_0p5()
                    .child(switch_view(
                        "label-shrink",
                        "Shrink text to fit length",
                        shrink,
                        None,
                        move |on, _, cx| {
                            studio.update(cx, |s, cx| {
                                s.update_doc(None, |d| d.length = LengthSettings::Fixed { mm, shrink: on }, cx)
                            })
                        },
                        cx,
                    ))
                    .child(div().pl(px(36.)).child(hint(
                        format!(
                            "Text that is too long for the {} mm label gets smaller instead of being cut off.",
                            mm.round()
                        ),
                        cx,
                    ))),
            );
        }

        col = col.child(slider_view(
            "Boldness",
            boldness_text(boldness),
            self.fields.slider("boldness"),
            Some(format!("Thickens or thins text and lines at {dpi} dpi.").into()),
            cx,
        ));

        let studio = self.studio.clone();
        col = col
            .child(switch_view(
                "label-frame",
                "Frame",
                frame.is_some(),
                Some("Border around the whole label"),
                move |on, _, cx| {
                    studio.update(cx, |s, cx| {
                        s.update_doc(None, |d| d.frame = on.then_some(LabelFrame::UI_DEFAULT), cx)
                    })
                },
                cx,
            ))
            .when(frame.is_some(), |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(num_view(self.fields.num("frame_t"), None, cx)))
                        .child(div().flex_1().min_w_0().child(num_view(self.fields.num("frame_r"), None, cx)))
                        .child(div().flex_1().min_w_0().child(num_view(self.fields.num("frame_i"), None, cx))),
                )
            });
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::boldness_text;

    #[test]
    fn boldness_readout() {
        assert_eq!(boldness_text(128.0), "Normal");
        assert_eq!(boldness_text(160.0), "Bolder +2");
        assert_eq!(boldness_text(96.0), "Lighter -2");
    }
}
