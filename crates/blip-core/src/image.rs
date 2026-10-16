//! The decoded image shared by the FITS and XISF readers, and the reader dispatch.

use crate::{fits, xisf};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes read up front. FITS headers are usually 1-3 blocks of 2880 bytes; one generous
/// read keeps them (and often the start of the data) to a single round trip on network
/// drives. Longer headers take a second read.
const HEAD: usize = 64 * 2880;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bayer {
    Rggb,
    Bggr,
    Grbg,
    Gbrg,
}

impl fmt::Display for Bayer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Rggb => "RGGB",
            Self::Bggr => "BGGR",
            Self::Grbg => "GRBG",
            Self::Gbrg => "GBRG",
        })
    }
}

impl Bayer {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "RGGB" => Some(Self::Rggb),
            "BGGR" => Some(Self::Bggr),
            "GRBG" => Some(Self::Grbg),
            "GBRG" => Some(Self::Gbrg),
            _ => None,
        }
    }

    /// Pattern from BAYERPAT (or COLORTYP), shifted by XBAYROFF / YBAYROFF.
    pub(crate) fn from_header(header: &[(String, String)]) -> Option<Self> {
        let b = lookup(header, "BAYERPAT")
            .or(lookup(header, "COLORTYP"))
            .and_then(Self::parse)?;
        // Only the parity matters; ignore malformed offsets.
        let off = |k: &str| (lookup(header, k).and_then(parse_int).unwrap_or(0) & 1) as usize;
        Some(b.offset(off("XBAYROFF"), off("YBAYROFF")))
    }

    /// Channel index (0 = R, 1 = G, 2 = B) of the pixel at (x, y).
    pub fn channel(self, x: usize, y: usize) -> usize {
        let pattern = match self {
            Self::Rggb => [0, 1, 1, 2],
            Self::Bggr => [2, 1, 1, 0],
            Self::Grbg => [1, 0, 2, 1],
            Self::Gbrg => [1, 2, 0, 1],
        };
        pattern[(y & 1) * 2 + (x & 1)]
    }

    fn offset(self, dx: usize, dy: usize) -> Self {
        let mut b = self;
        if dx & 1 == 1 {
            b = match b {
                Self::Rggb => Self::Grbg,
                Self::Grbg => Self::Rggb,
                Self::Bggr => Self::Gbrg,
                Self::Gbrg => Self::Bggr,
            };
        }
        if dy & 1 == 1 {
            b = match b {
                Self::Rggb => Self::Gbrg,
                Self::Gbrg => Self::Rggb,
                Self::Bggr => Self::Grbg,
                Self::Grbg => Self::Bggr,
            };
        }
        b
    }
}

/// An image with all sample types normalized to u16, so the rest of the pipeline can
/// work with exact 16-bit histograms and lookup tables.
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// 1 (mono / Bayer) or 3 (RGB planes).
    pub planes: usize,
    /// Plane-major, row-major samples.
    pub data: Vec<u16>,
    pub bayer: Option<Bayer>,
    /// (keyword, value) FITS cards with quotes and comments stripped. FITS extension
    /// images list the extension's cards first, followed by the inherited primary header.
    /// XISF images list their FITS keywords, followed by values taken from XISF properties
    /// that have no keyword (see `xisf::PROPERTIES`).
    pub header: Vec<(String, String)>,
}

impl Image {
    pub fn get(&self, key: &str) -> Option<&str> {
        lookup(&self.header, key)
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(parse_float)
    }

    /// One-line summary of the capture from the header.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.get("OBJECT") {
            parts.push(v.to_string());
        }
        if let Some(v) = self.get_f64("EXPTIME").or(self.get_f64("EXPOSURE")) {
            parts.push(format!("{v}s"));
        }
        if let Some(v) = self.get("FILTER") {
            parts.push(v.to_string());
        }
        if let Some(v) = self.get("GAIN") {
            parts.push(format!("gain {v}"));
        }
        if let Some(v) = self.get_f64("CCD-TEMP") {
            parts.push(format!("{v:.1}°C"));
        }
        if let Some(v) = self.get("DATE-OBS") {
            let without_fraction = v.split('.').next().unwrap_or(v);
            parts.push(without_fraction.replace('T', " "));
        }
        parts.join(" · ")
    }
}

/// Read a FITS or XISF file, told apart by the XISF signature.
pub fn read_file(path: &Path) -> Result<Image, String> {
    let err = |e: std::io::Error| e.to_string();
    let mut file = File::open(path).map_err(err)?;
    let len = usize::try_from(file.metadata().map_err(err)?.len()).map_err(|_| "file too large")?;
    let mut head = vec![0u8; len.min(HEAD)];
    file.read_exact(&mut head).map_err(err)?;
    if head.starts_with(xisf::SIGNATURE) {
        xisf::read(file, head, len)
    } else {
        fits::read(path, file, head, len)
    }
}

/// Parse a FITS or XISF file held in memory, told apart by the XISF signature.
pub fn parse(bytes: &[u8]) -> Result<Image, String> {
    if bytes.starts_with(xisf::SIGNATURE) {
        xisf::parse(bytes)
    } else {
        fits::parse(bytes)
    }
}

/// Fill `dst` with the file's bytes from `start` on, reusing what `head` (the file's
/// first bytes) already holds.
pub(crate) fn read_data(
    file: &mut File,
    head: &[u8],
    start: usize,
    dst: &mut [u8],
) -> Result<(), String> {
    let cached = head.get(start..).unwrap_or_default();
    let have = cached.len().min(dst.len());
    dst[..have].copy_from_slice(&cached[..have]);
    if have < dst.len() {
        file.seek(SeekFrom::Start((start + have) as u64))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut dst[have..])
            .map_err(|e| format!("file truncated: {e}"))?;
    }
    Ok(())
}

pub(crate) fn lookup<'a>(header: &'a [(String, String)], key: &str) -> Option<&'a str> {
    header
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Accepts Fortran-style `D` exponents.
pub(crate) fn parse_float(v: &str) -> Option<f64> {
    let v = v.trim();
    let x = if v.contains(['D', 'd']) {
        v.replace(['D', 'd'], "E").parse::<f64>().ok()?
    } else {
        v.parse::<f64>().ok()?
    };
    x.is_finite().then_some(x)
}

/// Integers may also be written in float notation ("16.", "1.6D1").
pub(crate) fn parse_int(v: &str) -> Option<i64> {
    v.trim().parse::<i64>().ok().or_else(|| {
        let x = parse_float(v)?;
        (x.fract() == 0.0 && x.abs() < 9.2e18).then_some(x as i64)
    })
}
