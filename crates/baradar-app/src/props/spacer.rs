//! Properties of a spacer block (port of `editor/props/SpacerProps.svelte`): its width.

use baradar_core::doc::{Item, SpacerItem};
use gpui_kit::*;

use super::PropertiesView;
use super::widgets::{Fields, NumSpec, item_num_get, item_num_set, num_view};
use crate::studio::Studio;

pub struct SpacerEditor {
    pub fields: Fields,
}

impl SpacerEditor {
    pub fn new(item: &SpacerItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        fields.add_num(
            "width",
            NumSpec::new("Width", "mm", 0.0, 500.0, 0.5),
            studio,
            item_num_get(&item.id, |i| match i {
                Item::Spacer(s) => Some(s.width_mm),
                _ => None,
            }),
            item_num_set(&item.id, "width", |i, v| {
                if let Item::Spacer(s) = i {
                    s.width_mm = v;
                }
            }),
            window,
            cx,
        );
        Self { fields }
    }

    pub fn render(&mut self, _item: &SpacerItem, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        num_view(
            self.fields.num("width"),
            Some("Blank space between the blocks on either side.".into()),
            cx,
        )
    }
}
