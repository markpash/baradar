//! Properties of a shape block (port of `editor/props/ShapeProps.svelte`): box, ellipse or line,
//! its length and stroke, and (not for lines) its height and fill.

use baradar_core::doc::{Item, ShapeItem, ShapeKind};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PropertiesView;
use super::widgets::{
    Fields, NumSpec, add_item_size, item_num_get, item_num_set, item_size_view, num_view, segmented_field,
    switch_view,
};
use crate::studio::Studio;

pub struct ShapeEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
}

impl ShapeEditor {
    pub fn new(item: &ShapeItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        let id = item.id.as_str();
        fields.add_num(
            "width",
            NumSpec::new("Length", "mm", 0.5, 500.0, 0.5),
            studio,
            item_num_get(id, |i| match i {
                Item::Shape(s) => Some(s.width_mm),
                _ => None,
            }),
            item_num_set(id, "width", |i, v| {
                if let Item::Shape(s) = i {
                    s.width_mm = v;
                }
            }),
            window,
            cx,
        );
        fields.add_num(
            "stroke",
            NumSpec::new("Stroke", "mm", 0.1, 10.0, 0.1),
            studio,
            item_num_get(id, |i| match i {
                Item::Shape(s) => Some(s.stroke_mm),
                _ => None,
            }),
            item_num_set(id, "stroke", |i, v| {
                if let Item::Shape(s) = i {
                    s.stroke_mm = v;
                }
            }),
            window,
            cx,
        );
        add_item_size(&mut fields, id, "Height", studio, window, cx);
        Self { fields, studio: studio.clone() }
    }

    pub fn render(&mut self, item: &ShapeItem, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let max_mm = self.studio.read(cx).doc.tape.width_mm;
        let (s1, s2) = (self.studio.clone(), self.studio.clone());
        let (id1, id2) = (item.id.clone(), item.id.clone());
        v_flex()
            .gap_3()
            .child(segmented_field(
                "shape-kind",
                "Shape",
                &[(ShapeKind::Rect, "Box"), (ShapeKind::Ellipse, "Ellipse"), (ShapeKind::Line, "Line")],
                Some(item.shape),
                move |shape, _, cx| {
                    s1.update(cx, |s, cx| {
                        s.update_item(
                            &id1,
                            None,
                            |i| {
                                if let Item::Shape(sh) = i {
                                    sh.shape = shape;
                                }
                            },
                            cx,
                        )
                    })
                },
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(num_view(self.fields.num("width"), None, cx)))
                    .child(div().flex_1().min_w_0().child(num_view(self.fields.num("stroke"), None, cx))),
            )
            .when(item.shape != ShapeKind::Line, |d| {
                d.child(item_size_view(&item.id, "Height", item.size, max_mm, &self.fields, &self.studio, cx))
                    .child(switch_view(
                        "shape-fill",
                        "Filled",
                        item.fill,
                        None,
                        move |fill, _, cx| {
                            s2.update(cx, |s, cx| {
                                s.update_item(
                                    &id2,
                                    None,
                                    |i| {
                                        if let Item::Shape(sh) = i {
                                            sh.fill = fill;
                                        }
                                    },
                                    cx,
                                )
                            })
                        },
                        cx,
                    ))
            })
            .into_any_element()
    }
}
