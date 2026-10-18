//! Bundled icon set and icon drawing (port of `web/src/render/icons.ts`, `icon-set.ts` and the
//! icon part of `renderer.ts` `prepareIcon`).
//!
//! The icons are a subset of Lucide 1.52.0 (<https://lucide.dev>, ISC); some shapes derive from
//! Feather (MIT). Every element is path data in a 24×24 box, stroke-only (Lucide draws them with
//! stroke-width 2, round caps and joins). The data lives in `icon_data.rs`, generated from
//! `icons.ts`. The catalogue is compiled in, so the studio's lazy loading (`icon-set.ts`) and its
//! `icons-missing` failure have no native counterpart.
//!
//! ISC License — Copyright (c) 2026 Lucide Icons and Contributors
//! Permission to use, copy, modify, and/or distribute this software for any purpose with or
//! without fee is hereby granted, provided that the above copyright notice and this permission
//! notice appear in all copies.
//! THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
//! SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
//! AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
//! WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
//! NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
//! PERFORMANCE OF THIS SOFTWARE.
//!
//! Feather-derived icons used here (arrow-*, calendar, check, clock, corner-down-right, info, key,
//! lock, minus, monitor, move, music, plus, power, server, x): MIT License, Copyright (c)
//! 2013-present Cole Bemis. Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the "Software"), to deal in the
//! Software without restriction, including without limitation the rights to use, copy, modify,
//! merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
//! persons to whom the Software is furnished to do so, subject to the following conditions: The
//! above copyright notice and this permission notice shall be included in all copies or
//! substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
//! KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
//! COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
//! CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE
//! OR OTHER DEALINGS IN THE SOFTWARE.

use std::sync::OnceLock;

use kurbo::{BezPath, PathEl};
use tiny_skia::{LineCap, LineJoin, Mask, Paint, Path, PathBuilder, PixmapMut, Stroke, Transform};

use crate::doc::ItemSize;
use crate::render::units::mm_to_dots;

/// Icon picker category (`IconCategory`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconCategory {
    Electrical,
    Tools,
    Arrows,
    Hazard,
    Home,
    Misc,
}

impl IconCategory {
    /// The id used in `icons.ts` (`'electrical'`, …).
    pub fn id(self) -> &'static str {
        match self {
            Self::Electrical => "electrical",
            Self::Tools => "tools",
            Self::Arrows => "arrows",
            Self::Hazard => "hazard",
            Self::Home => "home",
            Self::Misc => "misc",
        }
    }

    /// The picker label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Electrical => "Electrical & IT",
            Self::Tools => "Tools",
            Self::Arrows => "Arrows",
            Self::Hazard => "Safety",
            Self::Home => "Home",
            Self::Misc => "Other",
        }
    }
}

/// Category order for the icon picker (`ICON_CATEGORIES`).
pub const ICON_CATEGORIES: [IconCategory; 6] = [
    IconCategory::Electrical,
    IconCategory::Tools,
    IconCategory::Arrows,
    IconCategory::Hazard,
    IconCategory::Home,
    IconCategory::Misc,
];

/// One bundled icon (`IconDef`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconDef {
    pub id: &'static str,
    pub label: &'static str,
    pub category: IconCategory,
    /// SVG path data (one or more `d` strings) in a 24×24 box, stroked (never filled).
    pub paths: &'static [&'static str],
    /// Searchable keywords.
    pub keywords: &'static [&'static str],
    /// Upstream Lucide icon name.
    pub lucide: Option<&'static str>,
}

/// Width (and height) of the icon view box; paths are in this coordinate space (`ICON_VIEWBOX`).
pub const ICON_VIEWBOX: f32 = 24.0;
/// Lucide's stroke width in view-box units (`ICON_STROKE`).
pub const ICON_STROKE: f32 = 2.0;
/// The icon drawn for an unknown id.
pub const FALLBACK_ICON_ID: &str = "question";

include!("icon_data.rs");

/// The icon with `id` (`iconById`).
pub fn icon_by_id(id: &str) -> Option<&'static IconDef> {
    ICONS.iter().find(|i| i.id == id)
}

/// The icon with `id`, or the `question` icon for an unknown id (`prepareIcon`).
pub fn icon_or_fallback(id: &str) -> &'static IconDef {
    icon_by_id(id)
        .or_else(|| icon_by_id(FALLBACK_ICON_ID))
        .unwrap_or(&ICONS[ICONS.len() - 1])
}

/// Case-insensitive search over id, label and keywords; every term must match. An empty query
/// returns every icon (`searchIcons`).
pub fn search_icons(query: &str) -> Vec<&'static IconDef> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return ICONS.iter().collect();
    }
    ICONS
        .iter()
        .filter(|i| {
            let hay = format!(
                "{} {} {}",
                i.id,
                i.label.to_lowercase(),
                i.keywords.join(" ")
            );
            q.split_whitespace().all(|t| hay.contains(t))
        })
        .collect()
}

/// Warning when a fixed icon size is taller than the band (`prepareIcon`).
pub const ICON_REDUCED_WARNING: &str =
    "The icon is larger than the printable area of this tape and was reduced.";

/// Side of the square icon box in dots and whether it was reduced to the band (`prepareIcon`):
/// fit = `min(band, maxW ?? band)`, else `mmToDots(mm)`; clamped to the band, at least 1.
pub fn icon_side_dots(size: &ItemSize, band: i64, max_w: Option<i64>, dpi: u16) -> (i64, bool) {
    let mut side = match size {
        ItemSize::Fit => band.min(max_w.unwrap_or(band)),
        ItemSize::Mm { mm } => mm_to_dots(*mm, dpi),
    };
    let reduced = side > band;
    if reduced {
        side = band;
    }
    (side.max(1), reduced)
}

// ------------------------------------------------------------------------------------------
// Paths
// ------------------------------------------------------------------------------------------

/// Converts a kurbo path to a tiny-skia path (`None` when it has no drawable segment).
pub fn bez_to_skia(bez: &BezPath) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let p = |pt: kurbo::Point| (pt.x as f32, pt.y as f32);
    for el in bez.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                let (x, y) = p(a);
                pb.move_to(x, y);
            }
            PathEl::LineTo(a) => {
                let (x, y) = p(a);
                pb.line_to(x, y);
            }
            PathEl::QuadTo(a, b) => {
                let ((x1, y1), (x, y)) = (p(a), p(b));
                pb.quad_to(x1, y1, x, y);
            }
            PathEl::CurveTo(a, b, c) => {
                let ((x1, y1), (x2, y2), (x, y)) = (p(a), p(b), p(c));
                pb.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathEl::ClosePath => pb.close(),
        }
    }
    pb.finish()
}

/// Parses an SVG path `d` string (kurbo `BezPath::from_svg`) into a tiny-skia path.
pub fn svg_path(d: &str) -> Option<Path> {
    BezPath::from_svg(d).ok().as_ref().and_then(bez_to_skia)
}

static PARSED: OnceLock<Vec<Vec<Path>>> = OnceLock::new();

/// The icon's paths in view-box units, parsed once per process (`iconPaths`). Unparseable path
/// strings are skipped.
pub fn icon_paths(icon: &IconDef) -> &'static [Path] {
    let all = PARSED.get_or_init(|| {
        ICONS
            .iter()
            .map(|i| i.paths.iter().filter_map(|d| svg_path(d)).collect())
            .collect()
    });
    ICONS
        .iter()
        .position(|i| i.id == icon.id)
        .and_then(|n| all.get(n))
        .map_or(&[], Vec::as_slice)
}

// ------------------------------------------------------------------------------------------
// Drawing
// ------------------------------------------------------------------------------------------

/// Strokes `icon` centred in a `bw × bh` px box at the origin of `transform` (the crisp draw of
/// `prepareIcon`): scale `s = side·f/24`, round caps and joins, line width `max(2, 2f/s)` view-box
/// units, so never thinner than 2 printer dots. Black ink; each path is stroked separately, like
/// `ctx.stroke(path)` per `Path2D`.
#[allow(clippy::too_many_arguments)]
pub fn draw_icon(
    pixmap: &mut PixmapMut<'_>,
    transform: Transform,
    clip: Option<&Mask>,
    icon: &IconDef,
    side_dots: i64,
    bw: f32,
    bh: f32,
    f: u32,
) {
    let side_px = side_dots as f32 * f as f32;
    let s = side_px / ICON_VIEWBOX;
    if s.is_nan() || s <= 0.0 {
        return;
    }
    let t = transform
        .pre_translate((bw - side_px) / 2.0, (bh - side_px) / 2.0)
        .pre_scale(s, s);
    let stroke = Stroke {
        width: ICON_STROKE.max(2.0 * f as f32 / s),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let paint = black();
    for path in icon_paths(icon) {
        pixmap.stroke_path(path, &paint, &stroke, t, clip);
    }
}

/// Opaque black, anti-aliased (the studio's `'#000'` on the crisp canvas).
pub(crate) fn black() -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    paint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue() {
        assert_eq!(ICONS.len(), 97);
        assert_eq!(icon_by_id("question").map(|i| i.label), Some("Question"));
        assert_eq!(icon_or_fallback("nope").id, "question");
        assert_eq!(icon_or_fallback("bolt").lucide, Some("zap"));
        let mut ids: Vec<&str> = ICONS.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ICONS.len(), "ids are unique");
        for c in ICON_CATEGORIES {
            assert!(ICONS.iter().any(|i| i.category == c), "{}", c.id());
        }
    }

    #[test]
    fn every_path_parses() {
        for icon in ICONS {
            for d in icon.paths {
                assert!(svg_path(d).is_some(), "{}: {d}", icon.id);
            }
            assert_eq!(icon_paths(icon).len(), icon.paths.len());
        }
    }

    #[test]
    fn search() {
        assert_eq!(search_icons("").len(), ICONS.len());
        assert_eq!(search_icons("  ").len(), ICONS.len());
        assert!(search_icons("POWER").iter().any(|i| i.id == "bolt"));
        assert!(
            search_icons("help question")
                .iter()
                .any(|i| i.id == "question")
        );
        assert!(search_icons("zzzz-not-an-icon").is_empty());
    }

    #[test]
    fn side_rule() {
        assert_eq!(icon_side_dots(&ItemSize::Fit, 128, None, 180), (128, false));
        assert_eq!(
            icon_side_dots(&ItemSize::Fit, 128, Some(40), 180),
            (40, false)
        );
        assert_eq!(
            icon_side_dots(&ItemSize::Mm { mm: 10.0 }, 128, None, 180),
            (71, false)
        );
        assert_eq!(
            icon_side_dots(&ItemSize::Mm { mm: 30.0 }, 128, None, 180),
            (128, true)
        );
        assert_eq!(
            icon_side_dots(&ItemSize::Mm { mm: 0.0 }, 128, None, 180),
            (1, false)
        );
    }

    #[test]
    fn arcs_are_accurate() {
        // A full circle of radius 10 (two arcs): kurbo flattens arcs at 0.1 tolerance; check the
        // real deviation is far below a printer dot at the largest icon size (~0.06 units/px).
        use kurbo::ParamCurve;
        let mut worst = 0f32;
        let bez = BezPath::from_svg("M2 12a10 10 0 1 0 20 0a10 10 0 1 0 -20 0").unwrap();
        for seg in bez.segments() {
            for i in 0..=20 {
                let pt = seg.eval(f64::from(i) / 20.0);
                let r = ((pt.x - 12.0).powi(2) + (pt.y - 12.0).powi(2)).sqrt();
                worst = worst.max((r - 10.0).abs() as f32);
            }
        }
        assert!(worst < 0.02, "worst radial error {worst}");
    }
}
