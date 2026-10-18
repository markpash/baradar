//! Properties of an icon block (port of `editor/props/IconProps.svelte`): a searchable icon grid
//! with a category filter, and the icon size.

use baradar_core::doc::{IconItem, Item};
use baradar_core::render::icons::{ICON_CATEGORIES, ICON_VIEWBOX, ICONS, IconCategory, IconDef};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    input::{Input, InputEvent, InputState},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::PropertiesView;
use super::widgets::{Fields, add_item_size, hint, item_size_view, select};
use crate::studio::Studio;

/// Category names of the icon picker (`CATEGORY_LABEL`).
pub fn category_label(c: IconCategory) -> &'static str {
    match c {
        IconCategory::Electrical => "Electrical",
        IconCategory::Tools => "Tools",
        IconCategory::Arrows => "Arrows",
        IconCategory::Hazard => "Hazard",
        IconCategory::Home => "Home",
        IconCategory::Misc => "Misc",
    }
}

/// The icons shown for a query and category (`shown`): the label or a keyword contains the
/// trimmed, lower-cased query.
pub fn shown_icons(query: &str, category: Option<IconCategory>) -> Vec<&'static IconDef> {
    let q = query.trim().to_lowercase();
    ICONS
        .iter()
        .filter(|i| {
            category.is_none_or(|c| i.category == c)
                && (q.is_empty() || i.label.to_lowercase().contains(&q) || i.keywords.iter().any(|k| k.contains(&q)))
        })
        .collect()
}

pub struct IconEditor {
    pub fields: Fields,
    studio: Entity<Studio>,
    query: Entity<InputState>,
    category: Option<IconCategory>,
    _subs: Vec<Subscription>,
}

impl IconEditor {
    pub fn new(item: &IconItem, studio: &Entity<Studio>, window: &mut Window, cx: &mut Context<PropertiesView>) -> Self {
        let mut fields = Fields::default();
        add_item_size(&mut fields, &item.id, "Height", studio, window, cx);
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search icons"));
        let subs = vec![cx.subscribe(&query, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })];
        Self {
            fields,
            studio: studio.clone(),
            query,
            category: None,
            _subs: subs,
        }
    }

    pub fn render(&mut self, item: &IconItem, _window: &mut Window, cx: &mut Context<PropertiesView>) -> AnyElement {
        let theme = cx.theme().clone();
        let q = self.query.read(cx).value().to_string();
        let shown = shown_icons(&q, self.category);
        let max_mm = self.studio.read(cx).doc.tape.width_mm;

        let mut cats: Vec<(Option<IconCategory>, SharedString)> = vec![(None, "All".into())];
        cats.extend(
            ICON_CATEGORIES
                .iter()
                .filter(|c| ICONS.iter().any(|i| i.category == **c))
                .map(|c| (Some(*c), category_label(*c).into())),
        );
        let pick_cat = cx.listener(|this, c: &Option<IconCategory>, _, cx| {
            if let Some(super::ItemEditor::Icon(e)) = &mut this.editor {
                e.category = *c;
                cx.notify();
            }
        });
        let category = select("icon-category", cats, Some(self.category), move |c, w, cx| pick_cat(&c, w, cx));

        let grid = if shown.is_empty() {
            hint(format!("No icon matches “{q}”."), cx)
        } else {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(shown.into_iter().map(|icon| {
                    let selected = icon.id == item.icon_id;
                    let (studio, item_id, icon_id) = (self.studio.clone(), item.id.clone(), icon.id);
                    div()
                        .id(SharedString::from(format!("icon-{}", icon.id)))
                        .size(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .border_1()
                        .border_color(if selected { theme.primary } else { theme.border })
                        .when(selected, |d| d.bg(theme.primary.opacity(0.12)))
                        .hover(|d| d.bg(theme.muted))
                        .cursor_pointer()
                        .tooltip({
                            let label: SharedString = icon.label.into();
                            move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            studio.update(cx, |s, cx| {
                                s.update_item(
                                    &item_id,
                                    None,
                                    |i| {
                                        if let Item::Icon(ic) = i {
                                            ic.icon_id = icon_id.into();
                                        }
                                    },
                                    cx,
                                )
                            })
                        })
                        .child(icon_glyph(icon, 22.0, theme.foreground))
                        .test_support()
                }))
                .into_any_element()
        };

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.query).small().cleanable(true)))
                            .child(div().w(px(120.)).child(category)),
                    )
                    .child(div().id("icon-grid").max_h(px(220.)).overflow_y_scroll().p_0p5().child(grid)),
            )
            .child(item_size_view(&item.id, "Icon size", item.size, max_mm, &self.fields, &self.studio, cx))
            .into_any_element()
    }
}

/// An icon drawn from its 24×24 SVG paths, stroked like the studio's picker (stroke width 2).
pub fn icon_glyph(icon: &'static IconDef, side: f32, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let s = side / ICON_VIEWBOX;
            let origin = bounds.origin + point(px((f32::from(bounds.size.width) - side) / 2.), px((f32::from(bounds.size.height) - side) / 2.));
            for d in icon.paths {
                let mut b = PathBuilder::stroke(px(2.0 * s));
                if svg_path(d, s, origin, &mut b).is_some()
                    && let Ok(path) = b.build()
                {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(side))
    .into_any_element()
}

/// Tokens of an SVG path `d` string.
struct PathTokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> PathTokens<'a> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\n' | b'\t' | b'\r') {
            self.i += 1;
        }
    }
    /// The next command letter, if one comes next.
    fn command(&mut self) -> Option<u8> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }
    fn at_number(&mut self) -> bool {
        self.skip_sep();
        self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        let s = self.s;
        if matches!(s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(&c) = s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if matches!(s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            while s.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }
    /// An arc flag: a single `0` or `1`, which may be packed without separators.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Feeds an SVG path `d` (24×24 view box) into `b`, scaled by `scale` and moved to `origin`.
/// `None` on malformed data.
fn svg_path(d: &str, scale: f32, origin: Point<Pixels>, b: &mut PathBuilder) -> Option<()> {
    let mut t = PathTokens { s: d.as_bytes(), i: 0 };
    let at = |x: f32, y: f32| point(origin.x + px(x * scale), origin.y + px(y * scale));
    let (mut cx, mut cy, mut sx, mut sy) = (0f32, 0f32, 0f32, 0f32);
    // Last control point of a C/S (cubic) or Q/T (quadratic) segment, for reflection.
    let mut last_cubic: Option<(f32, f32)> = None;
    let mut last_quad: Option<(f32, f32)> = None;
    let mut cmd = t.command()?;
    let mut open = false;
    loop {
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.move_to(at(x, y));
                open = true;
                (cx, cy, sx, sy) = (x, y, x, y);
                last_cubic = None;
                last_quad = None;
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                if !t.at_number() {
                    match t.command() {
                        Some(c) => {
                            cmd = c;
                            continue;
                        }
                        None => break,
                    }
                }
                continue;
            }
            b'L' => {
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.line_to(at(x, y));
                (cx, cy) = (x, y);
                last_cubic = None;
                last_quad = None;
            }
            b'H' => {
                let x = t.number()? + ox;
                b.line_to(at(x, cy));
                cx = x;
                last_cubic = None;
                last_quad = None;
            }
            b'V' => {
                let y = t.number()? + oy;
                b.line_to(at(cx, y));
                cy = y;
                last_cubic = None;
                last_quad = None;
            }
            b'C' => {
                let (x1, y1) = (t.number()? + ox, t.number()? + oy);
                let (x2, y2) = (t.number()? + ox, t.number()? + oy);
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.cubic_bezier_to(at(x, y), at(x1, y1), at(x2, y2));
                last_cubic = Some((x2, y2));
                last_quad = None;
                (cx, cy) = (x, y);
            }
            b'S' => {
                let (x1, y1) = last_cubic.map_or((cx, cy), |(px_, py_)| (2.0 * cx - px_, 2.0 * cy - py_));
                let (x2, y2) = (t.number()? + ox, t.number()? + oy);
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.cubic_bezier_to(at(x, y), at(x1, y1), at(x2, y2));
                last_cubic = Some((x2, y2));
                last_quad = None;
                (cx, cy) = (x, y);
            }
            b'Q' => {
                let (x1, y1) = (t.number()? + ox, t.number()? + oy);
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.curve_to(at(x, y), at(x1, y1));
                last_quad = Some((x1, y1));
                last_cubic = None;
                (cx, cy) = (x, y);
            }
            b'T' => {
                let (x1, y1) = last_quad.map_or((cx, cy), |(px_, py_)| (2.0 * cx - px_, 2.0 * cy - py_));
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                b.curve_to(at(x, y), at(x1, y1));
                last_quad = Some((x1, y1));
                last_cubic = None;
                (cx, cy) = (x, y);
            }
            b'A' => {
                let (rx, ry) = (t.number()?, t.number()?);
                let rot = t.number()?;
                let (large, sweep) = (t.flag()?, t.flag()?);
                let (x, y) = (t.number()? + ox, t.number()? + oy);
                if rx.abs() < 1e-6 || ry.abs() < 1e-6 {
                    b.line_to(at(x, y));
                } else {
                    b.arc_to(point(px(rx.abs() * scale), px(ry.abs() * scale)), px(rot), large, sweep, at(x, y));
                }
                last_cubic = None;
                last_quad = None;
                (cx, cy) = (x, y);
            }
            b'Z' => {
                if open {
                    b.close();
                    open = false;
                }
                (cx, cy) = (sx, sy);
                last_cubic = None;
                last_quad = None;
                match t.command() {
                    Some(c) => {
                        cmd = c;
                        continue;
                    }
                    None => break,
                }
            }
            _ => return None,
        }
        if !t.at_number() {
            match t.command() {
                Some(c) => cmd = c,
                None => break,
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::{category_label, shown_icons};
    use baradar_core::render::icons::{ICONS, IconCategory};

    #[test]
    fn search_and_filter() {
        assert_eq!(shown_icons("", None).len(), ICONS.len());
        assert!(shown_icons("BOLT", None).iter().any(|i| i.id == "bolt"));
        assert!(shown_icons("", Some(IconCategory::Tools)).iter().all(|i| i.category == IconCategory::Tools));
        assert!(shown_icons("zzzz-nothing", None).is_empty());
        assert_eq!(category_label(IconCategory::Misc), "Misc");
    }
}
