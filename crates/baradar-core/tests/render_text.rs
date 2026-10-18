//! Text layout, sizing and quick sizes: ports of web/tests/unit/render/{text,shrink,text-size}
//! and ui/text-defaults tests (fake fonts), plus sanity checks with the bundled fonts.

use baradar_core::doc::schema::{Align, Item, ItemKind, TextItem, TextSize};
use baradar_core::render::fonts::{FontRegistry, PixelGrid};
use baradar_core::render::text::*;
use baradar_core::render::units::mm_to_dots;
use baradar_core::store::DefaultTextSize;

/// `toBeCloseTo(b, digits)`.
#[track_caller]
fn close(a: f64, b: f64, digits: i32) {
    assert!(
        (a - b).abs() < 10f64.powi(-digits) / 2.0,
        "{a} is not close to {b} ({digits} digits)"
    );
}

fn has(text: &str, set: &str) -> bool {
    text.chars().any(|c| set.contains(c))
}

/// Fake proportional font: 0.6 em advance, caps/ascenders 0.72 em, x-height 0.5, descenders 0.22.
fn fake(text: &str, px: f64) -> LineInk {
    let caps = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789bdfhklt";
    let asc = if has(text, caps) { 0.72 } else { 0.5 };
    let desc = if has(text, "gjpqy") { 0.22 } else { 0.0 };
    let advance = 0.6 * px * text.chars().count() as f64;
    LineInk {
        advance,
        left: 0.0,
        right: advance,
        ascent: asc * px,
        descent: desc * px,
    }
}

const FACE: FaceMetrics = FaceMetrics {
    cap: 0.7,
    desc: 0.2,
};

fn fit(avail_px: f64) -> FitOpts {
    FitOpts::Fit { avail_px }
}

fn em(em_px: f64) -> FitOpts {
    FitOpts::Em {
        em_px,
        grid: None,
        metric_height: true,
    }
}

// ---------------------------------------------------------------- text layout

#[test]
fn splits_lines_on_newlines() {
    assert_eq!(split_lines("a\r\nb\nc"), ["a", "b", "c"]);
    assert_eq!(split_lines("a\rb"), ["a", "b"]);
    assert_eq!(split_lines(""), [""]);
}

#[test]
fn fit_single_line_of_caps_fills_the_height() {
    let b = fit_text_block(&["HELLO"], FACE, 1.2, Align::Center, &fake, fit(384.0));
    close(b.height, 384.0, 3);
    close(b.font_px, 384.0 / 0.72, 3);
    close(b.width, 0.6 * b.font_px * 5.0, 3);
    close(b.lines[0].baseline, 384.0, 3);
}

#[test]
fn fit_descenders_make_text_smaller_but_still_fit() {
    let caps = fit_text_block(&["HELLO"], FACE, 1.2, Align::Center, &fake, fit(384.0));
    let desc = fit_text_block(&["Hello g"], FACE, 1.2, Align::Center, &fake, fit(384.0));
    assert!(desc.font_px < caps.font_px);
    assert!(desc.height <= 384.0 + 1e-6);
    assert!(desc.height > 383.0);
}

#[test]
fn fit_two_lines_share_the_band() {
    let b = fit_text_block(&["AB", "CD"], FACE, 1.2, Align::Center, &fake, fit(300.0));
    // top 0.72 + pitch 1.2 × 0.9 = 1.8 em
    close(b.font_px, 300.0 / 1.8, 3);
    close(b.height, 300.0, 3);
    close(
        b.lines[1].baseline - b.lines[0].baseline,
        1.2 * 0.9 * b.font_px,
        3,
    );
}

#[test]
fn pt_is_the_em_size_per_line() {
    use baradar_core::render::units::pt_to_px;
    assert_eq!(pt_to_px(12.0, 180, 1), 30.0);
    assert_eq!(pt_to_px(12.0, 180, 3), 90.0);
    close(pt_to_px(7.2, 360, 1), 36.0, 9);
    let one = fit_text_block(&["HELLO"], FACE, 1.2, Align::Center, &fake, em(90.0));
    assert_eq!(one.font_px, 90.0);
    close(one.line_box, 0.9 * 90.0, 6);
    let two = fit_text_block(&["A", "B"], FACE, 1.5, Align::Center, &fake, em(90.0));
    assert_eq!(two.font_px, 90.0);
    close(two.height, one.height + 1.5 * 0.9 * 90.0, 6);
}

#[test]
fn mm_is_the_cap_to_descender_height() {
    let t = |lines: &[&str], lh: f64| {
        fit_text_block(
            lines,
            FACE,
            lh,
            Align::Center,
            &fake,
            FitOpts::Target { target_px: 90.0 },
        )
    };
    let one = t(&["HELLO"], 1.2);
    close(one.font_px, 100.0, 6);
    close(one.line_box, 90.0, 6);
    close(t(&["gggg"], 1.2).font_px, 100.0, 6);
    close(t(&["A", "B"], 1.5).font_px, 90.0 / (0.9 * 2.5), 6);
}

#[test]
fn aligns_lines_inside_the_block() {
    let x = |align| {
        layout_lines(
            &["WIDE LINE", "X"],
            100.0,
            FACE,
            1.0,
            align,
            &fake,
            false,
            None,
        )
        .lines[1]
            .x
    };
    let wide: f64 = 0.6 * 100.0 * 9.0;
    let narrow = 0.6 * 100.0;
    assert_eq!(x(Align::Start), 0.0);
    assert_eq!(x(Align::Center), ((wide - narrow) / 2.0).round());
    close(x(Align::End), wide - narrow, 6);
}

#[test]
fn blank_lines_take_a_pitch_and_blank_blocks_have_no_width() {
    let b = layout_lines(
        &["A", "", "B"],
        100.0,
        FACE,
        1.0,
        Align::Start,
        &fake,
        false,
        None,
    );
    let baselines: Vec<f64> = b.lines.iter().map(|l| l.baseline.round()).collect();
    assert_eq!(baselines, [72.0, 162.0, 252.0]);
    let blank = layout_lines(&["  "], 100.0, FACE, 1.0, Align::Start, &fake, false, None);
    assert_eq!(blank.width, 0.0);
}

#[test]
fn invert_padding_scales_with_the_band() {
    assert_eq!(invert_padding(384.0, 0.0, 3.0).v, 30.0);
    assert_eq!(invert_padding(72.0, 0.0, 3.0).v, 7.0);
    assert_eq!(invert_padding(3.0, 0.0, 3.0).v, 3.0);
    assert_eq!(invert_padding(384.0, 200.0, 3.0).h, 50.0);
}

#[test]
fn line_height_is_clamped() {
    assert_eq!(line_height_of(f64::NAN), 1.1);
    assert_eq!(line_height_of(0.1), 0.5);
    assert_eq!(line_height_of(9.0), 3.0);
    assert_eq!(line_height_of(1.25), 1.25);
}

// ---------------------------------------------------------------- sizeTextBlock

/// Fake pixel font on a 10-pixel em: every extent is a whole number of design pixels (0.1 em).
fn pixel_fake(text: &str, px: f64) -> LineInk {
    let desc = if has(text, "gjpqy") { 0.2 } else { 0.0 };
    let advance = 0.6 * px * text.chars().count() as f64;
    LineInk {
        advance,
        left: 0.0,
        right: advance - 0.1 * px,
        ascent: 0.7 * px,
        descent: desc * px,
    }
}

fn sizing(size: TextSize) -> TextSizing {
    TextSizing {
        size,
        align: Align::Center,
        invert: false,
        line_height: 1.1,
        clip_tall: false,
    }
}

fn env(band_dots: f64) -> SizeEnv {
    SizeEnv {
        band_px: band_dots * 3.0,
        f: 3,
        dpi: 180,
        pixel: None,
        scale: None,
        max_w_px: None,
    }
}

fn multiple_of(v: f64, m: f64) -> bool {
    (v / m - (v / m).round()).abs() < 1e-6
}

#[test]
fn size_fit_mm_pt_match_fit_text_block() {
    let e = env(70.0);
    let f = size_text_block(&["HELLO"], FACE, &sizing(TextSize::Fit), &fake, &e);
    assert_eq!(
        f.block,
        fit_text_block(&["HELLO"], FACE, 1.1, Align::Center, &fake, fit(210.0))
    );
    assert!(!f.reduced);
    assert_eq!(f.pixel_scale, None);
    let pt = size_text_block(
        &["HELLO"],
        FACE,
        &sizing(TextSize::Pt { pt: 12.0 }),
        &fake,
        &e,
    );
    assert_eq!(pt.block.font_px, 90.0);
    let mm = size_text_block(
        &["HELLO"],
        FACE,
        &sizing(TextSize::Mm { mm: 5.0 }),
        &fake,
        &e,
    );
    close(mm.block.line_box, 5.0 * 180.0 * 3.0 / 25.4, 6);
    assert_eq!(mm.w_px, mm.block.width);
    assert_eq!(mm.h_px, mm.block.height);
}

#[test]
fn a_fixed_size_taller_than_the_band_is_reduced() {
    let e = env(70.0);
    let tall = size_text_block(
        &["Hxg"],
        FACE,
        &sizing(TextSize::Mm { mm: 15.0 }),
        &fake,
        &e,
    );
    assert!(tall.reduced);
    assert!((tall.h_px / 3.0 - 1e-6).ceil() <= 70.0);
    assert!(tall.h_px > 69.0 * 3.0);
    let ok = size_text_block(&["Hxg"], FACE, &sizing(TextSize::Mm { mm: 9.0 }), &fake, &e);
    assert!(!ok.reduced);
    let three = size_text_block(&["A", "B", "C"], FACE, &sizing(TextSize::Fit), &fake, &e);
    assert!(!three.reduced);
    let inv = TextSizing {
        invert: true,
        ..sizing(TextSize::Pt { pt: 40.0 })
    };
    let inv = size_text_block(&["Hxg"], FACE, &inv, &fake, &e);
    assert!(inv.reduced);
    assert!((inv.h_px / 3.0 - 1e-6).ceil() <= 70.0);
}

#[test]
fn clip_tall_keeps_v1_sizes() {
    let e = env(70.0);
    let v1 = TextSizing {
        clip_tall: true,
        ..sizing(TextSize::Mm { mm: 15.0 })
    };
    let v1 = size_text_block(&["Hxg"], FACE, &v1, &fake, &e);
    assert!(!v1.reduced);
    close(v1.block.line_box, 15.0 * 180.0 * 3.0 / 25.4, 6);
    assert!(v1.h_px > 70.0 * 3.0);
    let inv_v1 = TextSizing {
        invert: true,
        clip_tall: true,
        ..sizing(TextSize::Mm { mm: 9.0 })
    };
    let inv_now = TextSizing {
        clip_tall: false,
        ..inv_v1
    };
    let a = size_text_block(&["Hxg"], FACE, &inv_v1, &fake, &e);
    let b = size_text_block(&["Hxg"], FACE, &inv_now, &fake, &e);
    assert!(!a.reduced);
    close(a.block.line_box, 9.0 * 180.0 * 3.0 / 25.4, 6);
    assert!(b.block.line_box < a.block.line_box);
}

#[test]
fn scale_and_frame_width_shrink() {
    let e = env(70.0);
    let pt = sizing(TextSize::Pt { pt: 12.0 });
    let one = size_text_block(&["HELLO"], FACE, &pt, &fake, &e);
    let half_env = SizeEnv {
        scale: Some(0.5),
        ..e
    };
    let half = size_text_block(&["HELLO"], FACE, &pt, &fake, &half_env);
    close(half.block.font_px, one.block.font_px / 2.0, 6);
    close(half.w_px, one.w_px / 2.0, 6);
    let fit_half = size_text_block(&["HELLO"], FACE, &sizing(TextSize::Fit), &fake, &half_env);
    close(fit_half.block.height, 105.0, 3);
    let framed_env = SizeEnv {
        max_w_px: Some(150.0),
        ..e
    };
    let framed = size_text_block(&["HELLO"], FACE, &sizing(TextSize::Fit), &fake, &framed_env);
    assert!(framed.w_px <= 150.0 + 1e-6);
}

// ---------------------------------------------------------------- pixel fonts

#[test]
fn pixel_scale_for_rounds_down_for_limits() {
    assert_eq!(pixel_scale_for(107.0, 10.0, true), 10);
    assert_eq!(pixel_scale_for(107.0, 10.0, false), 11);
    assert_eq!(pixel_scale_for(104.0, 10.0, false), 10);
    assert_eq!(pixel_scale_for(100.0, 10.0, true), 10);
    assert_eq!(pixel_scale_for(99.999_999_9, 10.0, true), 10);
    assert_eq!(pixel_scale_for(4.0, 10.0, true), 1);
    assert_eq!(pixel_scale_for(0.0, 10.0, false), 1);
    assert_eq!(pixel_scale_for(50.0, 0.0, false), 1);
}

#[test]
fn layout_on_a_grid_snaps_to_whole_dots() {
    let b = layout_lines(
        &["AB", "C", "Dg"],
        77.0,
        FACE,
        1.15,
        Align::Center,
        &pixel_fake,
        true,
        Some(3.0),
    );
    for l in &b.lines {
        assert!(multiple_of(l.baseline, 3.0), "baseline {}", l.baseline);
        assert!(multiple_of(l.x, 3.0), "x {}", l.x);
    }
    assert!(multiple_of(b.width, 3.0));
    assert!(multiple_of(b.height, 3.0));
    let pitch = b.lines[1].baseline - b.lines[0].baseline;
    assert_eq!(b.lines[2].baseline - b.lines[1].baseline, pitch);
    let free = layout_lines(
        &["AB", "C"],
        77.0,
        FACE,
        1.15,
        Align::Center,
        &pixel_fake,
        true,
        None,
    );
    close(free.lines[1].baseline, 0.7 * 77.0 + 0.9 * 77.0 * 1.15, 9);
}

#[test]
fn pixel_fonts_take_whole_scales() {
    let e = SizeEnv {
        pixel: Some(PixelGrid { em_px: 10 }),
        ..env(75.0)
    };
    let s = |size, e: &SizeEnv, text: &str| {
        size_text_block(&[text], FACE, &sizing(size), &pixel_fake, e)
    };
    let fit = s(TextSize::Fit, &e, "HELLO");
    assert_eq!(fit.pixel_scale, Some(10));
    assert_eq!(fit.block.font_px, 300.0);
    assert!(fit.block.height <= 225.0);
    let pt = s(TextSize::Pt { pt: 30.0 }, &e, "HELLO");
    assert_eq!(pt.pixel_scale, Some(8));
    assert_eq!(pt.block.font_px, 240.0);
    assert_eq!(
        s(TextSize::Mm { mm: 10.0 }, &e, "HELLO").pixel_scale,
        Some(8)
    );
    let big = s(TextSize::Pt { pt: 34.0 }, &e, "Hg");
    assert_eq!(big.pixel_scale, Some(8));
    assert!((big.h_px / 3.0 - 1e-6).ceil() <= 75.0);
    let shrunk_env = SizeEnv {
        scale: Some(0.9),
        ..e
    };
    assert_eq!(
        s(TextSize::Pt { pt: 30.0 }, &shrunk_env, "HELLO").pixel_scale,
        Some(6)
    );
    assert_eq!(
        s(TextSize::Pt { pt: 4.0 }, &e, "HELLO").pixel_scale,
        Some(1)
    );
}

// ---------------------------------------------------------------- shrinkFactor

#[test]
fn shrink_factor_cases() {
    assert_eq!(shrink_factor(200.0, 50.0, 100.0), Some(1.0));
    assert_eq!(shrink_factor(200.0, 50.0, 150.0), Some(1.0));
    assert_eq!(shrink_factor(200.0, 0.0, 0.0), Some(1.0));
    close(shrink_factor(200.0, 80.0, 300.0).unwrap_or(0.0), 0.4, 9);
    close(
        80.0 + 300.0 * shrink_factor(200.0, 80.0, 300.0).unwrap_or(0.0),
        200.0,
        9,
    );
    close(shrink_factor(100.0, 0.0, 125.0).unwrap_or(0.0), 0.8, 9);
    assert_eq!(shrink_factor(100.0, 120.0, 50.0), None);
    assert_eq!(shrink_factor(100.0, 100.0, 50.0), None);
    close(
        shrink_factor(100.0, 0.0, 100.0 / MIN_SHRINK).unwrap_or(0.0),
        MIN_SHRINK,
        9,
    );
    assert_eq!(
        shrink_factor(100.0, 0.0, 100.0 / MIN_SHRINK + 1.0),
        Some(MIN_SHRINK)
    );
    assert_eq!(shrink_factor(100.0, 0.0, 20000.0), Some(MIN_SHRINK));
}

// ---------------------------------------------------------------- quick sizes

const BANDS: [(f64, i64); 6] = [
    (3.5, 24),
    (6.0, 32),
    (9.0, 50),
    (12.0, 70),
    (18.0, 112),
    (24.0, 128),
];

fn mm_of(size: TextSize) -> f64 {
    match size {
        TextSize::Mm { mm } => mm,
        other => panic!("not an mm size: {other:?}"),
    }
}

fn ids(q: &[QuickTextSize], pred: impl Fn(&QuickTextSize) -> bool) -> Vec<QuickSizeId> {
    q.iter().filter(|s| pred(s)).map(|s| s.id).collect()
}

#[test]
fn quick_sizes_are_fractions_of_the_band() {
    let q = quick_text_sizes(70, 180);
    let labels: Vec<&str> = q.iter().map(|s| s.label).collect();
    assert_eq!(labels, ["XS", "S", "M", "L", "Fit"]);
    for s in &q {
        assert!(s.name.starts_with(s.label), "{}", s.name);
    }
    let dots: Vec<i64> = q.iter().map(|s| s.dots).collect();
    assert_eq!(dots, [18, 23, 35, 53, 70]);
    assert_eq!(q[4].size, TextSize::Fit);
    assert_eq!(q[2].size, TextSize::Mm { mm: 4.94 });
}

#[test]
fn quick_sizes_round_trip_on_every_tape() {
    for (_, band) in BANDS {
        let q = quick_text_sizes(band, 180);
        for s in &q {
            if let TextSize::Mm { mm } = s.size {
                assert_eq!(mm_to_dots(mm, 180), s.dots, "{:?}", s.id);
            }
            assert!(s.dots <= band);
            assert_eq!(s.too_small, s.dots < MIN_READABLE_DOTS);
            let want = if s.id == QuickSizeId::Xs && q[1].dots == s.dots {
                QuickSizeId::Xs
            } else {
                s.id
            };
            assert_eq!(match_quick_size(s.size, band, 180), Some(want));
        }
    }
}

#[test]
fn quick_sizes_flag_small_text() {
    assert_eq!(
        ids(&quick_text_sizes(32, 180), |s| s.too_small),
        [QuickSizeId::Xs, QuickSizeId::S]
    );
    assert!(!quick_text_sizes(128, 180).iter().any(|s| s.too_small));
}

#[test]
fn readable_minimum_is_2mm_at_any_dpi() {
    assert_eq!(MIN_READABLE_MM, 2.0);
    assert_eq!(MIN_READABLE_DOTS, 14);
    assert_eq!(min_readable_dots(180), 14);
    assert_eq!(min_readable_dots(360), 28);
    assert_eq!(
        ids(&quick_text_sizes(64, 360), |s| s.too_small),
        [QuickSizeId::Xs, QuickSizeId::S]
    );
    let fine = quick_text_sizes(64, 360);
    for (i, s) in quick_text_sizes(32, 180).iter().enumerate() {
        match (s.size, fine[i].size) {
            (TextSize::Mm { mm: a }, TextSize::Mm { mm: b }) => {
                assert!((a - b).abs() < 25.4 / 180.0)
            }
            (a, b) => assert_eq!(a, b),
        }
    }
    assert_eq!(
        ids(&quick_text_sizes(48, 360), |s| s.too_small),
        [QuickSizeId::Xs, QuickSizeId::S, QuickSizeId::M]
    );
}

#[test]
fn quick_sizes_at_360_dpi() {
    for band in [48, 64, 106, 150, 234, 320, 454] {
        let q = quick_text_sizes(band, 360);
        for s in &q {
            if let TextSize::Mm { mm } = s.size {
                assert_eq!(mm_to_dots(mm, 360), s.dots, "{band} {:?}", s.id);
            }
            assert_eq!(s.too_small, s.dots < min_readable_dots(360));
            let want = if s.id == QuickSizeId::Xs && q[1].dots == s.dots {
                QuickSizeId::Xs
            } else {
                s.id
            };
            assert_eq!(match_quick_size(s.size, band, 360), Some(want));
        }
    }
    assert_eq!(
        quick_text_sizes(454, 360)[2].size,
        TextSize::Mm { mm: 16.02 }
    );
    assert_eq!(
        quick_text_sizes(320, 360)[2].size,
        TextSize::Mm { mm: 11.29 }
    );
}

#[test]
fn other_sizes_are_no_quick_size() {
    assert_eq!(match_quick_size(TextSize::Mm { mm: 6.0 }, 70, 180), None);
    assert_eq!(match_quick_size(TextSize::Pt { pt: 12.0 }, 70, 180), None);
    assert_eq!(
        match_quick_size(TextSize::Fit, 70, 180),
        Some(QuickSizeId::Fit)
    );
}

#[test]
fn em_dots_to_points() {
    assert_eq!(em_dots_to_pt(30.0, 180), 12.0);
    assert_eq!(em_dots_to_pt(31.0, 180), 12.5);
    assert_eq!(em_dots_to_pt(2.0, 180), 4.0);
    assert_eq!(em_dots_to_pt(1000.0, 180), 144.0);
    assert_eq!(em_dots_to_pt(f64::NAN, 180), 12.0);
}

#[test]
fn default_size_of_a_block() {
    let q = quick_text_sizes(70, 180);
    let size = |id| {
        q.iter()
            .find(|s| s.id == id)
            .map(|s| s.size)
            .unwrap_or(TextSize::Fit)
    };
    assert_eq!(
        default_size_of(TextSize::Fit, 70, 180, 0.0),
        DefaultTextSize::Fit
    );
    assert_eq!(
        default_size_of(size(QuickSizeId::M), 70, 180, 40.0),
        DefaultTextSize::Half
    );
    assert_eq!(
        default_size_of(size(QuickSizeId::S), 70, 180, 26.0),
        DefaultTextSize::Third
    );
    assert_eq!(
        default_size_of(TextSize::Pt { pt: 9.0 }, 70, 180, 22.5),
        DefaultTextSize::Pt(9.0)
    );
    assert_eq!(
        default_size_of(size(QuickSizeId::Xs), 70, 180, 20.0),
        DefaultTextSize::Pt(8.0)
    );
    assert_eq!(
        default_size_of(TextSize::Mm { mm: 6.0 }, 70, 180, 47.0),
        DefaultTextSize::Pt(19.0)
    );
}

#[test]
fn estimate_label_length() {
    assert_eq!(estimate_label_mm(40.0, 300.0, 300.0, 180, 0.0, None), 40.0);
    close(
        estimate_label_mm(40.0, 300.0, 158.3, 180, 4.4, None),
        20.0,
        1,
    );
    assert_eq!(estimate_label_mm(10.0, 100.0, 0.0, 180, 4.4, None), 4.4);
    assert_eq!(
        estimate_label_mm(40.0, 300.0, 600.0, 180, 4.4, Some(50.0)),
        50.0
    );
}

// ---------------------------------------------------------------- text defaults

const P710_WIDTHS: [f64; 6] = [3.5, 6.0, 9.0, 12.0, 18.0, 24.0];

fn band_of(w: f64) -> i64 {
    fallback_band(w).map_or(0, |b| b.0)
}

#[test]
fn auto_fits_narrow_tape_and_halves_from_12mm() {
    for w in P710_WIDTHS {
        let band = band_of(w);
        let s = resolve_default_text_size(DefaultTextSize::Auto, w, band, 180);
        if w < 12.0 {
            assert_eq!(s, TextSize::Fit, "{w} mm");
        } else {
            assert_eq!(
                mm_to_dots(mm_of(s), 180),
                (band as f64 / 2.0).round() as i64,
                "{w} mm"
            );
        }
    }
}

#[test]
fn fit_half_third_and_points() {
    assert_eq!(
        resolve_default_text_size(DefaultTextSize::Fit, 24.0, 128, 180),
        TextSize::Fit
    );
    assert_eq!(
        resolve_default_text_size(DefaultTextSize::Half, 6.0, 32, 180),
        TextSize::Mm { mm: 2.26 }
    );
    let third = resolve_default_text_size(DefaultTextSize::Third, 24.0, 128, 180);
    assert_eq!(mm_to_dots(mm_of(third), 180), 43);
    assert_eq!(
        resolve_default_text_size(DefaultTextSize::Pt(14.0), 3.5, 24, 180),
        TextSize::Pt { pt: 14.0 }
    );
}

#[test]
fn new_text_font_is_the_nearest_weight() {
    let d = new_text_defaults("fira-sans", 600, DefaultTextSize::Auto, 12.0, 70, 180);
    assert_eq!(
        d,
        NewTextDefaults {
            font_family: "fira-sans",
            font_weight: 600,
            size: TextSize::Mm { mm: 4.94 }
        }
    );
    let atk = new_text_defaults(
        "atkinson-hyperlegible",
        600,
        DefaultTextSize::Fit,
        9.0,
        50,
        180,
    );
    assert_eq!(atk.font_family, "atkinson-hyperlegible");
    assert_eq!(atk.font_weight, 700);
    assert_eq!(atk.size, TextSize::Fit);
}

#[test]
fn fallback_bands() {
    assert_eq!(FALLBACK_BAND_DOTS, BANDS);
    assert_eq!(fallback_band(24.0), Some((128, 180)));
    assert_eq!(fallback_band(36.0), Some((454, 360)));
    assert_eq!(fallback_band(5.0), None);
}

#[test]
fn quick_size_keeps_its_name_across_dpi() {
    let m36 = quick_text_sizes(454, 360)[2].size;
    let m24 = retape_text_size(m36, 454, 128, 360, None, 180);
    assert_eq!(match_quick_size(m24, 128, 180), Some(QuickSizeId::M));
}

#[test]
fn retape_keeps_quick_sizes() {
    let m24 = resolve_default_text_size(DefaultTextSize::Half, 24.0, 128, 180);
    assert_eq!(m24, TextSize::Mm { mm: 9.03 });
    let m12 = retape_text_size(m24, 128, 70, 180, None, 180);
    assert_eq!(match_quick_size(m12, 70, 180), Some(QuickSizeId::M));
    assert_eq!(mm_to_dots(mm_of(m12), 180), 35);
    for id in [
        QuickSizeId::Xs,
        QuickSizeId::S,
        QuickSizeId::M,
        QuickSizeId::L,
    ] {
        for from in P710_WIDTHS {
            for to in P710_WIDTHS {
                let (bf, bt) = (band_of(from), band_of(to));
                let size = quick_text_sizes(bf, 180)
                    .into_iter()
                    .find(|q| q.id == id)
                    .map_or(TextSize::Fit, |q| q.size);
                assert_eq!(match_quick_size(size, bf, 180), Some(id));
                let moved = retape_text_size(size, bf, bt, 180, None, 180);
                assert_eq!(
                    match_quick_size(moved, bt, 180),
                    Some(id),
                    "{id:?} {from} → {to}"
                );
            }
        }
    }
}

#[test]
fn retape_keeps_fit_points_and_own_sizes() {
    assert_eq!(
        retape_text_size(TextSize::Fit, 128, 50, 180, None, 180),
        TextSize::Fit
    );
    let pt = TextSize::Pt { pt: 12.0 };
    assert_eq!(retape_text_size(pt, 128, 50, 180, None, 180), pt);
    let own = TextSize::Mm { mm: 7.0 };
    assert_eq!(retape_text_size(own, 128, 70, 180, None, 180), own);
}

#[test]
fn retape_an_untouched_label() {
    let from = resolve_default_text_size(DefaultTextSize::Auto, 24.0, 128, 180);
    let to = resolve_default_text_size(DefaultTextSize::Auto, 9.0, 50, 180);
    assert_eq!(
        retape_text_size(from, 128, 50, 180, Some((from, to)), 180),
        TextSize::Fit
    );
    let to12 = resolve_default_text_size(DefaultTextSize::Auto, 12.0, 70, 180);
    assert_eq!(
        retape_text_size(from, 128, 70, 180, Some((from, to12)), 180),
        to12
    );
    let pt = TextSize::Pt { pt: 20.0 };
    assert_eq!(
        retape_text_size(pt, 128, 50, 180, Some((from, to)), 180),
        pt
    );
}

#[test]
fn finds_the_band_a_size_was_made_in() {
    let m180 = resolve_default_text_size(DefaultTextSize::Auto, 24.0, 128, 180);
    assert!(is_band_size(m180, 128, 180, None));
    assert!(!is_band_size(m180, 320, 360, None));
    let fresh360 = resolve_default_text_size(DefaultTextSize::Auto, 24.0, 320, 360);
    assert!(!is_band_size(m180, 320, 360, Some(fresh360)));
    assert!(is_band_size(m180, 320, 360, Some(m180)));
    assert!(is_band_size(TextSize::Fit, 320, 360, None));
    assert!(!is_band_size(TextSize::Mm { mm: 7.0 }, 128, 180, None));
    let to36 = resolve_default_text_size(DefaultTextSize::Auto, 36.0, 454, 360);
    let m36 = retape_text_size(m180, 128, 454, 180, Some((m180, to36)), 360);
    assert_eq!(m36, to36);
    close(mm_of(m36), 16.02, 2);
}

// ---------------------------------------------------------------- bundled fonts

fn text_item(text: &str, family: &str, weight: u16, size: TextSize) -> TextItem {
    let Item::Text(mut t) = Item::new(ItemKind::Text) else {
        panic!("not a text item")
    };
    t.text = text.into();
    t.font_family = family.into();
    t.font_weight = weight;
    t.size = size;
    t
}

/// Draws a prepared item on a white crisp plane and thresholds it like the renderer.
fn print(p: &PreparedText, band: u16, f: u32, threshold: u8) -> ptouch::Bitmap {
    let len = u32::try_from(p.w.max(1)).unwrap_or(1);
    let (w, h) = (len * f, u32::from(band) * f);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);
    let y0 = (f64::from(band) - p.h as f64) / 2.0 * f64::from(f);
    p.draw(
        &mut pixmap.as_mut(),
        p.w as f64 * f64::from(f),
        p.h as f64 * f64::from(f),
        tiny_skia::Transform::from_translate(0.0, y0.round() as f32),
        None,
    );
    let mut raster = ptouch::LabelRaster::new(len, band);
    raster
        .blit_crisp(pixmap.data(), w, h, u8::try_from(f).unwrap_or(3), threshold)
        .expect("blit");
    raster.finish()
}

fn ink_rows(b: &ptouch::Bitmap) -> Option<(u16, u16)> {
    let rows: Vec<u16> = (0..b.height())
        .filter(|&y| (0..b.length()).any(|x| b.get(x, y)))
        .collect();
    Some((*rows.first()?, *rows.last()?))
}

#[test]
fn label_at_fit_fills_a_70_dot_band() {
    let reg = FontRegistry::new();
    let item = text_item("Label", "fira-sans", 600, TextSize::Fit);
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    assert!(p.h <= 70 && p.h >= 69, "box height {}", p.h);
    let bmp = print(&p, 70, 3, 128);
    let (top, bottom) = ink_rows(&bmp).expect("ink");
    let ink_h = i32::from(bottom) - i32::from(top) + 1;
    assert!(
        (ink_h - 70).abs() <= 1,
        "ink height {ink_h} (rows {top}..={bottom})"
    );
    // Ink starts at the box edges along the label too (widths are measured to ink).
    let cols: Vec<u32> = (0..bmp.length())
        .filter(|&x| (0..bmp.height()).any(|y| bmp.get(x, y)))
        .collect();
    assert!(
        cols.first().is_some_and(|&c| c <= 1),
        "first ink column {:?}",
        cols.first()
    );
    assert!(
        cols.last().is_some_and(|&c| c + 2 >= bmp.length()),
        "last ink column {:?} of {}",
        cols.last(),
        bmp.length()
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert_eq!(p.info.width_dots, p.w);
}

#[test]
fn pt_sizing_gives_2_5_dots_per_point() {
    let reg = FontRegistry::new();
    let item = text_item("Hello", "fira-sans", 400, TextSize::Pt { pt: 12.0 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 128, 3, 180, None, 1.0).expect("prepared");
    close(p.info.em_dots, 30.0, 9);
    let item = text_item("Hello", "fira-sans", 400, TextSize::Pt { pt: 7.0 });
    let p = prepare_text(&item, &font, 128, 3, 180, None, 1.0).expect("prepared");
    close(p.info.em_dots, 17.5, 9);
    // At 360 dpi a point is 5 dots.
    let p = prepare_text(&item, &font, 320, 3, 360, None, 1.0).expect("prepared");
    close(p.info.em_dots, 35.0, 9);
}

#[test]
fn silkscreen_snaps_to_whole_dots() {
    let reg = FontRegistry::new();
    for (size, band) in [
        (TextSize::Fit, 70),
        (TextSize::Pt { pt: 10.0 }, 128),
        (TextSize::Mm { mm: 5.0 }, 128),
    ] {
        let item = text_item("PIXEL 8", "silkscreen", 400, size);
        let font = reg.resolve_item(&item).expect("font");
        assert_eq!(font.pixel_grid(), Some(PixelGrid { em_px: 8 }));
        let p = prepare_text(&item, &font, band, 3, 180, None, 1.0).expect("prepared");
        let k = p.info.pixel_scale.expect("pixel scale");
        assert_eq!(p.info.em_dots, f64::from(k * 8), "{size:?}");
        let b = p.block();
        assert!(multiple_of(b.width, 3.0) && multiple_of(b.height, 3.0));
        for l in &b.lines {
            assert!(multiple_of(l.x, 3.0) && multiple_of(l.baseline, 3.0));
        }
        // Every font pixel is a whole k×k block of dots: hardly any grey on the crisp plane (the
        // only grey comes from the font itself: P's counter ends at 495 units, not 500).
        let len = u32::try_from(p.w).unwrap_or(1);
        let (w, h) = (len * 3, u32::from(u16::try_from(band).unwrap_or(1)) * 3);
        let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("pixmap");
        pixmap.fill(tiny_skia::Color::WHITE);
        p.draw(
            &mut pixmap.as_mut(),
            p.w as f64 * 3.0,
            p.h as f64 * 3.0,
            tiny_skia::Transform::identity(),
            None,
        );
        let data = pixmap.data();
        let red = || data.iter().step_by(4);
        let grey = red().filter(|&&r| r != 0 && r != 255).count();
        let ink = red().filter(|&&r| r == 0).count();
        assert!(grey * 50 < ink, "{size:?}: {grey} grey pixels, {ink} ink");
        // Ink starts on whole dots.
        let first_ink_col = (0..w).find(|&x| (0..h).any(|y| data[((y * w + x) * 4) as usize] == 0));
        assert!(
            first_ink_col.is_some_and(|c| c % 3 == 0),
            "{first_ink_col:?}"
        );
    }
    // Italic keeps the snapping but does not report the scale.
    let mut item = text_item("PIXEL", "silkscreen", 400, TextSize::Fit);
    item.italic = true;
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    assert_eq!(p.info.pixel_scale, None);
    assert!(p.grid.is_some());
    assert!(p.clip_slack > 0.0);
}

#[test]
fn measure_matches_canvas_semantics() {
    let reg = FontRegistry::new();
    let font = reg.resolve("fira-sans", 400, false, None).expect("font");
    let m = measure("H", 100.0, &font.face);
    assert!(m.ascent > 60.0 && m.ascent < 75.0, "{m:?}");
    assert_eq!(m.descent, 0.0);
    assert!(m.left < 0.0, "H starts right of the origin: {m:?}");
    assert!(m.right < m.advance);
    let g = measure("gjpqy", 100.0, &font.face);
    assert!(g.descent > 15.0 && g.descent < 30.0, "{g:?}");
    // Quantised to 1/1024 px.
    for v in [m.advance, m.left, m.right, m.ascent, m.descent] {
        assert_eq!((v * 1024.0).fract(), 0.0);
    }
    // Leading space moves the ink right; the advance includes it.
    let sp = measure(" H", 100.0, &font.face);
    assert!(sp.advance > m.advance);
    assert!(sp.left < m.left);
    // Kerning applies by default: "AV" is narrower than "A" + "V".
    let av = measure("AV", 100.0, &font.face).advance;
    let sum = measure("A", 100.0, &font.face).advance + measure("V", 100.0, &font.face).advance;
    assert!(av < sum, "AV {av} vs {sum}");
    // Synthesised italic leans right: more ink right of the origin, same heights.
    let it = font.face.with_italic(true);
    let mi = measure("H", 100.0, &it);
    assert!(mi.right > m.right + 10.0);
    assert_eq!(mi.ascent, m.ascent);
    assert_eq!(mi.advance, m.advance);
    // Scales linearly.
    let big = measure("Hello", 200.0, &font.face);
    let small = measure("Hello", 100.0, &font.face);
    close(big.advance, 2.0 * small.advance, 2);
    let fm = face_metrics(&font.face);
    assert!(fm.cap > 0.6 && fm.cap < 0.75, "{fm:?}");
    assert!(fm.desc > 0.15 && fm.desc < 0.3, "{fm:?}");
}

#[test]
fn quicksand_weight_axis_changes_the_ink() {
    let reg = FontRegistry::new();
    let light = reg.resolve("quicksand", 500, false, None).expect("font");
    let bold = reg.resolve("quicksand", 700, false, None).expect("font");
    assert_eq!(light.face.wght(), Some(500.0));
    assert_eq!(bold.face.wght(), Some(700.0));
    let (l, b) = (
        measure("Spices", 100.0, &light.face),
        measure("Spices", 100.0, &bold.face),
    );
    assert!(
        b.advance > l.advance,
        "bold {} vs light {}",
        b.advance,
        l.advance
    );
}

#[test]
fn warnings_small_text_quality_and_reduced() {
    let reg = FontRegistry::new();
    // 1.5 mm cap-to-descender on 12 mm tape.
    let item = text_item("tiny", "fira-sans", 400, TextSize::Mm { mm: 1.5 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    let small = p
        .warnings
        .iter()
        .find(|w| w.code == "small-text")
        .expect("small-text");
    assert_eq!(
        small.message,
        "Text is only 1.5 mm tall and may be hard to read. Use fewer lines or wider tape."
    );
    // Caveat (script) at 3 mm block: cap below 3 mm.
    let item = text_item("Jam", "caveat", 400, TextSize::Mm { mm: 3.0 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    let q = p
        .warnings
        .iter()
        .find(|w| w.code == "font-quality")
        .expect("font-quality");
    assert!(
        q.message
            .starts_with("Script letters of Caveat print poorly below 3 mm (this text is "),
        "{}",
        q.message
    );
    assert!(
        q.message
            .ends_with(" mm): make it larger (M or more) or choose a sturdier font.")
    );
    // Quicksand 700 is not thin.
    let item = text_item("Spices", "quicksand", 700, TextSize::Mm { mm: 3.0 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    assert!(!p.warnings.iter().any(|w| w.code == "font-quality"));
    let item = text_item("Spices", "quicksand", 500, TextSize::Mm { mm: 3.0 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    let q = p
        .warnings
        .iter()
        .find(|w| w.code == "font-quality")
        .expect("thin");
    assert!(q.message.starts_with("Thin strokes of Quicksand"));
    // 30 mm on 12 mm tape is reduced.
    let item = text_item("Big", "fira-sans", 400, TextSize::Mm { mm: 30.0 });
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    assert!(p.sized.reduced && p.h <= 70);
    assert!(p.warnings.iter().any(|w| w.message
        == "Text is taller than the printable area of this tape and was reduced to fit."));
    // Blank text takes no space.
    let item = text_item("  \n ", "fira-sans", 400, TextSize::Fit);
    assert!(prepare_text(&item, &font, 70, 3, 180, None, 1.0).is_none());
}

#[test]
fn inverted_text_is_white_on_black() {
    let reg = FontRegistry::new();
    let mut item = text_item("ON", "fira-sans", 700, TextSize::Fit);
    item.invert = true;
    let font = reg.resolve_item(&item).expect("font");
    let p = prepare_text(&item, &font, 70, 3, 180, None, 1.0).expect("prepared");
    assert_eq!(p.h, 70);
    let bmp = print(&p, 70, 3, 128);
    // Corners are ink (the box), the middle has paper (the letters).
    assert!(bmp.get(0, 0) && bmp.get(bmp.length() - 1, 69));
    let paper = (0..bmp.length())
        .flat_map(|x| (0..70u16).map(move |y| (x, y)))
        .filter(|&(x, y)| !bmp.get(x, y))
        .count();
    assert!(paper > 100, "{paper} paper dots");
}

#[test]
fn measure_text_at_and_readouts() {
    let reg = FontRegistry::new();
    let item = text_item("Hello", "fira-sans", 600, TextSize::Fit);
    let font = reg.resolve_item(&item).expect("font");
    let (w_fit, em_fit) = measure_text_at(&item, &font, TextSize::Fit, 70, 180);
    let (w_m, em_m) = measure_text_at(&item, &font, quick_text_sizes(70, 180)[2].size, 70, 180);
    assert!(w_m < w_fit && em_m < em_fit);
    let blank = text_item("", "fira-sans", 600, TextSize::Pt { pt: 12.0 });
    assert_eq!(
        measure_text_at(&blank, &font, TextSize::Pt { pt: 12.0 }, 70, 180),
        (0, 30.0)
    );
    let sizes = quick_text_sizes(70, 180);
    let input = ReadoutInput {
        shown: &item,
        font: &font,
        size: TextSize::Fit,
        current: Some(QuickSizeId::Fit),
        sizes: &sizes,
        band_dots: 70,
        dpi: 180,
        min_mm: 4.4,
        length_mm: 40.0,
        rendered_width_dots: Some(w_fit),
        grows: true,
    };
    let r = quick_readouts(&input);
    assert_eq!(r.len(), 5);
    assert_eq!(r[4], "40 mm");
    assert!(
        r[..4]
            .iter()
            .all(|s| s.starts_with("≈ ") && s.ends_with(" mm"))
    );
    let none = ReadoutInput {
        rendered_width_dots: None,
        ..input
    };
    assert!(quick_readouts(&none).is_empty());
}
