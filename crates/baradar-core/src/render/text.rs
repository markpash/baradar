//! Text: measuring, line layout, sizing (fit / mm / pt, pixel fonts), quick sizes and drawing on
//! the crisp plane (port of `web/src/render/text.ts`, `text-size.ts`,
//! `web/src/ui/state/text-defaults.ts` and the text parts of `renderer.ts`).
//!
//! Size semantics (`TextItem.size`):
//! - `Mm` is the cap-to-descender height of the whole block from font metrics; extra lines add
//!   `lineHeight × (cap + descender)` each.
//! - `Pt` is the em size of every line (1 pt = 1/72 in = 2.5 dots at 180 dpi).
//! - `Fit` is the largest size whose *ink* fits the available height.
//!
//! The pure functions take a measure callback ([`MeasureFn`]) so they can be tested with a fake
//! font. [`measure`] is the real one: rustybuzz shaping (kerning and standard ligatures, like
//! Canvas) and the ink bounds of the ttf-parser outlines, as Canvas `actualBoundingBox*`.

use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use tiny_skia::{Color, FillRule, Mask, Paint, PathBuilder, PixmapMut, Rect, Transform};

use crate::doc::schema::{Align, TextItem, TextSize};
use crate::render::fonts::{FontFace, FontQuality, MIN_QUALITY_CAP_MM, PixelGrid, ResolvedFont};
use crate::render::units::{dots_to_mm, js_round, mm_to_dots, pt_to_px};
use crate::store::DefaultTextSize;

// ------------------------------------------------------------------------------------------
// Types
// ------------------------------------------------------------------------------------------

/// Ink and advance of one line at some font size (`LineInk`, Canvas `TextMetrics` subset), px.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LineInk {
    pub advance: f64,
    /// Ink extent left of the origin (positive = left of it).
    pub left: f64,
    /// Ink extent right of the origin.
    pub right: f64,
    /// Ink above the baseline.
    pub ascent: f64,
    /// Ink below the baseline.
    pub descent: f64,
}

/// Font metrics per 1 px of font size (`FaceMetrics`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    /// Cap height ("H" ascent).
    pub cap: f64,
    /// Descender depth ("gjpqy" descent).
    pub desc: f64,
}

/// Measures a line at a font size in px (`MeasureFn`).
pub type MeasureFn<'a> = dyn Fn(&str, f64) -> LineInk + 'a;

/// One laid-out line (`TextLine`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// Origin x relative to the block's left edge, px.
    pub x: f64,
    /// Baseline relative to the block's top edge, px.
    pub baseline: f64,
    /// Ink width, px.
    pub width: f64,
}

/// A laid-out block (`TextBlock`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub font_px: f64,
    pub lines: Vec<TextLine>,
    /// Block width (ink), px, without invert padding.
    pub width: f64,
    /// Block height, px, without invert padding.
    pub height: f64,
    /// Cap-to-descender height of one line (font metrics), px.
    pub line_box: f64,
}

/// JavaScript's `String.prototype.trim() === ''`.
fn is_blank(s: &str) -> bool {
    s.chars().all(|c| c.is_whitespace() || c == '\u{feff}')
}

/// Splits into lines on `\n`, `\r\n` and `\r`; never empty (`splitLines`).
pub fn split_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// `alignOffset` (layout.ts): offset of `inner` in `outer`.
fn align_offset(align: Align, outer: f64, inner: f64) -> f64 {
    match align {
        Align::Start => 0.0,
        Align::End => outer - inner,
        Align::Center => js_round((outer - inner) / 2.0) as f64,
    }
}

/// Lays out `lines` at `px`: baselines, alignment and the block box (`layoutLines`). With `grid`
/// (px per dot, for pixel fonts) the first baseline, the pitch, the bottom, the width and every
/// line's origin are snapped to whole multiples of `grid`.
#[allow(clippy::too_many_arguments)]
pub fn layout_lines<S: AsRef<str>>(
    lines: &[S],
    px: f64,
    face: FaceMetrics,
    line_height: f64,
    align: Align,
    measure: &MeasureFn<'_>,
    metric_height: bool,
    grid: Option<f64>,
) -> TextBlock {
    let grid = grid.filter(|g| *g > 0.0);
    let snap = |v: f64| grid.map_or(v, |g| js_round(v / g) as f64 * g);
    let snap_up = |v: f64| grid.map_or(v, |g| (v / g - 1e-6).ceil() * g);
    let inks: Vec<LineInk> = lines
        .iter()
        .map(|l| {
            let l = l.as_ref();
            if is_blank(l) {
                LineInk::default()
            } else {
                measure(l, px)
            }
        })
        .collect();
    let cap_px = face.cap * px;
    let line_box = (face.cap + face.desc) * px;
    let pitch = match grid {
        Some(g) => g.max(snap(line_box * line_height)),
        None => line_box * line_height,
    };
    let first = inks.first().copied().unwrap_or_default();
    let last = inks.last().copied().unwrap_or_default();
    // Above the first baseline: at least the cap height (so "...." does not grow huge).
    let top = snap_up(cap_px.max(first.ascent));
    let bottom = snap_up(if metric_height {
        (face.desc * px).max(last.descent)
    } else {
        0f64.max(last.descent)
    });
    // Horizontal extent = ink (not advance): margins and gaps are measured to the printed dots.
    let spans: Vec<(f64, f64)> = inks
        .iter()
        .map(|m| {
            if m.right + m.left > 0.0 {
                (-m.left, m.right + m.left)
            } else {
                (0.0, 0.0)
            }
        })
        .collect();
    let width = snap_up(spans.iter().fold(0f64, |a, s| a.max(s.1)));
    let out = lines
        .iter()
        .zip(&spans)
        .enumerate()
        .map(|(i, (text, &(l, w)))| TextLine {
            text: text.as_ref().to_owned(),
            x: snap(align_offset(align, width, w) - l),
            baseline: top + i as f64 * pitch,
            width: w,
        })
        .collect();
    let height = top + (lines.len() as f64 - 1.0) * pitch + bottom;
    TextBlock {
        font_px: px,
        lines: out,
        width,
        height,
        line_box,
    }
}

/// How [`fit_text_block`] picks the font size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FitOpts {
    /// The largest size whose ink fits `avail_px`.
    Fit { avail_px: f64 },
    /// `target_px` is the cap-to-descender height of the block.
    Target { target_px: f64 },
    /// `em_px` is the font size.
    Em {
        em_px: f64,
        grid: Option<f64>,
        metric_height: bool,
    },
}

/// Font size for an available height or a target (`fitTextBlock`).
pub fn fit_text_block<S: AsRef<str>>(
    lines: &[S],
    face: FaceMetrics,
    line_height: f64,
    align: Align,
    measure: &MeasureFn<'_>,
    opts: FitOpts,
) -> TextBlock {
    let n = lines.len().max(1) as f64;
    let layout = |px: f64, metric: bool, grid: Option<f64>| {
        layout_lines(lines, px, face, line_height, align, measure, metric, grid)
    };
    match opts {
        FitOpts::Em {
            em_px,
            grid,
            metric_height,
        } => layout(em_px.max(0.0), metric_height, grid),
        FitOpts::Target { target_px } => {
            // Cap-to-descender of the block = (cap + desc)·px·(1 + (n−1)·lineHeight).
            let per = (face.cap + face.desc) * (1.0 + (n - 1.0) * line_height);
            let px = if per > 0.0 { target_px / per } else { 0.0 };
            layout(px, true, None)
        }
        FitOpts::Fit { avail_px } => {
            let reference = 100.0;
            let mut block = layout(reference, false, None);
            let mut px = if block.height > 0.0 {
                reference * avail_px / block.height
            } else {
                0.0
            };
            let mut i = 0;
            while i < 4 && px > 0.0 {
                block = layout(px, false, None);
                if block.height <= avail_px + 1e-6 {
                    break;
                }
                px *= (avail_px / block.height) * 0.999;
                i += 1;
            }
            block
        }
    }
}

/// Line-height multiplier as the renderer applies it: 0.5–3, 1.1 when not a number
/// (`lineHeightOf`).
pub fn line_height_of(line_height: f64) -> f64 {
    if line_height.is_finite() {
        line_height.clamp(0.5, 3.0)
    } else {
        1.1
    }
}

/// Dots per font pixel for a pixel font whose em would be `em_dots` (`pixelScaleFor`): the
/// largest whole number not above it for a limit, else the nearest; never below 1.
pub fn pixel_scale_for(em_dots: f64, em_px: f64, limit: bool) -> u32 {
    if em_px.is_nan() || em_px <= 0.0 || em_dots.is_nan() || em_dots <= 0.0 {
        return 1;
    }
    let r = em_dots / em_px;
    let k = if limit {
        (r + 1e-6).floor()
    } else {
        js_round(r) as f64
    };
    k.clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Default crisp-plane factor, px per dot (`DEFAULT_CRISP_FACTOR`).
pub const DEFAULT_CRISP_FACTOR: u32 = 3;

/// Text is never shrunk below this fraction to fit a fixed length (`MIN_SHRINK`).
pub const MIN_SHRINK: f64 = 0.1;

/// "Shrink to fit length": the common factor (≤ 1) for text blocks `text_dots` long when the
/// label has `avail_dots` and the other content takes `other_dots` (`shrinkFactor`). `None` when
/// shrinking the text cannot help.
pub fn shrink_factor(avail_dots: f64, other_dots: f64, text_dots: f64) -> Option<f64> {
    if text_dots.is_nan() || text_dots <= 0.0 || other_dots + text_dots <= avail_dots {
        return Some(1.0);
    }
    let room = avail_dots - other_dots;
    if room <= 0.0 {
        return None;
    }
    Some(MIN_SHRINK.max(room / text_dots))
}

/// What sizing needs of a text item (`TextSizing`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSizing {
    pub size: TextSize,
    pub align: Align,
    pub invert: bool,
    /// Already clamped ([`line_height_of`]).
    pub line_height: f64,
    /// Migrated v1 text: a fixed size taller than the band is clipped, not reduced.
    pub clip_tall: bool,
}

impl TextSizing {
    /// The sizing of `item` (the renderer's `{ size, align, invert, lineHeight, clipTall }`).
    pub fn of(item: &TextItem) -> Self {
        Self {
            size: item.size,
            align: item.align,
            invert: item.invert,
            line_height: line_height_of(item.line_height),
            clip_tall: item.clip_tall,
        }
    }
}

/// The band and options a block is sized in (`SizeEnv`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeEnv {
    /// Height available across the tape (the item's band), px.
    pub band_px: f64,
    /// px per dot (the crisp-plane factor).
    pub f: u32,
    pub dpi: u16,
    /// Design grid of a pixel font, when its bundled face is drawn.
    pub pixel: Option<PixelGrid>,
    /// Common "shrink to fit length" factor (< 1 shrinks).
    pub scale: Option<f64>,
    /// Width available along the label (free-layout frame), px.
    pub max_w_px: Option<f64>,
}

/// Invert padding, px (`invertPadding`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvertPadding {
    pub v: f64,
    pub h: f64,
}

/// Invert padding around a block of `line_box_px` in a band of `band_px` (`invertPadding`).
pub fn invert_padding(band_px: f64, line_box_px: f64, factor: f64) -> InvertPadding {
    let v = factor.max(js_round((band_px * 0.1).min(10.0 * factor)) as f64);
    let h = (2.0 * factor).max(js_round(line_box_px * 0.25) as f64);
    InvertPadding { v, h }
}

/// A sized block and its box (`SizedTextBlock`).
#[derive(Debug, Clone, PartialEq)]
pub struct SizedTextBlock {
    pub block: TextBlock,
    pub pad: InvertPadding,
    /// The item's box (block + invert padding), px.
    pub w_px: f64,
    pub h_px: f64,
    /// A fixed size taller than the band was reduced to fit it.
    pub reduced: bool,
    /// Pixel font drawn on its grid: dots per font pixel.
    pub pixel_scale: Option<u32>,
}

/// Sizes a block in a band (`sizeTextBlock`): fit / mm / pt, then a frame width (shrink only),
/// then the band for fixed sizes (reduce only, unless `clip_tall`), then pixel-grid snapping.
pub fn size_text_block<S: AsRef<str>>(
    lines: &[S],
    face: FaceMetrics,
    t: &TextSizing,
    measure: &MeasureFn<'_>,
    env: &SizeEnv,
) -> SizedTextBlock {
    let f = f64::from(env.f);
    let band_px = env.band_px;
    let lh = t.line_height;
    let align = t.align;
    let fit = matches!(t.size, TextSize::Fit);
    let pad_v = if t.invert {
        invert_padding(band_px, 0.0, f).v
    } else {
        0.0
    };
    let avail_px = f.max(band_px - 2.0 * pad_v);
    let fit_in = |s: f64| -> TextBlock {
        let opts = match t.size {
            TextSize::Fit => FitOpts::Fit {
                avail_px: avail_px * s,
            },
            TextSize::Pt { pt } => FitOpts::Em {
                em_px: pt_to_px(pt, env.dpi, env.f) * s,
                grid: None,
                metric_height: true,
            },
            TextSize::Mm { mm } => FitOpts::Target {
                target_px: (mm * f64::from(env.dpi) * f / 25.4) * s,
            },
        };
        fit_text_block(lines, face, lh, align, measure, opts)
    };
    let pad_h_of = |b: &TextBlock| {
        if t.invert {
            invert_padding(band_px, b.line_box, f).h
        } else {
            0.0
        }
    };
    let height_of = |b: &TextBlock| match (t.invert, fit) {
        (true, true) => band_px,
        (true, false) => b.height + 2.0 * pad_v,
        _ => b.height,
    };
    let band_dots = js_round(band_px / f) as f64;
    let too_tall = |b: &TextBlock| (height_of(b) / f - 1e-6).ceil() > band_dots;
    let too_wide = |b: &TextBlock| {
        env.max_w_px
            .is_some_and(|m| b.width + 2.0 * pad_h_of(b) > m)
    };

    let mut s = env.scale.unwrap_or(1.0);
    let mut limit = fit || s < 1.0;
    let mut block = fit_in(s);
    // Free layout: also fit the frame's other side (shrink only).
    if let Some(max_w) = env.max_w_px
        && block.width > 0.0
        && too_wide(&block)
    {
        s *= 0.01f64.max((max_w - 2.0 * pad_h_of(&block)) / block.width);
        block = fit_in(s);
        limit = true;
    }
    // A fixed size taller than the band is reduced to it (never clipped), except v1 labels.
    let mut reduced = false;
    let mut i = 0;
    while i < 6 && !fit && !t.clip_tall && block.height > 0.0 && too_tall(&block) {
        s *= (avail_px / block.height) * 0.999;
        block = fit_in(s);
        reduced = true;
        limit = true;
        i += 1;
    }
    let mut pixel_scale = None;
    if let Some(grid) = env.pixel
        && grid.em_px > 0
        && block.font_px > 0.0
    {
        let em_px = f64::from(grid.em_px);
        let at = |k: u32| {
            layout_lines(
                lines,
                f64::from(k) * em_px * f,
                face,
                lh,
                align,
                measure,
                !fit,
                Some(f),
            )
        };
        let over = |b: &TextBlock| {
            (if fit {
                b.height > avail_px + 1e-6
            } else {
                too_tall(b)
            }) || too_wide(b)
        };
        let mut k = pixel_scale_for(block.font_px / f, em_px, limit);
        block = at(k);
        while k > 1 && over(&block) {
            k -= 1;
            block = at(k);
        }
        pixel_scale = Some(k);
    }
    let pad_h = pad_h_of(&block);
    SizedTextBlock {
        w_px: block.width + 2.0 * pad_h,
        h_px: height_of(&block),
        pad: InvertPadding { v: pad_v, h: pad_h },
        block,
        reduced,
        pixel_scale,
    }
}

// ------------------------------------------------------------------------------------------
// Shaping, measuring and drawing with a real face (canvasMeasure, faceMetrics, drawTextBlock)
// ------------------------------------------------------------------------------------------

/// Horizontal skew of synthesised italic: Blink sets Skia's `skewX` to −1/4 (y down), i.e.
/// `x' = x + y/4` with y up. The renderer's italic clip slack (`lineBox·0.25` per side) assumes
/// the same.
pub const SYNTHETIC_ITALIC_SKEW: f64 = 0.25;

/// Rounds a metric to 1/1024 px (`quantize`), as `Math.round` does.
fn quantize(v: f64) -> f64 {
    (v * 1024.0 + 0.5).floor() / 1024.0
}

/// Maps glyph outline points to px (y down) and tracks their bounds; optionally builds a path.
struct GlyphSink<'a> {
    path: Option<&'a mut PathBuilder>,
    /// Glyph origin in font units (pen + offset).
    gx: f64,
    gy: f64,
    skew: f64,
    scale: f64,
    ox: f64,
    oy: f64,
    min: (f64, f64),
    max: (f64, f64),
}

impl GlyphSink<'_> {
    fn map(&mut self, x: f32, y: f32) -> (f32, f32) {
        let uy = f64::from(y) + self.gy;
        let ux = f64::from(x) + self.gx + self.skew * uy;
        let (px, py) = (self.ox + ux * self.scale, self.oy - uy * self.scale);
        self.min = (self.min.0.min(px), self.min.1.min(py));
        self.max = (self.max.0.max(px), self.max.1.max(py));
        (px as f32, py as f32)
    }
}

impl OutlineBuilder for GlyphSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        if let Some(pb) = self.path.as_deref_mut() {
            pb.move_to(p.0, p.1);
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        if let Some(pb) = self.path.as_deref_mut() {
            pb.line_to(p.0, p.1);
        }
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = self.map(x1, y1);
        let p = self.map(x, y);
        if let Some(pb) = self.path.as_deref_mut() {
            pb.quad_to(c.0, c.1, p.0, p.1);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.map(x1, y1);
        let c2 = self.map(x2, y2);
        let p = self.map(x, y);
        if let Some(pb) = self.path.as_deref_mut() {
            pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
        }
    }
    fn close(&mut self) {
        if let Some(pb) = self.path.as_deref_mut() {
            pb.close();
        }
    }
}

/// Ink bounds (min, max) in px, y down; `None` without ink.
type InkBounds = Option<((f64, f64), (f64, f64))>;

/// Shapes `text` and feeds every glyph outline at `px` with the origin at (`ox`, `oy`) into a
/// sink. Returns (advance px, ink bounds min/max in px, y down) or `None` if the face is broken.
fn run_line(
    face: &FontFace,
    text: &str,
    px: f64,
    ox: f64,
    oy: f64,
    mut path: Option<&mut PathBuilder>,
) -> Option<(f64, InkBounds)> {
    let skew = if face.italic() {
        SYNTHETIC_ITALIC_SKEW
    } else {
        0.0
    };
    let mut pen = 0f64;
    let mut min = (f64::INFINITY, f64::INFINITY);
    let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut shaped_any = false;
    for (seg_face, seg, rtl) in shaping_segments(face, text) {
        let seg_face = seg_face.with_italic(false);
        let done = seg_face.with_shaper(|hb| {
            let upem = f64::from(hb.units_per_em()).max(1.0);
            let scale = px / upem;
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(seg);
            buf.guess_segment_properties();
            buf.set_direction(if rtl {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            });
            let glyphs = rustybuzz::shape(hb, &[], buf);
            for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
                let gid = u16::try_from(info.glyph_id).unwrap_or(0);
                let mut sink = GlyphSink {
                    path: path.as_deref_mut(),
                    gx: pen / scale + f64::from(pos.x_offset),
                    gy: f64::from(pos.y_offset),
                    skew,
                    scale,
                    ox,
                    oy,
                    min,
                    max,
                };
                hb.outline_glyph(GlyphId(gid), &mut sink);
                min = sink.min;
                max = sink.max;
                pen += f64::from(pos.x_advance) * scale;
            }
        });
        shaped_any |= done.is_some();
    }
    if !shaped_any && !text.is_empty() {
        return None;
    }
    let ink = (min.0 <= max.0).then_some((min, max));
    Some((pen, ink))
}

/// Splits a line into shaping segments in visual (left-to-right) order: bidi runs (Persian,
/// Arabic and Hebrew right to left, with numbers and Latin inside them laid out correctly), each
/// split again where the font changes. Characters `face` lacks come from an installed font that
/// has them (`fallback_face`), as Chrome does; marks and joiners stay with the character before.
fn shaping_segments<'t>(face: &FontFace, text: &'t str) -> Vec<(FontFace, &'t str, bool)> {
    use unicode_bidi::BidiInfo;
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    let weight = face.weight_or_regular();
    let pick = |c: char, prev: Option<&FontFace>| -> FontFace {
        if face.has_char(c) {
            return face.clone();
        }
        if inherits_font(c)
            && let Some(p) = prev
        {
            return p.clone();
        }
        crate::render::fonts::fallback_face(c, weight).unwrap_or_else(|| face.clone())
    };
    let bidi = BidiInfo::new(text, None);
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(|l| l.is_rtl());
            // Font segments of this run, in logical order.
            let mut segs: Vec<(FontFace, std::ops::Range<usize>)> = Vec::new();
            for (i, c) in text[run.clone()].char_indices() {
                let at = run.start + i;
                let f = pick(c, segs.last().map(|s| &s.0));
                match segs.last_mut() {
                    Some((last, r)) if same_face(last, &f) => r.end = at + c.len_utf8(),
                    _ => segs.push((f, at..at + c.len_utf8())),
                }
            }
            // A right-to-left run's segments are laid out right to left.
            if rtl {
                segs.reverse();
            }
            out.extend(segs.into_iter().map(|(f, r)| (f, &text[r], rtl)));
        }
    }
    out
}

/// Marks, joiners and variation selectors draw with the font of the character before them.
fn inherits_font(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{0610}'..='\u{061A}'
        | '\u{064B}'..='\u{065F}'
        | '\u{0670}'
        | '\u{06D6}'..='\u{06ED}'
        | '\u{200C}'..='\u{200F}'
        | '\u{FE00}'..='\u{FE0F}'
        | '\u{20D0}'..='\u{20FF}')
}

fn same_face(a: &FontFace, b: &FontFace) -> bool {
    a.same_source(b)
}

/// Measures `text` at `px` with `face` (`canvasMeasure`): the advance and the ink box relative to
/// the origin and the alphabetic baseline, like Canvas `actualBoundingBox*`, quantised to 1/1024.
/// Synthesised italic is included. No ink (or a broken face) gives zero extents.
pub fn measure(text: &str, px: f64, face: &FontFace) -> LineInk {
    let Some((advance, ink)) = run_line(face, text, px, 0.0, 0.0, None) else {
        return LineInk::default();
    };
    let (left, right, ascent, descent) = match ink {
        Some((min, max)) => (-min.0, max.0, -min.1, max.1),
        None => (0.0, 0.0, 0.0, 0.0),
    };
    LineInk {
        advance: quantize(advance),
        left: quantize(left),
        right: quantize(right),
        ascent: quantize(ascent),
        descent: quantize(descent),
    }
}

/// A [`MeasureFn`] for a face.
pub fn face_measure(face: &FontFace) -> impl Fn(&str, f64) -> LineInk + '_ {
    move |text, px| measure(text, px, face)
}

/// Cap height and descender of a face per px (`faceMetrics`): the ink ascent of "H" and the ink
/// descent of "gjpqy" at 100 px, falling back to 0.7 and 0.2. Cached per face.
pub fn face_metrics(face: &FontFace) -> FaceMetrics {
    *face.metrics_cell().get_or_init(|| {
        let upright = face.with_italic(false);
        let cap = measure("H", 100.0, &upright).ascent / 100.0;
        let desc = measure("gjpqy", 100.0, &upright).descent / 100.0;
        FaceMetrics {
            cap: if cap > 0.0 { cap } else { 0.7 },
            desc: if desc > 0.0 { desc } else { 0.2 },
        }
    })
}

/// Draws a laid-out block with its top-left at (`x`, `y`) px (`drawTextBlock`): every non-blank
/// line at its origin and alphabetic baseline, anti-aliased, non-zero fill. With `grid` (px per
/// dot; pixel fonts) the block origin is snapped to whole dots. `transform` and `mask` play the
/// part of the canvas transform and clip.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_block(
    pixmap: &mut PixmapMut<'_>,
    face: &FontFace,
    block: &TextBlock,
    x: f64,
    y: f64,
    color: Color,
    grid: Option<f64>,
    transform: Transform,
    mask: Option<&Mask>,
) {
    let snap = |v: f64| match grid {
        Some(g) if g > 0.0 => js_round(v / g) as f64 * g,
        _ => v,
    };
    let (ox, oy) = (snap(x), snap(y));
    let mut pb = PathBuilder::new();
    for l in block.lines.iter().filter(|l| !is_blank(&l.text)) {
        run_line(
            face,
            &l.text,
            block.font_px,
            ox + l.x,
            oy + l.baseline,
            Some(&mut pb),
        );
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
    let embolden = embolden_px();
    // Pixel fonts drawn on whole dots stay crisp.
    if embolden > 0.0 && grid.is_none() {
        let stroke = tiny_skia::Stroke {
            width: embolden,
            line_join: tiny_skia::LineJoin::Round,
            ..tiny_skia::Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
    }
}

/// Extra outline width (crisp-plane px at the default factor) that matches the slightly heavier
/// glyphs of Chrome's text rasteriser, which the web studio's thresholds were tuned against.
fn embolden_px() -> f32 {
    static PX: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *PX.get_or_init(|| {
        std::env::var("BARADAR_EMBOLDEN_PX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(TEXT_EMBOLDEN_PX)
    })
}

/// Extra glyph outline width, crisp-plane px at the default 3× factor (see [`embolden_px`];
/// `BARADAR_EMBOLDEN_PX` overrides it for tuning).
///
/// Chrome (Skia/CoreText on macOS) draws glyphs a little heavier than an exact outline fill,
/// and the web studio's 128 threshold was tuned against that. Compared with the live studio on
/// 26 test labels (`examples/golden_docs.rs`), 0.6 px gave the smallest total ink difference
/// (1863 → 1174 dots), leaving every case within about 4 %. Layout, codes, icons and pixel fonts
/// matched dot for dot; the remaining text difference varies with font size, so a constant
/// cannot remove it entirely. Pixel fonts drawn on the dot grid are not emboldened.
pub const TEXT_EMBOLDEN_PX: f32 = 0.6;

// ------------------------------------------------------------------------------------------
// Renderer: prepareText, warnings, drawing a text item
// ------------------------------------------------------------------------------------------

/// Below this cap-to-descender height (mm) text prints poorly (`SMALL_TEXT_MM`).
pub const SMALL_TEXT_MM: f64 = 2.0;

/// A warning about a text item (`RenderWarning` minus `itemId`/`blocking`: never blocking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextWarning {
    /// `small-text`, `content-overflow` or `font-quality`.
    pub code: &'static str,
    pub message: String,
}

/// How a text block was sized (`TextRenderInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextRenderInfo {
    pub item_id: String,
    /// Em size drawn (after fit / shrink / pixel snapping), dots.
    pub em_dots: f64,
    /// Cap height of one line, dots.
    pub cap_dots: f64,
    /// Ink width of the block (its length along the label when not rotated), dots.
    pub width_dots: i64,
    /// "Shrink to fit length" factor (< 1), when applied.
    pub shrink: Option<f64>,
    /// Pixel font on its grid: dots per font pixel. Omitted when italic.
    pub pixel_scale: Option<u32>,
}

/// A non-empty text item measured and ready to draw (renderer.ts `Prepared` for text).
#[derive(Debug, Clone)]
pub struct PreparedText {
    /// Box size along / across the tape, dots.
    pub w: i64,
    pub h: i64,
    /// Taller than the band it was given (clipped).
    pub too_tall: bool,
    /// Horizontal clip slack, px (italic overhang): clip the box to `-slack .. w·f + slack`.
    pub clip_slack: f64,
    pub info: TextRenderInfo,
    pub warnings: Vec<TextWarning>,
    pub sized: SizedTextBlock,
    pub face: FontFace,
    pub invert: bool,
    /// Pixel grid (px per dot) for the block origin, when snapped.
    pub grid: Option<f64>,
}

/// JavaScript's `Number.prototype.toFixed(digits)` for finite numbers: exact decimal value,
/// halves rounded up in magnitude.
pub fn to_fixed(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let exact = format!("{:.*}", digits + 40, v.abs());
    let (int, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut keep: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut i = keep.len();
        loop {
            if i == 0 {
                keep.insert(0, b'1');
                break;
            }
            i -= 1;
            if keep[i] == b'9' {
                keep[i] = b'0';
            } else {
                keep[i] += 1;
                break;
            }
        }
    }
    let int_len = keep.len() - digits;
    let mut s = String::from_utf8_lossy(&keep[..int_len]).into_owned();
    if digits > 0 {
        s.push('.');
        s.push_str(&String::from_utf8_lossy(&keep[int_len..]));
    }
    // JS keeps the sign of any negative number ("-0.0"), but not of −0.
    if v < 0.0 {
        s.insert(0, '-');
    }
    s
}

/// `font-quality` (renderer.ts `qualityWarning`): a thin or script library font below a readable
/// cap height. Thin families count only below 600; never for custom fonts.
pub fn quality_warning(font: &ResolvedFont, cap_dots: f64, dpi: u16) -> Option<TextWarning> {
    if font.custom {
        return None;
    }
    let def = font.def;
    let quality = def.quality?;
    if quality == FontQuality::Thin && font.weight >= 600 {
        return None;
    }
    let cap_mm = dots_to_mm(cap_dots, dpi);
    if cap_mm.is_nan() || cap_mm >= MIN_QUALITY_CAP_MM {
        return None;
    }
    let what = match quality {
        FontQuality::Thin => "Thin strokes",
        FontQuality::Script => "Script letters",
    };
    Some(TextWarning {
        code: "font-quality",
        message: format!(
            "{what} of {} print poorly below {MIN_QUALITY_CAP_MM} mm (this text is {} mm): make it larger (M or more) or choose a sturdier font.",
            def.label,
            to_fixed(cap_mm, 1)
        ),
    })
}

/// Measures a text item in a band (renderer.ts `prepareText`). `None` for a blank item (it takes
/// no space). `band` and `max_w` are dots; `scale` < 1 is "shrink to fit length".
pub fn prepare_text(
    item: &TextItem,
    font: &ResolvedFont,
    band: i64,
    f: u32,
    dpi: u16,
    max_w: Option<f64>,
    scale: f64,
) -> Option<PreparedText> {
    let lines = split_lines(&item.text);
    if lines.iter().all(|l| is_blank(l)) {
        return None;
    }
    let ff = f64::from(f);
    let face = face_metrics(&font.face);
    let m = face_measure(&font.face);
    let sized = size_text_block(
        &lines,
        face,
        &TextSizing::of(item),
        &m,
        &SizeEnv {
            band_px: band as f64 * ff,
            f,
            dpi,
            pixel: font.pixel_grid(),
            scale: (scale < 1.0).then_some(scale),
            max_w_px: max_w.map(|w| w * ff),
        },
    );
    let block = &sized.block;
    let w = (sized.w_px / ff - 1e-6).ceil() as i64;
    let h = (sized.h_px / ff - 1e-6).ceil() as i64;
    let mut warnings = Vec::new();
    let line_mm = dots_to_mm(block.line_box / ff, dpi);
    if line_mm < SMALL_TEXT_MM {
        warnings.push(TextWarning {
            code: "small-text",
            message: format!(
                "Text is only {} mm tall and may be hard to read. Use fewer lines or wider tape.",
                to_fixed(line_mm, 1)
            ),
        });
    }
    if sized.reduced {
        warnings.push(TextWarning {
            code: "content-overflow",
            message: "Text is taller than the printable area of this tape and was reduced to fit."
                .into(),
        });
    }
    let too_tall = h > band;
    if too_tall {
        warnings.push(TextWarning {
            code: "content-overflow",
            message: "Text is taller than the printable area of this tape and will be cut off."
                .into(),
        });
    }
    let cap_dots = face.cap * block.font_px / ff;
    warnings.extend(quality_warning(font, cap_dots, dpi));
    let clip_slack = if item.italic {
        block.line_box * 0.25
    } else {
        0.0
    };
    let grid = sized.pixel_scale.map(|_| ff);
    let info = TextRenderInfo {
        item_id: item.id.clone(),
        em_dots: block.font_px / ff,
        cap_dots,
        width_dots: w,
        shrink: (scale < 1.0).then_some(scale),
        pixel_scale: sized.pixel_scale.filter(|_| !item.italic),
    };
    Some(PreparedText {
        w,
        h,
        too_tall,
        clip_slack,
        info,
        warnings,
        face: font.face.clone(),
        invert: item.invert,
        grid,
        sized,
    })
}

impl PreparedText {
    pub fn block(&self) -> &TextBlock {
        &self.sized.block
    }

    /// Draws the item in a box of `bw` × `bh` px at the origin of `transform` (the renderer's
    /// `crisp` closure): inverted fills the box black and draws white text; the block is centred.
    pub fn draw(
        &self,
        pixmap: &mut PixmapMut<'_>,
        bw: f64,
        bh: f64,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        if self.invert
            && let Some(rect) = Rect::from_xywh(0.0, 0.0, bw as f32, bh as f32)
        {
            let mut paint = Paint::default();
            paint.set_color(Color::BLACK);
            pixmap.fill_rect(rect, &paint, transform, mask);
        }
        let block = self.block();
        let color = if self.invert {
            Color::WHITE
        } else {
            Color::BLACK
        };
        draw_text_block(
            pixmap,
            &self.face,
            block,
            (bw - block.width) / 2.0,
            (bh - block.height) / 2.0,
            color,
            self.grid,
            transform,
            mask,
        );
    }
}

// ------------------------------------------------------------------------------------------
// Quick sizes (text-size.ts)
// ------------------------------------------------------------------------------------------

/// `QuickSizeId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickSizeId {
    Xs,
    S,
    M,
    L,
    Fit,
}

impl QuickSizeId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xs => "xs",
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
            Self::Fit => "fit",
        }
    }

    /// [button text, accessible name] (`LABELS`).
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Xs => ("XS", "XS, extra small"),
            Self::S => ("S", "S, small"),
            Self::M => ("M", "M, medium"),
            Self::L => ("L", "L, large"),
            Self::Fit => ("Fit", "Fit tape"),
        }
    }

    /// Fraction of the band (`QUICK_FRACTIONS`); `None` for Fit.
    pub fn fraction(self) -> Option<f64> {
        match self {
            Self::Xs => Some(1.0 / 4.0),
            Self::S => Some(1.0 / 3.0),
            Self::M => Some(1.0 / 2.0),
            Self::L => Some(3.0 / 4.0),
            Self::Fit => None,
        }
    }
}

/// One quick size button (`QuickTextSize`).
#[derive(Debug, Clone, PartialEq)]
pub struct QuickTextSize {
    pub id: QuickSizeId,
    /// "XS", "S", "M", "L", "Fit".
    pub label: &'static str,
    /// Accessible name, starting with the button text.
    pub name: &'static str,
    /// What the button stores in `TextItem.size`.
    pub size: TextSize,
    /// Cap-to-descender height of the block in dots (the band for Fit).
    pub dots: i64,
    /// Below the readable minimum on this band.
    pub too_small: bool,
}

/// Fractions of the band per quick size, XS → L (`QUICK_FRACTIONS`).
pub const QUICK_FRACTIONS: [(QuickSizeId, f64); 4] = [
    (QuickSizeId::Xs, 1.0 / 4.0),
    (QuickSizeId::S, 1.0 / 3.0),
    (QuickSizeId::M, 1.0 / 2.0),
    (QuickSizeId::L, 3.0 / 4.0),
];

/// Cap-to-descender height (mm) below which one line prints poorly (`MIN_READABLE_MM`).
pub const MIN_READABLE_MM: f64 = 2.0;

/// The readable minimum at 180 dpi, dots (`MIN_READABLE_DOTS`).
pub const MIN_READABLE_DOTS: i64 = 14;

/// [`MIN_READABLE_MM`] in dots at `dpi`: 14 at 180 dpi, 28 at 360 (`minReadableDots`).
pub fn min_readable_dots(dpi: u16) -> i64 {
    mm_to_dots(MIN_READABLE_MM, dpi)
}

/// mm of `dots`, rounded to 0.01 mm (`dotsMm`).
fn dots_mm(dots: i64, dpi: u16) -> f64 {
    js_round(dots_to_mm(dots as f64, dpi) * 100.0) as f64 / 100.0
}

/// The quick sizes for a band, XS → L, then Fit (`quickTextSizes`).
pub fn quick_text_sizes(band_dots: i64, dpi: u16) -> Vec<QuickTextSize> {
    let band = band_dots.max(1);
    let min_dots = min_readable_dots(dpi);
    let mut out: Vec<QuickTextSize> = QUICK_FRACTIONS
        .iter()
        .map(|&(id, frac)| {
            let dots = js_round(band as f64 * frac).max(1);
            let (label, name) = id.labels();
            QuickTextSize {
                id,
                label,
                name,
                size: TextSize::Mm {
                    mm: dots_mm(dots, dpi),
                },
                dots,
                too_small: dots < min_dots,
            }
        })
        .collect();
    let (label, name) = QuickSizeId::Fit.labels();
    out.push(QuickTextSize {
        id: QuickSizeId::Fit,
        label,
        name,
        size: TextSize::Fit,
        dots: band,
        too_small: false,
    });
    out
}

/// The quick size `size` equals on this band (`matchQuickSize`).
pub fn match_quick_size(size: TextSize, band_dots: i64, dpi: u16) -> Option<QuickSizeId> {
    match size {
        TextSize::Fit => Some(QuickSizeId::Fit),
        TextSize::Pt { .. } => None,
        TextSize::Mm { mm } => {
            let dots = mm_to_dots(mm, dpi);
            quick_text_sizes(band_dots, dpi)
                .into_iter()
                .find(|q| q.id != QuickSizeId::Fit && q.dots == dots)
                .map(|q| q.id)
        }
    }
}

/// Point size of an em of `em_dots` to the nearest half point, within 4–144 pt (`emDotsToPt`).
pub fn em_dots_to_pt(em_dots: f64, dpi: u16) -> f64 {
    let half_pts = em_dots * 72.0 / f64::from(dpi) * 2.0;
    let pt = if half_pts.is_finite() {
        js_round(half_pts) as f64 / 2.0
    } else {
        12.0
    };
    pt.clamp(4.0, 144.0)
}

/// "Use for new text": the default-size preference that gives new text this block's size
/// (`defaultSizeOf`). Fit, M and S stay relative to the tape; other sizes become points.
pub fn default_size_of(size: TextSize, band_dots: i64, dpi: u16, em_dots: f64) -> DefaultTextSize {
    match size {
        TextSize::Fit => DefaultTextSize::Fit,
        TextSize::Pt { pt } => DefaultTextSize::Pt(pt),
        TextSize::Mm { .. } => match match_quick_size(size, band_dots, dpi) {
            Some(QuickSizeId::M) => DefaultTextSize::Half,
            Some(QuickSizeId::S) => DefaultTextSize::Third,
            _ => DefaultTextSize::Pt(em_dots_to_pt(em_dots, dpi)),
        },
    }
}

/// Estimated label length (mm) if a text block `from_dots` long became `to_dots` long, never
/// below `min_mm`; a fixed length does not change (`estimateLabelMm`).
pub fn estimate_label_mm(
    label_mm: f64,
    from_dots: f64,
    to_dots: f64,
    dpi: u16,
    min_mm: f64,
    fixed_mm: Option<f64>,
) -> f64 {
    if let Some(fixed) = fixed_mm {
        return fixed;
    }
    min_mm.max(label_mm + dots_to_mm(to_dots - from_dots, dpi))
}

/// Measured instead of a blank block for its em (`EM_SAMPLE`).
const EM_SAMPLE: &str = "Hxg";

fn same_size(a: TextSize, b: TextSize) -> bool {
    match (a, b) {
        (TextSize::Fit, TextSize::Fit) => true,
        (TextSize::Mm { mm: x }, TextSize::Mm { mm: y }) => x == y,
        (TextSize::Pt { pt: x }, TextSize::Pt { pt: y }) => x == y,
        _ => false,
    }
}

/// How long (dots along the label) and how large (em, dots) `item` would print at `size` in a
/// band (`measureTextAt`), with the renderer's sizing at the default crisp factor. A blank block
/// is 0 dots long with the em of a one-line sample.
pub fn measure_text_at(
    item: &TextItem,
    font: &ResolvedFont,
    size: TextSize,
    band_dots: i64,
    dpi: u16,
) -> (i64, f64) {
    let lines = split_lines(&item.text);
    if lines.iter().all(|l| is_blank(l)) {
        let sample = TextItem {
            text: EM_SAMPLE.into(),
            ..item.clone()
        };
        return (0, measure_text_at(&sample, font, size, band_dots, dpi).1);
    }
    let f = DEFAULT_CRISP_FACTOR;
    let ff = f64::from(f);
    let face = face_metrics(&font.face);
    let m = face_measure(&font.face);
    let sizing = TextSizing {
        size,
        // v1 clipping only applies to the item's own size: choosing another drops it.
        clip_tall: item.clip_tall && same_size(size, item.size),
        ..TextSizing::of(item)
    };
    let sized = size_text_block(
        &lines,
        face,
        &sizing,
        &m,
        &SizeEnv {
            band_px: band_dots.max(1) as f64 * ff,
            f,
            dpi,
            pixel: font.pixel_grid(),
            scale: None,
            max_w_px: None,
        },
    );
    (
        (sized.w_px / ff - 1e-6).ceil() as i64,
        sized.block.font_px / ff,
    )
}

/// What the quick size readouts need (`ReadoutInput`).
#[derive(Debug, Clone)]
pub struct ReadoutInput<'a> {
    /// The block as the preview shows it (placeholders filled).
    pub shown: &'a TextItem,
    pub font: &'a ResolvedFont,
    /// Its stored size and the quick size it matches, if any.
    pub size: TextSize,
    pub current: Option<QuickSizeId>,
    pub sizes: &'a [QuickTextSize],
    pub band_dots: i64,
    pub dpi: u16,
    pub min_mm: f64,
    /// `RenderResult.lengthMm`.
    pub length_mm: f64,
    /// `TextRenderInfo.widthDots` of the block in the last render; `None` before it rendered.
    pub rendered_width_dots: Option<i64>,
    /// The label grows with the text (auto length, flow); else the readouts are text lengths.
    pub grows: bool,
}

/// "≈ 31 mm" per quick size, the current one exact ("31 mm"); empty before the block rendered
/// (`quickReadouts`).
pub fn quick_readouts(o: &ReadoutInput<'_>) -> Vec<String> {
    let Some(info_w) = o.rendered_width_dots else {
        return Vec::new();
    };
    let from = measure_text_at(o.shown, o.font, o.size, o.band_dots, o.dpi).0;
    o.sizes
        .iter()
        .map(|q| {
            let now = Some(q.id) == o.current;
            let to = if now {
                from
            } else {
                measure_text_at(o.shown, o.font, q.size, o.band_dots, o.dpi).0
            };
            let mm = if o.grows {
                estimate_label_mm(o.length_mm, from as f64, to as f64, o.dpi, o.min_mm, None)
            } else {
                dots_to_mm(if now { info_w } else { to } as f64, o.dpi)
            };
            format!("{}{} mm", if now { "" } else { "≈ " }, js_round(mm))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------
// New text defaults and tape changes (ui/state/text-defaults.ts)
// ------------------------------------------------------------------------------------------

/// Narrowest tape on which 'auto' picks half height instead of fit (`AUTO_HALF_MIN_TAPE_MM`).
pub const AUTO_HALF_MIN_TAPE_MM: f64 = 12.0;

/// Printable band in dots per tape width on the PT-P710BT (180 dpi), for before the exact print
/// area is known (`FALLBACK_BAND_DOTS`). No 36 mm: see [`fallback_band`].
pub const FALLBACK_BAND_DOTS: [(f64, i64); 6] = [
    (3.5, 24),
    (6.0, 32),
    (9.0, 50),
    (12.0, 70),
    (18.0, 112),
    (24.0, 128),
];

/// The band to size text in before the print area is known (`fallbackBand`): the PT-P710BT's,
/// and for 36 mm the PT-P910BT's (454 dots at 360 dpi). `None` for a width no tape has.
pub fn fallback_band(width_mm: f64) -> Option<(i64, u16)> {
    if width_mm == 36.0 {
        return Some((454, 360));
    }
    FALLBACK_BAND_DOTS
        .iter()
        .find(|(w, _)| *w == width_mm)
        .map(|&(_, b)| (b, 180))
}

/// The `TextSize` a default-size preference stands for on this tape (`resolveDefaultTextSize`).
pub fn resolve_default_text_size(
    pref: DefaultTextSize,
    tape_width_mm: f64,
    band_dots: i64,
    dpi: u16,
) -> TextSize {
    let want = match pref {
        DefaultTextSize::Pt(pt) => return TextSize::Pt { pt },
        DefaultTextSize::Fit => return TextSize::Fit,
        DefaultTextSize::Auto if tape_width_mm < AUTO_HALF_MIN_TAPE_MM => return TextSize::Fit,
        DefaultTextSize::Auto | DefaultTextSize::Half => QuickSizeId::M,
        DefaultTextSize::Third => QuickSizeId::S,
    };
    quick_text_sizes(band_dots, dpi)
        .into_iter()
        .find(|q| q.id == want)
        .map_or(TextSize::Fit, |q| q.size)
}

/// Font and size fields of a new text block (`newTextDefaults`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewTextDefaults {
    pub font_family: &'static str,
    pub font_weight: u16,
    pub size: TextSize,
}

/// The font (preferred family at its nearest weight) and size of a new text block
/// (`newTextDefaults`).
pub fn new_text_defaults(
    family: &str,
    weight: u16,
    size_pref: DefaultTextSize,
    tape_width_mm: f64,
    band_dots: i64,
    dpi: u16,
) -> NewTextDefaults {
    let def = crate::render::fonts::font_def(family);
    NewTextDefaults {
        font_family: def.id,
        font_weight: crate::render::fonts::resolve_weight(def, weight),
        size: resolve_default_text_size(size_pref, tape_width_mm, band_dots, dpi),
    }
}

/// `sameTextSize`.
pub fn same_text_size(a: TextSize, b: TextSize) -> bool {
    same_size(a, b)
}

/// Whether `size` reads as sized in this band: one of its quick sizes (or Fit), or `fresh`, the
/// default new text gets there (`isBandSize`).
pub fn is_band_size(size: TextSize, band_dots: i64, dpi: u16, fresh: Option<TextSize>) -> bool {
    fresh.is_some_and(|f| same_size(size, f)) || match_quick_size(size, band_dots, dpi).is_some()
}

/// A block's size after the tape width changes (`retapeTextSize`): a quick size of the old band
/// becomes the same quick size of the new one; `fresh` = (old default, new default) of an
/// untouched label; Fit, points and other sizes are kept. `new_dpi` when the model changes too.
pub fn retape_text_size(
    size: TextSize,
    old_band_dots: i64,
    new_band_dots: i64,
    dpi: u16,
    fresh: Option<(TextSize, TextSize)>,
    new_dpi: u16,
) -> TextSize {
    if let Some((from, to)) = fresh
        && same_size(size, from)
    {
        return to;
    }
    match match_quick_size(size, old_band_dots, dpi) {
        None | Some(QuickSizeId::Fit) => size,
        Some(q) => quick_text_sizes(new_band_dots, new_dpi)
            .into_iter()
            .find(|s| s.id == q)
            .map_or(size, |s| s.size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_to_fixed() {
        assert_eq!(to_fixed(0.25, 1), "0.3");
        assert_eq!(to_fixed(1.05, 1), "1.1"); // 1.05 is 1.0500000000000000444
        assert_eq!(to_fixed(2.0, 1), "2.0");
        assert_eq!(to_fixed(9.96, 1), "10.0");
        assert_eq!(to_fixed(-0.04, 1), "-0.0");
        assert_eq!(to_fixed(-0.0, 1), "0.0");
        assert_eq!(to_fixed(1.45, 1), "1.4"); // 1.45 is 1.4499999999999999556
    }

    #[test]
    fn quantize_rounds_like_math_round() {
        assert_eq!(quantize(5.99992), 6.0);
        assert_eq!(quantize(-0.5 / 1024.0), 0.0);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod script_tests {
    use super::shaping_segments;
    use crate::render::fonts::{bundled_face, font_def};

    fn fira() -> crate::render::fonts::FontFace {
        bundled_face(font_def("fira-sans"), 600).expect("bundled Fira Sans")
    }

    /// No glyph in any segment is .notdef (glyph 0).
    fn no_missing_glyphs(text: &str) -> bool {
        shaping_segments(&fira(), text).iter().all(|(face, seg, _)| {
            face.with_shaper(|hb| seg.chars().all(|c| super::inherits_font(c) || hb.glyph_index(c).is_some()))
                .unwrap_or(false)
        })
    }

    #[test]
    fn persian_draws_with_an_installed_font() {
        let segs = shaping_segments(&fira(), "هانایی");
        assert_eq!(segs.len(), 1);
        let (face, text, rtl) = &segs[0];
        assert!(face.key().starts_with("fallback:"), "{}", face.key());
        assert_eq!(*text, "هانایی");
        assert!(rtl);
        assert!(no_missing_glyphs("هانایی"));
        // ZWNJ (common in Persian) stays inside the run.
        assert!(no_missing_glyphs("می\u{200C}خواهم"));
    }

    #[test]
    fn mixed_lines_are_reordered_visually() {
        // A left-to-right paragraph: Latin first, the Persian word last (on the right).
        let segs = shaping_segments(&fira(), "Rack 7 هانایی");
        let texts: Vec<&str> = segs.iter().map(|s| s.1).collect();
        assert_eq!(texts, ["Rack 7 ", "هانایی"]);
        assert!(!segs[0].0.key().starts_with("fallback:") && !segs[0].2);
        assert!(segs[1].2);
        // A right-to-left paragraph: the Persian word sits on the right, so it comes last in
        // visual (left-to-right) order.
        let segs = shaping_segments(&fira(), "قفسه Rack");
        let texts: Vec<&str> = segs.iter().map(|s| s.1).collect();
        assert_eq!(texts.last().copied(), Some("قفسه"));
        assert!(texts.first().is_some_and(|t| t.contains("Rack")));
    }

    #[test]
    fn latin_text_stays_in_one_segment() {
        let segs = shaping_segments(&fira(), "Label 123");
        assert_eq!(segs.len(), 1);
        assert!(!segs[0].2);
    }
}
