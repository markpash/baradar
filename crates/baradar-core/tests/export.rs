//! PNG and PDF writers parsed back byte by byte (port of web/tests/unit/render/export.test.ts).

use std::io::Read as _;

use baradar_core::export::pdf::{PdfExportOptions, export_pdf, pdf_page_mm};
use baradar_core::export::png::{
    ExportError, crc32, export_base_name, export_png, pack_rows, pixels_per_metre,
};
use flate2::read::ZlibDecoder;
use ptouch::Bitmap;

fn fake(length: u32, height: u16, ink: impl Fn(u32, u16) -> bool) -> Bitmap {
    Bitmap::from_fn(length, height, ink)
}

fn diag() -> Bitmap {
    fake(19, 11, |x, y| x == u32::from(y) || (x == 18 && y == 0))
}

fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut out).unwrap();
    out
}

struct Chunk {
    kind: String,
    data: Vec<u8>,
    crc_ok: bool,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn chunks(png: &[u8]) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut at = 8;
    while at < png.len() {
        let len = be32(&png[at..]) as usize;
        let kind = String::from_utf8(png[at + 4..at + 8].to_vec()).unwrap();
        let data = png[at + 8..at + 8 + len].to_vec();
        let crc_ok = crc32(&png[at + 4..at + 8 + len]) == be32(&png[at + 8 + len..]);
        out.push(Chunk { kind, data, crc_ok });
        at += 12 + len;
    }
    out
}

#[test]
fn pack_rows_turns_lines_into_rows_with_ink_as_zero() {
    let bmp = diag();
    let rows = pack_rows(&bmp, false);
    let row_bytes = 3;
    assert_eq!(rows.len(), row_bytes * 11);
    for y in 0..11usize {
        for x in 0..19usize {
            let bit = (rows[y * row_bytes + (x >> 3)] >> (7 - (x & 7))) & 1;
            let inked = x == y || (x == 18 && y == 0);
            assert_eq!(bit, u8::from(!inked), "{x},{y}");
        }
    }
    let filtered = pack_rows(&bmp, true);
    assert_eq!(filtered.len(), (row_bytes + 1) * 11);
    assert_eq!(filtered[0], 0);
    assert_eq!(filtered[row_bytes + 1], 0);
    // Padding bits past the last column stay white.
    assert_eq!(rows[2] & 0b0001_1111, 0b0001_1111);
}

#[test]
fn png_has_signature_ihdr_phys_and_valid_crcs() {
    let png = export_png(&diag(), 180).unwrap();
    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    let cs = chunks(&png);
    let kinds: Vec<&str> = cs.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["IHDR", "pHYs", "IDAT", "IEND"]);
    assert!(cs.iter().all(|c| c.crc_ok));
    let ihdr = &cs[0].data;
    assert_eq!((be32(ihdr), be32(&ihdr[4..])), (19, 11));
    assert_eq!(&ihdr[8..], &[1, 0, 0, 0, 0]);
    assert_eq!(pixels_per_metre(180), 7087);
    assert_eq!(pixels_per_metre(360), 14173);
    let phys = &cs[1].data;
    assert_eq!((be32(phys), be32(&phys[4..]), phys[8]), (7087, 7087, 1));
    assert!(cs[3].data.is_empty());
}

#[test]
fn png_idat_inflates_to_filtered_rows() {
    let bmp = diag();
    let png = export_png(&bmp, 180).unwrap();
    let idat = chunks(&png).into_iter().find(|c| c.kind == "IDAT").unwrap();
    assert_eq!(inflate(&idat.data), pack_rows(&bmp, true));
}

#[test]
fn png_decodes_with_the_image_crate() {
    let bmp = diag();
    let png = export_png(&bmp, 180).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_luma8();
    assert_eq!(img.dimensions(), (19, 11));
    assert_eq!(img.get_pixel(3, 3).0[0], 0);
    assert_eq!(img.get_pixel(4, 3).0[0], 255);
    assert_eq!(img.get_pixel(18, 0).0[0], 0);
}

#[test]
fn crc32_is_standard() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32(b"IEND"), 0xae42_6082);
}

#[test]
fn empty_bitmaps_are_refused() {
    let e = export_png(&fake(0, 70, |_, _| false), 180).unwrap_err();
    assert!(matches!(e, ExportError::Empty));
    assert_eq!(e.to_string(), "The label is empty.");
    let o = PdfExportOptions {
        dpi: 180,
        tape_width_dots: 84,
        margin_dots: 0,
        title: None,
    };
    assert!(matches!(
        export_pdf(&fake(0, 70, |_, _| false), &o),
        Err(ExportError::Empty)
    ));
}

fn pdf_bmp() -> Bitmap {
    fake(200, 70, |x, y| (x + u32::from(y)) % 3 == 0)
}

fn pdf_opts() -> PdfExportOptions {
    PdfExportOptions {
        dpi: 180,
        tape_width_dots: 84,
        margin_dots: 14,
        title: Some("Shelf 3 — “screws”".into()),
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// Byte offset of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).position(|w| w == needle).unwrap()
}

#[test]
fn pdf_xref_offsets_point_at_objects() {
    let pdf = export_pdf(&pdf_bmp(), &pdf_opts()).unwrap();
    assert!(pdf.starts_with(b"%PDF-1.4\n"));
    assert_eq!(&pdf[9..15], &[37, 226, 227, 207, 211, 10]);
    assert!(pdf.ends_with(b"%%EOF\n"));
    let tail = latin1(&pdf[pdf.len() - 40..]);
    let sx = tail.rsplit("startxref\n").next().unwrap();
    let startxref: usize = sx.trim_end_matches("\n%%EOF\n").parse().unwrap();
    assert_eq!(&pdf[startxref..startxref + 4], b"xref");
    let table = latin1(&pdf[startxref..]);
    let rows: Vec<&str> = table.split('\n').skip(2).take(7).collect();
    assert_eq!(rows[0], "0000000000 65535 f ");
    for (n, row) in rows.iter().enumerate().skip(1) {
        assert_eq!(row.len() + 1, 20);
        assert!(row.ends_with(" 00000 n "));
        let off: usize = row[..10].parse().unwrap();
        let head = format!("{n} 0 obj");
        assert_eq!(&pdf[off..off + head.len()], head.as_bytes());
    }
    assert!(table.contains("trailer\n<< /Size 7 /Root 1 0 R /Info 6 0 R >>\nstartxref\n"));
}

#[test]
fn pdf_mediabox_and_placement_are_true_size() {
    let pdf = export_pdf(&pdf_bmp(), &pdf_opts()).unwrap();
    let text = latin1(&pdf);
    // (200 + 28) dots = 91.2 pt; 84 dots = 33.6 pt.
    assert!(text.contains("/MediaBox [0 0 91.2 33.6]"), "{text}");
    let (w_mm, h_mm) = pdf_page_mm(200, 180, 84, 14);
    assert!((w_mm - 228.0 * 25.4 / 180.0).abs() < 1e-9);
    assert!((h_mm - 84.0 * 25.4 / 180.0).abs() < 1e-9);
    // 200 × 70 dots at 14 dots in, centred: (84 − 70)/2 = 7 dots up.
    assert!(text.contains("5 0 obj\n<< /Length 33 >>\nstream\nq\n80 0 0 28 5.6 2.8 cm\n/Im1 Do\nQ\n\nendstream\nendobj\n"));
}

#[test]
fn pdf_image_is_flate_gray_equal_to_packed_rows() {
    let bmp = pdf_bmp();
    let pdf = export_pdf(&bmp, &pdf_opts()).unwrap();
    let head = "4 0 obj\n<< /Type /XObject /Subtype /Image /Width 200 /Height 70 /ColorSpace /DeviceGray /BitsPerComponent 1 /Interpolate false /Filter /FlateDecode /Length ";
    let at = find(&pdf, head.as_bytes()) + head.len();
    let len_end = at + find(&pdf[at..], b" ");
    let len: usize = latin1(&pdf[at..len_end]).parse().unwrap();
    let start = len_end + " >>\nstream\n".len();
    assert_eq!(&pdf[start + len..start + len + 10], b"\nendstream");
    assert_eq!(inflate(&pdf[start..start + len]), pack_rows(&bmp, false));
}

#[test]
fn pdf_title_is_utf16_hex() {
    let text = latin1(&export_pdf(&pdf_bmp(), &pdf_opts()).unwrap());
    assert!(text.contains("/Producer (ptouch studio) /Title <FEFF005300680065006C0066"));
    assert!(text.contains("2014")); // em dash as a UTF-16 unit
    let no_title = PdfExportOptions {
        title: None,
        ..pdf_opts()
    };
    let text = latin1(&export_pdf(&pdf_bmp(), &no_title).unwrap());
    assert!(text.contains("6 0 obj\n<< /Producer (ptouch studio) >>\nendobj\n"));
    let empty = PdfExportOptions {
        title: Some(String::new()),
        ..pdf_opts()
    };
    let text = latin1(&export_pdf(&pdf_bmp(), &empty).unwrap());
    assert!(!text.contains("/Title"));
}

#[test]
fn pdf_page_never_smaller_than_the_band() {
    // A tape narrower than the bitmap: the page is the bitmap height, no offset.
    let o = PdfExportOptions {
        dpi: 360,
        tape_width_dots: 10,
        margin_dots: 0,
        title: None,
    };
    let text = latin1(&export_pdf(&fake(36, 72, |_, _| true), &o).unwrap());
    assert!(text.contains("/MediaBox [0 0 7.2 14.4]"));
    assert!(text.contains("q\n7.2 0 0 14.4 0 0 cm\n"));
}

#[test]
fn export_names() {
    assert_eq!(export_base_name("cable-tags", 0, 4), "cable-tags");
    assert_eq!(export_base_name("cable-tags", 10, 2), "cable-tags-3");
}
