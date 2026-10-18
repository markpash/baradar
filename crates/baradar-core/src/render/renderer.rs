//! `LabelDoc` → 1-bit `Bitmap` (port of `web/src/render/renderer.ts`).
//!
//! The crisp plane (text, icons, shapes, the frame, code text) is drawn anti-aliased at
//! factor f onto a white `tiny_skia::Pixmap` and thresholded by `LabelRaster::blit_crisp`;
//! images go through `blit_tone`, codes through `blit_code`. Compositing order: tone → crisp →
//! codes. The returned bitmap is both the preview and the printed page: x runs along the label
//! (column 0 = left end as read), y across the tape (row 0 = top of the printable band).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ptouch::{Bitmap, Dither, LabelRaster, ModelProfile, TapeSpec, ToneAdjust};
use tiny_skia::{Color, FillRule, Mask, PathBuilder, Pixmap, PixmapMut, Transform};

use crate::doc::files::data_url_to_bytes;
use crate::doc::{
    Align, CodeItem, IconItem, ImageItem, Item, LabelDoc, LengthSettings, ShapeItem, ShapeKind,
    TextItem,
};
use crate::render::codes::{CodeBlit, CodeContext, code_text_refit_px, plan_code, rotate_matrix, tape_margin_dots};
use crate::render::fonts::{CODE_TEXT_FONT, FontFace, FontRegistry};
use crate::render::icons::{ICON_REDUCED_WARNING, IconDef, draw_icon, icon_or_fallback, icon_side_dots};
use crate::render::images::{
    DecodedImage, IMAGE_GONE_WARNING, IMAGE_NONE_WARNING, IMAGE_REDUCED_WARNING,
    IMAGE_UNREADABLE_WARNING, RgbaImage, fit_frame_width, image_height_dots, image_too_wide_warning,
    tone_options,
};
use crate::render::layout::{
    Clamped, Layout, LayoutLimits, MeasuredItem, Placement, content_length, flow_settings,
    layout_flow, layout_free,
};
use crate::render::shapes::{
    FrameGeom, SHAPE_REDUCED_WARNING, draw_frame, draw_invalid_code, draw_shape, frame_geom,
    shape_size,
};
use crate::render::text::{
    DEFAULT_CRISP_FACTOR, FitOpts, MIN_SHRINK, PreparedText, TextBlock, TextRenderInfo,
    draw_text_block, face_measure, face_metrics, fit_text_block, prepare_text, shrink_factor,
};
use crate::render::units::{dots_to_mm, js_round, mm_to_dots};

/// Widest crisp canvas drawn at once (px); longer labels are drawn in tiles.
const MAX_TILE_PX: i64 = 8190;

/// Shrinking keeps auto linear codes at their default module while the text needs no more than
/// this factor (`KEEP_CODE_MODULE_SHRINK`).
const KEEP_CODE_MODULE_SHRINK: f64 = 0.5;

/// What a model can print on one tape (`PrintArea` of the wasm bindings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintArea {
    pub model: &'static str,
    pub media_id: &'static str,
    pub dpi: u16,
    /// Printable band across the tape (`TapeSpec::print_pins`).
    pub height_dots: i64,
    pub tape_width_dots: i64,
    pub min_length_dots: i64,
    pub max_length_dots: Option<i64>,
    /// Feed margin the printer adds at each end.
    pub default_feed_dots: i64,
    pub max_feed_dots: i64,
}

/// What we render for: a model and its loaded (or chosen) tape.
#[derive(Debug, Clone, Copy)]
pub struct RenderTarget {
    pub profile: &'static ModelProfile,
    pub tape: &'static TapeSpec,
}

impl RenderTarget {
    /// `printArea(model, mediaId)`.
    pub fn area(&self) -> PrintArea {
        let (p, t) = (self.profile, self.tape);
        PrintArea {
            model: p.name,
            media_id: t.id,
            dpi: p.dpi,
            height_dots: i64::from(t.print_pins),
            tape_width_dots: i64::from(t.tape_width_dots),
            min_length_dots: i64::from(p.min_lines(t.default_feed_dots, false)),
            max_length_dots: p.max_lines_for(t, false).map(i64::from),
            default_feed_dots: i64::from(t.default_feed_dots),
            max_feed_dots: i64::from(p.max_margin_dots(false)),
        }
    }
}

/// A problem with the design (`RenderWarning`). `code` is the studio's warning code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderWarning {
    pub code: &'static str,
    pub item_id: Option<String>,
    /// User-facing sentence (English).
    pub message: String,
    /// Blocks printing. Blocking warnings come first in [`RenderResult::warnings`].
    pub blocking: bool,
}

/// Item bounding box in dots (`ItemBox`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemBox {
    pub item_id: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// Result of [`Renderer::render`] (`RenderResult`).
#[derive(Debug, Clone)]
pub struct RenderResult {
    /// The exact dots that are previewed and printed.
    pub bitmap: Bitmap,
    pub length_dots: i64,
    pub height_dots: i64,
    pub length_mm: f64,
    /// Printer feed margin at each end (mm), drawn by the preview, not part of the bitmap.
    pub feed_margin_mm: f64,
    pub boxes: Vec<ItemBox>,
    pub warnings: Vec<RenderWarning>,
    pub blocking: bool,
    /// Content is longer than a fixed length / the printer maximum, or taller than the band.
    pub overflow: bool,
    /// One entry per non-empty text block.
    pub texts: Vec<TextRenderInfo>,
}

/// Rendering was cancelled through [`RenderOptions::cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Rendering was cancelled")]
pub struct Cancelled;

/// Options for one render.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions<'a> {
    /// Crisp-plane supersampling factor (1–7; 0 = default 3).
    pub factor: u32,
    /// Checked between steps; set it to abandon a stale render.
    pub cancel: Option<&'a AtomicBool>,
}

/// Resolves an image `blobRef` to its bytes (the store's `get_blob`).
pub type BlobLoader = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

/// Shared rendering resources: fonts, image loading and a small decoded-image cache. Cheap to
/// share across threads (`Arc<Renderer>`).
pub struct Renderer {
    pub fonts: Arc<FontRegistry>,
    blobs: Option<BlobLoader>,
    images: Mutex<HashMap<String, Arc<DecodedImage>>>,
}

/// Most decoded images kept between renders.
const IMAGE_CACHE: usize = 16;

/// Content drawn on the crisp plane inside an item's box.
enum Crisp {
    Text(Box<PreparedText>),
    Icon { icon: &'static IconDef, side: i64 },
    Shape { shape: ShapeKind, fill: bool, stroke: i64 },
    Invalid,
    CodeText { face: FontFace, block: TextBlock, top: i64, text_h: i64 },
}

/// An item measured and ready to draw. Sizes in dots, unrotated (`Prepared`).
struct Prepared {
    id: String,
    w: i64,
    h: i64,
    spacer: bool,
    crisp: Option<Crisp>,
    /// Horizontal clip slack in px (italic overhang).
    clip_slack: f64,
    tone: Option<(RgbaImage, Dither, ToneAdjust)>,
    codes: Vec<CodeBlit>,
    too_tall: bool,
    /// Already rotated (images are decoded rotated).
    prerotated: bool,
    /// A linear code with `moduleDots: 'auto'`.
    auto_linear: bool,
    text: Option<TextRenderInfo>,
}

impl Prepared {
    fn empty(id: &str, w: i64, h: i64) -> Self {
        Self {
            id: id.to_owned(),
            w,
            h,
            spacer: false,
            crisp: None,
            clip_slack: 0.0,
            tone: None,
            codes: Vec::new(),
            too_tall: false,
            prerotated: false,
            auto_linear: false,
            text: None,
        }
    }

    fn measured(&self) -> MeasuredItem {
        MeasuredItem {
            item_id: self.id.clone(),
            w: self.w,
            h: self.h,
            spacer: self.spacer,
        }
    }
}

/// A prepared item at its final place (dots); `w`/`h` are the placed (rotated) box.
struct Placed {
    p: Prepared,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    rotation: u16,
}

#[derive(Clone, Copy)]
struct Ctx<'a> {
    f: u32,
    dpi: u16,
    /// Unprinted tape beyond the band edge usable as a code's vertical quiet zone.
    margin_dots: i64,
    /// Length a linear `'auto'` code may take on a fixed-length label.
    auto_linear_w: Option<i64>,
    r: &'a Renderer,
}

/// Warnings with the studio's de-duplication (`code|itemId|message`).
#[derive(Default)]
struct Warnings {
    list: Vec<RenderWarning>,
    seen: HashSet<String>,
}

impl Warnings {
    fn push(&mut self, w: RenderWarning) {
        let key = format!("{}|{}|{}", w.code, w.item_id.as_deref().unwrap_or(""), w.message);
        if self.seen.insert(key) {
            self.list.push(w);
        }
    }

    fn extend(&mut self, ws: impl IntoIterator<Item = RenderWarning>) {
        for w in ws {
            self.push(w);
        }
    }
}

fn warning(code: &'static str, item_id: &str, message: impl Into<String>, blocking: bool) -> RenderWarning {
    RenderWarning {
        code,
        item_id: Some(item_id.to_owned()),
        message: message.into(),
        blocking,
    }
}

fn quarter(r: u16) -> bool {
    r == 90 || r == 270
}

fn valid_rotation(r: u16) -> u16 {
    if matches!(r, 0 | 90 | 180 | 270) { r } else { 0 }
}

/// `oddFactor`: rounded, clamped to 1–7 (default 3).
fn crisp_factor(f: u32) -> u32 {
    if f == 0 { DEFAULT_CRISP_FACTOR } else { f.clamp(1, 7) }
}

impl Renderer {
    pub fn new(fonts: Arc<FontRegistry>) -> Self {
        Self {
            fonts,
            blobs: None,
            images: Mutex::new(HashMap::new()),
        }
    }

    /// Sets how image `blobRef`s are resolved.
    pub fn with_blob_loader(mut self, loader: BlobLoader) -> Self {
        self.blobs = Some(loader);
        self
    }

    /// `renderLabel`: renders `doc` for `target`. Problems with the design are reported as
    /// warnings (and `blocking`); the only error is cancellation.
    pub fn render(
        &self,
        doc: &LabelDoc,
        target: &RenderTarget,
        opts: &RenderOptions<'_>,
    ) -> Result<RenderResult, Cancelled> {
        let check = || match opts.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(Cancelled),
            _ => Ok(()),
        };
        let area = target.area();
        let f = crisp_factor(opts.factor);
        let dpi = area.dpi;
        let band = area.height_dots;
        let mut warnings = Warnings::default();

        check()?;
        for issue in self.fonts.ensure_fonts(doc).issues() {
            warnings.push(RenderWarning {
                code: issue.code(),
                item_id: None,
                message: issue.message(),
                blocking: false,
            });
        }

        let frame = frame_geom(doc.frame.as_ref(), band, dpi);
        let reserve = frame.map_or(0, |g| g.reserve);
        let inner = (band - 2 * reserve).max(1);
        // A code in the flow may count the unprinted tape margin as quiet zone, unless a frame
        // line runs along the band edge; codes in free-layout frames never do.
        let c = Ctx {
            f,
            dpi,
            margin_dots: if frame.is_some() {
                0
            } else {
                tape_margin_dots(area.tape_width_dots, area.height_dots)
            },
            auto_linear_w: None,
            r: self,
        };
        let c_framed = Ctx { margin_dots: 0, ..c };

        let free = matches!(doc.layout, crate::doc::LayoutSettings::Free);
        let shrinking = matches!(doc.length, LengthSettings::Fixed { shrink: true, .. });
        let mut flow: Vec<Prepared> = Vec::new();
        let mut flow_source: Vec<&Item> = Vec::new();
        // Flow text measured with its warnings held back until its final size.
        let mut held: HashMap<usize, Vec<RenderWarning>> = HashMap::new();
        let mut framed: Vec<Placed> = Vec::new();
        for item in &doc.items {
            let frame_box = if free { item.frame() } else { None };
            if let Some(fr) = frame_box {
                let rotation = valid_rotation(fr.rotation);
                let bw = mm_to_dots(fr.w_mm, dpi).max(1);
                let bh = mm_to_dots(fr.h_mm, dpi).max(1);
                // Images are decoded already rotated; everything else is measured upright and
                // rotated when drawn.
                let upright = matches!(item, Item::Image(_)) || !quarter(rotation);
                let (p, ws) = prepare_item(
                    item,
                    if upright { bh } else { bw },
                    &c_framed,
                    rotation,
                    Some(if upright { bw } else { bh }),
                );
                warnings.extend(ws);
                let (pw, ph) = if p.prerotated || !quarter(rotation) {
                    (p.w, p.h)
                } else {
                    (p.h, p.w)
                };
                if !p.spacer && (pw > bw || ph > bh) {
                    warnings.push(warning(
                        "content-overflow",
                        item.id(),
                        "This item is larger than its frame and is cut off at the frame edge.",
                        false,
                    ));
                }
                let rotation = if p.prerotated { 0 } else { rotation };
                framed.push(Placed {
                    p,
                    x: mm_to_dots(fr.x_mm, dpi),
                    y: mm_to_dots(fr.y_mm, dpi),
                    w: bw,
                    h: bh,
                    rotation,
                });
            } else if let (true, Item::Text(t)) = (shrinking, item) {
                let (p, ws) = prepare_text_item(t, inner, &c, None, 1.0);
                held.insert(flow.len(), ws);
                flow.push(p);
                flow_source.push(item);
            } else {
                let (p, ws) = prepare_item(item, inner, &c, 0, None);
                warnings.extend(ws);
                flow.push(p);
                flow_source.push(item);
            }
            check()?;
        }

        let limits = LayoutLimits {
            min_length_dots: area.min_length_dots,
            max_length_dots: area.max_length_dots,
            inset_dots: reserve,
        };
        if shrinking {
            shrink_text_to_length(doc, &mut flow, &flow_source, inner, &c, &limits, &mut held);
            let mut keys: Vec<_> = held.keys().copied().collect();
            keys.sort_unstable();
            for k in keys {
                warnings.extend(held.remove(&k).unwrap_or_default());
            }
        }
        size_auto_linear_codes(doc, &mut flow, &flow_source, inner, &c, &limits, &mut warnings);
        check()?;

        let measured: Vec<MeasuredItem> = flow.iter().map(Prepared::measured).collect();
        let mut layout: Layout = layout_flow(doc, &measured, band, dpi, &limits);
        let flow_count = flow.len();
        let mut placed: Vec<Placed> = flow
            .into_iter()
            .zip(&layout.placements)
            .map(|(p, pl)| Placed {
                p,
                x: pl.x,
                y: pl.y,
                w: pl.w,
                h: pl.h,
                rotation: 0,
            })
            .collect();
        if !framed.is_empty() {
            let boxes: Vec<Placement> = framed
                .iter()
                .map(|fi| Placement {
                    item_id: fi.p.id.clone(),
                    x: fi.x,
                    y: fi.y,
                    w: fi.w,
                    h: fi.h,
                    spacer: false,
                })
                .collect();
            let fl = layout_free(doc, &boxes, band, dpi, &limits);
            let flow_len = if flow_count > 0 { layout.length_dots } else { 0 };
            layout = Layout {
                length_dots: fl.length_dots.max(flow_len),
                overflow: fl.overflow || (flow_count > 0 && layout.overflow),
                ..fl
            };
            for fi in framed {
                // Centre the (rotated) content in its frame box.
                let (cw, ch) = if quarter(fi.rotation) {
                    (fi.p.h, fi.p.w)
                } else {
                    (fi.p.w, fi.p.h)
                };
                let (w, h) = (cw.min(fi.w), ch.min(fi.h));
                placed.push(Placed {
                    x: fi.x + (fi.w - w).div_euclid(2),
                    y: fi.y + (fi.h - h).div_euclid(2),
                    w,
                    h,
                    rotation: fi.rotation,
                    p: fi.p,
                });
            }
        }
        let length = layout.length_dots;

        match layout.clamped {
            Some(Clamped::Min) => warnings.push(RenderWarning {
                code: "length-clamped",
                item_id: None,
                message: format!(
                    "The label was lengthened to the printer minimum of {} mm.",
                    to_fixed1(dots_to_mm(length as f64, dpi))
                ),
                blocking: false,
            }),
            Some(Clamped::Max) => warnings.push(RenderWarning {
                code: "length-clamped",
                item_id: None,
                message: format!(
                    "The label was shortened to the printer maximum of {} mm.",
                    js_round(dots_to_mm(length as f64, dpi))
                ),
                blocking: false,
            }),
            None => {}
        }
        if layout.overflow {
            let fixed = matches!(doc.length, LengthSettings::Fixed { .. });
            let hint = if fixed && !shrinking && flow_source.iter().any(|i| matches!(i, Item::Text(_))) {
                " Turn on “Shrink text to fit length” or make the label longer."
            } else {
                ""
            };
            warnings.push(RenderWarning {
                code: "content-overflow",
                item_id: None,
                message: if fixed {
                    format!("The content is longer than the fixed label length and will be cut off.{hint}")
                } else {
                    "The content does not fit on the label and will be cut off.".into()
                },
                blocking: false,
            });
        }
        check()?;

        // Compose: tone → crisp → codes.
        let length_u = u32::try_from(length.max(1)).unwrap_or(u32::MAX);
        let band_u = u16::try_from(band.max(1)).unwrap_or(u16::MAX);
        let mut raster = match LabelRaster::try_new(length_u, band_u) {
            Ok(r) => r,
            Err(e) => {
                warnings.push(RenderWarning {
                    code: "content-overflow",
                    item_id: None,
                    message: format!("The label is too large to render: {e}"),
                    blocking: true,
                });
                LabelRaster::new(1, band_u)
            }
        };
        for pl in &placed {
            if let Some((img, dither, adj)) = &pl.p.tone {
                let dy = (pl.h - i64::from(img.height)).div_euclid(2);
                let dx = (pl.w - i64::from(img.width)).div_euclid(2);
                let _ = raster.blit_tone(
                    &img.rgba,
                    img.width,
                    img.height,
                    (pl.x + dx) as i32,
                    (pl.y + dy) as i32,
                    *dither,
                    *adj,
                );
            }
        }
        let threshold = doc.print.threshold;
        paint_crisp(&mut raster, &placed, frame.as_ref(), length, band, f, threshold, opts.cancel)?;
        for pl in &placed {
            for cb in &pl.p.codes {
                let uw = pl.p.w;
                let uh = if quarter(pl.rotation) { pl.w } else { pl.h };
                // Unrotated box is (p.w × placed cross size); centre vertically if the band clipped it.
                let r = rotate_rect(
                    (
                        cb.dx,
                        cb.dy + (uh - pl.p.h).div_euclid(2),
                        i64::from(cb.m.width) * i64::from(cb.md),
                        i64::from(cb.m.height) * i64::from(cb.md),
                    ),
                    uw,
                    uh,
                    pl.rotation,
                );
                // The padded light modules are the quiet zone: the core clears and protects them.
                let _ = raster.blit_code(
                    &rotate_matrix(&cb.m, pl.rotation),
                    (pl.x + r.0) as i32,
                    (pl.y + r.1) as i32,
                    cb.md,
                    false,
                );
            }
        }
        let bitmap = raster.finish();

        if bitmap.is_blank() && !warnings.list.iter().any(|w| w.blocking) {
            warnings.push(RenderWarning {
                code: "empty",
                item_id: None,
                message: if doc.items.is_empty() {
                    "The label is empty. Add some text, an icon or a code.".into()
                } else {
                    "Nothing would be printed. Add some text, an icon or a code.".into()
                },
                blocking: true,
            });
        }
        let boxes = placed
            .iter()
            .filter(|pl| pl.w > 0 || pl.p.spacer)
            .map(|pl| {
                if pl.p.spacer {
                    ItemBox {
                        item_id: pl.p.id.clone(),
                        x: pl.x,
                        y: reserve,
                        w: pl.w.max(1),
                        h: inner,
                    }
                } else {
                    ItemBox {
                        item_id: pl.p.id.clone(),
                        x: pl.x,
                        y: pl.y,
                        w: pl.w,
                        h: pl.h,
                    }
                }
            })
            .collect();
        let mut list = warnings.list;
        let blocking = list.iter().any(|w| w.blocking);
        list.sort_by_key(|w| !w.blocking);
        let overflow = layout.overflow || placed.iter().any(|pl| pl.p.too_tall);
        let texts = placed.into_iter().filter_map(|pl| pl.p.text).collect();
        Ok(RenderResult {
            bitmap,
            length_dots: length,
            height_dots: band,
            length_mm: dots_to_mm(length as f64, dpi),
            feed_margin_mm: dots_to_mm(area.default_feed_dots as f64, dpi),
            boxes,
            warnings: list,
            blocking,
            overflow,
            texts,
        })
    }

    /// Decoded image for an item (cached by blob ref or data URL).
    fn decoded_image(&self, item: &ImageItem) -> Result<Option<Arc<DecodedImage>>, ()> {
        let key = if !item.blob_ref.is_empty() {
            item.blob_ref.clone()
        } else if let Some(url) = &item.data_url {
            format!("data:{}", fxhash(url))
        } else {
            return Ok(None);
        };
        if let Ok(cache) = self.images.lock()
            && let Some(img) = cache.get(&key)
        {
            return Ok(Some(img.clone()));
        }
        let bytes = if !item.blob_ref.is_empty() {
            match self.blobs.as_ref().and_then(|load| load(&item.blob_ref)) {
                Some(b) => Some(b),
                None => item.data_url.as_deref().and_then(|u| data_url_to_bytes(u).ok()).map(|(b, _)| b),
            }
        } else {
            item.data_url.as_deref().and_then(|u| data_url_to_bytes(u).ok()).map(|(b, _)| b)
        };
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let img = Arc::new(DecodedImage::decode(&bytes).map_err(|_| ())?);
        if let Ok(mut cache) = self.images.lock() {
            if cache.len() >= IMAGE_CACHE {
                cache.clear();
            }
            cache.insert(key, img.clone());
        }
        Ok(Some(img))
    }
}

/// `toFixed(1)` (JS rounds halves up).
fn to_fixed1(v: f64) -> String {
    crate::render::codes::js_to_fixed(v, 1)
}

fn fxhash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `rotateRect`: rotates a rect inside a `w × h` box clockwise.
fn rotate_rect(r: (i64, i64, i64, i64), w: i64, h: i64, rot: u16) -> (i64, i64, i64, i64) {
    let (x, y, rw, rh) = r;
    match rot {
        90 => (h - (y + rh), x, rh, rw),
        180 => (w - (x + rw), h - (y + rh), rw, rh),
        270 => (y, w - (x + rw), rh, rw),
        _ => r,
    }
}

// ------------------------------------------------------------------------------------------
// Items
// ------------------------------------------------------------------------------------------

/// `prepareItem`: `band` = available height across the item; `max_w` (free layout) = width.
fn prepare_item(item: &Item, band: i64, c: &Ctx<'_>, rotation: u16, max_w: Option<i64>) -> (Prepared, Vec<RenderWarning>) {
    match item {
        Item::Text(t) => prepare_text_item(t, band, c, max_w, 1.0),
        Item::Icon(i) => prepare_icon(i, band, c, max_w),
        Item::Shape(s) => prepare_shape(s, band, c),
        Item::Code(code) => prepare_code(code, band, c, max_w),
        Item::Image(img) => prepare_image(img, band, c, rotation, max_w),
        Item::Spacer(s) => {
            let mut p = Prepared::empty(&s.id, mm_to_dots(s.width_mm, c.dpi).max(0), 0);
            p.spacer = true;
            (p, Vec::new())
        }
    }
}

fn prepare_text_item(
    item: &TextItem,
    band: i64,
    c: &Ctx<'_>,
    max_w: Option<i64>,
    scale: f64,
) -> (Prepared, Vec<RenderWarning>) {
    let font = match c.r.fonts.resolve_item(item) {
        Ok(f) => f,
        Err(e) => {
            let w = warning("font-fallback", &item.id, format!("The font could not be loaded: {e}"), false);
            return (Prepared::empty(&item.id, 0, 0), vec![w]);
        }
    };
    let Some(t) = prepare_text(item, &font, band, c.f, c.dpi, max_w.map(|w| w as f64), scale) else {
        return (Prepared::empty(&item.id, 0, 0), Vec::new());
    };
    let warnings = t
        .warnings
        .iter()
        .map(|w| warning(w.code, &item.id, w.message.clone(), false))
        .collect();
    let mut p = Prepared::empty(&item.id, t.w, t.h);
    p.too_tall = t.too_tall;
    p.clip_slack = t.clip_slack;
    p.text = Some(t.info.clone());
    p.crisp = Some(Crisp::Text(Box::new(t)));
    (p, warnings)
}

fn prepare_icon(item: &IconItem, band: i64, c: &Ctx<'_>, max_w: Option<i64>) -> (Prepared, Vec<RenderWarning>) {
    let (side, reduced) = icon_side_dots(&item.size, band, max_w, c.dpi);
    let side = side.max(1);
    let mut warnings = Vec::new();
    if reduced {
        warnings.push(warning("content-overflow", &item.id, ICON_REDUCED_WARNING, false));
    }
    let mut p = Prepared::empty(&item.id, side, side);
    p.crisp = Some(Crisp::Icon {
        icon: icon_or_fallback(&item.icon_id),
        side,
    });
    (p, warnings)
}

fn prepare_shape(item: &ShapeItem, band: i64, c: &Ctx<'_>) -> (Prepared, Vec<RenderWarning>) {
    let s = shape_size(item, band, c.dpi);
    let mut warnings = Vec::new();
    if s.reduced {
        warnings.push(warning("content-overflow", &item.id, SHAPE_REDUCED_WARNING, false));
    }
    let mut p = Prepared::empty(&item.id, s.w, s.h);
    p.crisp = Some(Crisp::Shape {
        shape: item.shape,
        fill: item.fill,
        stroke: s.stroke_dots,
    });
    (p, warnings)
}

fn prepare_code(item: &CodeItem, band: i64, c: &Ctx<'_>, max_w: Option<i64>) -> (Prepared, Vec<RenderWarning>) {
    let ctx = CodeContext {
        dpi: c.dpi,
        margin_dots: c.margin_dots,
        auto_linear_w: c.auto_linear_w,
    };
    let plan = plan_code(item, band, &ctx, max_w);
    let warnings = plan
        .warnings
        .iter()
        .map(|w| warning(w.code, &item.id, w.message.clone(), w.blocking))
        .collect();
    let mut p = Prepared::empty(&item.id, plan.w, plan.h);
    p.auto_linear = plan.auto_linear;
    if plan.invalid {
        p.crisp = Some(Crisp::Invalid);
        return (p, warnings);
    }
    p.codes = plan.blits;
    if let Some(text) = plan.text
        && let Some((face, block)) = code_text_block(&text.text, text.strip.text_h, plan.w, c)
    {
        p.crisp = Some(Crisp::CodeText {
            face,
            block,
            top: text.strip.top,
            text_h: text.strip.text_h,
        });
    }
    (p, warnings)
}

/// The human-readable line under a linear code: JetBrains Mono fitted to the strip, refitted
/// narrower when wider than the code.
fn code_text_block(label: &str, text_h: i64, code_w: i64, c: &Ctx<'_>) -> Option<(FontFace, TextBlock)> {
    let (family, weight) = CODE_TEXT_FONT;
    let font = c.r.fonts.resolve(family, weight, false, None).ok()?;
    let face = font.face;
    let metrics = face_metrics(&face);
    let measure = face_measure(&face);
    let f = f64::from(c.f);
    let lines = [label];
    let mut block = fit_text_block(&lines, metrics, 1.0, Align::Center, &measure, FitOpts::Fit {
        avail_px: text_h as f64 * f,
    });
    if block.width > code_w as f64 * f {
        let target_px = code_text_refit_px(block.font_px, block.width, code_w, c.f, metrics.cap, metrics.desc);
        block = fit_text_block(&lines, metrics, 1.0, Align::Center, &measure, FitOpts::Target { target_px });
    }
    drop(measure);
    Some((face, block))
}

fn prepare_image(
    item: &ImageItem,
    band: i64,
    c: &Ctx<'_>,
    rotation: u16,
    max_w: Option<i64>,
) -> (Prepared, Vec<RenderWarning>) {
    let mut warnings = Vec::new();
    let (mut h, reduced) = image_height_dots(&item.size, band, c.dpi);
    if reduced {
        warnings.push(warning("content-overflow", &item.id, IMAGE_REDUCED_WARNING, false));
    }
    let placeholder = Prepared::empty(&item.id, h.max(1), h);
    let decoded = match c.r.decoded_image(item) {
        Ok(Some(d)) => d,
        Ok(None) => {
            let msg = if item.blob_ref.is_empty() && item.data_url.is_none() {
                IMAGE_NONE_WARNING
            } else {
                IMAGE_GONE_WARNING
            };
            warnings.push(warning("image-missing", &item.id, msg, false));
            return (placeholder, warnings);
        }
        Err(()) => {
            warnings.push(warning("image-missing", &item.id, IMAGE_UNREADABLE_WARNING, false));
            return (placeholder, warnings);
        }
    };
    if let Some(max_w) = max_w {
        let (nw, nh) = decoded.size();
        h = fit_frame_width(h, nw, nh, rotation, max_w);
    }
    let img = match decoded.scaled(h, rotation) {
        Ok(img) => img,
        Err(_) => {
            warnings.push(warning("image-missing", &item.id, IMAGE_UNREADABLE_WARNING, false));
            return (placeholder, warnings);
        }
    };
    if i64::from(img.height) < h {
        warnings.push(warning("content-overflow", &item.id, image_too_wide_warning(c.dpi), false));
    }
    let (dither, adj) = tone_options(item.dither, &item.adjust);
    let mut p = Prepared::empty(&item.id, i64::from(img.width), i64::from(img.height));
    p.prerotated = true;
    p.tone = Some((img, dither, adj));
    (p, warnings)
}

// ------------------------------------------------------------------------------------------
// Fixed-length rules
// ------------------------------------------------------------------------------------------

fn fixed_mm(doc: &LabelDoc) -> Option<f64> {
    match doc.length {
        LengthSettings::Fixed { mm, .. } => Some(mm),
        LengthSettings::Auto => None,
    }
}

fn label_ends(doc: &LabelDoc, dpi: u16, limits: &LayoutLimits) -> i64 {
    mm_to_dots(doc.margins_mm.start, dpi).max(0) + mm_to_dots(doc.margins_mm.end, dpi).max(0) + 2 * limits.inset_dots
}

/// `sizeAutoLinearCodes`: linear `'auto'` codes on a fixed-length label share the length the
/// other flow blocks leave (largest module up to 4 dots at 180 dpi that fits).
fn size_auto_linear_codes(
    doc: &LabelDoc,
    prepared: &mut [Prepared],
    items: &[&Item],
    band: i64,
    c: &Ctx<'_>,
    limits: &LayoutLimits,
    warnings: &mut Warnings,
) {
    let Some(mm) = fixed_mm(doc) else { return };
    let auto: Vec<usize> = (0..prepared.len()).filter(|&i| prepared[i].auto_linear).collect();
    if auto.is_empty() {
        return;
    }
    let measured: Vec<MeasuredItem> = prepared.iter().map(Prepared::measured).collect();
    let content = layout_flow(doc, &measured, band, c.dpi, &LayoutLimits::default()).content_dots;
    let others = content - auto.iter().map(|&i| prepared[i].w).sum::<i64>();
    let each = (mm_to_dots(mm, c.dpi) - label_ends(doc, c.dpi, limits) - others).div_euclid(auto.len() as i64);
    for i in auto {
        if let Item::Code(code) = items[i] {
            let ctx = Ctx {
                auto_linear_w: Some(each.max(0)),
                ..*c
            };
            let (p, ws) = prepare_code(code, band, &ctx, None);
            prepared[i] = p;
            warnings.extend(ws);
        }
    }
}

/// `shrinkTextToLength`: "Shrink to fit length" measures every flow text block again at one
/// common scale (< 1) until the flow content fits the fixed length. `held` maps flow indices
/// of text blocks to their (replaced) warnings.
fn shrink_text_to_length(
    doc: &LabelDoc,
    prepared: &mut [Prepared],
    items: &[&Item],
    band: i64,
    c: &Ctx<'_>,
    limits: &LayoutLimits,
    held: &mut HashMap<usize, Vec<RenderWarning>>,
) {
    let Some(mm) = fixed_mm(doc) else { return };
    if held.is_empty() {
        return;
    }
    let (gap, _) = flow_settings(doc, c.dpi);
    let avail = mm_to_dots(mm, c.dpi) - label_ends(doc, c.dpi, limits);
    let mut texts: Vec<usize> = held.keys().copied().collect();
    texts.sort_unstable();
    let content = |prepared: &[Prepared]| {
        let m: Vec<MeasuredItem> = prepared.iter().map(Prepared::measured).collect();
        content_length(&m, gap)
    };

    let run = |prepared: &mut [Prepared], held: &mut HashMap<usize, Vec<RenderWarning>>| -> (f64, bool) {
        let mut scale = 1.0_f64;
        for round in 0..8 {
            let text_dots: i64 = texts.iter().map(|&i| prepared[i].w).sum();
            let s = shrink_factor(avail as f64, (content(prepared) - text_dots) as f64, text_dots as f64);
            let Some(s) = s.filter(|s| *s < 1.0) else { break };
            let next = (scale * s * if round == 0 { 1.0 } else { 0.995 }).max(MIN_SHRINK);
            if next >= scale {
                break;
            }
            scale = next;
            for &i in &texts {
                if let Item::Text(t) = items[i] {
                    let (p, ws) = prepare_text_item(t, band, c, None, scale);
                    prepared[i] = p;
                    held.insert(i, ws);
                }
            }
        }
        (scale, content(prepared) <= avail)
    };

    let autos: Vec<usize> = (0..prepared.len())
        .filter(|&i| prepared[i].auto_linear && matches!(items[i], Item::Code(_)))
        .collect();
    // Re-preparing is cheap enough that "snapshots" are kept as the inputs needed to rebuild.
    let a = run(prepared, held);
    if autos.is_empty() || (a.1 && a.0 >= KEEP_CODE_MODULE_SHRINK) {
        return;
    }
    let keep_a: Vec<(usize, Prepared, Option<Vec<RenderWarning>>)> = texts
        .iter()
        .chain(&autos)
        .map(|&i| (i, std::mem::replace(&mut prepared[i], Prepared::empty("", 0, 0)), held.get(&i).cloned()))
        .collect();
    // Try again from the original sizes with the auto codes at 1 dot per module.
    for &i in &texts {
        if let Item::Text(t) = items[i] {
            let (p, ws) = prepare_text_item(t, band, c, None, 1.0);
            prepared[i] = p;
            held.insert(i, ws);
        }
    }
    let quiet = Ctx {
        auto_linear_w: Some(0),
        ..*c
    };
    for &i in &autos {
        if let Item::Code(code) = items[i] {
            prepared[i] = prepare_code(code, band, &quiet, None).0;
        }
    }
    let b = run(prepared, held);
    if !(b.1 && (!a.1 || b.0 > a.0)) {
        for (i, p, ws) in keep_a {
            prepared[i] = p;
            if let Some(ws) = ws {
                held.insert(i, ws);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Crisp plane
// ------------------------------------------------------------------------------------------

impl Crisp {
    fn draw(&self, pm: &mut PixmapMut<'_>, bw: f32, bh: f32, f: u32, t: Transform, mask: Option<&Mask>) {
        match self {
            Self::Text(text) => text.draw(pm, f64::from(bw), f64::from(bh), t, mask),
            Self::Icon { icon, side } => draw_icon(pm, t, mask, icon, *side, bw, bh, f),
            Self::Shape { shape, fill, stroke } => draw_shape(pm, t, mask, *shape, *fill, *stroke, bw, bh, f),
            Self::Invalid => draw_invalid_code(pm, t, mask, bw, bh, f),
            Self::CodeText { face, block, top, text_h } => {
                let ff = f64::from(f);
                let x = (f64::from(bw) - block.width) / 2.0;
                let y = *top as f64 * ff + (*text_h as f64 * ff - block.height) / 2.0;
                draw_text_block(pm, face, block, x, y, Color::BLACK, None, t, mask);
            }
        }
    }
}

/// `drawCrispScene` for the tile `[x0, x1)` (dots). Returns whether anything was drawn.
#[allow(clippy::too_many_arguments)]
fn draw_crisp_scene(
    pixmap: &mut Pixmap,
    placed: &[Placed],
    frame: Option<&FrameGeom>,
    length: i64,
    band: i64,
    f: u32,
    x0: i64,
    x1: i64,
) -> bool {
    let ff = f as f32;
    let base = Transform::from_translate(-(x0 as f32) * ff, 0.0);
    let (pw, ph) = (pixmap.width(), pixmap.height());
    let mut drew = false;
    if let Some(g) = frame {
        draw_frame(&mut pixmap.as_mut(), base, None, g, length, band, f);
        drew = true;
    }
    for pl in placed {
        let Some(crisp) = &pl.p.crisp else { continue };
        if pl.w <= 0 || pl.h <= 0 {
            continue;
        }
        let slack_dots = (pl.p.clip_slack / f64::from(f)).ceil() as i64;
        if pl.x + pl.w + slack_dots < x0 || pl.x - slack_dots > x1 {
            continue;
        }
        let (uw, uh) = if quarter(pl.rotation) { (pl.h, pl.w) } else { (pl.w, pl.h) };
        let t = base
            .pre_translate((pl.x as f32 + pl.w as f32 / 2.0) * ff, (pl.y as f32 + pl.h as f32 / 2.0) * ff)
            .pre_rotate(f32::from(pl.rotation))
            .pre_translate(-(uw as f32) / 2.0 * ff, -(uh as f32) / 2.0 * ff);
        let slack = pl.p.clip_slack as f32;
        let mask = PathBuilder::from_rect(
            tiny_skia::Rect::from_xywh(-slack, 0.0, uw as f32 * ff + 2.0 * slack, uh as f32 * ff)
                .unwrap_or_else(|| tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit rect")),
        );
        let mut clip = Mask::new(pw, ph);
        if let Some(clip) = clip.as_mut() {
            clip.fill_path(&mask, FillRule::Winding, true, t);
        }
        crisp.draw(&mut pixmap.as_mut(), uw as f32 * ff, uh as f32 * ff, f, t, clip.as_ref());
        drew = true;
    }
    drew
}

/// `paintCrisp`: draws the crisp plane in tiles and ORs it into the raster.
#[allow(clippy::too_many_arguments)]
fn paint_crisp(
    raster: &mut LabelRaster,
    placed: &[Placed],
    frame: Option<&FrameGeom>,
    length: i64,
    band: i64,
    f: u32,
    threshold: u8,
    cancel: Option<&AtomicBool>,
) -> Result<(), Cancelled> {
    if frame.is_none() && !placed.iter().any(|p| p.p.crisp.is_some() && p.w > 0 && p.h > 0) {
        return Ok(());
    }
    let tile_dots = (MAX_TILE_PX / i64::from(f)).max(1);
    let band_u = u16::try_from(band.max(1)).unwrap_or(u16::MAX);
    let mut x0 = 0;
    while x0 < length {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(Cancelled);
        }
        let len = tile_dots.min(length - x0);
        let (w, h) = ((len * i64::from(f)) as u32, (band * i64::from(f)) as u32);
        let Some(mut pixmap) = Pixmap::new(w, h) else {
            x0 += tile_dots;
            continue;
        };
        pixmap.fill(Color::WHITE);
        if draw_crisp_scene(&mut pixmap, placed, frame, length, band, f, x0, x0 + len) {
            // An opaque pixmap is the same premultiplied or straight.
            if len == length {
                let _ = raster.blit_crisp(pixmap.data(), w, h, f as u8, threshold);
            } else {
                // Crisp blits only add ink, so OR-ing tiles in is the same as one big blit.
                let mut tile = LabelRaster::new(len as u32, band_u);
                if tile.blit_crisp(pixmap.data(), w, h, f as u8, threshold).is_ok() {
                    raster.blit_bitmap(&tile.finish(), x0 as i32, 0);
                }
            }
        }
        x0 += tile_dots;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ItemKind, LabelDoc, ModuleSize, Symbology};

    fn renderer() -> Renderer {
        Renderer::new(Arc::new(FontRegistry::new()))
    }

    fn target(width_byte: u8) -> RenderTarget {
        let profile = ptouch::profile(ptouch::Model::PtE720bt).expect("model");
        let tape = profile.tape(width_byte, ptouch::TapeKind::Tze).expect("tape");
        RenderTarget { profile, tape }
    }

    fn ink_bounds(b: &Bitmap) -> Option<(u32, u16, u32, u16)> {
        let mut out: Option<(u32, u16, u32, u16)> = None;
        for x in 0..b.length() {
            for y in 0..b.height() {
                if b.get(x, y) {
                    out = Some(match out {
                        None => (x, y, x, y),
                        Some((a, b0, c, d)) => (a.min(x), b0.min(y), c.max(x), d.max(y)),
                    });
                }
            }
        }
        out
    }

    #[test]
    fn default_label_renders_with_margins() {
        let doc = LabelDoc::new();
        let r = renderer().render(&doc, &target(12), &RenderOptions::default()).unwrap();
        assert_eq!(r.height_dots, 70);
        assert!(!r.blocking, "{:?}", r.warnings);
        let (x0, y0, x1, y1) = ink_bounds(&r.bitmap).expect("ink");
        // 2 mm margins = 14 dots each side; Fit fills the band within the 1-dot inset.
        assert_eq!(x0, 14);
        let b = &r.boxes[0];
        assert_eq!((b.x, b.x + b.w + 14), (14, r.length_dots));
        // The box is the ink width rounded up to whole dots.
        assert!(b.x + b.w - (i64::from(x1) + 1) <= 1);
        assert!(y0 <= 2 && y1 >= 67, "ink rows {y0}..{y1}");
        assert_eq!(r.boxes.len(), 1);
        assert_eq!(r.texts.len(), 1);
    }

    #[test]
    fn empty_label_blocks() {
        let doc = LabelDoc::with_items(vec![]);
        let r = renderer().render(&doc, &target(24), &RenderOptions::default()).unwrap();
        assert!(r.blocking);
        assert_eq!(r.warnings[0].code, "empty");
    }

    #[test]
    fn qr_and_barcode_blit_and_protect() {
        let mut doc = LabelDoc::with_items(vec![Item::new(ItemKind::Code)]);
        let mut bar = Item::new(ItemKind::Code);
        if let Item::Code(c) = &mut bar {
            c.symbology = Symbology::Ean13;
            c.data = "590123412345".into();
            c.show_text = true;
            c.module_dots = ModuleSize::Auto;
        }
        doc.items.push(bar);
        let r = renderer().render(&doc, &target(24), &RenderOptions::default()).unwrap();
        assert!(!r.blocking, "{:?}", r.warnings);
        assert_eq!(r.boxes.len(), 2);
        // EAN-13: 95 modules + 2×10 quiet modules at 2 dots.
        assert_eq!(r.boxes[1].w, (95 + 20) * 2);
    }

    #[test]
    fn invalid_code_blocks() {
        let mut item = Item::new(ItemKind::Code);
        if let Item::Code(c) = &mut item {
            c.symbology = Symbology::Ean13;
            c.data = "12".into();
        }
        let doc = LabelDoc::with_items(vec![item]);
        let r = renderer().render(&doc, &target(24), &RenderOptions::default()).unwrap();
        assert!(r.blocking);
        assert_eq!(r.warnings[0].code, "code-invalid");
    }

    #[test]
    fn every_kind_and_a_frame_render() {
        let mut doc = LabelDoc::with_items(ItemKind::ALL.iter().map(|&k| Item::new(k)).collect());
        doc.frame = Some(crate::doc::LabelFrame::UI_DEFAULT);
        let r = renderer().render(&doc, &target(24), &RenderOptions::default()).unwrap();
        // The image has no file: a non-blocking "Choose an image file." warning.
        assert!(r.warnings.iter().any(|w| w.message == IMAGE_NONE_WARNING));
        assert!(!r.blocking, "{:?}", r.warnings);
        assert_eq!(r.boxes.len(), 6);
    }

    #[test]
    fn long_labels_are_tiled_identically() {
        let mut item = Item::new(ItemKind::Text);
        if let Item::Text(t) = &mut item {
            t.text = "A long label that needs several crisp tiles to draw at three times".repeat(3);
        }
        let doc = LabelDoc::with_items(vec![item]);
        let r = renderer().render(&doc, &target(24), &RenderOptions::default()).unwrap();
        assert!(r.length_dots > MAX_TILE_PX / 3);
        assert!(ink_bounds(&r.bitmap).is_some());
    }

    #[test]
    fn shrink_to_fit_length() {
        let mut item = Item::new(ItemKind::Text);
        if let Item::Text(t) = &mut item {
            t.text = "Much too long for thirty millimetres".into();
        }
        let mut doc = LabelDoc::with_items(vec![item]);
        doc.length = LengthSettings::Fixed { mm: 30.0, shrink: true };
        let r = renderer().render(&doc, &target(12), &RenderOptions::default()).unwrap();
        assert_eq!(r.length_dots, mm_to_dots(30.0, 180));
        assert!(!r.overflow, "{:?}", r.warnings);
        assert!(r.texts[0].shrink.is_some_and(|s| s < 1.0));
    }

    #[test]
    fn cancellation() {
        let cancel = AtomicBool::new(true);
        let opts = RenderOptions { factor: 0, cancel: Some(&cancel) };
        assert_eq!(
            renderer().render(&LabelDoc::new(), &target(12), &opts).unwrap_err(),
            Cancelled
        );
    }
}
