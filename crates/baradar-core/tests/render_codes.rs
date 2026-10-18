//! Codes: geometry preflight (port of web/tests/unit/render/codes.test.ts), the encoder's known
//! vectors (port of crates/ptouch-wasm/src/codes.rs tests) and `plan_code` (renderer.ts
//! `prepareCode`).

use std::sync::Arc;

use baradar_core::doc::{
    CodeContent, CodeItem, Item, ItemKind, ModuleSize, QrEcc, QuietZone, Symbology, WifiSecurity,
    WifiSettings,
};
use baradar_core::render::codes::*;
use datamatrix::DataMatrix;

fn qr_m(n: u32) -> ModuleMatrix {
    ModuleMatrix {
        width: n,
        height: n,
        modules: (0..n * n).map(|i| ((i % n + i / n) % 2) as u8).collect(),
    }
}

fn bars(w: u32) -> ModuleMatrix {
    ModuleMatrix {
        width: w,
        height: 1,
        modules: (0..w).map(|i| (i % 2) as u8).collect(),
    }
}

fn code(symbology: Symbology, data: &str) -> CodeItem {
    let Item::Code(mut c) = Item::new(ItemKind::Code) else {
        unreachable!()
    };
    c.symbology = symbology;
    c.data = data.into();
    c
}

fn wifi(ssid: &str, password: &str) -> WifiSettings {
    WifiSettings {
        ssid: ssid.into(),
        password: password.into(),
        security: WifiSecurity::Wpa,
        hidden: false,
    }
}

const STD: QuietZone = QuietZone::Standard;
const CMP: QuietZone = QuietZone::Compact;
const NONE: QuietZone = QuietZone::None;
const DM: Option<Symbology> = Some(Symbology::Datamatrix);

fn opts(md: ModuleSize, qz: QuietZone, band: i64) -> ModuleOptions {
    ModuleOptions {
        module_dots: md,
        quiet_zone: qz,
        symbology: None,
        band_dots: band,
        margin_dots: 0,
        limit_w: None,
        dpi: 180,
    }
}

// ------------------------------------------------------------------------------------------
// Geometry (codes.test.ts)
// ------------------------------------------------------------------------------------------

#[test]
fn detects_linear_codes_like_the_core() {
    assert!(is_linear(&bars(10)));
    assert!(is_linear(&repeat_row(&bars(10), 5)));
    assert!(!is_linear(&qr_m(21)));
}

#[test]
fn quiet_zones_and_sizes() {
    assert_eq!(quiet_modules(&qr_m(21), STD, None), (4, 4));
    assert_eq!(quiet_modules(&bars(95), STD, None), (10, 0));
    assert_eq!(quiet_modules(&qr_m(21), NONE, None), (0, 0));
    assert_eq!(
        code_size_dots(&qr_m(21), 3, STD, 128, None),
        CodeSize { w: 87, h: 87 }
    );
    assert_eq!(
        code_size_dots(&qr_m(21), 3, NONE, 128, None),
        CodeSize { w: 63, h: 63 }
    );
    assert_eq!(
        code_size_dots(&bars(95), 2, STD, 128, None),
        CodeSize { w: 230, h: 128 }
    );
}

#[test]
fn max_module_dots_fits_the_band() {
    assert_eq!(max_module_dots(&qr_m(21), 128, NONE, 0, None), 6);
    assert_eq!(max_module_dots(&qr_m(21), 128, STD, 21, None), 5);
    assert_eq!(max_module_dots(&qr_m(21), 128, STD, 0, None), 4);
    assert_eq!(max_module_dots(&qr_m(21), 112, STD, 8, None), 4);
    assert_eq!(max_module_dots(&qr_m(21), 70, STD, 7, None), 2);
    assert_eq!(max_module_dots(&qr_m(25), 70, STD, 7, None), 2);
    assert_eq!(max_module_dots(&qr_m(25), 24, STD, 0, None), 0);
    assert_eq!(max_module_dots(&bars(95), 128, STD, 0, None), 128);
}

#[test]
fn quiet_zone_modes_per_symbology() {
    assert_eq!(quiet_modules(&qr_m(21), STD, None), (4, 4));
    assert_eq!(quiet_modules(&qr_m(21), CMP, None), (2, 2));
    assert_eq!(quiet_modules(&qr_m(21), NONE, None), (0, 0));
    assert_eq!(quiet_modules(&qr_m(10), STD, DM), (1, 1));
    assert_eq!(quiet_modules(&qr_m(10), CMP, DM), (1, 1));
    assert_eq!(quiet_modules(&qr_m(10), NONE, DM), (0, 0));
    assert_eq!(
        quiet_modules(&bars(95), STD, Some(Symbology::Code128)),
        (10, 0)
    );
    assert_eq!(
        quiet_modules(&bars(95), CMP, Some(Symbology::Code128)),
        (5, 0)
    );
    assert_eq!(quiet_modules(&bars(95), NONE, None), (0, 0));
    assert_eq!(
        code_size_dots(&qr_m(21), 3, CMP, 70, None),
        CodeSize { w: 75, h: 75 }
    );
    assert_eq!(
        code_size_dots(&qr_m(10), 4, STD, 70, DM),
        CodeSize { w: 48, h: 48 }
    );
    assert_eq!(
        code_size_dots(&bars(95), 2, CMP, 70, None),
        CodeSize { w: 210, h: 70 }
    );
}

#[test]
fn max_module_dots_per_tape_and_mode() {
    // [tape, side, symbology, standard, compact, none]
    let cases: [(u8, u32, Symbology, i64, i64, i64); 10] = [
        (12, 21, Symbology::Qr, 2, 3, 3),
        (12, 25, Symbology::Qr, 2, 2, 2),
        (12, 29, Symbology::Qr, 2, 2, 2),
        (24, 21, Symbology::Qr, 5, 6, 6),
        (24, 25, Symbology::Qr, 5, 5, 5),
        (24, 33, Symbology::Qr, 3, 3, 3),
        (12, 10, Symbology::Datamatrix, 7, 7, 7),
        (12, 16, Symbology::Datamatrix, 4, 4, 4),
        (12, 22, Symbology::Datamatrix, 3, 3, 3),
        (24, 22, Symbology::Datamatrix, 5, 5, 5),
    ];
    for (tape, n, sym, standard, compact, none) in cases {
        let (band, margin) = if tape == 12 { (70, 7) } else { (128, 21) };
        let m = qr_m(n);
        let got: Vec<i64> = [STD, CMP, NONE]
            .iter()
            .map(|&q| max_module_dots(&m, band, q, margin, Some(sym)))
            .collect();
        assert_eq!(got, vec![standard, compact, none], "{tape} mm {n} {sym:?}");
        let (_, qy) = quiet_modules(&m, STD, Some(sym));
        assert!(band - i64::from(n) * standard + 2 * margin >= 2 * qy * standard);
        assert!(i64::from(n) * compact <= band);
    }
}

#[test]
fn compact_keeps_its_zone_inside_a_framed_band() {
    assert_eq!(max_module_dots(&qr_m(21), 128, CMP, 0, None), 5);
    assert_eq!(max_module_dots(&qr_m(21), 128, STD, 0, None), 4);
    assert_eq!(max_module_dots(&qr_m(22), 70, CMP, 0, DM), 2);
}

#[test]
fn choose_module_dots_2d() {
    let o = |md, qz, band, margin| ModuleOptions {
        margin_dots: margin,
        ..opts(md, qz, band)
    };
    let auto = ModuleSize::Auto;
    let c = |md: u8, max: i64, reduced: bool| ModuleChoice { md, max, reduced };
    assert_eq!(
        choose_module_dots(&qr_m(21), &o(auto, STD, 70, 7)),
        c(2, 2, false)
    );
    assert_eq!(
        choose_module_dots(&qr_m(21), &o(auto, CMP, 70, 7)),
        c(3, 3, false)
    );
    assert_eq!(
        choose_module_dots(&qr_m(21), &o(ModuleSize::Dots(5), STD, 70, 7)),
        c(2, 2, true)
    );
    assert_eq!(
        choose_module_dots(&qr_m(21), &o(ModuleSize::Dots(2), STD, 70, 7)),
        c(2, 2, false)
    );
    let framed = ModuleOptions {
        limit_w: Some(60),
        ..o(auto, STD, 128, 0)
    };
    assert_eq!(choose_module_dots(&qr_m(21), &framed).md, 2);
    assert_eq!(
        choose_module_dots(&qr_m(25), &opts(auto, STD, 24)),
        c(1, 0, false)
    );
}

#[test]
fn choose_module_dots_linear() {
    let b = bars(95);
    let auto = ModuleSize::Auto;
    let lim = |md, qz, band, w| ModuleOptions {
        limit_w: Some(w),
        ..opts(md, qz, band)
    };
    assert_eq!(choose_module_dots(&b, &opts(auto, STD, 128)).md, 2);
    assert_eq!(choose_module_dots(&b, &lim(auto, CMP, 128, 400)).md, 3);
    assert_eq!(choose_module_dots(&b, &lim(auto, STD, 128, 2000)).md, 4);
    assert_eq!(
        choose_module_dots(&b, &lim(auto, NONE, 128, 50)),
        ModuleChoice {
            md: 1,
            max: 0,
            reduced: false
        }
    );
    assert_eq!(
        choose_module_dots(&b, &lim(ModuleSize::Dots(3), STD, 128, 200)),
        ModuleChoice {
            md: 1,
            max: 1,
            reduced: true
        }
    );
    assert_eq!(
        choose_module_dots(&b, &opts(ModuleSize::Dots(6), STD, 128)),
        ModuleChoice {
            md: 6,
            max: 255,
            reduced: false
        }
    );
}

#[test]
fn linear_auto_sizes_are_physical() {
    let b = bars(95);
    let auto = ModuleSize::Auto;
    assert_eq!(dots_at(2, 180), 2);
    assert_eq!(dots_at(2, 360), 4);
    assert_eq!(dots_at(1, 90), 1);
    let at = |md, qz, band, w: Option<i64>, dpi| ModuleOptions {
        limit_w: w,
        dpi,
        ..opts(md, qz, band)
    };
    assert_eq!(choose_module_dots(&b, &at(auto, STD, 128, None, 180)).md, 2);
    assert_eq!(choose_module_dots(&b, &at(auto, STD, 320, None, 360)).md, 4);
    assert_eq!(
        choose_module_dots(&b, &at(auto, STD, 320, Some(4000), 360)).md,
        8
    );
    assert_eq!(
        choose_module_dots(&b, &at(auto, CMP, 320, Some(400), 360)).md,
        3
    );
    assert_eq!(
        choose_module_dots(&b, &at(ModuleSize::Dots(3), STD, 320, None, 360)).md,
        3
    );
    let q = ModuleOptions {
        margin_dots: 10,
        ..at(auto, CMP, 150, None, 360)
    };
    assert_eq!(choose_module_dots(&qr_m(21), &q).md, 7);
}

#[test]
fn pad_matrix_surrounds_with_light_modules() {
    let m = ModuleMatrix {
        width: 2,
        height: 1,
        modules: vec![1, 1],
    };
    assert_eq!(pad_matrix(&m, 0, 0), m);
    assert_eq!(
        pad_matrix(&m, 1, 1),
        ModuleMatrix {
            width: 4,
            height: 3,
            modules: vec![0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0]
        }
    );
    assert!(is_linear(&pad_matrix(&repeat_row(&bars(6), 3), 5, 0)));
}

#[test]
fn readability_ratings() {
    assert_eq!(readability(4, 180).level, Readability::Good);
    assert_eq!(readability(3, 180).text, "Good for phone cameras.");
    assert_eq!(readability(2, 180).level, Readability::Ok);
    assert_eq!(
        readability(2, 180).text,
        "OK close up: hold the phone near the label."
    );
    assert_eq!(readability(1, 180).level, Readability::Poor);
    assert_eq!(
        readability(1, 180).text,
        "Unreliable: modules of 1 dot often do not scan. Use wider tape or less data."
    );
    assert_eq!(readability(6, 360).level, Readability::Good);
    assert_eq!(readability(5, 360).level, Readability::Ok);
    assert_eq!(readability(4, 360).level, Readability::Ok);
    assert_eq!(
        readability(3, 360),
        ReadabilityRating {
            level: Readability::Poor,
            text: "Unreliable: modules of 3 dots (0.21 mm) often do not scan. Use wider tape or less data.".into()
        }
    );
    assert_eq!(readability(3, 180), readability(6, 360));
}

#[test]
fn code_payload_text_or_wifi() {
    let mut item = code(Symbology::Qr, "x");
    item.wifi = Some(wifi("Home", "secret123"));
    assert_eq!(code_payload(&item).unwrap(), "x");
    item.content = CodeContent::Wifi;
    assert_eq!(
        code_payload(&item).unwrap(),
        "WIFI:T:WPA;S:Home;P:secret123;;"
    );
    item.symbology = Symbology::Datamatrix;
    assert_eq!(code_payload(&item).unwrap(), "x");
    item.symbology = Symbology::Qr;
    item.wifi = None;
    let e = code_payload(&item).unwrap_err();
    assert!(e.to_string().contains("network name"), "{e}");
}

#[test]
fn repeat_and_rotate() {
    let m = ModuleMatrix {
        width: 3,
        height: 2,
        modules: vec![1, 0, 0, 0, 0, 1],
    };
    assert_eq!(
        repeat_row(&bars(4), 3).modules,
        vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]
    );
    assert_eq!(
        rotate_matrix(&m, 90),
        ModuleMatrix {
            width: 2,
            height: 3,
            modules: vec![0, 1, 0, 0, 1, 0]
        }
    );
    assert_eq!(
        rotate_matrix(&m, 180),
        ModuleMatrix {
            width: 3,
            height: 2,
            modules: vec![1, 0, 0, 0, 0, 1]
        }
    );
    assert_eq!(
        rotate_matrix(&m, 270),
        ModuleMatrix {
            width: 2,
            height: 3,
            modules: vec![0, 1, 0, 0, 1, 0]
        }
    );
    assert_eq!(rotate_matrix(&rotate_matrix(&m, 90), 270), m);
    let asym = ModuleMatrix {
        width: 3,
        height: 2,
        modules: vec![1, 1, 0, 0, 0, 0],
    };
    // 1 1 0      0 1
    // 0 0 0  →   0 1   (90° cw)
    //            0 0
    assert_eq!(rotate_matrix(&asym, 90).modules, vec![0, 1, 0, 1, 0, 0]);
    assert_eq!(rotate_matrix(&asym, 270).modules, vec![0, 0, 1, 0, 1, 0]);
    assert_eq!(rotate_matrix(&asym, 0), asym);
}

#[test]
fn ean13_check_digit_and_text() {
    assert_eq!(ean13_check_digit("590123412345"), Some(7));
    assert_eq!(ean13_check_digit("400638133393"), Some(1));
    assert_eq!(ean13_check_digit("12"), None);
    assert_eq!(
        human_readable(Symbology::Ean13, "590123412345"),
        "5 901234 123457"
    );
    assert_eq!(
        human_readable(Symbology::Ean13, "4006381333931"),
        "4 006381 333931"
    );
    assert_eq!(human_readable(Symbology::Code128, " ABC-123 "), "ABC-123");
    assert_eq!(human_readable(Symbology::Ean13, "12345"), "12345");
}

#[test]
fn tape_margin() {
    assert_eq!(tape_margin_dots(170, 128), 21);
    assert_eq!(tape_margin_dots(84, 70), 7);
    assert_eq!(tape_margin_dots(60, 70), 0);
}

#[test]
fn text_strip_geometry() {
    assert_eq!(
        code_text_strip(128, 180),
        Some(CodeTextStrip {
            text_h: 26,
            gap: 7,
            bar_h: 95,
            top: 102
        })
    );
    // 12 mm: textH = max(11, 14) = 14, gap 4, bars 52 ≥ 35.
    assert_eq!(code_text_strip(70, 180).map(|s| s.bar_h), Some(52));
    // 3.5 mm: no room.
    assert_eq!(code_text_strip(24, 180), None);
    assert!((code_text_refit_px(30.0, 600.0, 100, 3, 0.7, 0.2) - 13.23).abs() < 1e-9);
}

// ------------------------------------------------------------------------------------------
// Encoder and cache (ptouch-wasm codes.rs tests, codes.test.ts codeMatrix)
// ------------------------------------------------------------------------------------------

const STOP: &str = "1100011101011";

fn bits(m: &ModuleMatrix) -> String {
    m.modules.iter().map(|b| char::from(b'0' + b)).collect()
}

fn c128(data: &str) -> ModuleMatrix {
    encode(&CodeSpec::Code128 { data: data.into() }).unwrap()
}

#[test]
fn code128_set_selection() {
    assert_eq!(code128_syntax("Hello").unwrap(), "ƁHello");
    assert_eq!(code128_syntax("1234").unwrap(), "Ć1234");
    assert_eq!(code128_syntax("12").unwrap(), "Ć12");
    assert_eq!(code128_syntax("123").unwrap(), "Ć12Ɓ3");
    assert_eq!(code128_syntax("12345").unwrap(), "Ć1234Ɓ5");
    assert_eq!(code128_syntax("AB123").unwrap(), "ƁAB123");
    assert_eq!(code128_syntax("AB1234").unwrap(), "ƁABĆ1234");
    assert_eq!(code128_syntax("A123456B").unwrap(), "ƁAĆ123456ƁB");
    assert_eq!(code128_syntax("A12345B").unwrap(), "ƁA12345B");
    assert_eq!(code128_syntax("\tX").unwrap(), "À\tX");
    assert!(code128_syntax("").is_err());
    let e = code128_syntax("é").unwrap_err();
    assert_eq!(e.code(), "INVALID_INPUT");
    assert_eq!(e.message(), "Code 128 supports plain ASCII characters only");
}

#[test]
fn code128_known_vectors() {
    // "1234": START-C (105) 12 34, checksum (105 + 12 + 2·34) % 103 = 82, STOP.
    let m = c128("1234");
    assert_eq!(m.height, 1);
    assert_eq!(m.width, 11 * 4 + 13);
    let s = bits(&m);
    assert_eq!(&s[..11], "11010011100");
    assert_eq!(&s[11..22], "10110011100");
    assert_eq!(&s[22..33], "10001011000");
    assert_eq!(&s[33..44], "10010011110");
    assert!(s.ends_with(STOP));
    // "Hi": START-B (104) H(40) i(73); checksum 84.
    let s = bits(&c128("Hi"));
    assert_eq!(&s[..11], "11010010000");
    assert_eq!(&s[11..22], "11000101000");
    assert_eq!(&s[22..33], "10000110100");
    assert_eq!(&s[33..44], "10011110100");
    assert!(s.ends_with(STOP));
    assert!(
        encode(&CodeSpec::Code128 {
            data: "a\u{7f}".into()
        })
        .is_ok()
    );
    assert!(
        encode(&CodeSpec::Code128 {
            data: "A\u{1}".into()
        })
        .is_ok()
    );
}

#[test]
fn ean13_vectors() {
    let a = encode(&CodeSpec::Ean13 {
        data: "400638133393".into(),
    })
    .unwrap();
    let b = encode(&CodeSpec::Ean13 {
        data: "4006381333931".into(),
    })
    .unwrap();
    assert_eq!(a, b);
    assert_eq!((a.width, a.height), (95, 1));
    let s = bits(&a);
    assert_eq!(&s[..3], "101");
    assert_eq!(&s[45..50], "01010");
    assert_eq!(&s[92..], "101");
    assert_eq!(&s[85..92], "1100110"); // check digit 1, R code
    // Full pattern of 5901234123457: first digit 5 → parity LGGLLG.
    let full = bits(
        &encode(&CodeSpec::Ean13 {
            data: "5901234123457".into(),
        })
        .unwrap(),
    );
    assert_eq!(
        full,
        concat!(
            "101", "0001011", "0100111", "0110011", "0010011", "0111101",
            "0011101", // 9 0 1 2 3 4 (L G G L L G)
            "01010", "1100110", "1101100", "1000010", "1011100", "1001110",
            "1000100", // 1 2 3 4 5 7 (R)
            "101"
        )
    );
    let e = encode(&CodeSpec::Ean13 {
        data: "4006381333932".into(),
    })
    .unwrap_err();
    assert_eq!(e.code(), "INVALID_INPUT");
    assert!(e.message().contains("check digit"));
    assert!(
        encode(&CodeSpec::Ean13 {
            data: "12345".into()
        })
        .is_err()
    );
    assert!(
        encode(&CodeSpec::Ean13 {
            data: "40063813339x".into()
        })
        .is_err()
    );
}

fn dm(data: &str) -> ModuleMatrix {
    encode(&CodeSpec::Datamatrix { data: data.into() }).unwrap()
}

fn dm_decode(m: &ModuleMatrix) -> Vec<u8> {
    let pixels: Vec<bool> = m.modules.iter().map(|&b| b == 1).collect();
    DataMatrix::decode(&pixels, m.width as usize).unwrap()
}

#[test]
fn datamatrix_vectors() {
    let m = dm("A1");
    assert_eq!((m.width, m.height), (10, 10));
    assert_eq!(dm_decode(&m), b"A1");
    let m = dm("Hello, World!");
    assert_eq!((m.width, m.height), (16, 16));
    assert_eq!(dm_decode(&m), b"Hello, World!");
    for data in ["A1", "https://fjanicki.github.io/ptouch/", &"7".repeat(300)] {
        let m = dm(data);
        let (w, h) = (m.width as usize, m.height as usize);
        assert_eq!(w, h);
        let at = |x: usize, y: usize| m.modules[y * w + x];
        for i in 0..h {
            assert_eq!(at(0, i), 1);
            assert_eq!(at(i, h - 1), 1);
            assert_eq!(at(i, 0), u8::from(i % 2 == 0));
            if i > 0 {
                assert_eq!(at(w - 1, i), u8::from(i % 2 == 1));
            }
        }
    }
    let long = "x".repeat(200);
    assert_eq!(dm_decode(&dm(&long)), long.as_bytes());
    let e = encode(&CodeSpec::Datamatrix {
        data: String::new(),
    })
    .unwrap_err();
    assert_eq!(e.message(), "DataMatrix data is empty");
    let e = encode(&CodeSpec::Datamatrix {
        data: "x".repeat(5000),
    })
    .unwrap_err();
    assert!(e.message().contains("too much data"));
}

#[test]
fn qr_vectors() {
    let m = encode(&CodeSpec::Qr {
        data: "HELLO".into(),
        ecc: None,
    })
    .unwrap();
    assert_eq!((m.width, m.height), (21, 21));
    let row = |y: usize| {
        bits(&ModuleMatrix {
            width: 8,
            height: 1,
            modules: m.modules[y * 21..y * 21 + 8].to_vec(),
        })
    };
    assert_eq!(row(0), "11111110");
    assert_eq!(row(1), "10000010");
    assert_eq!(row(2), "10111010");
    let timing: String = (8..13)
        .map(|x| char::from(b'0' + m.modules[6 * 21 + x]))
        .collect();
    assert_eq!(timing, "10101");
    // 19 bytes at ECC M: version 2 (25×25); at H: version 3 (29×29).
    let url = "https://example.com";
    assert_eq!(
        encode(&CodeSpec::Qr {
            data: url.into(),
            ecc: Some(QrEcc::M)
        })
        .unwrap()
        .width,
        25
    );
    assert_eq!(
        encode(&CodeSpec::Qr {
            data: url.into(),
            ecc: Some(QrEcc::H)
        })
        .unwrap()
        .width,
        29
    );
    let e = encode(&CodeSpec::Qr {
        data: String::new(),
        ecc: None,
    })
    .unwrap_err();
    assert_eq!(e.message(), "QR code data is empty");
    let e = encode(&CodeSpec::Qr {
        data: "x".repeat(4000),
        ecc: None,
    })
    .unwrap_err();
    assert_eq!(e.message(), "too much data for a QR code");
}

#[test]
fn cache_returns_the_same_matrix_or_error() {
    let mut cache = CodeCache::new();
    let item = code(Symbology::Qr, "https://example.com");
    let first = cache.get(&item).unwrap();
    assert!(Arc::ptr_eq(&first, &cache.get(&item).unwrap()));
    assert_eq!(first.modules.len(), (first.width * first.height) as usize);
    let bad = code(Symbology::Ean13, "12");
    let e1 = cache.get(&bad).unwrap_err();
    assert_eq!(e1.code(), "INVALID_INPUT");
    assert_eq!(cache.get(&bad).unwrap_err(), e1);
    assert_eq!(cache.len(), 2);
    // ECC only keys QR codes.
    let mut dm_item = code(Symbology::Datamatrix, "A1");
    let a = cache.get(&dm_item).unwrap();
    dm_item.ecc = QrEcc::H;
    assert!(Arc::ptr_eq(&a, &cache.get(&dm_item).unwrap()));
    // Wi-Fi payloads are cached by payload.
    let mut w = code(Symbology::Qr, "");
    w.content = CodeContent::Wifi;
    w.ecc = QrEcc::L;
    w.wifi = Some(wifi("Home", "secret123"));
    let mut plain = code(Symbology::Qr, "WIFI:T:WPA;S:Home;P:secret123;;");
    plain.ecc = QrEcc::L;
    assert!(Arc::ptr_eq(
        &cache.get(&w).unwrap(),
        &cache.get(&plain).unwrap()
    ));
    // LRU: at most CACHE_SIZE entries.
    for i in 0..100 {
        let _ = cache.get(&code(Symbology::Code128, &format!("n{i}")));
    }
    assert_eq!(cache.len(), CACHE_SIZE);
    // Wi-Fi errors are not cached.
    w.wifi = None;
    let before = cache.len();
    assert!(matches!(cache.get(&w), Err(CodeError::Wifi(_))));
    assert_eq!(cache.len(), before);
    // The process-wide cache behaves the same.
    assert!(code_matrix(&item).is_ok());
}

// ------------------------------------------------------------------------------------------
// plan_code (prepareCode)
// ------------------------------------------------------------------------------------------

fn ctx(margin: i64) -> CodeContext {
    CodeContext {
        dpi: 180,
        margin_dots: margin,
        auto_linear_w: None,
    }
}

fn messages(p: &CodePlan) -> Vec<(&str, &str, bool)> {
    p.warnings
        .iter()
        .map(|w| (w.code, w.message.as_str(), w.blocking))
        .collect()
}

#[test]
fn plan_qr_on_24mm() {
    let item = code(Symbology::Qr, "https://example.com");
    let p = plan_code(&item, 128, &ctx(21), None);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert!(!p.invalid && !p.auto_linear && p.text.is_none());
    // 25×25 at 5 dots, standard zone (4 modules) on all sides.
    assert_eq!((p.w, p.h), (165, 128));
    assert_eq!(p.blits.len(), 1);
    let b = &p.blits[0];
    assert_eq!((b.md, b.dx, b.dy), (5, 0, -19));
    assert_eq!((b.m.width, b.m.height), (33, 33));
}

#[test]
fn plan_qr_reduced_and_too_big() {
    let mut item = code(Symbology::Qr, "https://example.com");
    item.module_dots = ModuleSize::Dots(9);
    let p = plan_code(&item, 128, &ctx(21), None);
    assert_eq!(p.blits[0].md, 5);
    assert_eq!(
        messages(&p),
        vec![(
            "content-overflow",
            "QR code reduced to 5 dots per module to fit the tape.",
            false
        )]
    );
    let p = plan_code(&item, 24, &ctx(0), None);
    assert!(p.invalid && p.blits.is_empty());
    assert_eq!((p.w, p.h), (24, 24));
    assert_eq!(
        messages(&p),
        vec![(
            "code-too-small",
            "QR code: 25×25 modules do not fit on this tape. Shorten the data, lower the error correction or use wider tape.",
            true
        )]
    );
    let mut d = code(Symbology::Datamatrix, &"x".repeat(100));
    d.module_dots = ModuleSize::Auto;
    let p = plan_code(&d, 24, &ctx(0), None);
    assert!(
        p.warnings[0]
            .message
            .ends_with("Shorten the data or use wider tape.")
    );
    item.module_dots = ModuleSize::Dots(0);
    let p = plan_code(&item, 128, &ctx(21), None);
    assert_eq!(
        messages(&p)[0],
        (
            "code-too-small",
            "QR code: the module size must be at least 1 dot.",
            true
        )
    );
}

#[test]
fn plan_invalid_codes() {
    let p = plan_code(
        &code(Symbology::Ean13, "4006381333932"),
        128,
        &ctx(21),
        None,
    );
    assert!(p.invalid);
    assert_eq!((p.w, p.h), (85, 128));
    assert_eq!(
        messages(&p),
        vec![(
            "code-invalid",
            "EAN-13 barcode: EAN-13 check digit is wrong",
            true
        )]
    );
    let p = plan_code(&code(Symbology::Qr, ""), 70, &ctx(7), None);
    assert_eq!((p.w, p.h), (70, 70));
    assert_eq!(
        messages(&p),
        vec![("code-invalid", "QR code: Enter the data to encode.", true)]
    );
    let mut w = code(Symbology::Qr, "");
    w.content = CodeContent::Wifi;
    w.wifi = Some(wifi("", "x"));
    let p = plan_code(&w, 70, &ctx(7), None);
    assert_eq!(
        messages(&p),
        vec![(
            "code-invalid",
            "QR code: Enter the network name (SSID).",
            true
        )]
    );
    w.wifi = Some(wifi("Home", "short"));
    let p = plan_code(&w, 128, &ctx(21), None);
    assert!(!p.invalid);
    assert_eq!(
        messages(&p),
        vec![(
            "code-invalid",
            "QR code: A WPA password has 8 to 63 characters (or 64 hex digits); this one has 5. Phones may not join.",
            false
        )]
    );
}

#[test]
fn plan_linear_with_text() {
    let mut item = code(Symbology::Code128, "1234");
    item.show_text = true;
    let p = plan_code(&item, 128, &ctx(21), None);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert!(p.auto_linear);
    // 57 modules + 2 × 10 quiet, 2 dots each.
    assert_eq!((p.w, p.h), (154, 128));
    let text = p.text.as_ref().unwrap();
    assert_eq!(text.text, "1234");
    assert_eq!(text.strip.bar_h, 95);
    // 47 rows × 2 = 94 < 95: a one-row top-up at 93.
    assert_eq!(p.blits.len(), 2);
    assert_eq!(
        (p.blits[0].m.width, p.blits[0].m.height, p.blits[0].dy),
        (77, 47, 0)
    );
    assert_eq!((p.blits[1].m.height, p.blits[1].dy), (1, 93));
    assert!(is_linear(&p.blits[0].m));

    let mut e = code(Symbology::Ean13, "590123412345");
    e.show_text = true;
    let p = plan_code(&e, 70, &ctx(7), None);
    assert_eq!(p.text.unwrap().text, "5 901234 123457");
    assert_eq!(p.blits.len(), 1); // 52 = 26 × 2
}

#[test]
fn plan_linear_limits_and_warnings() {
    let mut item = code(Symbology::Code128, "1234");
    // A fixed-length label lets auto codes grow to 4 dots.
    let c = CodeContext {
        auto_linear_w: Some(1000),
        ..ctx(21)
    };
    assert_eq!(plan_code(&item, 128, &c, None).blits[0].md, 4);
    // A frame too short blocks.
    let p = plan_code(&item, 128, &ctx(0), Some(50));
    assert!(p.invalid);
    assert_eq!((p.w, p.h), (50, 128));
    assert_eq!(
        messages(&p),
        vec![(
            "code-too-small",
            "Code 128 barcode: 57 modules do not fit in its frame. Make the frame longer or shorten the data.",
            true
        )]
    );
    // A fixed module lowered to fit the frame; 1-dot bars warn.
    item.module_dots = ModuleSize::Dots(3);
    let p = plan_code(&item, 128, &ctx(0), Some(100));
    assert!(!p.auto_linear);
    assert_eq!(
        messages(&p),
        vec![
            (
                "content-overflow",
                "Code 128 barcode reduced to 1 dot per module to fit its frame.",
                false
            ),
            (
                "code-too-small",
                "Code 128 barcode: bars are 0.14 mm wide and may not scan. Use 2 or more dots per module.",
                false
            ),
        ]
    );
    // No room for the text on 3.5 mm tape; short bars warn.
    item.module_dots = ModuleSize::Auto;
    item.show_text = true;
    let p = plan_code(&item, 20, &ctx(0), None);
    assert!(p.text.is_none());
    assert_eq!(
        messages(&p),
        vec![
            (
                "content-overflow",
                "Code 128 barcode: there is not enough room for the text under the bars on this tape, so it is left out.",
                false
            ),
            (
                "code-too-small",
                "Code 128 barcode: bars are only 2.8 mm tall and may not scan.",
                false
            ),
        ]
    );
}

#[test]
fn symbology_names() {
    assert_eq!(symbology_name(Symbology::Qr), "QR code");
    assert_eq!(symbology_name(Symbology::Code128), "Code 128 barcode");
    assert_eq!(symbology_name(Symbology::Ean13), "EAN-13 barcode");
    assert_eq!(symbology_name(Symbology::Datamatrix), "DataMatrix code");
}

#[test]
fn plans_blit_into_a_raster() {
    // End to end: the padded QR blit clears its zone and paints the finder at the right place.
    let item = code(Symbology::Qr, "HELLO");
    let p = plan_code(&item, 128, &ctx(21), None);
    let mut r = ptouch::LabelRaster::new(u32::try_from(p.w).unwrap(), 128);
    for b in &p.blits {
        r.blit_code(&b.m, b.dx as i32, b.dy as i32, b.md, false)
            .unwrap();
    }
    let bmp = r.finish();
    let md = i64::from(p.blits[0].md);
    let x0 = 4 * md; // quiet zone
    let y0 = p.blits[0].dy + 4 * md;
    assert!(bmp.get(x0 as u32, y0 as u16));
    assert!(!bmp.get((x0 - 1) as u32, y0 as u16));
}
