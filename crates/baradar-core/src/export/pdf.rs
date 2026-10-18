//! True-size PDF export of the exact print bitmap (port of the PDF part of
//! `web/src/render/export.ts`): a hand-written one-page PDF 1.4 with catalog, pages, page, a
//! 1-bit DeviceGray image XObject, the content stream, an info dictionary and a byte-exact xref.

use ptouch::Bitmap;

use super::png::{ExportError, pack_rows, zlib_deflate};
use crate::render::codes::js_to_fixed;
use crate::render::units::js_round;

/// PDF user-space units per inch.
const PT_PER_INCH: f64 = 72.0;

/// Options of [`export_pdf`] (`PdfExportOptions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfExportOptions {
    pub dpi: u16,
    /// Physical tape width in dots (`PrintArea.tapeWidthDots`): the page height. The printable
    /// band (the bitmap) is centred on it, like on the tape.
    pub tape_width_dots: u32,
    /// Blank tape before and after the printed dots (the feed margin), in dots.
    pub margin_dots: u32,
    /// Document title (Info dictionary); `None` or empty for none.
    pub title: Option<String>,
}

/// A PDF number: up to 3 decimals, no exponent, no trailing zeros (`pdfNum`).
pub fn pdf_num(n: f64) -> String {
    let s = js_to_fixed(js_round(n * 1000.0) as f64 / 1000.0, 3);
    let t = s.trim_end_matches('0');
    let t = t.strip_suffix('.').unwrap_or(t);
    if t.is_empty() { "0".into() } else { t.into() }
}

/// A PDF text string as UTF-16BE hex with BOM (`pdfText`).
pub fn pdf_text(s: &str) -> String {
    let mut hex = String::from("<FEFF");
    for unit in s.encode_utf16() {
        hex.push_str(&format!("{unit:04X}"));
    }
    hex.push('>');
    hex
}

/// Page size of a PDF export in mm: `(width along the tape, tape width)` (`pdfPageMm`).
pub fn pdf_page_mm(
    length_dots: u32,
    dpi: u16,
    tape_width_dots: u32,
    margin_dots: u32,
) -> (f64, f64) {
    let dpi = f64::from(dpi);
    (
        (f64::from(length_dots) + 2.0 * f64::from(margin_dots)) * 25.4 / dpi,
        f64::from(tape_width_dots) * 25.4 / dpi,
    )
}

/// A one-page PDF 1.4 at true physical size (`exportPdf`): the page is the label (length + feed
/// margins) by the tape width; the bitmap is a 1-bit DeviceGray image placed at the band offset.
///
/// # Errors
/// [`ExportError::Empty`] for a bitmap without dots.
pub fn export_pdf(bitmap: &Bitmap, opts: &PdfExportOptions) -> Result<Vec<u8>, ExportError> {
    let w = bitmap.length();
    let h = u32::from(bitmap.height());
    if w < 1 || h < 1 {
        return Err(ExportError::Empty);
    }
    let dpi = f64::from(opts.dpi);
    let pt = |dots: f64| dots * PT_PER_INCH / dpi;
    let margin = f64::from(opts.margin_dots);
    let tape = h.max(opts.tape_width_dots);
    let page_w = pt(f64::from(w) + 2.0 * margin);
    let page_h = pt(f64::from(tape));
    // PDF y grows upwards: the band's bottom edge sits (tape − band)/2 above the page bottom.
    let x = pt(margin);
    let y = pt(f64::from(tape - h) / 2.0);
    let image = zlib_deflate(&pack_rows(bitmap, false))?;
    let content = format!(
        "q\n{} 0 0 {} {} {} cm\n/Im1 Do\nQ\n",
        pdf_num(pt(f64::from(w))),
        pdf_num(pt(f64::from(h))),
        pdf_num(x),
        pdf_num(y)
    );

    let mut out: Vec<u8> = Vec::with_capacity(image.len() + 1024);
    let mut offsets = [0usize; 7];
    let mut obj = |out: &mut Vec<u8>, n: usize, dict: &str, stream: Option<&[u8]>| {
        offsets[n] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n{dict}\n").as_bytes());
        if let Some(s) = stream {
            out.extend_from_slice(b"stream\n");
            out.extend_from_slice(s);
            out.extend_from_slice(b"\nendstream\n");
        }
        out.extend_from_slice(b"endobj\n");
    };

    // Header + a comment with high bytes so tools treat the file as binary.
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(&[37, 226, 227, 207, 211, 10]);
    obj(&mut out, 1, "<< /Type /Catalog /Pages 2 0 R >>", None);
    obj(
        &mut out,
        2,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        None,
    );
    obj(
        &mut out,
        3,
        &format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << /Im1 4 0 R >> >> /Contents 5 0 R >>",
            pdf_num(page_w),
            pdf_num(page_h)
        ),
        None,
    );
    obj(
        &mut out,
        4,
        &format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Interpolate false /Filter /FlateDecode /Length {} >>",
            image.len()
        ),
        Some(&image),
    );
    obj(
        &mut out,
        5,
        &format!("<< /Length {} >>", content.len()),
        Some(content.as_bytes()),
    );
    let title = match opts.title.as_deref() {
        Some(t) if !t.is_empty() => format!(" /Title {}", pdf_text(t)),
        _ => String::new(),
    };
    obj(
        &mut out,
        6,
        &format!("<< /Producer (ptouch studio){title} >>"),
        None,
    );
    let xref = out.len();
    let mut table = String::from("xref\n0 7\n0000000000 65535 f \n");
    for off in &offsets[1..] {
        table.push_str(&format!("{off:010} 00000 n \n"));
    }
    out.extend_from_slice(
        format!(
            "{table}trailer\n<< /Size 7 /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(pdf_num(0.0), "0");
        assert_eq!(pdf_num(100.0), "100");
        assert_eq!(pdf_num(10.5), "10.5");
        assert_eq!(pdf_num(80.0004), "80");
        assert_eq!(pdf_num(2.0 / 3.0), "0.667");
        assert_eq!(pdf_num(0.0004), "0");
        assert_eq!(pdf_num(5.6), "5.6");
    }

    #[test]
    fn text() {
        assert_eq!(pdf_text("Ab"), "<FEFF00410062>");
        assert_eq!(pdf_text("😀"), "<FEFFD83DDE00>");
        assert_eq!(pdf_text(""), "<FEFF>");
    }
}
