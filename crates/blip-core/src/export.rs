//! Export the stretched image at full resolution as PNG or JPEG.

use crate::process;
use image::ImageEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

const JPEG_QUALITY: u8 = 92;

/// Re-read `src`, render it at full resolution with `params` and write it to `dst`.
/// The format follows the extension (`.jpg`/`.jpeg`, otherwise PNG).
pub fn export(src: &Path, dst: &Path, params: process::StretchParams) -> Result<PathBuf, String> {
    let img = crate::image::read_file(src)?;
    let r = process::render_full::<[u8; 3]>(&img, params);
    write_image(dst, r.pixels.as_flattened(), r.width, r.height)
}

/// Encode packed RGB8 pixels to `dst` as JPEG (`.jpg`/`.jpeg`) or PNG (any other
/// extension is replaced by `.png`). Returns the path actually written.
pub fn write_image(dst: &Path, rgb: &[u8], width: usize, height: usize) -> Result<PathBuf, String> {
    let ext = dst
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let (dst, jpeg) = match ext.as_deref() {
        Some("jpg" | "jpeg") => (dst.to_path_buf(), true),
        Some("png") => (dst.to_path_buf(), false),
        _ => (dst.with_extension("png"), false),
    };
    // Encode before creating the file so a failure leaves nothing behind.
    let data = encode(rgb, width, height, jpeg)?;
    std::fs::write(&dst, data).map_err(|e| e.to_string())?;
    Ok(dst)
}

/// Encode packed RGB8 pixels as JPEG or PNG.
pub fn encode(rgb: &[u8], width: usize, height: usize, jpeg: bool) -> Result<Vec<u8>, String> {
    if jpeg {
        return encode_jpeg(rgb, width, height);
    }
    let mut out = Vec::new();
    // Paeth alone is noticeably faster than adaptive filtering at a similar size.
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Paeth)
        .write_image(
            rgb,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Baseline 4:4:4 JPEG. The encoder is single-threaded, so horizontal strips are
/// encoded in parallel and joined with restart markers: the DC prediction resets at
/// each marker, which makes the strips' entropy-coded data independent.
pub fn encode_jpeg(rgb: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err("image too large for JPEG (max 65535 px per side)".into());
    };
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let row_bytes = 3 * width;
    if rgb.len() != row_bytes * height {
        return Err("pixel buffer does not match the image size".into());
    }

    // The restart interval counts 8x8 MCUs and is a u16.
    let mcu_cols = width.div_ceil(8);
    let strip_mcu_rows = height
        .div_ceil(8 * rayon::current_num_threads())
        .clamp(1, 65535 / mcu_cols);
    let strip_h = 8 * strip_mcu_rows;
    let strips = rgb
        .par_chunks(strip_h * row_bytes)
        .map(|rows| {
            let mut out = Vec::with_capacity(rows.len() / 4);
            let mut enc = Encoder::new(&mut out, JPEG_QUALITY);
            enc.set_sampling_factor(SamplingFactor::F_1_1);
            enc.encode(rows, w, (rows.len() / row_bytes) as u16, ColorType::Rgb)
                .map_err(|e| e.to_string())?;
            Ok(out)
        })
        .collect::<Result<Vec<Vec<u8>>, String>>()?;
    if strips.len() == 1 {
        return Ok(strips.into_iter().next().unwrap_or_default());
    }

    // Headers of the first strip with the full height and a restart interval, then
    // every strip's scan data followed by RST0..7 (cycling), then EOI.
    let (sof, sos, _) = jpeg_layout(&strips[0])?;
    let mut out = Vec::with_capacity(strips.iter().map(Vec::len).sum::<usize>() + 64);
    out.extend_from_slice(&strips[0][..sos]);
    out[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
    let interval = (mcu_cols * strip_mcu_rows) as u16;
    out.extend_from_slice(&[0xFF, 0xDD, 0, 4]);
    out.extend_from_slice(&interval.to_be_bytes());
    for (i, strip) in strips.iter().enumerate() {
        let (_, sos, data) = jpeg_layout(strip)?;
        if i == 0 {
            out.extend_from_slice(&strip[sos..data]);
        }
        out.extend_from_slice(&strip[data..strip.len() - 2]);
        if i + 1 < strips.len() {
            out.extend_from_slice(&[0xFF, 0xD0 + (i % 8) as u8]);
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// Offsets of the SOF0 and SOS markers and of the scan data in a single-scan JPEG
/// that ends with EOI.
fn jpeg_layout(jpeg: &[u8]) -> Result<(usize, usize, usize), String> {
    let bad = || "unexpected JPEG layout".to_string();
    if !jpeg.ends_with(&[0xFF, 0xD9]) {
        return Err(bad());
    }
    let (mut pos, mut sof) = (2, None);
    loop {
        let seg = jpeg.get(pos..pos + 4).ok_or_else(bad)?;
        if seg[0] != 0xFF {
            return Err(bad());
        }
        let len = u16::from_be_bytes([seg[2], seg[3]]) as usize;
        match seg[1] {
            0xC0 => sof = Some(pos),
            0xDA => return Ok((sof.ok_or_else(bad)?, pos, pos + 2 + len)),
            _ => {}
        }
        pos += 2 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(jpeg: &[u8]) -> Vec<u8> {
        image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8()
            .into_raw()
    }

    #[test]
    fn striped_jpeg_matches_single_strip() {
        // Tall enough for several strips, with a partial last strip and MCU column.
        let (w, h) = (37, 8 * rayon::current_num_threads() * 3 + 5);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7) ^ (i / 97)) as u8).collect();
        let striped = encode_jpeg(&rgb, w, h).unwrap();
        let mut single = Vec::new();
        let mut enc = Encoder::new(&mut single, JPEG_QUALITY);
        enc.set_sampling_factor(SamplingFactor::F_1_1);
        enc.encode(&rgb, w as u16, h as u16, ColorType::Rgb)
            .unwrap();
        assert!(striped.windows(2).any(|m| m == [0xFF, 0xD0]));
        assert_eq!(decode(&striped), decode(&single));
    }

    #[test]
    fn tiny_and_empty_jpeg() {
        assert_eq!(decode(&encode_jpeg(&[10, 20, 30], 1, 1).unwrap()).len(), 3);
        assert!(encode_jpeg(&[], 0, 0).is_err());
    }
}
