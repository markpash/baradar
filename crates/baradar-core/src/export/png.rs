//! 1-bit PNG export of the exact print bitmap (port of the PNG part of
//! `web/src/render/export.ts`, plus the export file-name rule of `ExportDialog.svelte`).
//!
//! Orientation is the canvas one used everywhere in render/: x = along the label (left end as
//! read), y = across the tape (row 0 = top of the printable band). Ink is black.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use ptouch::Bitmap;

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A bitmap with no lines or no rows (`'The label is empty.'`).
    #[error("The label is empty.")]
    Empty,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Row-major 1-bit rows (`ceil(length/8)` bytes per row, MSB first, 1 = WHITE, 0 = ink), the
/// layout of both PNG grey 1-bit and PDF DeviceGray 1-bit (`packRows`). `filter_byte` prefixes
/// each row with PNG filter type 0.
pub fn pack_rows(bitmap: &Bitmap, filter_byte: bool) -> Vec<u8> {
    let w = bitmap.length() as usize;
    let h = usize::from(bitmap.height());
    let stride = bitmap.stride();
    let src = bitmap.as_packed();
    let row_bytes = w.div_ceil(8);
    let lead = usize::from(filter_byte);
    let mut out = vec![255u8; (row_bytes + lead) * h];
    for y in 0..h {
        let row = y * (row_bytes + lead);
        if filter_byte {
            out[row] = 0;
        }
        let byte_y = y >> 3;
        let bit_y = 7 - (y & 7);
        for x in 0..w {
            if (src.get(x * stride + byte_y).copied().unwrap_or(0) >> bit_y) & 1 == 1 {
                out[row + lead + (x >> 3)] &= !(1 << (7 - (x & 7)));
            }
        }
    }
    out
}

/// zlib-wrapped deflate (RFC 1950), the format of PNG IDAT and PDF `/FlateDecode`
/// (`zlibDeflate`, which used `CompressionStream('deflate')`).
///
/// # Errors
/// I/O errors from the encoder (not expected for an in-memory buffer).
pub fn zlib_deflate(data: &[u8]) -> Result<Vec<u8>, ExportError> {
    let mut enc = ZlibEncoder::new(
        Vec::with_capacity(data.len() / 4 + 64),
        Compression::default(),
    );
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

/// CRC-32 (ISO-HDLC, the PNG chunk checksum) (`crc32`).
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

/// PNG signature: `\x89 P N G \r \n \x1a \n`.
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Pixels per metre for a dpi (pHYs): 180 dpi → 7087, 360 dpi → 14173 (`pixelsPerMetre`).
pub fn pixels_per_metre(dpi: u16) -> u32 {
    u32::try_from(crate::render::units::js_round(f64::from(dpi) / 0.0254)).unwrap_or(0)
}

/// The bitmap as a 1-bit greyscale PNG whose pHYs makes it open at real size (`exportPng`):
/// signature, IHDR (bit depth 1, colour type 0), pHYs (unit metre), IDAT, IEND.
///
/// # Errors
/// [`ExportError::Empty`] for a bitmap without dots.
pub fn export_png(bitmap: &Bitmap, dpi: u16) -> Result<Vec<u8>, ExportError> {
    let w = bitmap.length();
    let h = u32::from(bitmap.height());
    if w < 1 || h < 1 {
        return Err(ExportError::Empty);
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    // Bit depth 1, colour type 0 (grey), deflate, filter 0, no interlace.
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]);
    let ppm = pixels_per_metre(dpi);
    let mut phys = Vec::with_capacity(9);
    phys.extend_from_slice(&ppm.to_be_bytes());
    phys.extend_from_slice(&ppm.to_be_bytes());
    phys.push(1); // unit: metre
    let idat = zlib_deflate(&pack_rows(bitmap, true))?;
    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&PNG_SIGNATURE);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"pHYs", &phys);
    png_chunk(&mut out, b"IDAT", &idat);
    png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// Base name of an exported file (`baseName` in `ExportDialog.svelte`): the label's file slug
/// (`labelFileName(doc.name)` without `.ptlabel.json`), or `<slug>-<row + 1>` when a batch is
/// loaded. Add `.png` / `.pdf`.
pub fn export_base_name(slug: &str, batch_count: usize, preview_row: usize) -> String {
    if batch_count > 0 {
        format!("{slug}-{}", preview_row + 1)
    } else {
        slug.to_owned()
    }
}
