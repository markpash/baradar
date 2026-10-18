//! Fonts: ports of web/tests/unit/render/{font-catalog,font-library} and persist/fonts tests,
//! plus WOFF decoding, the registry and custom fonts.

use std::io::Write as _;
use std::sync::Arc;

use baradar_core::doc::schema::{FONT_FAMILY_IDS, FontSource, Item, ItemKind, LabelDoc};
use baradar_core::render::fonts::*;
use rustybuzz::ttf_parser::{self, Tag};
use sha2::{Digest, Sha256};

const CORE: [&str; 4] = [
    "fira-sans",
    "archivo-narrow",
    "jetbrains-mono",
    "atkinson-hyperlegible",
];
const CORE_PREFIXES: [&str; 4] = [
    "FiraSans",
    "ArchivoNarrow",
    "JetBrainsMono",
    "AtkinsonHyperlegible",
];
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts");

fn files_of(def: &FontDef) -> Vec<&'static str> {
    let mut v: Vec<&str> = def.files.iter().map(|(_, f)| *f).collect();
    v.dedup();
    v
}

fn sfnt(file: &str) -> Vec<u8> {
    assert!(bundled_font_file(file).is_some(), "{file} not embedded");
    bundled_sfnt(file)
        .unwrap_or_else(|e| panic!("{file}: {e}"))
        .to_vec()
}

/// Every string of name ID `id` (Windows/Unicode and Mac Roman records).
fn names(face: &ttf_parser::Face<'_>, id: u16) -> Vec<String> {
    face.names()
        .into_iter()
        .filter(|n| n.name_id == id)
        .filter_map(|n| {
            n.to_string().or_else(|| {
                (n.platform_id == ttf_parser::PlatformId::Macintosh)
                    .then(|| n.name.iter().map(|&b| char::from(b)).collect())
            })
        })
        .collect()
}

// ---------------------------------------------------------------- catalogue

#[test]
fn ids_are_unique_schema_ids_with_core_first() {
    let ids: Vec<&str> = FONTS.iter().map(|f| f.id).collect();
    assert_eq!(ids, FONT_FAMILY_IDS, "schema order");
    assert_eq!(&ids[..4], CORE);
    let core: Vec<&str> = FONTS.iter().filter(|f| f.core).map(|f| f.id).collect();
    assert_eq!(core, CORE);
    assert_eq!(FONTS.iter().filter(|f| !f.core).count(), 21);
}

#[test]
fn every_entry_is_well_formed() {
    let cats: Vec<FontCategory> = FONT_CATEGORIES.iter().map(|c| c.0).collect();
    for f in &FONTS {
        assert_eq!(f.family, format!("ptouch {}", f.label), "{}", f.id);
        assert!(cats.contains(&f.category), "{}", f.id);
        let mut keys: Vec<u16> = f.files.iter().map(|(w, _)| *w).collect();
        keys.sort_unstable();
        let mut weights = f.weights.to_vec();
        weights.sort_unstable();
        assert_eq!(keys, weights, "{}", f.id);
        assert!(
            !f.preview.trim().is_empty() && !f.hint.trim().is_empty(),
            "{}",
            f.id
        );
        if let Some(p) = f.pixel {
            assert!(p.em_px > 0, "{}", f.id);
        }
        for (_, file) in f.files {
            let stem = file.strip_suffix(".woff2").unwrap_or("");
            assert!(
                !stem.is_empty()
                    && stem
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{file}"
            );
        }
        let prefix = match f.license {
            FontLicense::Ofl11 => "OFL-",
            FontLicense::Apache20 => "Apache-",
        };
        assert!(f.license_file.starts_with(prefix), "{}", f.license_file);
    }
}

#[test]
fn unknown_ids_fall_back_to_the_first_family() {
    assert!(find_font("nope").is_none());
    assert_eq!(font_def("nope").id, "fira-sans");
}

#[test]
fn library_files_are_lazy_and_small() {
    for def in FONTS.iter().filter(|f| !f.core) {
        assert!(def.weights.len() <= 3, "{}", def.id);
        for file in files_of(def) {
            for p in CORE_PREFIXES {
                assert!(!file.starts_with(p), "{file}");
            }
            let n = bundled_font_file(file).map_or(0, <[u8]>::len);
            assert!(n > 0 && n <= 80 * 1024, "{file}: {n} bytes");
        }
    }
    assert_eq!(
        core_font_files().len(),
        FONTS
            .iter()
            .filter(|f| f.core)
            .map(|f| f.files.len())
            .sum::<usize>()
    );
}

/// Every bundled WOFF2 decodes straight from its bytes (14 of them need the overlap-bitmap
/// workaround) and names the family the catalogue says.
#[test]
fn every_bundled_woff2_decodes_to_its_family() {
    for def in &FONTS {
        for file in files_of(def) {
            let woff2 = bundled_font_file(file).expect("embedded");
            let ttf = decode_font_bytes(woff2).unwrap_or_else(|e| panic!("{file}: {e}"));
            let face = ttf_parser::Face::parse(&ttf, 0).unwrap_or_else(|e| panic!("{file}: {e}"));
            let family: Vec<String> = names(&face, 16)
                .into_iter()
                .chain(names(&face, 1))
                .collect();
            assert!(
                family.iter().any(|n| n.starts_with(def.label)),
                "{file}: {family:?}"
            );
            assert!(
                face.glyph_index('A')
                    .and_then(|g| face.glyph_bounding_box(g))
                    .is_some(),
                "{file}"
            );
        }
    }
}

/// An uploaded WOFF2 with an `overlapSimpleBitmap` in its glyf transform (Oswald, instanced from
/// a variable font) loads and draws.
#[test]
fn an_uploaded_woff2_with_an_overlap_bitmap_loads() {
    let bytes = bundled_font_file("Oswald-Regular.woff2").expect("file");
    let mut plain = bytes;
    assert!(
        woff2_patched::convert_woff2_to_ttf(&mut plain).is_err(),
        "woff2-patched reads it now: the workaround can go"
    );
    let info = inspect_font_file(bytes, "oswald.woff2").expect("inspect");
    assert_eq!(
        (info.format, info.family.as_str()),
        (FontFormat::Woff2, "Oswald")
    );
    let reg = FontRegistry::new();
    let reference = "sha256-cccccccccccccccccccccccccccccccc";
    assert_eq!(reg.add_user_font(reference, bytes).as_deref(), Ok("Oswald"));
    let src = user(reference, "Oswald");
    let r = reg
        .resolve("fira-sans", 400, false, Some(&src))
        .expect("font");
    assert!(r.custom && r.issue.is_none());
    let m = baradar_core::render::text::measure("Oswald", 100.0, &r.face);
    assert!(m.advance > 100.0 && m.ascent > 50.0, "{m:?}");
}

#[test]
fn every_asset_belongs_to_a_family() {
    let used: Vec<&str> = FONTS
        .iter()
        .flat_map(|d| files_of(d).into_iter().chain([d.license_file]))
        .collect();
    let mut orphans = Vec::new();
    for entry in std::fs::read_dir(ASSETS).expect("assets/fonts") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if (name.ends_with(".woff2") || name.ends_with(".txt")) && !used.contains(&name.as_str()) {
            orphans.push(name);
        }
    }
    assert!(orphans.is_empty(), "{orphans:?}");
}

#[test]
fn only_silkscreen_is_on_a_pixel_grid() {
    let pixel: Vec<(&str, u32)> = FONTS
        .iter()
        .filter_map(|f| f.pixel.map(|p| (f.id, p.em_px)))
        .collect();
    assert_eq!(pixel, [("silkscreen", 8)]);
    let cat: Vec<&str> = FONTS
        .iter()
        .filter(|f| f.category == FontCategory::Pixel)
        .map(|f| f.id)
        .collect();
    assert_eq!(cat, ["silkscreen", "vt323", "pixelify-sans"]);
}

#[test]
fn flags() {
    assert!(font_def("jetbrains-mono").monospace() && font_def("courier-prime").monospace());
    assert!(font_def("roboto-slab").serif() && font_def("special-elite").serif());
    assert!(font_def("caveat").cursive() && font_def("pacifico").cursive());
    assert_eq!(font_def("quicksand").quality, Some(FontQuality::Thin));
    assert_eq!(font_def("caveat").quality, Some(FontQuality::Script));
    assert!(font_def("quicksand").variable() && !font_def("oswald").variable());
    let condensed: Vec<&str> = FONTS.iter().filter(|f| f.condensed).map(|f| f.id).collect();
    assert_eq!(
        condensed,
        [
            "archivo-narrow",
            "oswald",
            "bebas-neue",
            "barlow-condensed",
            "anton",
            "big-shoulders-stencil"
        ]
    );
    assert_eq!(CODE_TEXT_FONT, ("jetbrains-mono", 400));
}

// ---------------------------------------------------------------- the files

#[test]
fn every_file_decodes_and_is_the_family_and_weight() {
    for def in &FONTS {
        for &(w, file) in def.files {
            let bytes = sfnt(file);
            let face = ttf_parser::Face::parse(&bytes, 0).unwrap_or_else(|e| panic!("{file}: {e}"));
            let family: Vec<String> = names(&face, 16)
                .into_iter()
                .chain(names(&face, 1))
                .collect();
            assert!(
                family.iter().any(|n| n.starts_with(def.label)),
                "{}: {family:?}",
                def.id
            );
            let wght = face
                .variation_axes()
                .into_iter()
                .find(|a| a.tag == Tag::from_bytes(b"wght"));
            match wght {
                Some(a) => assert!(a.min_value <= f32::from(w) && f32::from(w) <= a.max_value),
                None => assert_eq!(face.weight().to_number(), w, "{} {w}", def.id),
            }
            // Loads as a face (cached) with the axis set for variable files.
            let f = bundled_face(def, w).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(f.wght().is_some(), wght.is_some(), "{file}");
        }
    }
}

#[test]
fn files_cover_ascii_and_the_preview() {
    for def in &FONTS {
        for file in files_of(def) {
            let bytes = sfnt(file);
            let face = ttf_parser::Face::parse(&bytes, 0).expect("face");
            for c in (0x20u8..0x7f).map(char::from).chain(def.preview.chars()) {
                assert!(face.glyph_index(c).is_some(), "{file} {c:?}");
            }
        }
    }
}

#[test]
fn files_keep_copyright_and_licence_names() {
    for def in &FONTS {
        for file in files_of(def) {
            let bytes = sfnt(file);
            let face = ttf_parser::Face::parse(&bytes, 0).expect("face");
            assert!(
                names(&face, 0)
                    .join(" ")
                    .to_lowercase()
                    .contains("copyright"),
                "{file}"
            );
            let licence = names(&face, 13).join(" ");
            let want = match def.license {
                FontLicense::Ofl11 => "Open Font License",
                FontLicense::Apache20 => "Apache License",
            };
            assert!(licence.contains(want), "{file}: {licence}");
        }
    }
}

#[test]
fn pixel_grid_divides_the_em_and_every_advance() {
    for def in FONTS.iter().filter(|f| f.pixel.is_some()) {
        let em_px = def.pixel.map_or(1, |p| p.em_px) as u16;
        for file in files_of(def) {
            let bytes = sfnt(file);
            let face = ttf_parser::Face::parse(&bytes, 0).expect("face");
            let em = face.units_per_em();
            assert_eq!(em % em_px, 0, "{file}");
            let grid = em / em_px;
            let off: Vec<u16> = (0..face.number_of_glyphs())
                .filter_map(|g| face.glyph_hor_advance(ttf_parser::GlyphId(g)))
                .filter(|a| a % grid != 0)
                .collect();
            assert!(off.is_empty(), "{file}: {off:?}");
        }
    }
}

#[test]
fn licence_texts_are_bundled_and_hashed_in_sources() {
    for def in &FONTS {
        let text = def
            .license_text()
            .unwrap_or_else(|| panic!("{}", def.license_file));
        let hex: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(
            FONT_SOURCES_MD.contains(&hex),
            "{} hash not in SOURCES.md",
            def.license_file
        );
        if def.license == FontLicense::Apache20 {
            assert!(text.contains("Version 2.0, January 2004"));
        }
    }
    assert!(FONT_SOURCES_MD.contains("f2bd09badbc763d8757951d52deec29da27e85fb"));
}

// ---------------------------------------------------------------- uploads (persist-fonts)

fn sig(tag: &[u8]) -> Vec<u8> {
    let mut v = tag.to_vec();
    v.resize(16, 0);
    v
}

#[test]
fn recognises_the_four_signatures() {
    assert_eq!(
        sniff_font_format(&sig(&[0, 1, 0, 0])),
        Some(FontFormat::Ttf)
    );
    assert_eq!(sniff_font_format(&sig(b"true")), Some(FontFormat::Ttf));
    assert_eq!(sniff_font_format(&sig(b"OTTO")), Some(FontFormat::Otf));
    assert_eq!(sniff_font_format(&sig(b"wOFF")), Some(FontFormat::Woff));
    assert_eq!(sniff_font_format(&sig(b"wOF2")), Some(FontFormat::Woff2));
}

#[test]
fn rejects_other_empty_and_oversized_files() {
    assert_eq!(sniff_font_format(&sig(b"%PDF")), None);
    assert_eq!(sniff_font_format(&[0, 1, 0, 0]), None);
    let not = check_font_file(&sig(&[137, 80, 78, 71]), 16).unwrap_err();
    assert!(not.to_string().contains("TTF, OTF, WOFF or WOFF2"));
    assert_eq!(
        check_font_file(&[], 0).unwrap_err().to_string(),
        "This file is empty."
    );
    let big = check_font_file(&sig(&[0, 1, 0, 0]), MAX_FONT_BYTES + 1).unwrap_err();
    assert_eq!(
        big.to_string(),
        "This font is too large (10.0 MB). Fonts up to 10 MB can be added."
    );
    assert_eq!(
        check_font_file(&sig(&[0, 1, 0, 0]), MAX_FONT_BYTES),
        Ok(FontFormat::Ttf)
    );
    assert_eq!(
        check_font_count(MAX_FONTS).unwrap_err().to_string(),
        "You already have 50 fonts. Remove one before adding another."
    );
    assert!(check_font_count(49).is_ok());
}

#[test]
fn names_a_font_after_its_file() {
    assert_eq!(name_from_file_name("Inter-Bold_v4.ttf"), "Inter Bold v4");
    assert_eq!(name_from_file_name("C:\\fonts\\My Font.WOFF2"), "My Font");
    assert_eq!(name_from_file_name(".ttf"), "Uploaded font");
    assert_eq!(
        name_from_file_name(&format!("{}.otf", "x".repeat(300)))
            .chars()
            .count(),
        100
    );
}

/// A `name` table from (platform, encoding, language, id, raw bytes) records.
fn name_table(records: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_be_bytes());
    push(&mut out, 0);
    push(&mut out, records.len() as u16);
    push(&mut out, (6 + 12 * records.len()) as u16);
    let mut off = 0u16;
    for (p, e, l, id, raw) in records {
        for v in [*p, *e, *l, *id, raw.len() as u16, off] {
            push(&mut out, v);
        }
        off += raw.len() as u16;
    }
    for r in records {
        out.extend_from_slice(&r.4);
    }
    out
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn win(id: u16, s: &str) -> (u16, u16, u16, u16, Vec<u8>) {
    (3, 1, 0x409, id, utf16(s))
}

/// An sfnt holding the given tables (enough for the name readers).
fn sfnt_with(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let mut out = vec![0, 1, 0, 0];
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    let mut off = 12 + 16 * n;
    for (tag, data) in tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        off += data.len().div_ceil(4) * 4;
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    out
}

/// Wraps an sfnt as WOFF 1.0 with zlib-compressed tables (`toWoff`).
fn to_woff(sfnt: &[u8]) -> Vec<u8> {
    let n = usize::from(u16::from_be_bytes([sfnt[4], sfnt[5]]));
    let mut tables = Vec::new();
    for i in 0..n {
        let r = &sfnt[12 + i * 16..28 + i * 16];
        let off = u32::from_be_bytes([r[8], r[9], r[10], r[11]]) as usize;
        let len = u32::from_be_bytes([r[12], r[13], r[14], r[15]]) as usize;
        let data = &sfnt[off..off + len];
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(data).expect("deflate");
        let comp = z.finish().expect("deflate");
        let stored = if comp.len() < data.len() {
            comp
        } else {
            data.to_vec()
        };
        tables.push((r[..4].to_vec(), r[4..8].to_vec(), stored, len));
    }
    let mut out = b"wOFF".to_vec();
    out.extend_from_slice(&sfnt[..4]);
    out.extend_from_slice(&[0; 4]); // length (patched below)
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0; 2 + 4 + 4 + 20]);
    let mut off = 44 + 20 * n;
    for (tag, sum, data, orig) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(&(*orig as u32).to_be_bytes());
        out.extend_from_slice(sum);
        off += data.len().div_ceil(4) * 4;
    }
    for (_, _, data, _) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    let len = (out.len() as u32).to_be_bytes();
    out[8..12].copy_from_slice(&len);
    out
}

#[test]
fn reads_family_and_subfamily_of_a_truetype_file() {
    let font = sfnt_with(&[(
        b"name",
        name_table(&[win(1, "Gridfinity Sans"), win(2, "Bold")]),
    )]);
    assert_eq!(
        font_display_name(&font, FontFormat::Ttf).as_deref(),
        Some("Gridfinity Sans Bold")
    );
    let plain = sfnt_with(&[(b"name", name_table(&[win(1, "Plain"), win(2, "Regular")]))]);
    assert_eq!(
        font_display_name(&plain, FontFormat::Ttf).as_deref(),
        Some("Plain")
    );
    let dup = sfnt_with(&[(b"name", name_table(&[win(1, "Inter Bold"), win(2, "Bold")]))]);
    assert_eq!(
        font_display_name(&dup, FontFormat::Ttf).as_deref(),
        Some("Inter Bold")
    );
}

#[test]
fn reads_names_through_woff() {
    let font = sfnt_with(&[(
        b"name",
        name_table(&[
            win(16, "Woffy"),
            win(17, "Light"),
            win(1, "Woffy Light"),
            win(2, "Regular"),
        ]),
    )]);
    let woff = to_woff(&font);
    assert_eq!(sniff_font_format(&woff), Some(FontFormat::Woff));
    assert_eq!(
        font_display_name(&woff, FontFormat::Woff).as_deref(),
        Some("Woffy Light")
    );
}

#[test]
fn prefers_windows_english_and_typographic_ids() {
    let t = name_table(&[
        (1, 0, 0, 1, b"Mac".to_vec()),
        (3, 1, 0x409, 1, utf16("Win")),
        (3, 1, 0x409, 16, utf16("Typo")),
    ]);
    assert_eq!(
        parse_name_table(&t),
        NameInfo {
            family: Some("Typo".into()),
            subfamily: None
        }
    );
    let mac_only = name_table(&[
        (1, 0, 0, 1, b"Mac".to_vec()),
        (3, 1, 0x407, 1, utf16("Deutsch")),
    ]);
    assert_eq!(
        parse_name_table(&mac_only).family.as_deref(),
        Some("Deutsch")
    );
}

#[test]
fn never_panics_on_garbage() {
    assert_eq!(parse_name_table(&[0, 0, 0, 200, 0, 0]), NameInfo::default());
    let mut broken = sfnt_with(&[(b"name", name_table(&[win(1, "X")]))]);
    broken[12..28].fill(255);
    assert_eq!(font_display_name(&broken, FontFormat::Ttf), None);
    assert_eq!(font_display_name(&sig(b"wOF2"), FontFormat::Woff2), None);
    assert_eq!(font_display_name(&sig(b"wOFF"), FontFormat::Woff), None);
    assert!(decode_font_bytes(&sig(b"wOF2")).is_err());
    assert!(decode_font_bytes(&sig(b"wOFF")).is_err());
    assert!(inspect_font_file(&sig(&[0, 1, 0, 0]), "x.ttf").is_err());
    let mut junk = vec![0u8; 4096];
    junk[..4].copy_from_slice(b"wOFF");
    junk[12] = 0xff;
    assert!(decode_font_bytes(&junk).is_err());
}

#[test]
fn woff1_decodes_to_the_same_font() {
    let ttf = sfnt("Anton-Regular.woff2");
    let woff = to_woff(&ttf);
    let back = decode_font_bytes(&woff).expect("woff");
    let a = ttf_parser::Face::parse(&ttf, 0).expect("ttf");
    let b = ttf_parser::Face::parse(&back, 0).expect("woff → ttf");
    assert_eq!(a.number_of_glyphs(), b.number_of_glyphs());
    let g = a.glyph_index('A').expect("A");
    assert_eq!(a.glyph_bounding_box(g), b.glyph_bounding_box(g));
    let info = inspect_font_file(&woff, "anton.woff").expect("inspect");
    assert_eq!(info.format, FontFormat::Woff);
    assert_eq!(info.family, "Anton");
    // WOFF2 uploads are named from their name table too (the studio uses the file name).
    let w2 = inspect_font_file(
        bundled_font_file("Barlow-SemiBold.woff2").expect("file"),
        "x.woff2",
    )
    .expect("inspect");
    assert_eq!(
        (w2.format, w2.family.as_str()),
        (FontFormat::Woff2, "Barlow SemiBold")
    );
}

// ---------------------------------------------------------------- registry

fn doc_with(items: Vec<Item>) -> LabelDoc {
    let mut doc = LabelDoc::new();
    doc.items = items;
    doc
}

fn text(t: &str, family: &str, weight: u16, custom: Option<FontSource>) -> Item {
    let mut item = Item::new(ItemKind::Text);
    if let Item::Text(x) = &mut item {
        x.text = t.into();
        x.font_family = family.into();
        x.font_weight = weight;
        x.custom_font = custom;
    }
    item
}

fn user(reference: &str, family: &str) -> FontSource {
    FontSource::User {
        reference: reference.into(),
        family: family.into(),
    }
}

#[test]
fn fonts_used_deduplicates_resolved_weights() {
    let mut code = Item::new(ItemKind::Code);
    if let Item::Code(c) = &mut code {
        c.symbology = baradar_core::doc::schema::Symbology::Code128;
        c.show_text = true;
    }
    let doc = doc_with(vec![
        text("a", "fira-sans", 600, None),
        text("b", "fira-sans", 650, None),
        text("  ", "oswald", 400, None),
        text("c", "nope", 400, None),
        code,
    ]);
    assert_eq!(
        fonts_used(&doc),
        [
            ("fira-sans", 600),
            ("fira-sans", 400),
            ("jetbrains-mono", 400)
        ]
    );
}

#[test]
fn resolves_bundled_and_unknown_families() {
    let reg = FontRegistry::new();
    let r = reg
        .resolve("barlow-condensed", 600, true, None)
        .expect("font");
    assert_eq!(
        (r.def.id, r.weight, r.custom),
        ("barlow-condensed", 500, false)
    );
    assert!(r.face.italic());
    assert!(r.issue.is_none());
    let unknown = reg.resolve("nope", 700, false, None).expect("font");
    assert_eq!((unknown.def.id, unknown.weight), ("fira-sans", 700));
    assert_eq!(
        reg.resolve("silkscreen", 400, false, None)
            .expect("font")
            .pixel_grid(),
        Some(PixelGrid { em_px: 8 })
    );
}

#[test]
fn a_missing_custom_font_falls_back_with_a_warning() {
    let reg = FontRegistry::new();
    let src = user("sha256-0123456789abcdef0123456789abcdef", "Brand Sans");
    let r = reg.resolve("oswald", 700, false, Some(&src)).expect("font");
    assert!(!r.custom);
    assert_eq!(r.def.id, "oswald");
    let issue = r.issue.expect("issue");
    assert_eq!(issue.code(), "font-missing");
    assert_eq!(
        issue.message(),
        "The font “Brand Sans” is not available on this device, so the label uses its built-in font instead. Add the font under Fonts to print it as designed."
    );
    assert!(reg.custom_font_missing(&src));
    let doc = doc_with(vec![
        text("x", "oswald", 700, Some(src.clone())),
        text("y", "anton", 400, Some(src)),
    ]);
    let report = reg.ensure_fonts(&doc);
    assert_eq!(report.fallbacks, Vec::<String>::new());
    assert_eq!(report.missing, ["Brand Sans"]);
    assert_eq!(report.issues().len(), 1);
    let local = FontSource::Local {
        postscript_name: "No\"Such".into(),
        family: "Nope".into(),
    };
    assert!(reg.load_custom_font(&local).is_none());
    assert_eq!(
        FontIssue::Fallback("Oswald 700".into()).message(),
        "The font “Oswald 700” could not be loaded; a system font is used instead. Reload the page while online to fix this."
    );
}

#[test]
fn uploaded_fonts_from_bytes_or_a_loader() {
    let reg = FontRegistry::new();
    let src = user("sha256-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Anton");
    let anton = bundled_font_file("Anton-Regular.woff2")
        .expect("file")
        .to_vec();
    assert_eq!(
        reg.add_user_font("sha256-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &anton)
            .as_deref(),
        Ok("Anton")
    );
    assert!(reg.custom_font_ready(&src));
    let r = reg
        .resolve("fira-sans", 400, false, Some(&src))
        .expect("font");
    assert!(r.custom && r.issue.is_none() && r.pixel_grid().is_none());
    reg.forget_custom_font(&src);
    assert!(!reg.custom_font_ready(&src));

    // A loader fetches it on first use; a failed attempt is retried once a loader is set.
    let src2 = user("sha256-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "Pacifico");
    assert!(reg.load_custom_font(&src2).is_none());
    let pacifico = bundled_font_file("Pacifico-Regular.woff2")
        .expect("file")
        .to_vec();
    reg.set_user_font_loader(Arc::new(move |r: &str| {
        r.ends_with('b').then(|| pacifico.clone())
    }));
    let r = reg
        .resolve("fira-sans", 400, false, Some(&src2))
        .expect("font");
    assert!(r.custom, "{:?}", r.issue);
    // Quality warnings and pixel snapping skip custom fonts.
    let silk = reg
        .resolve("silkscreen", 400, false, Some(&src2))
        .expect("font");
    assert!(silk.custom && silk.pixel_grid().is_none());
}

#[test]
fn registry_is_shareable_across_threads() {
    let reg = Arc::new(FontRegistry::new());
    let handles: Vec<_> = ["fira-sans", "oswald", "quicksand", "silkscreen"]
        .into_iter()
        .map(|id| {
            let reg = Arc::clone(&reg);
            std::thread::spawn(move || reg.resolve(id, 700, false, None).map(|r| r.def.id))
        })
        .collect();
    for h in handles {
        assert!(h.join().expect("thread").is_ok());
    }
}

#[test]
fn local_font_names_are_validated_and_sorted() {
    let sf = |ps: &str, full: &str| SystemFont {
        path: "/x.ttf".into(),
        index: 0,
        family: "F".into(),
        subfamily: String::new(),
        full_name: full.into(),
        postscript_name: ps.into(),
        weight: 400,
        italic: false,
        variable: false,
    };
    let list = to_local_fonts(&[
        sf("Zeta-Bold", "Zeta  Bold"),
        sf("Alpha", ""),
        sf("Bad\"Quote", "Bad"),
        sf("", "Empty"),
        sf("Alpha", "Duplicate"),
    ]);
    let got: Vec<(&str, &str)> = list
        .iter()
        .map(|l| (l.postscript_name.as_str(), l.name.as_str()))
        .collect();
    assert_eq!(got, [("Alpha", "Alpha"), ("Zeta-Bold", "Zeta Bold")]);
    assert!(valid_postscript_name("Helvetica-Bold"));
    assert!(!valid_postscript_name("a\\b") && !valid_postscript_name(&"x".repeat(101)));
}

#[test]
fn installed_fonts_group_by_family() {
    let face = |family: &str, ps: &str, weight: u16, italic: bool, variable: bool| SystemFont {
        path: format!("/{ps}.ttf").into(),
        index: 0,
        family: family.into(),
        subfamily: String::new(),
        full_name: ps.into(),
        postscript_name: ps.into(),
        weight,
        italic,
        variable,
    };
    let fams = to_local_families(&[
        face("Static", "Static-Bold", 700, false, false),
        face("Static", "Static-Regular", 400, false, false),
        face("Static", "Static-Italic", 400, true, false),
        face("Static", "Static-Light", 300, false, false),
        face("Vary", "Vary-Regular", 400, false, false),
        face("Vary", "Vary-Regular", 400, false, true),
        face(".Hidden", "Hidden-Regular", 400, false, false),
    ]);
    let got: Vec<(&str, &str, &[u16], bool)> = fams
        .iter()
        .map(|f| (f.family.as_str(), f.postscript_name.as_str(), f.weights.as_slice(), f.variable))
        .collect();
    assert_eq!(
        got,
        [
            ("Static", "Static-Regular", &[300, 400, 700][..], false),
            ("Vary", "Vary-Regular", &[100, 200, 300, 400, 500, 600, 700, 800, 900][..], true),
        ]
    );
}

/// On this machine Vazirmatn is installed; when it is, Persian falls back to it and an installed
/// family follows the item's weight.
#[test]
fn persian_prefers_vazirmatn_when_installed() {
    let reg = FontRegistry::new();
    let Some(vazir) = reg.local_families().iter().find(|f| f.family == "Vazirmatn").cloned() else {
        return; // not installed here
    };
    let fb = fallback_face('\u{0647}', 700).expect("a font for Persian");
    assert!(fb.key().contains("Vazirmatn"), "{}", fb.key());
    let src = FontSource::Local { postscript_name: vazir.postscript_name.clone(), family: vazir.family.clone() };
    let bold = reg.resolve("anton", 700, false, Some(&src)).expect("resolve");
    assert!(bold.custom && bold.weight == 700);
    if vazir.variable {
        assert_eq!(bold.face.wght(), Some(700.0));
    }
}

#[test]
fn installed_fonts_load_by_postscript_name() {
    let reg = FontRegistry::new();
    let Some(info) = reg.local_fonts().into_iter().next() else {
        return; // no fonts installed (CI container)
    };
    let src = FontSource::Local {
        postscript_name: info.postscript_name.clone(),
        family: info.name.clone(),
    };
    let r = reg
        .resolve("fira-sans", 400, false, Some(&src))
        .expect("font");
    // Some installed files (bitmap-only, broken) cannot be used: then it is reported missing.
    assert!(r.custom || r.issue.is_some(), "{}", info.postscript_name);
}
