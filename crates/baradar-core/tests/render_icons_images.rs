//! Icons drawn on the crisp plane and images prepared for the tone plane, end to end through
//! `ptouch::LabelRaster`.

use std::io::Cursor;

use baradar_core::doc::{DitherKind, ImageAdjust};
use baradar_core::render::icons::{ICONS, draw_icon, icon_by_id, icon_or_fallback};
use baradar_core::render::images::{
    DecodedImage, ImageLoadError, MAX_IMAGE_DOTS, decode_image, tone_options,
};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use ptouch::LabelRaster;
use tiny_skia::{Color, Pixmap, Transform};

fn white(w: u32, h: u32) -> Pixmap {
    let mut p = Pixmap::new(w, h).unwrap();
    p.fill(Color::WHITE);
    p
}

fn ink(p: &Pixmap, x: u32, y: u32) -> bool {
    p.pixel(x, y).unwrap().red() < 128
}

#[test]
fn minus_icon_stroke_geometry() {
    // "M5 12h14" in a 24-dot box at f = 3: s = 3, width max(2, 6/3) = 2 units = 6 px, round caps.
    let minus = icon_by_id("minus").unwrap();
    let mut p = white(72, 72);
    draw_icon(
        &mut p.as_mut(),
        Transform::identity(),
        None,
        minus,
        24,
        72.0,
        72.0,
        3,
    );
    assert!(ink(&p, 36, 36) && ink(&p, 36, 34) && ink(&p, 36, 38));
    assert!(!ink(&p, 36, 31) && !ink(&p, 36, 41));
    assert!(ink(&p, 13, 36), "round cap extends past the end point");
    assert!(!ink(&p, 10, 36) && !ink(&p, 62, 36));
}

#[test]
fn icon_lines_are_never_thinner_than_two_dots() {
    // 8-dot icon: s = 1 px/unit, width = max(2, 6) units = 6 px = 2 dots.
    let minus = icon_by_id("minus").unwrap();
    let mut p = white(24, 24);
    draw_icon(
        &mut p.as_mut(),
        Transform::identity(),
        None,
        minus,
        8,
        24.0,
        24.0,
        3,
    );
    for y in 10..14 {
        assert!(ink(&p, 12, y), "row {y}");
    }
    assert!(!ink(&p, 12, 7) && !ink(&p, 12, 16));
}

#[test]
fn icon_is_centred_in_a_wider_box() {
    let minus = icon_by_id("minus").unwrap();
    let mut p = white(120, 72);
    draw_icon(
        &mut p.as_mut(),
        Transform::identity(),
        None,
        minus,
        24,
        120.0,
        72.0,
        3,
    );
    // Box offset (120 − 72)/2 = 24 px.
    assert!(ink(&p, 24 + 13, 36) && !ink(&p, 24 + 10, 36) && !ink(&p, 24 + 62, 36));
}

#[test]
fn every_icon_draws_ink_through_blit_crisp() {
    let f = 3u32;
    for icon in ICONS {
        let mut p = white(32 * f, 32 * f);
        draw_icon(
            &mut p.as_mut(),
            Transform::identity(),
            None,
            icon,
            32,
            96.0,
            96.0,
            f,
        );
        let mut r = LabelRaster::new(32, 32);
        r.blit_crisp(p.data(), 32 * f, 32 * f, 3, 128).unwrap();
        let bmp = r.finish();
        assert!(!bmp.is_blank(), "{}", icon.id);
    }
    assert_eq!(icon_or_fallback("no-such-icon").id, "question");
}

fn encode(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// 40 × 20: left half black, right half white, with a transparent top-right corner.
fn sample() -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_fn(40, 20, |x, y| {
        if x >= 30 && y < 5 {
            Rgba([255, 0, 0, 0])
        } else if x < 20 {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    }))
}

#[test]
fn decodes_every_supported_format() {
    let rgba = sample();
    let rgb = DynamicImage::ImageRgb8(rgba.to_rgb8());
    for (img, fmt) in [
        (&rgba, ImageFormat::Png),
        (&rgb, ImageFormat::Jpeg),
        (&rgba, ImageFormat::Gif),
        (&rgba, ImageFormat::Bmp),
        (&rgba, ImageFormat::WebP),
    ] {
        let bytes = encode(img, fmt);
        let d = DecodedImage::decode(&bytes).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
        assert_eq!(d.size(), (40, 20), "{fmt:?}");
    }
    assert!(matches!(
        DecodedImage::decode(b"not an image"),
        Err(ImageLoadError::Io(_) | ImageLoadError::Decode(_))
    ));
}

#[test]
fn scales_to_height_and_rotates() {
    let png = encode(&sample(), ImageFormat::Png);
    let img = decode_image(&png, 10, 0).unwrap();
    assert_eq!((img.width, img.height), (20, 10));
    assert_eq!(img.rgba.len(), 20 * 10 * 4);
    // Left is black, right white, top-right transparent.
    let px = |x: u32, y: u32| &img.rgba[((y * img.width + x) * 4) as usize..][..4];
    assert!(px(2, 5)[0] < 40 && px(2, 5)[3] == 255);
    assert!(px(13, 8)[0] > 215);
    assert_eq!(px(19, 0)[3], 0);
    // Quarter turn: the height applies to the rotated image (20 wide → 10 tall: 5 × 10).
    let r = decode_image(&png, 10, 90).unwrap();
    assert_eq!((r.width, r.height), (5, 10));
    // 90° cw: the black left half ends up on top.
    let rp = |x: u32, y: u32| r.rgba[((y * r.width + x) * 4) as usize];
    assert!(rp(2, 1) < 40 && rp(2, 8) > 215);
    let r = decode_image(&png, 10, 180).unwrap();
    assert_eq!((r.width, r.height), (20, 10));
    assert_eq!(
        r.rgba[((9 * 20) * 4 + 3) as usize],
        0,
        "transparent corner moved to the bottom left"
    );
    assert_eq!(r.rgba[3], 255);
    assert!(decode_image(&png, 0, 0).is_ok(), "height at least 1");
}

#[test]
fn very_wide_images_are_shortened() {
    let wide = DynamicImage::ImageRgba8(RgbaImage::from_pixel(9000, 10, Rgba([0, 0, 0, 255])));
    let img = decode_image(&encode(&wide, ImageFormat::Png), 128, 0).unwrap();
    assert_eq!(img.width, MAX_IMAGE_DOTS);
    assert_eq!(img.height, 4);
}

#[test]
fn transparent_edges_do_not_bleed() {
    // Opaque white next to transparent pure red: scaled down, no pixel turns pinkish dark.
    let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(64, 8, |x, _| {
        if x < 32 {
            Rgba([255, 255, 255, 255])
        } else {
            Rgba([255, 0, 0, 0])
        }
    }));
    let out = decode_image(&encode(&img, ImageFormat::Png), 4, 0).unwrap();
    for p in out.rgba.as_chunks::<4>().0 {
        if p[3] > 0 {
            assert!(p[1] > 240 && p[2] > 240, "{p:?}");
        }
    }
}

#[test]
fn tone_plane_end_to_end() {
    let png = encode(&sample(), ImageFormat::Png);
    let img = decode_image(&png, 16, 0).unwrap();
    let (d, adj) = tone_options(DitherKind::Threshold, &ImageAdjust::default());
    let mut r = LabelRaster::new(40, 16);
    // Pre-ink the whole raster: transparent pixels must leave it untouched.
    let all = [0u8, 0, 0, 255].repeat(40 * 3 * 16 * 3);
    r.blit_crisp(&all, 120, 48, 3, 128).unwrap();
    r.blit_tone(&img.rgba, img.width, img.height, 4, 0, d, adj)
        .unwrap();
    let bmp = r.finish();
    assert!(bmp.get(5, 8), "black half inks");
    assert!(!bmp.get(30, 12), "white half clears");
    assert!(bmp.get(35, 0), "transparent corner keeps earlier ink");
}
