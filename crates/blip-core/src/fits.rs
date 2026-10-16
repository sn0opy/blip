//! Minimal FITS reader: 2D (mono / Bayer) or 3D (RGB planes) images from the primary
//! HDU or, if that is empty, the first IMAGE extension.

use crate::image::{Bayer, Image, lookup, parse_float, parse_int, read_data};
use rayon::prelude::*;
use std::fs::File;
use std::path::Path;

const BLOCK: usize = 2880;
const CARD: usize = 80;
const TRUNCATED_HEADER: &str = "unexpected end of file in header";

/// (keyword, value) cards.
type Header = Vec<(String, String)>;

/// Decode a FITS file of `len` bytes whose first bytes `head` were already read.
pub(crate) fn read(
    path: &Path,
    mut file: File,
    head: Vec<u8>,
    len: usize,
) -> Result<Image, String> {
    // Longer headers and extension images take the slow path.
    let slow = || parse(&std::fs::read(path).map_err(|e| e.to_string())?);
    let (header, data_start) = match parse_header(&head) {
        Ok(h) => h,
        Err(e) if e == TRUNCATED_HEADER && head.len() < len => return slow(),
        Err(e) => return Err(e),
    };
    if header_int(&header, "NAXIS")? == Some(0) {
        return slow();
    }
    let layout = Layout::new(&header)?;
    layout.check_size(data_start, len)?;

    let data = if layout.bitpix == 16 {
        // Read straight into the final buffer and convert in place.
        let mut data = vec![0u16; layout.samples];
        read_data(
            &mut file,
            &head,
            data_start,
            bytemuck::cast_slice_mut(&mut data),
        )?;
        convert_i16(data, &layout)
    } else {
        let mut bytes = vec![0u8; layout.data_len];
        read_data(&mut file, &head, data_start, &mut bytes)?;
        decode(&bytes, &layout)?
    };
    Ok(layout.into_image(data, header))
}

pub fn parse(bytes: &[u8]) -> Result<Image, String> {
    let (mut header, mut data_start) = parse_header(bytes)?;
    let layout = if header_int(&header, "NAXIS")? == Some(0) {
        let (ext, layout, start) = find_image_extension(bytes, data_start)?;
        // Extension keywords take precedence; the primary ones are inherited.
        header = ext.into_iter().chain(header).collect();
        data_start = start;
        layout
    } else {
        Layout::new(&header)?
    };
    layout.check_size(data_start, bytes.len())?;
    let data = decode(&bytes[data_start..data_start + layout.data_len], &layout)?;
    Ok(layout.into_image(data, header))
}

/// Walk the extension HDUs after the primary one for the first IMAGE with data.
fn find_image_extension(bytes: &[u8], mut pos: usize) -> Result<(Header, Layout, usize), String> {
    loop {
        let rest = bytes.get(pos..).unwrap_or_default();
        if !rest.starts_with(b"XTENSION") {
            return Err("no image data in file".into());
        }
        let (ext, len) = parse_cards(rest)?;
        let start = pos + len;
        let kind = lookup(&ext, "XTENSION").unwrap_or_default();
        if kind == "BINTABLE" && lookup(&ext, "ZIMAGE") == Some("T") {
            return Err("compressed FITS (.fz) not supported".into());
        }
        if kind == "IMAGE" && header_int(&ext, "NAXIS")? != Some(0) {
            let layout = Layout::new(&ext)?;
            return Ok((ext, layout, start));
        }
        pos = hdu_data_len(&ext)?
            .checked_next_multiple_of(BLOCK)
            .and_then(|n| n.checked_add(start))
            .ok_or("invalid extension size")?;
    }
}

/// Size in bytes of the data unit described by a (non-image) extension header.
fn hdu_data_len(header: &[(String, String)]) -> Result<usize, String> {
    let req = |k: &str| header_int(header, k)?.ok_or_else(|| format!("missing {k}"));
    let size = |k: &str| {
        let n = req(k)?;
        usize::try_from(n).map_err(|_| format!("invalid {k} = {n}"))
    };
    let naxis = size("NAXIS")?;
    if naxis == 0 {
        return Ok(0);
    }
    let mut n = 1usize;
    for i in 1..=naxis {
        n = n
            .checked_mul(size(&format!("NAXIS{i}"))?)
            .ok_or("invalid extension size")?;
    }
    let bpp = (req("BITPIX")?.unsigned_abs() / 8) as usize;
    let pcount = header_int(header, "PCOUNT")?.unwrap_or(0);
    let gcount = header_int(header, "GCOUNT")?.unwrap_or(1);
    usize::try_from(pcount)
        .ok()
        .and_then(|p| n.checked_add(p))
        .zip(usize::try_from(gcount).ok())
        .and_then(|(n, g)| n.checked_mul(g))
        .and_then(|n| n.checked_mul(bpp))
        .ok_or_else(|| "invalid extension size".into())
}

fn parse_header(bytes: &[u8]) -> Result<(Header, usize), String> {
    if !bytes.starts_with(b"SIMPLE  =") {
        return Err("not a FITS file".into());
    }
    parse_cards(bytes)
}

/// Parse header cards up to END. Returns the cards and the block-aligned header length.
fn parse_cards(bytes: &[u8]) -> Result<(Header, usize), String> {
    let mut header = Vec::new();
    for (i, card) in bytes.as_chunks::<CARD>().0.iter().enumerate() {
        let key = std::str::from_utf8(&card[..8])
            .ok()
            .filter(|k| k.is_ascii())
            .ok_or("invalid header keyword")?
            .trim_end();
        if key == "END" {
            return Ok((header, ((i + 1) * CARD).div_ceil(BLOCK) * BLOCK));
        }
        if &card[8..10] != b"= " {
            continue;
        }
        header.push((key.to_string(), parse_value(&card[10..])));
    }
    Err(TRUNCATED_HEADER.into())
}

/// Value field of a card with quotes and comment removed. Works on bytes so stray
/// non-ASCII (often Latin-1) characters in comments don't break parsing.
pub(crate) fn parse_value(raw: &[u8]) -> String {
    let raw = raw.trim_ascii();
    if let Some(rest) = raw.strip_prefix(b"'") {
        // Quoted string; '' is an escaped quote.
        let mut out = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            if rest[i] != b'\'' {
                out.push(rest[i]);
                i += 1;
            } else if rest.get(i + 1) == Some(&b'\'') {
                out.push(b'\'');
                i += 2;
            } else {
                break;
            }
        }
        text(out.trim_ascii_end())
    } else {
        text(
            raw.split(|&b| b == b'/')
                .next()
                .unwrap_or_default()
                .trim_ascii(),
        )
    }
}

/// UTF-8 if valid, otherwise Latin-1.
fn text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// `Ok(None)` if the key is absent, `Err` if present but not an integer.
fn header_int(header: &[(String, String)], key: &str) -> Result<Option<i64>, String> {
    lookup(header, key)
        .map(|v| parse_int(v).ok_or_else(|| format!("invalid {key} = {v:?}")))
        .transpose()
}

fn header_float(header: &[(String, String)], key: &str) -> Result<Option<f64>, String> {
    lookup(header, key)
        .map(|v| parse_float(v).ok_or_else(|| format!("invalid {key} = {v:?}")))
        .transpose()
}

struct Layout {
    width: usize,
    height: usize,
    planes: usize,
    bitpix: i64,
    bzero: f64,
    bscale: f64,
    /// Raw integer value marking undefined pixels.
    blank: Option<i64>,
    bayer: Option<Bayer>,
    samples: usize,
    /// Data unit size in bytes (without padding).
    data_len: usize,
}

impl Layout {
    fn new(header: &[(String, String)]) -> Result<Self, String> {
        let int = |k: &str| header_int(header, k);
        let req = |k: &str| int(k)?.ok_or_else(|| format!("missing {k}"));
        let size = |k: &str| {
            let n = req(k)?;
            usize::try_from(n).map_err(|_| format!("invalid {k} = {n}"))
        };

        let naxis = req("NAXIS")?;
        if !(2..=3).contains(&naxis) {
            return Err(format!("unsupported NAXIS = {naxis}"));
        }
        let (width, height) = (size("NAXIS1")?, size("NAXIS2")?);
        if width == 0 || height == 0 {
            return Err(format!("invalid image size {width}x{height}"));
        }
        let planes = match naxis {
            3 => match size("NAXIS3")? {
                n @ (1 | 3) => n,
                n => return Err(format!("unsupported NAXIS3 = {n} (expected 1 or 3 planes)")),
            },
            _ => 1,
        };
        let bitpix = req("BITPIX")?;
        if ![8, 16, 32, 64, -32, -64].contains(&bitpix) {
            return Err(format!("unsupported BITPIX = {bitpix}"));
        }
        let samples = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(planes))
            .ok_or("image dimensions too large")?;
        let data_len = samples
            .checked_mul((bitpix.unsigned_abs() / 8) as usize)
            .ok_or("image dimensions too large")?;

        let bayer = (planes == 1).then(|| Bayer::from_header(header)).flatten();
        Ok(Self {
            width,
            height,
            planes,
            bitpix,
            bzero: header_float(header, "BZERO")?.unwrap_or(0.0),
            bscale: header_float(header, "BSCALE")?.unwrap_or(1.0),
            blank: if bitpix > 0 { int("BLANK")? } else { None },
            bayer,
            samples,
            data_len,
        })
    }

    /// Ensure the data unit starting at `data_start` fits in a file of `file_len` bytes.
    fn check_size(&self, data_start: usize, file_len: usize) -> Result<(), String> {
        match data_start.checked_add(self.data_len) {
            Some(end) if end <= file_len => Ok(()),
            _ => Err(format!(
                "file truncated: image needs {} data bytes, {} present",
                self.data_len,
                file_len.saturating_sub(data_start)
            )),
        }
    }

    fn into_image(self, data: Vec<u16>, header: Vec<(String, String)>) -> Image {
        Image {
            width: self.width,
            height: self.height,
            planes: self.planes,
            data,
            bayer: self.bayer,
            header,
        }
    }
}

/// Convert raw big-endian samples of any BITPIX to u16. `raw` holds exactly
/// `l.data_len` bytes.
fn decode(raw: &[u8], l: &Layout) -> Result<Vec<u16>, String> {
    let raw = raw.get(..l.data_len).ok_or("file truncated")?;
    let blank = l.blank;
    let int = move |r: i64| {
        if Some(r) == blank { f64::NAN } else { r as f64 }
    };
    let data = match l.bitpix {
        8 if l.bzero == 0.0 && l.bscale == 1.0 && blank.is_none() => {
            raw.par_iter().map(|&b| (b as u16) << 8).collect()
        }
        8 => normalize::<1>(raw, l, |c| int(c[0] as i64)),
        16 => convert_i16(
            raw.as_chunks::<2>()
                .0
                .par_iter()
                .map(|&c| u16::from_ne_bytes(c))
                .collect(),
            l,
        ),
        32 => normalize::<4>(raw, l, |c| int(i32::from_be_bytes(c) as i64)),
        64 => normalize::<8>(raw, l, |c| int(i64::from_be_bytes(c))),
        -32 => normalize::<4>(raw, l, |c| f32::from_be_bytes(c) as f64),
        -64 => normalize::<8>(raw, l, f64::from_be_bytes),
        _ => return Err(format!("unsupported BITPIX = {}", l.bitpix)),
    };
    Ok(data)
}

/// `data` holds big-endian i16 samples exactly as stored in the file.
fn convert_i16(mut data: Vec<u16>, l: &Layout) -> Vec<u16> {
    if l.bscale == 1.0 && l.blank.is_none() {
        if l.bzero == 32768.0 {
            // Unsigned 16-bit encoding: adding 32768 is a sign-bit flip.
            data.par_iter_mut()
                .for_each(|v| *v = u16::from_be(*v) ^ 0x8000);
            return data;
        }
        if l.bzero == 0.0 {
            // Signed data that is non-negative in practice converts in place;
            // otherwise fall back to min/max scaling of the (now native) samples.
            let sign = data
                .par_iter_mut()
                .map(|v| {
                    *v = u16::from_be(*v);
                    *v & 0x8000
                })
                .reduce(|| 0, |a, b| a | b);
            if sign == 0 {
                return data;
            }
            return normalize::<2>(bytemuck::cast_slice(&data), l, |c| {
                i16::from_ne_bytes(c) as f64
            });
        }
    }
    let blank = l.blank;
    normalize::<2>(bytemuck::cast_slice(&data), l, |c| {
        let r = i16::from_be_bytes(c) as i64;
        if Some(r) == blank { f64::NAN } else { r as f64 }
    })
}

/// Map samples (decoded by `read`, NaN = undefined) through BSCALE/BZERO onto
/// 0..=65535 without materializing them as f64. Data already in [0, 1] (typical for
/// stacked float images), [0, 255] for 8-bit or [0, 65535] keeps its scale; anything
/// else is min/max scaled.
fn normalize<const N: usize>(
    raw: &[u8],
    l: &Layout,
    read: impl Fn([u8; N]) -> f64 + Sync,
) -> Vec<u16> {
    let (bzero, bscale) = (l.bzero, l.bscale);
    let value = |c: &[u8; N]| read(*c) * bscale + bzero;
    let chunks = raw.as_chunks::<N>().0;
    let (lo, hi) = chunks
        .par_iter()
        .fold(
            || (f64::MAX, f64::MIN),
            |(lo, hi), c| {
                let v = value(c);
                if v.is_finite() {
                    (lo.min(v), hi.max(v))
                } else {
                    (lo, hi)
                }
            },
        )
        .reduce(|| (f64::MAX, f64::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
    let (lo, hi) = if lo >= 0.0 && hi <= 1.0 {
        (0.0, 1.0)
    } else if l.bitpix == 8 && lo >= 0.0 && hi <= 255.0 {
        // Same scale as the unscaled 8-bit fast path (v << 8).
        (0.0, 65535.0 / 256.0)
    } else if lo >= 0.0 && hi <= 65535.0 {
        (0.0, 65535.0)
    } else {
        (lo, hi.max(lo + f64::EPSILON))
    };
    let scale = 65535.0 / (hi - lo);
    chunks
        .par_iter()
        .map(|c| {
            let v = value(c);
            if v.is_finite() {
                ((v - lo) * scale).clamp(0.0, 65535.0) as u16
            } else {
                0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header cards (padded to 80 chars) + END, block padded, then block-padded data.
    fn fits_bytes(cards: &[&str], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in cards.iter().chain(&["END"]) {
            out.extend(format!("{c:<80}").bytes());
        }
        out.resize(out.len().div_ceil(BLOCK) * BLOCK, b' ');
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
        out
    }

    fn parse_err(bytes: &[u8]) -> String {
        parse(bytes).err().expect("parse should fail")
    }

    /// Simple 2D image with extra cards.
    fn image(bitpix: i64, w: usize, h: usize, extra: &[&str], data: &[u8]) -> Vec<u8> {
        let base = [
            "SIMPLE  =                    T".to_string(),
            format!("BITPIX  = {bitpix:>20}"),
            "NAXIS   =                    2".to_string(),
            format!("NAXIS1  = {w:>20}"),
            format!("NAXIS2  = {h:>20}"),
        ];
        let cards: Vec<&str> = base
            .iter()
            .map(String::as_str)
            .chain(extra.iter().copied())
            .collect();
        fits_bytes(&cards, data)
    }

    fn be<T: Copy, const N: usize>(vals: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
        vals.iter().flat_map(|&v| f(v)).collect()
    }

    #[test]
    fn header_values_and_padding() {
        let bytes = fits_bytes(
            &[
                "SIMPLE  =                    T / conforms",
                "OBJECT  = 'M 15 ''core'' '    / it's a / comment",
                "PATH    = 'a/b'",
                "EXPTIME =                120.0 / [s]",
                "COMMENT this card = has no value indicator",
            ],
            &[],
        );
        let (h, start) = parse_header(&bytes).unwrap();
        assert_eq!(start, BLOCK);
        assert_eq!(lookup(&h, "OBJECT"), Some("M 15 'core'"));
        assert_eq!(lookup(&h, "PATH"), Some("a/b"));
        assert_eq!(lookup(&h, "EXPTIME"), Some("120.0"));
        assert_eq!(lookup(&h, "COMMENT"), None);
        assert_eq!(lookup(&h, "SIMPLE"), Some("T"));
    }

    #[test]
    fn header_end_and_multi_block() {
        let many: Vec<String> = (0..40).map(|i| format!("KEY{i:<5}= {i}")).collect();
        let mut cards = vec!["SIMPLE  =                    T"];
        cards.extend(many.iter().map(String::as_str));
        let mut bytes = fits_bytes(&cards, &[]);
        let (h, start) = parse_header(&bytes).unwrap();
        assert_eq!(start, 2 * BLOCK);
        assert_eq!(h.len(), 41);
        assert_eq!(lookup(&h, "KEY39"), Some("39"));
        // Cards after END are ignored.
        let end = 41 * CARD;
        bytes[end + CARD..end + 2 * CARD].copy_from_slice(&[b'X'; CARD]);
        assert_eq!(parse_header(&bytes).unwrap().0.len(), 41);
        // Missing END.
        assert_eq!(parse_header(&bytes[..end]).unwrap_err(), TRUNCATED_HEADER);
        assert!(parse_header(b"NOT FITS").is_err());
    }

    #[test]
    fn header_latin1_bytes() {
        let mut bytes = fits_bytes(
            &[
                "SIMPLE  =                    T",
                "CCD-TEMP=                 -9.8 / temperature in xC",
                "SITE    = 'Mxnchen'",
            ],
            &[],
        );
        // Latin-1 '°' and 'ü' (not valid UTF-8 on their own).
        for pos in [CARD + 48, 2 * CARD + 12] {
            assert_eq!(bytes[pos], b'x');
            bytes[pos] = if pos < 2 * CARD { 0xB0 } else { 0xFC };
        }
        let (h, _) = parse_header(&bytes).unwrap();
        assert_eq!(lookup(&h, "CCD-TEMP"), Some("-9.8"));
        assert_eq!(lookup(&h, "SITE"), Some("München"));
        // Non-ASCII keyword is rejected.
        bytes[CARD] = 0xB0;
        assert!(parse_header(&bytes).is_err());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_int("16."), Some(16));
        assert_eq!(parse_int("1.6D1"), Some(16));
        assert_eq!(parse_int("-32"), Some(-32));
        assert_eq!(parse_int("1.5"), None);
        assert_eq!(parse_float("3.2768D4"), Some(32768.0));
        assert_eq!(parse_float("inf"), None);
        let header = |bitpix: &str| {
            fits_bytes(
                &[
                    "SIMPLE  =                    T",
                    "NAXIS   = 2.",
                    "NAXIS1  = 2.0",
                    "NAXIS2  = 1",
                    &format!("BITPIX  = {bitpix}"),
                ],
                &[0; 4],
            )
        };
        assert_eq!(parse(&header("1.6D1")).unwrap().width, 2);
        let err = parse_err(&header("sixteen"));
        assert!(err.contains("invalid BITPIX"), "{err}");
    }

    #[test]
    fn layout_validation() {
        let err = |naxis1: &str| {
            let b = image(16, 0, 1, &[], &[0; 64]);
            let s = std::str::from_utf8(&b[..BLOCK]).unwrap().replacen(
                &format!("NAXIS1  = {:>20}", 0),
                &format!("NAXIS1  = {naxis1:>20}"),
                1,
            );
            let mut b = b;
            b[..BLOCK].copy_from_slice(s.as_bytes());
            parse(&b).err().unwrap_or_default()
        };
        assert!(err("-5").contains("invalid NAXIS1"), "{}", err("-5"));
        assert!(err("0").contains("invalid image size"));
        assert!(err("abc").contains("invalid NAXIS1"));
        assert!(err("1000000000000000000").contains("truncated"));
        let huge = fits_bytes(
            &[
                "SIMPLE  =                    T",
                "BITPIX  = -64",
                "NAXIS   = 2",
                "NAXIS1  = 4000000000000",
                "NAXIS2  = 4000000000000",
            ],
            &[],
        );
        assert!(parse_err(&huge).contains("too large"));
        assert!(err("9000000000000").contains("truncated"));
        assert!(err("1").is_empty());

        let missing = fits_bytes(&["SIMPLE  =                    T", "NAXIS   = 2"], &[]);
        assert!(parse_err(&missing).contains("missing NAXIS1"));
        let cube = |n: usize| {
            fits_bytes(
                &[
                    "SIMPLE  =                    T",
                    "BITPIX  =                    8",
                    "NAXIS   =                    3",
                    "NAXIS1  =                    1",
                    "NAXIS2  =                    1",
                    &format!("NAXIS3  = {n}"),
                ],
                &[7, 8, 9, 10],
            )
        };
        assert_eq!(parse(&cube(1)).unwrap().planes, 1);
        assert_eq!(parse(&cube(3)).unwrap().planes, 3);
        assert!(parse_err(&cube(4)).contains("NAXIS3"));
        assert!(parse_err(&image(12, 1, 1, &[], &[0; 2])).contains("BITPIX"));

        // Data cut short (no block padding at all).
        let b = image(16, 4, 4, &[], &[0; 32]);
        assert!(parse_err(&b[..BLOCK + 31]).contains("truncated"));
    }

    #[test]
    fn decode_all_bitpix() {
        let b = image(8, 3, 1, &[], &[0, 1, 255]);
        assert_eq!(parse(&b).unwrap().data, [0, 256, 65280]);
        let b = image(8, 3, 1, &["BZERO   = -128"], &[0, 128, 255]);
        assert_eq!(parse(&b).unwrap().data, [0, 32896, 65535]);

        let raw = be(&[0i16, 1000, 32767], i16::to_be_bytes);
        assert_eq!(
            parse(&image(16, 3, 1, &[], &raw)).unwrap().data,
            [0, 1000, 32767]
        );
        // Negative signed data is min/max scaled rather than clamped.
        let raw = be(&[-100i16, 0, 100], i16::to_be_bytes);
        assert_eq!(
            parse(&image(16, 3, 1, &[], &raw)).unwrap().data,
            [0, 32767, 65535]
        );
        let raw = be(&[10i16, 20], i16::to_be_bytes);
        let b = image(16, 2, 1, &["BSCALE  = 2.0"], &raw);
        assert_eq!(parse(&b).unwrap().data, [20, 40]);

        let raw = be(&[-5i32, 0, 5], i32::to_be_bytes);
        assert_eq!(
            parse(&image(32, 3, 1, &[], &raw)).unwrap().data,
            [0, 32767, 65535]
        );
        let raw = be(&[0i64, 1234, 60000], i64::to_be_bytes);
        assert_eq!(
            parse(&image(64, 3, 1, &[], &raw)).unwrap().data,
            [0, 1234, 60000]
        );
        let raw = be(&[0.0f32, 0.5, 1.0, f32::NAN], f32::to_be_bytes);
        assert_eq!(
            parse(&image(-32, 4, 1, &[], &raw)).unwrap().data,
            [0, 32767, 65535, 0]
        );
        let raw = be(&[-1.0f64, 131071.0, f64::INFINITY], f64::to_be_bytes);
        assert_eq!(
            parse(&image(-64, 3, 1, &[], &raw)).unwrap().data,
            [0, 65535, 0]
        );
    }

    #[test]
    fn unsigned_16_and_blank() {
        let raw = be(&[0u16, 32768, 65535], |v| {
            ((v ^ 0x8000) as i16).to_be_bytes()
        });
        let b = image(16, 3, 1, &["BZERO   = 32768", "BSCALE  = 1"], &raw);
        assert_eq!(parse(&b).unwrap().data, [0, 32768, 65535]);

        // BLANK is in raw units, ignored for scaling and mapped to 0.
        let raw = be(&[-32768i16, -32000, -31000], i16::to_be_bytes);
        let b = image(16, 3, 1, &["BZERO   = 32768", "BLANK   = -32768"], &raw);
        assert_eq!(parse(&b).unwrap().data, [0, 768, 1768]);
        let raw = be(&[i32::MIN, -10, 10], i32::to_be_bytes);
        let b = image(32, 3, 1, &[&format!("BLANK   = {}", i32::MIN)], &raw);
        assert_eq!(parse(&b).unwrap().data, [0, 0, 65535]);
    }

    #[test]
    fn extension_hdu() {
        let primary = fits_bytes(
            &[
                "SIMPLE  =                    T",
                "BITPIX  =                    8",
                "NAXIS   =                    0",
                "EXTEND  =                    T",
                "EXPTIME =                 30.0",
            ],
            &[],
        );
        let table = fits_bytes(
            &[
                "XTENSION= 'BINTABLE'",
                "BITPIX  =                    8",
                "NAXIS   =                    2",
                "NAXIS1  =                    4",
                "NAXIS2  =                 1000",
                "PCOUNT  =                    0",
                "GCOUNT  =                    1",
            ],
            &[1; 4000],
        );
        let raw = be(&[1i16, 2], i16::to_be_bytes);
        let ext = fits_bytes(
            &[
                "XTENSION= 'IMAGE   '",
                "BITPIX  =                   16",
                "NAXIS   =                    2",
                "NAXIS1  =                    2",
                "NAXIS2  =                    1",
                "BAYERPAT= 'GBRG'",
                "PCOUNT  =                    0",
                "GCOUNT  =                    1",
            ],
            &raw,
        );
        let file = [primary.clone(), table, ext].concat();
        let img = parse(&file).unwrap();
        assert_eq!(
            (img.width, img.height, img.data.clone()),
            (2, 1, vec![1, 2])
        );
        assert_eq!(img.bayer, Some(Bayer::Gbrg));
        assert_eq!(img.get_f64("EXPTIME"), Some(30.0));
        assert_eq!(img.get("XTENSION"), Some("IMAGE"));

        let fz = fits_bytes(
            &[
                "XTENSION= 'BINTABLE'",
                "BITPIX  =                    8",
                "NAXIS   =                    2",
                "NAXIS1  =                    8",
                "NAXIS2  =                    1",
                "ZIMAGE  =                    T",
            ],
            &[0; 8],
        );
        let err = parse_err(&[primary.clone(), fz].concat());
        assert!(err.contains("compressed"), "{err}");
        assert!(parse_err(&primary).contains("no image"));
    }

    #[test]
    fn read_file_paths() {
        let dir = std::env::temp_dir();
        let check = |name: &str, bytes: &[u8]| {
            let path = dir.join(format!("blip-test-{}-{name}.fit", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            let r = crate::image::read_file(&path);
            std::fs::remove_file(&path).unwrap();
            let mem = parse(bytes);
            assert_eq!(r.as_ref().err(), mem.as_ref().err());
            r.map(|i| i.data)
        };
        let raw = be(&[1u16, 65535], |v| ((v ^ 0x8000) as i16).to_be_bytes());
        let b = image(16, 2, 1, &["BZERO   = 32768"], &raw);
        assert_eq!(check("u16", &b).unwrap(), [1, 65535]);
        let raw = be(&[0.25f32, 0.75], f32::to_be_bytes);
        assert_eq!(
            check("f32", &image(-32, 2, 1, &[], &raw)).unwrap(),
            [16383, 49151]
        );
        assert!(
            check("trunc", &b[..BLOCK + 3])
                .unwrap_err()
                .contains("truncated")
        );

        // Header longer than the initial 64-block read.
        let comments = vec!["COMMENT padding"; 64 * BLOCK / CARD];
        let raw = be(&[7i16, 8], i16::to_be_bytes);
        assert_eq!(
            check("long", &image(16, 2, 1, &comments, &raw)).unwrap(),
            [7, 8]
        );
    }
}
