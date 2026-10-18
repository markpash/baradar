//! Image decode and scaling for the tone plane (port of `web/src/render/images.ts` and the image
//! part of `renderer.ts` `prepareImage`): bytes → straight RGBA at 1× (printer dots), scaled to
//! the target height and rotated, for `LabelRaster::blit_tone` (the core dithers).
//!
//! The browser's `createImageBitmap` + `drawImage` (`imageSmoothingQuality = 'high'`) become the
//! `image` crate: PNG, JPEG, GIF (first frame), WebP and BMP, with the EXIF orientation applied
//! like `createImageBitmap`'s default `imageOrientation: 'from-image'`.
//!
//! Resampling uses Catmull-Rom (`FilterType::CatmullRom`). It is a cubic like Skia's high-quality
//! bicubic, keeps edges sharp, and rings far less than Lanczos3; ringing halos turn into speckles
//! once the tone plane is dithered. The `image` crate widens the filter support by the scale
//! ratio when shrinking, so large photos are area-averaged rather than aliased. Images with
//! transparency are resampled premultiplied (as Canvas2D does), so transparent pixels do not
//! bleed their hidden colour into the edges.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageBuffer, ImageDecoder, ImageReader, Rgba};
use ptouch::{Dither, ToneAdjust};

use crate::doc::{DitherKind, ImageAdjust, ItemSize};
use crate::render::units::{dots_to_mm, js_round, mm_to_dots};

/// Longest image side accepted along the label, dots (`MAX_IMAGE_DOTS`).
pub const MAX_IMAGE_DOTS: u32 = 4000;

/// Resampling filter (see the module docs).
pub const RESAMPLE_FILTER: FilterType = FilterType::CatmullRom;

/// Warning when a fixed image height is taller than the band (`prepareImage`).
pub const IMAGE_REDUCED_WARNING: &str =
    "The image is taller than the printable area of this tape and was reduced.";
/// `image-missing` when a stored or inlined image cannot be found (`prepareImage`).
pub const IMAGE_GONE_WARNING: &str = "The image is no longer available. Add it again.";
/// `image-missing` when no image was chosen (`prepareImage`).
pub const IMAGE_NONE_WARNING: &str = "Choose an image file.";
/// `image-missing` when the bytes cannot be decoded (`prepareImage`).
pub const IMAGE_UNREADABLE_WARNING: &str = "The image could not be read. Try a PNG or JPEG file.";

/// `content-overflow` when a very wide image came back shorter than asked (`prepareImage`).
pub fn image_too_wide_warning(dpi: u16) -> String {
    format!(
        "The image is very wide, so it was made smaller to stay within {} mm.",
        js_round(dots_to_mm(f64::from(MAX_IMAGE_DOTS), dpi))
    )
}

/// Straight (not premultiplied) RGBA at 1×, row-major (`RgbaImage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Why an image could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum ImageLoadError {
    #[error("The image is empty")]
    Empty,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Decode(#[from] image::ImageError),
}

/// Size in dots of an image of natural size `w × h` drawn `height_dots` tall, aspect kept
/// (`scaledSize`). A very wide image is limited to [`MAX_IMAGE_DOTS`] long and drawn shorter
/// (never squashed). `(0, 0)` for an empty image.
pub fn scaled_size(w: u32, h: u32, height_dots: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let (wf, hf) = (f64::from(w), f64::from(h));
    let width = js_round(wf * f64::from(height_dots) / hf).max(1);
    if width <= i64::from(MAX_IMAGE_DOTS) {
        return (u32::try_from(width).unwrap_or(MAX_IMAGE_DOTS), height_dots);
    }
    let height = js_round(hf * f64::from(MAX_IMAGE_DOTS) / wf).max(1);
    (MAX_IMAGE_DOTS, u32::try_from(height).unwrap_or(1))
}

/// Width in dots of an image drawn `height_dots` tall (`scaledWidth`).
pub fn scaled_width(w: u32, h: u32, height_dots: u32) -> u32 {
    scaled_size(w, h, height_dots).0
}

/// The image height asked for in a `band`-tall slot and whether it was reduced to the band
/// (`prepareImage`): fit = band, else `max(1, mmToDots(mm))`.
pub fn image_height_dots(size: &ItemSize, band: i64, dpi: u16) -> (i64, bool) {
    let h = match size {
        ItemSize::Fit => band,
        ItemSize::Mm { mm } => mm_to_dots(*mm, dpi).max(1),
    };
    if h > band { (band, true) } else { (h, false) }
}

/// Free layout: lowers `h` so the (rotated) image also fits the frame width `max_w`
/// (`prepareImage`). `nat_w × nat_h` is the natural, unrotated size.
pub fn fit_frame_width(h: i64, nat_w: u32, nat_h: u32, rotation: u16, max_w: i64) -> i64 {
    let (w, hh) = (f64::from(nat_w), f64::from(nat_h));
    let aspect = if matches!(rotation, 90 | 270) {
        hh / w
    } else {
        w / hh
    };
    if aspect > 0.0 && aspect.is_finite() && h as f64 * aspect > max_w as f64 {
        ((max_w as f64 / aspect).floor() as i64).max(1)
    } else {
        h
    }
}

/// A decoded image with its EXIF orientation applied.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    image: DynamicImage,
}

impl DecodedImage {
    /// Decodes PNG / JPEG / GIF / WebP / BMP bytes (format sniffed from the content).
    ///
    /// # Errors
    /// [`ImageLoadError`] for unknown formats, corrupt data or an empty image.
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageLoadError> {
        let mut decoder = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()?
            .into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        if image.width() == 0 || image.height() == 0 {
            return Err(ImageLoadError::Empty);
        }
        Ok(Self { image })
    }

    /// Natural size, oriented (`imageSize`).
    pub fn size(&self) -> (u32, u32) {
        (self.image.width(), self.image.height())
    }

    /// Scales to `target_height_dots` (aspect kept; a very wide image comes back shorter, see
    /// [`scaled_size`]) and rotates clockwise by `rotation` (0/90/180/270; the height applies to
    /// the rotated image), as straight RGBA at 1× (`decodeImage`).
    ///
    /// # Errors
    /// [`ImageLoadError::Empty`] when the result would have no pixels.
    pub fn scaled(
        &self,
        target_height_dots: i64,
        rotation: u16,
    ) -> Result<RgbaImage, ImageLoadError> {
        let quarter = matches!(rotation, 90 | 270);
        let (nw, nh) = self.size();
        let (nat_w, nat_h) = if quarter { (nh, nw) } else { (nw, nh) };
        let target = u32::try_from(target_height_dots.max(1)).unwrap_or(u32::MAX);
        let (width, height) = scaled_size(nat_w, nat_h, target);
        if width == 0 {
            return Err(ImageLoadError::Empty);
        }
        // Draw size before rotation.
        let (dw, dh) = if quarter {
            (height, width)
        } else {
            (width, height)
        };
        let scaled = resize_rgba(&self.image, dw, dh);
        let rotated = match rotation {
            90 => image::imageops::rotate90(&scaled),
            180 => image::imageops::rotate180(&scaled),
            270 => image::imageops::rotate270(&scaled),
            _ => scaled,
        };
        let (w, h) = rotated.dimensions();
        Ok(RgbaImage {
            rgba: rotated.into_raw(),
            width: w,
            height: h,
        })
    }
}

/// Decodes `bytes` and scales them (`decodeImage`): [`DecodedImage::decode`] then
/// [`DecodedImage::scaled`].
///
/// # Errors
/// See [`DecodedImage::decode`].
pub fn decode_image(
    bytes: &[u8],
    target_height_dots: i64,
    rotation: u16,
) -> Result<RgbaImage, ImageLoadError> {
    DecodedImage::decode(bytes)?.scaled(target_height_dots, rotation)
}

/// Resamples to `w × h` straight RGBA8; premultiplied when the image has transparency.
fn resize_rgba(img: &DynamicImage, w: u32, h: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let rgba = img.to_rgba8();
    if rgba.pixels().all(|p| p[3] == 255) {
        return image::imageops::resize(&rgba, w, h, RESAMPLE_FILTER);
    }
    // Premultiplied in u16: colour·alpha and alpha·255 share the 0…65025 scale.
    let pre: ImageBuffer<Rgba<u16>, Vec<u16>> =
        ImageBuffer::from_fn(rgba.width(), rgba.height(), |x, y| {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            let a16 = u16::from(a);
            Rgba([
                u16::from(r) * a16,
                u16::from(g) * a16,
                u16::from(b) * a16,
                a16 * 255,
            ])
        });
    let small = image::imageops::resize(&pre, w, h, RESAMPLE_FILTER);
    ImageBuffer::from_fn(w, h, |x, y| {
        let [r, g, b, a] = small.get_pixel(x, y).0;
        if a == 0 {
            return Rgba([0, 0, 0, 0]);
        }
        let un = |c: u16| {
            let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
            u8::try_from(v.min(255)).unwrap_or(255)
        };
        let alpha = u8::try_from((u32::from(a) + 127) / 255).unwrap_or(255);
        Rgba([un(r), un(g), un(b), alpha])
    })
}

/// Tone options for `blit_tone` from an image item (`prepareImage` + wasm `convert::tone`):
/// level 0–255 (Threshold only), brightness and contrast −100…100, gamma×100 10–1000.
pub fn tone_options(dither: DitherKind, adjust: &ImageAdjust) -> (Dither, ToneAdjust) {
    let d = match dither {
        DitherKind::Threshold => Dither::Threshold {
            level: adjust.level,
        },
        DitherKind::FloydSteinberg => Dither::FloydSteinberg,
        DitherKind::Atkinson => Dither::Atkinson,
        DitherKind::Bayer4 => Dither::Bayer4,
        DitherKind::Bayer8 => Dither::Bayer8,
    };
    let pct = |v: i32| i8::try_from(v.clamp(-100, 100)).unwrap_or(0);
    let adj = ToneAdjust {
        brightness: pct(adjust.brightness),
        contrast: pct(adjust.contrast),
        gamma_x100: adjust.gamma_x100.clamp(10, 1000),
        invert: adjust.invert,
    };
    (d, adj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_size_rules() {
        assert_eq!(scaled_size(200, 100, 128), (256, 128));
        assert_eq!(scaled_size(0, 100, 128), (0, 0));
        let (w, h) = scaled_size(8000, 100, 128);
        assert_eq!((w, h), (MAX_IMAGE_DOTS, 50));
        assert_eq!(scaled_width(8000, 100, 128), MAX_IMAGE_DOTS);
        assert_eq!(scaled_size(1, 1000, 128), (1, 128));
    }

    #[test]
    fn heights_and_frames() {
        assert_eq!(image_height_dots(&ItemSize::Fit, 70, 180), (70, false));
        assert_eq!(
            image_height_dots(&ItemSize::Mm { mm: 5.0 }, 70, 180),
            (35, false)
        );
        assert_eq!(
            image_height_dots(&ItemSize::Mm { mm: 50.0 }, 70, 180),
            (70, true)
        );
        // 2:1 image, 70 tall would be 140 wide; frame 100 wide → 50 tall.
        assert_eq!(fit_frame_width(70, 200, 100, 0, 100), 50);
        // Quarter turn: aspect 1:2 → 35 wide, fits.
        assert_eq!(fit_frame_width(70, 200, 100, 90, 100), 70);
        assert_eq!(
            image_too_wide_warning(180),
            "The image is very wide, so it was made smaller to stay within 564 mm."
        );
    }

    #[test]
    fn tone_clamps() {
        let adj = ImageAdjust {
            brightness: 250,
            contrast: -300,
            gamma_x100: 5,
            invert: true,
            level: 77,
        };
        let (d, t) = tone_options(DitherKind::Threshold, &adj);
        assert_eq!(d, Dither::Threshold { level: 77 });
        assert_eq!(
            t,
            ToneAdjust {
                brightness: 100,
                contrast: -100,
                gamma_x100: 10,
                invert: true
            }
        );
        let (d, t) = tone_options(DitherKind::default(), &ImageAdjust::default());
        assert_eq!(d, Dither::FloydSteinberg);
        assert_eq!(t, ToneAdjust::default());
        assert_eq!(
            tone_options(
                DitherKind::Bayer8,
                &ImageAdjust {
                    gamma_x100: 5000,
                    ..ImageAdjust::default()
                }
            )
            .1
            .gamma_x100,
            1000
        );
    }
}
