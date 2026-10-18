//! Pure layout: items + measured sizes → positions in dots (port of `web/src/render/layout.ts`).
//!
//! Coordinates are those of the printable band: x along the label (0 = left end as read), y
//! across the tape (0 = top edge of the printable band).
//!
//! Flow rules (`layout.mode = 'flow'`):
//! - Items run left → right from the start margin, separated by `gapMm`.
//! - A spacer *replaces* the gap on both sides: `A · spacer · B` is A, spacer width, B.
//! - Items with zero width (empty text) take no space and no gap.
//! - Cross axis: each item is aligned in the band by `align` (start = top, end = bottom).
//! - Auto length = start margin + content + end margin. Fixed length = that length; content is
//!   centred between the margins, or starts at the start margin and overflows.
//! - The length is clamped to the printer's `[min, max]`; padded up to the minimum the content
//!   is centred, cut to the maximum it overflows.
//! - A label frame insets the content on all sides by `inset_dots`.

use crate::doc::{Align, LabelDoc, LayoutSettings, LengthSettings};
use crate::render::units::{js_round, mm_to_dots};

/// An item's natural size (`MeasuredItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredItem {
    pub item_id: String,
    /// Natural width along the label, dots.
    pub w: i64,
    /// Height across the tape, dots (≤ band height).
    pub h: i64,
    /// Spacers replace the gaps next to them.
    pub spacer: bool,
}

/// A placed item (`Placement`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub item_id: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub spacer: bool,
}

/// Why the length was clamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clamped {
    /// Raised to the printer minimum.
    Min,
    /// Cut to the printer maximum.
    Max,
}

/// Result of [`layout_flow`] / [`layout_free`] (`FlowLayout`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub placements: Vec<Placement>,
    /// Label length in dots (auto) or the fixed length, after clamping.
    pub length_dots: i64,
    /// Content exceeds a fixed length (or the printer maximum).
    pub overflow: bool,
    pub clamped: Option<Clamped>,
    /// Extent of the content along the label (items + gaps), dots.
    pub content_dots: i64,
}

/// Printer limits and the frame inset (`LayoutLimits`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutLimits {
    /// Shortest page (`PrintArea.minLengthDots`).
    pub min_length_dots: i64,
    /// Longest page, if limited.
    pub max_length_dots: Option<i64>,
    /// Space reserved on every side for the label frame, dots.
    pub inset_dots: i64,
}

/// `alignOffset`: offset of `inner` inside `outer`.
pub fn align_offset(align: Align, outer: i64, inner: i64) -> i64 {
    match align {
        Align::Start => 0,
        Align::End => outer - inner,
        Align::Center => js_round((outer - inner) as f64 / 2.0),
    }
}

/// `flowSettings`: gap (dots) and cross-axis alignment of a document's layout.
pub fn flow_settings(doc: &LabelDoc, dpi: u16) -> (i64, Align) {
    match &doc.layout {
        LayoutSettings::Flow { gap_mm, align } => (mm_to_dots(*gap_mm, dpi).max(0), *align),
        LayoutSettings::Free => (0, Align::Center),
    }
}

/// `contentLength`: item widths plus the gaps between them (spacer and empty-item rules).
pub fn content_length(items: &[MeasuredItem], gap: i64) -> i64 {
    let mut total = 0;
    let mut prev: Option<&MeasuredItem> = None;
    for it in items {
        if it.w <= 0 && !it.spacer {
            continue;
        }
        if prev.is_some_and(|p| !p.spacer) && !it.spacer {
            total += gap;
        }
        total += it.w.max(0);
        prev = Some(it);
    }
    total
}

fn clamp_length(len: i64, limits: &LayoutLimits) -> (i64, Option<Clamped>) {
    let min = limits.min_length_dots.max(1);
    if len < min {
        return (min, Some(Clamped::Min));
    }
    match limits.max_length_dots {
        Some(max) if len > max => (max, Some(Clamped::Max)),
        _ => (len, None),
    }
}

fn fixed_length_dots(doc: &LabelDoc, dpi: u16) -> Option<i64> {
    match doc.length {
        LengthSettings::Fixed { mm, .. } => Some(mm_to_dots(mm, dpi).max(1)),
        LengthSettings::Auto => None,
    }
}

/// `layoutFlow`.
pub fn layout_flow(
    doc: &LabelDoc,
    items: &[MeasuredItem],
    band_dots: i64,
    dpi: u16,
    limits: &LayoutLimits,
) -> Layout {
    let inset = limits.inset_dots.max(0);
    let (gap, align) = flow_settings(doc, dpi);
    let start = mm_to_dots(doc.margins_mm.start, dpi).max(0) + inset;
    let end = mm_to_dots(doc.margins_mm.end, dpi).max(0) + inset;
    let inner = (band_dots - 2 * inset).max(0);
    let content = content_length(items, gap);

    let mut x0 = start;
    let mut overflow = false;
    let len = match fixed_length_dots(doc, dpi) {
        Some(len) => {
            let avail = len - start - end;
            if content > avail {
                overflow = true;
            } else {
                x0 = start + (avail - content).div_euclid(2);
            }
            len
        }
        None => start + content + end,
    };

    let (final_len, clamped) = clamp_length(len, limits);
    match clamped {
        Some(Clamped::Min) => x0 += (final_len - len).div_euclid(2),
        Some(Clamped::Max) => overflow = overflow || x0 + content + end > final_len,
        None => {}
    }

    let mut placements = Vec::with_capacity(items.len());
    let mut x = x0;
    let mut prev: Option<&MeasuredItem> = None;
    for it in items {
        let h = it.h.max(0).min(inner);
        let visible = it.w > 0 || it.spacer;
        if visible && prev.is_some_and(|p| !p.spacer) && !it.spacer {
            x += gap;
        }
        placements.push(Placement {
            item_id: it.item_id.clone(),
            x,
            y: inset + align_offset(align, inner, h),
            w: it.w,
            h,
            spacer: it.spacer,
        });
        if visible {
            x += it.w.max(0);
            prev = Some(it);
        }
    }
    Layout {
        placements,
        length_dots: final_len,
        overflow,
        clamped,
        content_dots: content,
    }
}

/// `layoutFree`: items placed by their own frames (already in dots). Auto length = right-most
/// edge + end margin; fixed = that length. Overflow when an item leaves the label or band.
pub fn layout_free(
    doc: &LabelDoc,
    items: &[Placement],
    band_dots: i64,
    dpi: u16,
    limits: &LayoutLimits,
) -> Layout {
    let end = mm_to_dots(doc.margins_mm.end, dpi).max(0);
    let right = items.iter().map(|it| it.x + it.w).fold(0, i64::max);
    let len = fixed_length_dots(doc, dpi).unwrap_or(right + end);
    let (final_len, clamped) = clamp_length(len, limits);
    let overflow = items
        .iter()
        .any(|it| it.x < 0 || it.y < 0 || it.x + it.w > final_len || it.y + it.h > band_dots);
    let left = items.iter().map(|it| it.x).fold(right, i64::min);
    Layout {
        placements: items.to_vec(),
        length_dots: final_len,
        overflow,
        clamped,
        content_dots: right - left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{LabelDoc, Margins};

    const DPI: u16 = 180;
    const BAND: i64 = 128;

    fn doc() -> LabelDoc {
        let mut d = LabelDoc::with_items(vec![]);
        d.margins_mm = Margins {
            start: 2.0,
            end: 2.0,
        };
        d.layout = LayoutSettings::Flow {
            gap_mm: 3.0,
            align: Align::Center,
        };
        d
    }

    fn item(id: &str, w: i64, h: i64) -> MeasuredItem {
        MeasuredItem {
            item_id: id.into(),
            w,
            h,
            spacer: false,
        }
    }

    fn xs(l: &Layout) -> Vec<i64> {
        l.placements.iter().map(|p| p.x).collect()
    }

    #[test]
    fn align_offsets() {
        assert_eq!(align_offset(Align::Start, 128, 28), 0);
        assert_eq!(align_offset(Align::Center, 128, 28), 50);
        assert_eq!(align_offset(Align::End, 128, 28), 100);
    }

    #[test]
    fn places_items_with_gaps_and_margins() {
        let l = layout_flow(
            &doc(),
            &[item("a", 100, 128), item("b", 50, 60)],
            BAND,
            DPI,
            &LayoutLimits::default(),
        );
        let gap = mm_to_dots(3.0, DPI);
        let pos: Vec<_> = l.placements.iter().map(|p| (p.x, p.y)).collect();
        assert_eq!(pos, [(14, 0), (14 + 100 + gap, 34)]);
        assert_eq!(l.content_dots, 100 + gap + 50);
        assert_eq!(l.length_dots, 14 + 100 + gap + 50 + 14);
        assert!(!l.overflow && l.clamped.is_none());
    }

    #[test]
    fn aligns_across_the_tape() {
        let y = |align| {
            let mut d = doc();
            d.layout = LayoutSettings::Flow { gap_mm: 0.0, align };
            layout_flow(&d, &[item("a", 10, 28)], BAND, DPI, &LayoutLimits::default())
                .placements[0]
                .y
        };
        assert_eq!([y(Align::Start), y(Align::Center), y(Align::End)], [0, 50, 100]);
    }

    #[test]
    fn clamps_item_height_to_the_band() {
        let l = layout_flow(&doc(), &[item("a", 10, 300)], BAND, DPI, &LayoutLimits::default());
        assert_eq!((l.placements[0].h, l.placements[0].y), (BAND, 0));
    }

    #[test]
    fn spacer_replaces_gaps_and_empty_items_take_nothing() {
        let mut s = item("s", 30, 0);
        s.spacer = true;
        let items = [item("a", 10, 10), s, item("b", 10, 10)];
        assert_eq!(content_length(&items, 21), 50);
        let l = layout_flow(&doc(), &items, BAND, DPI, &LayoutLimits::default());
        assert_eq!(xs(&l), [14, 24, 54]);

        let items = [item("a", 10, 10), item("empty", 0, 0), item("b", 10, 10)];
        assert_eq!(content_length(&items, 21), 41);
        let l = layout_flow(&doc(), &items, BAND, DPI, &LayoutLimits::default());
        assert_eq!(xs(&l), [14, 24, 45]);
    }

    #[test]
    fn fixed_length_centres_or_overflows() {
        let mut d = doc();
        d.length = LengthSettings::Fixed {
            mm: 50.0,
            shrink: false,
        };
        let l = layout_flow(&d, &[item("a", 100, 50)], BAND, DPI, &LayoutLimits::default());
        let len = mm_to_dots(50.0, DPI);
        assert_eq!(l.length_dots, len);
        assert!(!l.overflow);
        assert_eq!(l.placements[0].x, 14 + (len - 28 - 100) / 2);

        d.length = LengthSettings::Fixed {
            mm: 20.0,
            shrink: false,
        };
        let l = layout_flow(&d, &[item("a", 200, 50)], BAND, DPI, &LayoutLimits::default());
        assert_eq!(l.length_dots, mm_to_dots(20.0, DPI));
        assert!(l.overflow);
        assert_eq!(l.placements[0].x, 14);
    }

    #[test]
    fn printer_limits() {
        let mut d = doc();
        d.margins_mm = Margins {
            start: 0.0,
            end: 0.0,
        };
        let lim = LayoutLimits {
            min_length_dots: 40,
            ..LayoutLimits::default()
        };
        let l = layout_flow(&d, &[item("a", 10, 10)], BAND, DPI, &lim);
        assert_eq!((l.length_dots, l.clamped), (40, Some(Clamped::Min)));
        assert_eq!(l.placements[0].x, 15);

        let lim = LayoutLimits {
            min_length_dots: 3,
            max_length_dots: Some(7086),
            inset_dots: 0,
        };
        let l = layout_flow(&doc(), &[item("a", 8000, 10)], BAND, DPI, &lim);
        assert_eq!((l.length_dots, l.clamped, l.overflow), (7086, Some(Clamped::Max), true));
    }

    #[test]
    fn frame_insets_content() {
        let lim = LayoutLimits {
            min_length_dots: 3,
            max_length_dots: None,
            inset_dots: 10,
        };
        let l = layout_flow(&doc(), &[item("a", 10, 200)], BAND, DPI, &lim);
        let p = &l.placements[0];
        assert_eq!((p.x, p.y, p.h), (24, 10, BAND - 20));
        assert_eq!(l.length_dots, 24 + 10 + 24);
    }

    #[test]
    fn free_layout() {
        let mut d = doc();
        d.layout = LayoutSettings::Free;
        let at = |x, y| Placement {
            item_id: "a".into(),
            x,
            y,
            w: 50,
            h: 40,
            spacer: false,
        };
        let l = layout_free(&d, &[at(20, 0)], BAND, DPI, &LayoutLimits::default());
        assert_eq!(l.length_dots, 70 + 14);
        assert!(!l.overflow);
        assert!(layout_free(&d, &[at(0, 100)], BAND, DPI, &LayoutLimits::default()).overflow);
    }
}
