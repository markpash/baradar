//! Shapes, the label frame and the invalid-code box on the crisp plane (port of the shape and
//! frame parts of `web/src/render/renderer.ts`: `prepareShape`, `invalidCodeDraw`, `frameGeom`,
//! `itemSizingBand`, `drawFrame`).
//!
//! Drawing functions paint black, anti-aliased ink onto a white `tiny_skia` pixmap in crisp-plane
//! pixels (dots × f), inside a `bw × bh` px box at the origin of `transform`, like the studio's
//! `CrispDraw` callbacks on a Canvas2D context. Strokes follow Canvas2D defaults unless the source
//! sets otherwise: butt caps, miter joins with a miter limit of 10.

use kurbo::{RoundedRect, Shape};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, PathBuilder, PixmapMut, Rect, Stroke, StrokeDash, Transform,
};

use crate::doc::{Item, ItemSize, LabelDoc, LabelFrame, LayoutSettings, ShapeItem, ShapeKind};
use crate::render::codes::tape_margin_dots;
use crate::render::icons::{bez_to_skia, black};
use crate::render::units::{js_round, mm_to_dots};

/// Canvas2D's default `miterLimit`.
const CANVAS_MITER_LIMIT: f32 = 10.0;

fn canvas_stroke(width: f32) -> Stroke {
    Stroke {
        width,
        miter_limit: CANVAS_MITER_LIMIT,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        dash: None,
    }
}

// ------------------------------------------------------------------------------------------
// Label frame
// ------------------------------------------------------------------------------------------

/// Label frame geometry in dots (`FrameGeom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGeom {
    pub inset: i64,
    pub thickness: i64,
    pub radius: i64,
    /// Space reserved for the frame on every side (inset + thickness + padding).
    pub reserve: i64,
}

/// The frame geometry for a `band`-tall label, or `None` without a frame line (`frameGeom`):
/// `thickness = max(1, mmToDots(t))`, `pad = max(1, min(mmToDots(1), round(band·0.06)))`,
/// `reserve = inset + thickness + pad`.
pub fn frame_geom(frame: Option<&LabelFrame>, band: i64, dpi: u16) -> Option<FrameGeom> {
    let fr = frame?;
    if fr.thickness_mm.is_nan() || fr.thickness_mm <= 0.0 {
        return None;
    }
    let thickness = mm_to_dots(fr.thickness_mm, dpi).max(1);
    let inset = mm_to_dots(fr.inset_mm, dpi).max(0);
    let radius = mm_to_dots(fr.radius_mm, dpi).max(0);
    let pad = mm_to_dots(1.0, dpi)
        .min(js_round(band as f64 * 0.06))
        .max(1);
    Some(FrameGeom {
        inset,
        thickness,
        radius,
        reserve: inset + thickness + pad,
    })
}

/// The band an item is sized in (`itemSizingBand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizingBand {
    pub band_dots: i64,
    /// Unprinted tape a code may count as quiet zone.
    pub margin_dots: i64,
    /// The item sits in a free-layout frame.
    pub framed: bool,
}

/// The band an item is sized in and the tape margin a code may use as quiet zone, by the
/// renderer's rules (`itemSizingBand`): flow items get the band inside the label frame and the
/// margin only without one; free-layout items in a frame get the frame's height (its width when
/// turned a quarter) and no margin.
pub fn item_sizing_band(
    doc: &LabelDoc,
    item: &Item,
    height_dots: i64,
    tape_width_dots: i64,
    dpi: u16,
) -> SizingBand {
    let fr = if matches!(doc.layout, LayoutSettings::Free) {
        item.frame()
    } else {
        None
    };
    if let Some(fr) = fr {
        let turned = matches!(fr.rotation, 90 | 270);
        return SizingBand {
            band_dots: mm_to_dots(if turned { fr.w_mm } else { fr.h_mm }, dpi).max(1),
            margin_dots: 0,
            framed: true,
        };
    }
    let frame = frame_geom(doc.frame.as_ref(), height_dots, dpi);
    SizingBand {
        band_dots: (height_dots - 2 * frame.map_or(0, |g| g.reserve)).max(1),
        margin_dots: if frame.is_some() {
            0
        } else {
            tape_margin_dots(tape_width_dots, height_dots)
        },
        framed: false,
    }
}

/// Strokes the label frame (`drawFrame`): a line `thickness·f` px wide centred on
/// `inset·f + t/2`, `((length − 2·inset)·f − t) × ((band − 2·inset)·f − t)` px, corner radius
/// `min(r·f, w/2, h/2)` (square corners with miter joins at radius 0). Nothing when too small.
pub fn draw_frame(
    pixmap: &mut PixmapMut<'_>,
    transform: Transform,
    clip: Option<&Mask>,
    g: &FrameGeom,
    length: i64,
    band: i64,
    f: u32,
) {
    let f = f64::from(f);
    let t = g.thickness as f64 * f;
    let x = g.inset as f64 * f + t / 2.0;
    let y = x;
    let w = (length - 2 * g.inset) as f64 * f - t;
    let h = (band - 2 * g.inset) as f64 * f - t;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = (g.radius as f64 * f).min(w / 2.0).min(h / 2.0);
    let path = if r > 0.0 {
        bez_to_skia(&RoundedRect::new(x, y, x + w, y + h, r).to_path(0.01))
    } else {
        Rect::from_xywh(x as f32, y as f32, w as f32, h as f32).map(PathBuilder::from_rect)
    };
    if let Some(path) = path {
        pixmap.stroke_path(&path, &black(), &canvas_stroke(t as f32), transform, clip);
    }
}

// ------------------------------------------------------------------------------------------
// Shapes
// ------------------------------------------------------------------------------------------

/// Warning when a fixed shape height is taller than the band (`prepareShape`).
pub const SHAPE_REDUCED_WARNING: &str =
    "The shape is taller than the printable area of this tape and was reduced.";

/// A shape's box in dots (`prepareShape`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeSize {
    pub w: i64,
    pub h: i64,
    /// `max(1, mmToDots(strokeMm))`.
    pub stroke_dots: i64,
    /// A fixed height was reduced to the band (raise [`SHAPE_REDUCED_WARNING`]).
    pub reduced: bool,
}

/// The size rule of `prepareShape`: `h = fit ? band : max(1, mmToDots(mm))` clamped to the band,
/// `w = max(strokeDots, mmToDots(widthMm))`.
pub fn shape_size(item: &ShapeItem, band: i64, dpi: u16) -> ShapeSize {
    let stroke_dots = mm_to_dots(item.stroke_mm, dpi).max(1);
    let mut h = match item.size {
        ItemSize::Fit => band,
        ItemSize::Mm { mm } => mm_to_dots(mm, dpi).max(1),
    };
    let reduced = h > band;
    if reduced {
        h = band;
    }
    ShapeSize {
        w: stroke_dots.max(mm_to_dots(item.width_mm, dpi)),
        h,
        stroke_dots,
        reduced,
    }
}

/// Canvas `strokeRect(x, y, w, h)`: a closed rectangle, a two-point line when one side is 0,
/// nothing when both are.
fn stroke_rect(
    pixmap: &mut PixmapMut<'_>,
    transform: Transform,
    clip: Option<&Mask>,
    (x, y, w, h): (f32, f32, f32, f32),
    stroke: &Stroke,
) {
    let mut pb = PathBuilder::new();
    if w == 0.0 && h == 0.0 {
        return;
    } else if w == 0.0 || h == 0.0 {
        pb.move_to(x, y);
        pb.line_to(x + w, y + h);
    } else {
        pb.move_to(x, y);
        pb.line_to(x + w, y);
        pb.line_to(x + w, y + h);
        pb.line_to(x, y + h);
        pb.close();
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &black(), stroke, transform, clip);
    }
}

/// Draws a rect / ellipse / line in a `bw × bh` px box (the crisp draw of `prepareShape`).
/// Stroke width `s = min(strokeDots·f, bw, bh)`. Line: horizontal through the middle when
/// `bw ≥ bh`, else vertical, butt caps. Rect: `strokeRect(s/2, s/2, bw − s, bh − s)` or a filled
/// box. Ellipse: radii `bw/2 − s/2`, `bh/2 − s/2`, or filled to the box.
#[allow(clippy::too_many_arguments)]
pub fn draw_shape(
    pixmap: &mut PixmapMut<'_>,
    transform: Transform,
    clip: Option<&Mask>,
    shape: ShapeKind,
    fill: bool,
    stroke_dots: i64,
    bw: f32,
    bh: f32,
    f: u32,
) {
    let s = (stroke_dots as f32 * f as f32).min(bw).min(bh);
    let stroke = canvas_stroke(s);
    let paint = black();
    match shape {
        ShapeKind::Line => {
            let mut pb = PathBuilder::new();
            if bw >= bh {
                pb.move_to(0.0, bh / 2.0);
                pb.line_to(bw, bh / 2.0);
            } else {
                pb.move_to(bw / 2.0, 0.0);
                pb.line_to(bw / 2.0, bh);
            }
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, transform, clip);
            }
        }
        ShapeKind::Ellipse => {
            let (rx, ry) = if fill {
                (bw / 2.0, bh / 2.0)
            } else {
                ((bw / 2.0 - s / 2.0).max(0.0), (bh / 2.0 - s / 2.0).max(0.0))
            };
            let oval = Rect::from_ltrb(bw / 2.0 - rx, bh / 2.0 - ry, bw / 2.0 + rx, bh / 2.0 + ry)
                .and_then(PathBuilder::from_oval);
            if let Some(path) = oval {
                if fill {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, clip);
                } else {
                    pixmap.stroke_path(&path, &paint, &stroke, transform, clip);
                }
            }
        }
        ShapeKind::Rect => {
            if fill {
                if let Some(r) = Rect::from_xywh(0.0, 0.0, bw, bh) {
                    pixmap.fill_rect(r, &paint, transform, clip);
                }
            } else {
                let rect = (s / 2.0, s / 2.0, (bw - s).max(0.0), (bh - s).max(0.0));
                stroke_rect(pixmap, transform, clip, rect, &stroke);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Invalid code
// ------------------------------------------------------------------------------------------

/// The crossed box shown where a code cannot be encoded (`invalidCodeDraw`): a 1-dot line dashed
/// `[2f, 2f]` around the box inset by half a dot, plus both diagonals.
pub fn draw_invalid_code(
    pixmap: &mut PixmapMut<'_>,
    transform: Transform,
    clip: Option<&Mask>,
    bw: f32,
    bh: f32,
    f: u32,
) {
    let f = f as f32;
    let mut stroke = canvas_stroke(f);
    stroke.dash = StrokeDash::new(vec![2.0 * f, 2.0 * f], 0.0);
    stroke_rect(
        pixmap,
        transform,
        clip,
        (f / 2.0, f / 2.0, bw - f, bh - f),
        &stroke,
    );
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.line_to(bw, bh);
    pb.move_to(bw, 0.0);
    pb.line_to(0.0, bh);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &black(), &stroke, transform, clip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ItemKind, LabelDoc};
    use tiny_skia::Pixmap;

    fn white(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        p.fill(tiny_skia::Color::WHITE);
        p
    }

    fn ink(p: &Pixmap, x: u32, y: u32) -> bool {
        p.pixel(x, y).unwrap().red() < 128
    }

    #[test]
    fn frame_geometry() {
        let fr = LabelFrame::UI_DEFAULT;
        assert_eq!(
            frame_geom(Some(&fr), 128, 180),
            Some(FrameGeom {
                inset: 4,
                thickness: 4,
                radius: 7,
                reserve: 15
            })
        );
        assert_eq!(frame_geom(Some(&fr), 70, 180).map(|g| g.reserve), Some(12));
        assert_eq!(frame_geom(None, 128, 180), None);
        let none = LabelFrame {
            thickness_mm: 0.0,
            ..fr
        };
        assert_eq!(frame_geom(Some(&none), 128, 180), None);
        let thin = LabelFrame {
            thickness_mm: 0.01,
            ..fr
        };
        assert_eq!(
            frame_geom(Some(&thin), 128, 180).map(|g| g.thickness),
            Some(1)
        );
    }

    #[test]
    fn sizing_band() {
        let mut doc = LabelDoc::new();
        let item = Item::new(ItemKind::Code);
        assert_eq!(
            item_sizing_band(&doc, &item, 128, 170, 180),
            SizingBand {
                band_dots: 128,
                margin_dots: 21,
                framed: false
            }
        );
        doc.frame = Some(LabelFrame::UI_DEFAULT);
        assert_eq!(
            item_sizing_band(&doc, &item, 128, 170, 180),
            SizingBand {
                band_dots: 98,
                margin_dots: 0,
                framed: false
            }
        );
    }

    #[test]
    fn frame_stroke() {
        let g = FrameGeom {
            inset: 0,
            thickness: 2,
            radius: 0,
            reserve: 3,
        };
        let mut p = white(300, 192);
        draw_frame(&mut p.as_mut(), Transform::identity(), None, &g, 100, 64, 3);
        assert!(ink(&p, 1, 96) && ink(&p, 5, 96) && !ink(&p, 7, 96));
        assert!(ink(&p, 150, 0) && ink(&p, 150, 191) && ink(&p, 299, 96));
        assert!(!ink(&p, 150, 96));
        // Rounded corners leave the very corner blank.
        let g = FrameGeom { radius: 6, ..g };
        let mut p = white(300, 192);
        draw_frame(&mut p.as_mut(), Transform::identity(), None, &g, 100, 64, 3);
        assert!(!ink(&p, 0, 0) && ink(&p, 150, 1));
    }

    #[test]
    fn shapes() {
        let item = ShapeItem {
            id: "s".into(),
            frame: None,
            shape: ShapeKind::Rect,
            width_mm: 10.0,
            size: ItemSize::Fit,
            stroke_mm: 0.5,
            fill: false,
        };
        assert_eq!(
            shape_size(&item, 70, 180),
            ShapeSize {
                w: 71,
                h: 70,
                stroke_dots: 4,
                reduced: false
            }
        );
        let tall = ShapeItem {
            size: ItemSize::Mm { mm: 20.0 },
            ..item.clone()
        };
        assert_eq!(shape_size(&tall, 70, 180).h, 70);
        assert!(shape_size(&tall, 70, 180).reduced);
        let narrow = ShapeItem {
            width_mm: 0.0,
            ..item
        };
        assert_eq!(shape_size(&narrow, 70, 180).w, 4);

        // Rect outline: 2 dots × 3 = 6 px wide, inside the box.
        let mut p = white(60, 30);
        draw_shape(
            &mut p.as_mut(),
            Transform::identity(),
            None,
            ShapeKind::Rect,
            false,
            2,
            60.0,
            30.0,
            3,
        );
        assert!(ink(&p, 0, 0) && ink(&p, 5, 15) && !ink(&p, 7, 15) && ink(&p, 59, 29));
        assert!(!ink(&p, 30, 15));
        // Filled ellipse: centre inked, corners blank.
        let mut p = white(60, 30);
        draw_shape(
            &mut p.as_mut(),
            Transform::identity(),
            None,
            ShapeKind::Ellipse,
            true,
            2,
            60.0,
            30.0,
            3,
        );
        assert!(ink(&p, 30, 15) && !ink(&p, 0, 0) && !ink(&p, 59, 29));
        // Line: horizontal through the middle when wider than tall.
        let mut p = white(60, 30);
        draw_shape(
            &mut p.as_mut(),
            Transform::identity(),
            None,
            ShapeKind::Line,
            false,
            2,
            60.0,
            30.0,
            3,
        );
        assert!(ink(&p, 0, 15) && ink(&p, 59, 13) && !ink(&p, 30, 10) && !ink(&p, 30, 19));
        // A rect as wide as its stroke degenerates to a line (canvas strokeRect with w = 0).
        let mut p = white(6, 30);
        draw_shape(
            &mut p.as_mut(),
            Transform::identity(),
            None,
            ShapeKind::Rect,
            false,
            2,
            6.0,
            30.0,
            3,
        );
        assert!(ink(&p, 3, 15) && ink(&p, 0, 15) && !ink(&p, 3, 1));
    }

    #[test]
    fn invalid_box_is_dashed_and_crossed() {
        let mut p = white(60, 60);
        draw_invalid_code(&mut p.as_mut(), Transform::identity(), None, 60.0, 60.0, 3);
        // Top edge: dash 0–6 px, gap 6–12 px.
        assert!(ink(&p, 3, 1) && !ink(&p, 9, 1));
        // Both diagonals, dashed from their start: ink 12–18 px along, gap 6–12 px.
        assert!(ink(&p, 10, 10) && ink(&p, 49, 10));
        assert!(!ink(&p, 6, 6) && !ink(&p, 53, 6));
        assert!(!ink(&p, 30, 10));
    }
}
