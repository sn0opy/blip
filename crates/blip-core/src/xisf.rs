//! Minimal XISF reader (PixInsight's format, also written by N.I.N.A.): the first image
//! of a monolithic file, 2D gray or RGB with any integer or real sample format, stored
//! as an attachment, optionally zlib / LZ4 / Zstandard compressed and byte shuffled.
//! Samples are mapped onto u16 through the image's bounds. Further images (WBPP adds
//! a crop mask to its masters), checksums and the orientation attribute are ignored.

use crate::fits::parse_value;
use crate::image::{Bayer, Image, lookup, read_data};
use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use rayon::prelude::*;
use std::fs::File;
use std::io::Read;

pub(crate) const SIGNATURE: &[u8] = b"XISF0100";
/// Signature, header length and a reserved field precede the XML header.
const PREFIX: usize = 16;

/// XISF properties exposed as the FITS keyword they correspond to, for files that
/// don't carry that keyword themselves.
pub const PROPERTIES: [(&str, &str); 5] = [
    ("Observation:Object:Name", "OBJECT"),
    ("Instrument:ExposureTime", "EXPTIME"),
    ("Instrument:Filter:Name", "FILTER"),
    ("Instrument:Sensor:Temperature", "CCD-TEMP"),
    ("Observation:Time:Start", "DATE-OBS"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

impl Format {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "UInt8" => Self::U8,
            "UInt16" => Self::U16,
            "UInt32" => Self::U32,
            "UInt64" => Self::U64,
            "Float32" => Self::F32,
            "Float64" => Self::F64,
            _ => return Err(format!("unsupported XISF sample format {s:?}")),
        })
    }

    fn size(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U32 | Self::F32 => 4,
            Self::U64 | Self::F64 => 8,
        }
    }

    fn default_bounds(self) -> (f64, f64) {
        match self {
            Self::U8 => (0.0, u8::MAX as f64),
            Self::U16 => (0.0, u16::MAX as f64),
            Self::U32 => (0.0, u32::MAX as f64),
            Self::U64 => (0.0, u64::MAX as f64),
            Self::F32 | Self::F64 => (0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Zlib,
    /// Also LZ4-HC, which differs only in how it compresses.
    Lz4,
    Zstd,
}

struct Compression {
    codec: Codec,
    /// Byte shuffling item size.
    shuffle: Option<usize>,
    /// (compressed, uncompressed) sizes of the independently compressed parts.
    subblocks: Vec<(usize, usize)>,
}

impl Compression {
    /// `codec[+sh]:uncompressed-size[:item-size]` plus the optional `subblocks`
    /// attribute; `len` is the compressed block size.
    fn parse(spec: &str, subblocks: Option<&str>, len: usize) -> Result<Self, String> {
        let invalid = || format!("invalid XISF compression {spec:?}");
        let mut parts = spec.split(':');
        let name = parts.next().unwrap_or_default();
        let (name, shuffled) = match name.strip_suffix("+sh") {
            Some(n) => (n, true),
            None => (name, false),
        };
        let codec = match name {
            "zlib" => Codec::Zlib,
            "lz4" | "lz4hc" => Codec::Lz4,
            "zstd" => Codec::Zstd,
            _ => return Err(format!("unsupported XISF compression {name:?}")),
        };
        let mut size = || -> Result<usize, String> {
            parts
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or_else(invalid)
        };
        let raw_len = size()?;
        let shuffle = if shuffled { Some(size()?) } else { None };
        if shuffle == Some(0) {
            return Err(invalid());
        }
        let subblocks = match subblocks {
            None => vec![(len, raw_len)],
            Some(s) => s
                .split(':')
                .map(|b| {
                    let (c, u) = b.split_once(',')?;
                    Some((c.parse().ok()?, u.parse().ok()?))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("invalid XISF subblocks {s:?}"))?,
        };
        let total = |f: fn(&(usize, usize)) -> usize| {
            subblocks
                .iter()
                .map(f)
                .try_fold(0usize, |a, b| a.checked_add(b))
        };
        if total(|b| b.0) != Some(len) || total(|b| b.1) != Some(raw_len) {
            return Err("XISF subblock sizes don't add up".into());
        }
        Ok(Self {
            codec,
            shuffle,
            subblocks,
        })
    }
}

struct Layout {
    width: usize,
    height: usize,
    /// Channels stored in the file; only the first `planes` (1 for gray, 3 for RGB) are
    /// used, extra channels are alpha.
    channels: usize,
    planes: usize,
    format: Format,
    /// Sample values shown as black and white.
    bounds: (f64, f64),
    /// Channels stored one after another (Planar) rather than interleaved (Normal).
    planar: bool,
    big_endian: bool,
    /// Attachment position and size in the file.
    pos: usize,
    len: usize,
    compression: Option<Compression>,
    /// Uncompressed data block size.
    raw_len: usize,
    /// From the ColorFilterArray element; BAYERPAT takes precedence.
    cfa: Option<Bayer>,
}

impl Layout {
    fn new(attrs: &[(String, String)]) -> Result<Self, String> {
        let attr = |k: &str| lookup(attrs, k);
        let geometry = attr("geometry").ok_or("XISF image without geometry")?;
        let dims = geometry
            .split(':')
            .map(|n| n.parse::<usize>().ok().filter(|&n| n > 0))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("invalid XISF geometry {geometry:?}"))?;
        let [width, height, channels] = dims[..] else {
            return Err(format!(
                "unsupported XISF geometry {geometry:?} (expected a 2D image)"
            ));
        };
        let planes = match attr("colorSpace").unwrap_or("Gray") {
            "Gray" => 1,
            "RGB" if channels >= 3 => 3,
            "RGB" => return Err(format!("RGB image with {channels} channels")),
            s => return Err(format!("unsupported XISF color space {s:?}")),
        };
        let format = Format::parse(attr("sampleFormat").ok_or("XISF image without sampleFormat")?)?;
        let bounds = match attr("bounds") {
            None => format.default_bounds(),
            Some(b) => b
                .split_once(':')
                .and_then(|(lo, hi)| Some((lo.parse::<f64>().ok()?, hi.parse::<f64>().ok()?)))
                .filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo < hi)
                .ok_or_else(|| format!("invalid XISF bounds {b:?}"))?,
        };
        let planar = match attr("pixelStorage").unwrap_or("Planar") {
            "Planar" => true,
            "Normal" => false,
            s => return Err(format!("invalid XISF pixel storage {s:?}")),
        };
        let big_endian = match attr("byteOrder").unwrap_or("little") {
            "little" => false,
            "big" => true,
            s => return Err(format!("invalid XISF byte order {s:?}")),
        };

        let location = attr("location").ok_or("XISF image without location")?;
        let parts: Vec<&str> = location.split(':').collect();
        let ["attachment", pos, len] = parts[..] else {
            return Err(format!(
                "XISF image location {location:?} not supported (only attachments)"
            ));
        };
        let (pos, len) = pos
            .parse()
            .ok()
            .zip(len.parse().ok())
            .ok_or_else(|| format!("invalid XISF location {location:?}"))?;

        let raw_len = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(channels))
            .and_then(|n| n.checked_mul(format.size()))
            .ok_or("image dimensions too large")?;
        let compression = attr("compression")
            .map(|c| Compression::parse(c, attr("subblocks"), len))
            .transpose()?;
        let stored = match &compression {
            Some(c) => c.subblocks.iter().map(|b| b.1).sum(),
            None => len,
        };
        if stored != raw_len {
            return Err(format!(
                "XISF data block holds {stored} bytes, the image needs {raw_len}"
            ));
        }
        Ok(Self {
            width,
            height,
            channels,
            planes,
            format,
            bounds,
            planar,
            big_endian,
            pos,
            len,
            compression,
            raw_len,
            cfa: None,
        })
    }

    fn check_size(&self, file_len: usize) -> Result<(), String> {
        match self.pos.checked_add(self.len) {
            Some(end) if end <= file_len => Ok(()),
            _ => Err(format!(
                "file truncated: image needs {} data bytes at {}, file has {file_len}",
                self.len, self.pos
            )),
        }
    }

    fn samples(&self) -> usize {
        self.width * self.height * self.planes
    }

    /// Whether the uncompressed block already is the final u16 data (up to byte order).
    fn is_native_u16(&self) -> bool {
        self.format == Format::U16
            && self.bounds == Format::U16.default_bounds()
            && self.channels == self.planes
            && (self.planar || self.planes == 1)
            && self
                .compression
                .as_ref()
                .is_none_or(|c| c.shuffle.is_none())
    }
}

/// Decode an XISF file of `len` bytes whose first bytes `head` were already read.
pub(crate) fn read(mut file: File, mut head: Vec<u8>, len: usize) -> Result<Image, String> {
    let end = header_end(&head)?;
    if end > len {
        return Err("file truncated in XISF header".into());
    }
    if head.len() < end {
        let have = head.len();
        head.resize(end, 0);
        file.read_exact(&mut head[have..])
            .map_err(|e| e.to_string())?;
    }
    let (layout, header) = parse_header(&head[PREFIX..end])?;
    layout.check_size(len)?;
    decode(layout, header, |l, dst| match &l.compression {
        None => read_data(&mut file, &head, l.pos, dst),
        Some(c) => {
            let mut src = vec![0u8; l.len];
            read_data(&mut file, &head, l.pos, &mut src)?;
            decompress(&src, c, dst)
        }
    })
}

pub fn parse(bytes: &[u8]) -> Result<Image, String> {
    if !bytes.starts_with(SIGNATURE) {
        return Err("not an XISF file".into());
    }
    let end = header_end(bytes)?;
    let xml = bytes
        .get(PREFIX..end)
        .ok_or("file truncated in XISF header")?;
    let (layout, header) = parse_header(xml)?;
    layout.check_size(bytes.len())?;
    decode(layout, header, |l, dst| {
        let src = &bytes[l.pos..l.pos + l.len];
        match &l.compression {
            None => {
                dst.copy_from_slice(src);
                Ok(())
            }
            Some(c) => decompress(src, c, dst),
        }
    })
}

fn header_end(bytes: &[u8]) -> Result<usize, String> {
    let n = bytes
        .get(8..12)
        .ok_or("file truncated in XISF header")?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| "file truncated in XISF header")?;
    Ok(PREFIX + n as usize)
}

/// The first top-level image's layout and its FITS keywords, followed by the keywords
/// derived from [`PROPERTIES`].
fn parse_header(xml: &[u8]) -> Result<(Layout, Vec<(String, String)>), String> {
    let xml = std::str::from_utf8(xml).map_err(|_| "XISF header is not valid UTF-8")?;
    let err = |e: quick_xml::Error| format!("invalid XISF header: {e}");
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut depth = 0;
    let mut layout = None;
    let mut keywords = Vec::new();
    let mut properties = Vec::new();
    // Keyword of the String property whose text content is being read.
    let mut text_for: Option<&str> = None;
    let mut text = String::new();
    loop {
        let event = reader.read_event().map_err(err)?;
        let (e, empty) = match event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                depth -= 1;
                if let Some(key) = text_for.take() {
                    properties.push((key.to_string(), std::mem::take(&mut text)));
                }
                if depth == 1 && layout.is_some() {
                    break;
                }
                continue;
            }
            Event::Text(t) if text_for.is_some() => {
                text.push_str(&t.xml10_content());
                continue;
            }
            Event::CData(t) if text_for.is_some() => {
                text.push_str(&t.xml10_content());
                continue;
            }
            Event::GeneralRef(r) if text_for.is_some() => {
                match r.resolve_char_ref().map_err(err)? {
                    Some(c) => text.push(c),
                    None => text.push_str(
                        resolve_predefined_entity(&r.xml10_content()).unwrap_or_default(),
                    ),
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = e.local_name();
        match (depth, name.as_ref()) {
            (1, "Image") if layout.is_none() => {
                layout = Some(Layout::new(&attributes(&e)?)?);
                if empty {
                    break;
                }
            }
            (2, "FITSKeyword") if layout.is_some() => {
                let a = attributes(&e)?;
                let name = lookup(&a, "name").unwrap_or_default().trim();
                if !name.is_empty() && !matches!(name, "COMMENT" | "HISTORY") {
                    let value = parse_value(lookup(&a, "value").unwrap_or_default().as_bytes());
                    keywords.push((name.to_string(), value));
                }
            }
            (2, "Property") if layout.is_some() => {
                let a = attributes(&e)?;
                let id = lookup(&a, "id").unwrap_or_default();
                if let Some(&(_, key)) = PROPERTIES.iter().find(|(p, _)| *p == id) {
                    match lookup(&a, "value") {
                        Some(v) => properties.push((key.to_string(), v.trim().to_string())),
                        // String properties keep their value in the element's text.
                        None if !empty => text_for = Some(key),
                        None => {}
                    }
                }
            }
            (2, "ColorFilterArray") => {
                let a = attributes(&e)?;
                if let Some(l) = layout.as_mut()
                    && lookup(&a, "width") == Some("2")
                    && lookup(&a, "height") == Some("2")
                {
                    l.cfa = lookup(&a, "pattern").and_then(Bayer::parse);
                }
            }
            _ => {}
        }
        if !empty {
            depth += 1;
        }
    }

    let layout = layout.ok_or("no image in XISF file")?;
    for (key, value) in properties {
        if lookup(&keywords, &key).is_none() {
            keywords.push((key, value));
        }
    }
    Ok((layout, keywords))
}

fn attributes(e: &BytesStart) -> Result<Vec<(String, String)>, String> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|e| format!("invalid XISF header: {e}"))?;
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| format!("invalid XISF header: {e}"))?;
            let key = a.key.local_name().as_ref().to_string();
            Ok((key, value.into_owned()))
        })
        .collect()
}

/// Build the image, with `fill` writing the uncompressed (but possibly shuffled) data
/// block into the buffer it gets.
fn decode(
    l: Layout,
    header: Vec<(String, String)>,
    fill: impl FnOnce(&Layout, &mut [u8]) -> Result<(), String>,
) -> Result<Image, String> {
    let data = if l.is_native_u16() {
        let mut data = vec![0u16; l.samples()];
        fill(&l, bytemuck::cast_slice_mut(&mut data))?;
        if l.big_endian != cfg!(target_endian = "big") {
            data.par_iter_mut().for_each(|v| *v = v.swap_bytes());
        }
        data
    } else {
        let mut raw = vec![0u8; l.raw_len];
        fill(&l, &mut raw)?;
        let shuffle = l.compression.as_ref().and_then(|c| c.shuffle);
        let raw = match shuffle {
            Some(n) if n != l.format.size() => unshuffle(&raw, n),
            _ => raw,
        };
        convert(&raw, &l, shuffle == Some(l.format.size()))
    };
    let planes = l.planes;
    let bayer = (planes == 1)
        .then(|| Bayer::from_header(&header).or(l.cfa))
        .flatten();
    Ok(Image {
        width: l.width,
        height: l.height,
        planes,
        data,
        bayer,
        header,
    })
}

fn decompress(src: &[u8], c: &Compression, dst: &mut [u8]) -> Result<(), String> {
    let mut parts = Vec::with_capacity(c.subblocks.len());
    let (mut src, mut dst) = (src, dst);
    for &(compressed, uncompressed) in &c.subblocks {
        let (s, rest) = src.split_at(compressed);
        src = rest;
        let (d, rest) = std::mem::take(&mut dst).split_at_mut(uncompressed);
        dst = rest;
        parts.push((s, d));
    }
    parts
        .into_par_iter()
        .try_for_each(|(s, d)| decompress_part(c.codec, s, d))
}

fn decompress_part(codec: Codec, src: &[u8], dst: &mut [u8]) -> Result<(), String> {
    let corrupt = |e: &dyn std::fmt::Display| format!("corrupt compressed XISF data: {e}");
    let n = match codec {
        Codec::Zlib => {
            let mut z = flate2::Decompress::new(true);
            let status = z
                .decompress(src, dst, flate2::FlushDecompress::Finish)
                .map_err(|e| corrupt(&e))?;
            if status != flate2::Status::StreamEnd {
                return Err(corrupt(&"incomplete zlib stream"));
            }
            z.total_out() as usize
        }
        Codec::Lz4 => {
            let size = i32::try_from(dst.len()).map_err(|_| "LZ4 block too large")?;
            lz4::block::decompress_to_buffer(src, Some(size), dst).map_err(|e| corrupt(&e))?
        }
        Codec::Zstd => zstd::bulk::decompress_to_buffer(src, dst).map_err(|e| corrupt(&e))?,
    };
    if n != dst.len() {
        return Err(corrupt(&format!("{n} of {} bytes", dst.len())));
    }
    Ok(())
}

/// Undo byte shuffling with an item size other than the sample size (the common case
/// is folded into `convert`). Trailing bytes that don't form an item are not shuffled.
fn unshuffle(data: &[u8], item: usize) -> Vec<u8> {
    let items = data.len() / item;
    let mut out = data.to_vec();
    out[..items * item]
        .par_chunks_mut(item)
        .enumerate()
        .for_each(|(i, chunk)| {
            for (b, v) in chunk.iter_mut().enumerate() {
                *v = data[b * items + i];
            }
        });
    out
}

fn convert(raw: &[u8], l: &Layout, shuffled: bool) -> Vec<u16> {
    // One instantiation per byte order keeps the branch out of the per-sample loop.
    macro_rules! samples {
        ($t:ty) => {
            if l.big_endian {
                samples(raw, l, shuffled, |b| <$t>::from_be_bytes(b) as f64)
            } else {
                samples(raw, l, shuffled, |b| <$t>::from_le_bytes(b) as f64)
            }
        };
    }
    match l.format {
        Format::U8 => samples!(u8),
        Format::U16 => samples!(u16),
        Format::U32 => samples!(u32),
        Format::U64 => samples!(u64),
        Format::F32 => samples!(f32),
        Format::F64 => samples!(f64),
    }
}

/// Map the first `l.planes` channels onto 0..=65535 through the image bounds, in
/// plane-major order. `shuffled`: bytes are shuffled with the sample size as item size,
/// i.e. byte `b` of sample `j` is at `b * count + j`. NaN and infinities become 0.
fn samples<const N: usize>(
    raw: &[u8],
    l: &Layout,
    shuffled: bool,
    read: impl Fn([u8; N]) -> f64 + Sync,
) -> Vec<u16> {
    const CHUNK: usize = 1 << 16;
    let pixels = l.width * l.height;
    let chunks = raw.as_chunks::<N>().0;
    let count = chunks.len();
    let byte_planes: [&[u8]; N] = std::array::from_fn(|b| &raw[b * count..(b + 1) * count]);
    let (lo, hi) = l.bounds;
    let scale = 65535.0 / (hi - lo);
    let value = |bytes| {
        let v = read(bytes);
        if v.is_finite() {
            ((v - lo) * scale).clamp(0.0, 65535.0) as u16
        } else {
            0
        }
    };
    let mut out = vec![0u16; l.samples()];
    // Separate loops per storage layout so the common planar ones vectorize.
    out.par_chunks_mut(CHUNK).enumerate().for_each(|(k, dst)| {
        let start = k * CHUNK;
        if !l.planar {
            for (i, o) in (start..).zip(dst) {
                let j = (i % pixels) * l.channels + i / pixels;
                *o = value(if shuffled {
                    std::array::from_fn(|b| byte_planes[b][j])
                } else {
                    chunks[j]
                });
            }
        } else if shuffled {
            let src: [&[u8]; N] =
                std::array::from_fn(|b| &byte_planes[b][start..start + dst.len()]);
            for (i, o) in dst.iter_mut().enumerate() {
                *o = value(std::array::from_fn(|b| src[b][i]));
            }
        } else {
            for (o, c) in dst.iter_mut().zip(&chunks[start..]) {
                *o = value(*c);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Monolithic file with one attachment per image. `images` holds (attributes
    /// without location, child elements, data block).
    fn xisf(images: &[(&str, &str, &[u8])]) -> Vec<u8> {
        let xml = |positions: &[usize]| {
            let mut x = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?><xisf version="1.0" xmlns="http://www.pixinsight.com/xisf"><Metadata><Property id="Observation:Object:Name" type="String">metadata</Property></Metadata>"#,
            );
            for ((attrs, children, data), pos) in images.iter().zip(positions) {
                x += &format!(
                    r#"<Image {attrs} location="attachment:{pos:010}:{}">{children}</Image>"#,
                    data.len()
                );
            }
            x + "</xisf>"
        };
        let mut pos = PREFIX + xml(&vec![0; images.len()]).len();
        let positions: Vec<usize> = images
            .iter()
            .map(|(_, _, d)| {
                pos += d.len();
                pos - d.len()
            })
            .collect();
        let x = xml(&positions);
        let mut out = SIGNATURE.to_vec();
        out.extend((x.len() as u32).to_le_bytes());
        out.extend([0; 4]);
        out.extend(x.as_bytes());
        for (_, _, d) in images {
            out.extend_from_slice(d);
        }
        out
    }

    fn one(attrs: &str, children: &str, data: &[u8]) -> Vec<u8> {
        xisf(&[(attrs, children, data)])
    }

    fn data(bytes: &[u8]) -> Vec<u16> {
        parse(bytes).unwrap().data
    }

    fn parse_err(bytes: &[u8]) -> String {
        parse(bytes).err().expect("parse should fail")
    }

    fn bytes<T: Copy, const N: usize>(vals: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
        vals.iter().flat_map(|&v| f(v)).collect()
    }

    fn shuffle(data: &[u8], item: usize) -> Vec<u8> {
        let items = data.len() / item;
        let mut out = data.to_vec();
        for i in 0..items {
            for b in 0..item {
                out[b * items + i] = data[i * item + b];
            }
        }
        out
    }

    #[test]
    fn gray_u16_and_metadata() {
        let raw = bytes(&[0u16, 1000, 65535], u16::to_le_bytes);
        let children = r#"
            <FITSKeyword name="OBJECT" value="'M 15 ''core'' '" comment="target"/>
            <FITSKeyword name="EXPTIME" value="120." comment="[s]"/>
            <FITSKeyword name="COMMENT" value="" comment="a comment"/>
            <Property id="Observation:Object:Name" type="String">from property</Property>
            <Property id="Instrument:Filter:Name" type="String">Sloan r&apos; &#x2014; &lt;a&gt;</Property>
            <Property id="Instrument:Sensor:Temperature" type="Float32" value=" -10.5"/>
            <Property id="Instrument:Camera:Gain" type="Float32" value="0.8"/>"#;
        let img = parse(&one(
            r#"geometry="3:1:1" sampleFormat="UInt16""#,
            children,
            &raw,
        ))
        .unwrap();
        assert_eq!((img.width, img.height, img.planes), (3, 1, 1));
        assert_eq!(img.data, [0, 1000, 65535]);
        assert_eq!(img.get("OBJECT"), Some("M 15 'core'"));
        assert_eq!(img.get_f64("EXPTIME"), Some(120.0));
        assert_eq!(img.get("FILTER"), Some("Sloan r' \u{2014} <a>"));
        assert_eq!(img.get_f64("CCD-TEMP"), Some(-10.5));
        assert_eq!(img.get("COMMENT"), None);
        assert_eq!(img.get("GAIN"), None);
        assert_eq!(img.bayer, None);

        let raw = bytes(&[1u16, 65534], u16::to_be_bytes);
        let attrs = r#"geometry="2:1:1" sampleFormat="UInt16" byteOrder="big""#;
        assert_eq!(data(&one(attrs, "", &raw)), [1, 65534]);
    }

    #[test]
    fn sample_formats_and_bounds() {
        let gray = |format: &str, extra: &str, raw: &[u8], n: usize| {
            let attrs = format!(r#"geometry="{n}:1:1" sampleFormat="{format}" {extra}"#);
            data(&one(&attrs, "", raw))
        };
        let raw = bytes(&[0.0f32, 0.5, 1.0, 2.0, -1.0, f32::NAN], f32::to_le_bytes);
        assert_eq!(
            gray("Float32", r#"bounds="0:1""#, &raw, 6),
            [0, 32767, 65535, 65535, 0, 0]
        );
        // Floats default to [0, 1].
        assert_eq!(gray("Float32", "", &raw, 6)[..3], [0, 32767, 65535]);
        let raw = bytes(&[-1.0f64, 0.0, 1.0, f64::INFINITY], f64::to_be_bytes);
        assert_eq!(
            gray("Float64", r#"bounds="-1:1" byteOrder="big""#, &raw, 4),
            [0, 32767, 65535, 0]
        );
        assert_eq!(gray("UInt8", "", &[0, 128, 255], 3), [0, 32896, 65535]);
        let raw = bytes(&[0u32, 1 << 31, u32::MAX], u32::to_le_bytes);
        assert_eq!(gray("UInt32", "", &raw, 3), [0, 32767, 65535]);
        let raw = bytes(&[0u64, u64::MAX], u64::to_le_bytes);
        assert_eq!(gray("UInt64", "", &raw, 2), [0, 65535]);
        let raw = bytes(&[0u16, 4095, 5000], u16::to_le_bytes);
        assert_eq!(
            gray("UInt16", r#"bounds="0:4095""#, &raw, 3),
            [0, 65535, 65535]
        );
    }

    #[test]
    fn channels_and_pixel_storage() {
        // Interleaved RGB is returned as planes.
        let attrs =
            r#"geometry="2:1:3" sampleFormat="UInt8" colorSpace="RGB" pixelStorage="Normal""#;
        let img = parse(&one(attrs, "", &[1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!(img.planes, 3);
        assert_eq!(img.data, [1, 4, 2, 5, 3, 6].map(|v| v * 257));
        // Alpha channels are dropped, planar and interleaved.
        let attrs = r#"geometry="2:1:4" sampleFormat="UInt16" colorSpace="RGB""#;
        let raw = bytes(&[1u16, 2, 3, 4, 5, 6, 7, 8], u16::to_le_bytes);
        assert_eq!(data(&one(attrs, "", &raw)), [1, 2, 3, 4, 5, 6]);
        let attrs = r#"geometry="2:1:2" sampleFormat="UInt16" pixelStorage="Normal""#;
        let raw = bytes(&[1u16, 9, 2, 9], u16::to_le_bytes);
        assert_eq!(data(&one(attrs, "", &raw)), [1, 2]);
    }

    #[test]
    fn compression() {
        let values: Vec<u16> = (0..1000u32).map(|i| (i * i % 4099) as u16).collect();
        let u16_raw = bytes(&values, u16::to_le_bytes);
        let floats: Vec<f32> = values.iter().map(|&v| v as f32 / 65535.0).collect();
        let f32_raw = bytes(&floats, f32::to_le_bytes);
        let compress = |codec: &str, data: &[u8]| match codec {
            "zlib" => {
                let mut z =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(data).unwrap();
                z.finish().unwrap()
            }
            "lz4" => lz4::block::compress(data, None, false).unwrap(),
            "lz4hc" => {
                let mode = lz4::block::CompressionMode::HIGHCOMPRESSION(9);
                lz4::block::compress(data, Some(mode), false).unwrap()
            }
            "zstd" => zstd::bulk::compress(data, 3).unwrap(),
            _ => unreachable!(),
        };
        for codec in ["zlib", "lz4", "lz4hc", "zstd"] {
            for (format, raw, item) in [("UInt16", &u16_raw, 2), ("Float32", &f32_raw, 4)] {
                let expected = data(&one(
                    &format!(r#"geometry="100:10:1" sampleFormat="{format}""#),
                    "",
                    raw,
                ));
                for (sh, item) in [("", 0), ("+sh", item), ("+sh", 3)] {
                    let stored = if item > 0 {
                        shuffle(raw, item)
                    } else {
                        raw.clone()
                    };
                    let spec = match item {
                        0 => format!("{codec}:{}", raw.len()),
                        n => format!("{codec}{sh}:{}:{n}", raw.len()),
                    };
                    let attrs = format!(
                        r#"geometry="100:10:1" sampleFormat="{format}" compression="{spec}""#
                    );
                    let whole = compress(codec, &stored);
                    assert_eq!(data(&one(&attrs, "", &whole)), expected, "{spec}");

                    let (a, b) = stored.split_at(stored.len() / 3);
                    let (ca, cb) = (compress(codec, a), compress(codec, b));
                    let attrs = format!(
                        r#"{attrs} subblocks="{},{}:{},{}""#,
                        ca.len(),
                        a.len(),
                        cb.len(),
                        b.len()
                    );
                    let parts = [ca, cb].concat();
                    assert_eq!(data(&one(&attrs, "", &parts)), expected, "{spec} subblocks");
                }
            }
        }

        let attrs = format!(
            r#"geometry="100:10:1" sampleFormat="UInt16" compression="zstd:{}""#,
            u16_raw.len()
        );
        let mut corrupt = compress("zstd", &u16_raw);
        corrupt.truncate(corrupt.len() / 2);
        assert!(parse_err(&one(&attrs, "", &corrupt)).contains("corrupt"));
        let short = compress("zstd", &u16_raw[2..]);
        assert!(parse_err(&one(&attrs, "", &short)).contains("corrupt"));
        let bad = |compression: &str| {
            let attrs = format!(r#"geometry="1:1:1" sampleFormat="UInt8" {compression}"#);
            parse_err(&one(&attrs, "", &[0; 4]))
        };
        assert!(bad(r#"compression="rle:1""#).contains("unsupported"));
        assert!(bad(r#"compression="zlib""#).contains("invalid"));
        assert!(bad(r#"compression="zlib+sh:1""#).contains("invalid"));
        assert!(bad(r#"compression="zlib:1" subblocks="2,1:2,1""#).contains("add up"));
        assert!(bad(r#"compression="zlib:1" subblocks="4""#).contains("invalid"));
        assert!(bad(r#"compression="zlib:2""#).contains("needs 1"));
    }

    #[test]
    fn first_image_and_bayer() {
        // WBPP masters carry a crop mask as a second image.
        let first = bytes(&[7u16, 8], u16::to_le_bytes);
        let mask = bytes(&[1.0f32; 4], f32::to_le_bytes);
        let cfa = r#"<ColorFilterArray pattern="GBRG" width="2" height="2"/>"#;
        let file = xisf(&[
            (
                r#"id="integration" geometry="2:1:1" sampleFormat="UInt16""#,
                cfa,
                &first,
            ),
            (
                r#"id="crop_mask" geometry="2:2:1" sampleFormat="Float32" bounds="0:1""#,
                "",
                &mask,
            ),
        ]);
        let img = parse(&file).unwrap();
        assert_eq!(
            (img.width, img.height, img.data.clone()),
            (2, 1, vec![7, 8])
        );
        assert_eq!(img.bayer, Some(Bayer::Gbrg));
        // Only properties of the image count, not the file's metadata.
        assert_eq!(img.get("OBJECT"), None);

        let keyword = format!(
            r#"{cfa}<FITSKeyword name="BAYERPAT" value="'RGGB'" comment=""/><FITSKeyword name="XBAYROFF" value="1" comment=""/>"#
        );
        let attrs = r#"geometry="2:1:1" sampleFormat="UInt16""#;
        assert_eq!(
            parse(&one(attrs, &keyword, &first)).unwrap().bayer,
            Some(Bayer::Grbg)
        );
        let rgb = r#"geometry="1:1:3" sampleFormat="UInt16" colorSpace="RGB""#;
        assert_eq!(parse(&one(rgb, cfa, &[0; 6])).unwrap().bayer, None);
    }

    #[test]
    fn invalid_files() {
        let err = |attrs: &str, len: usize| parse_err(&one(attrs, "", &vec![0; len]));
        assert!(err(r#"geometry="2:1" sampleFormat="UInt8""#, 2).contains("2D"));
        assert!(err(r#"geometry="2:0:1" sampleFormat="UInt8""#, 0).contains("geometry"));
        assert!(err(r#"sampleFormat="UInt8""#, 1).contains("geometry"));
        assert!(err(r#"geometry="1:1:1""#, 1).contains("sampleFormat"));
        assert!(err(r#"geometry="1:1:1" sampleFormat="Complex32""#, 8).contains("Complex32"));
        assert!(
            err(
                r#"geometry="1:1:3" sampleFormat="UInt8" colorSpace="CIELab""#,
                3
            )
            .contains("CIELab")
        );
        assert!(
            err(
                r#"geometry="1:1:2" sampleFormat="UInt8" colorSpace="RGB""#,
                2
            )
            .contains("2 channels")
        );
        assert!(err(r#"geometry="1:1:1" sampleFormat="UInt8" bounds="1:0""#, 1).contains("bounds"));
        assert!(err(r#"geometry="2:2:1" sampleFormat="UInt16""#, 6).contains("needs 8"));

        let inline = br#"<Image geometry="1:1:1" sampleFormat="UInt8" location="inline:base64">AA==</Image>"#;
        let header = [br#"<xisf version="1.0">"#.as_slice(), inline, b"</xisf>"].concat();
        let file = [
            SIGNATURE,
            &(header.len() as u32).to_le_bytes(),
            &[0; 4],
            &header,
        ]
        .concat();
        assert!(parse_err(&file).contains("only attachments"));

        let ok = one(r#"geometry="4:1:1" sampleFormat="UInt16""#, "", &[0; 8]);
        assert!(parse_err(&ok[..ok.len() - 1]).contains("truncated"));
        assert!(parse_err(&ok[..20]).contains("truncated"));
        assert!(parse_err(&ok[..10]).contains("truncated"));
        assert!(parse_err(b"SIMPLE  =").contains("not an XISF"));
        let empty = [SIGNATURE, &7u32.to_le_bytes(), &[0; 4], b"<xisf/>"].concat();
        assert!(parse_err(&empty).contains("no image"));
        let broken = [SIGNATURE, &12u32.to_le_bytes(), &[0; 4], b"<xisf><Image"].concat();
        assert!(parse_err(&broken).contains("invalid XISF header"));
    }

    #[test]
    fn read_file_paths() {
        let dir = std::env::temp_dir();
        let check = |name: &str, bytes: &[u8]| {
            let path = dir.join(format!("blip-test-{}-{name}.xisf", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            let r = crate::image::read_file(&path);
            std::fs::remove_file(&path).unwrap();
            let mem = parse(bytes);
            assert_eq!(r.as_ref().err(), mem.as_ref().err());
            r.map(|i| (i.get("FILTER").map(str::to_string), i.data))
        };
        let raw = bytes(&[1u16, 65535], u16::to_le_bytes);
        let attrs = r#"geometry="2:1:1" sampleFormat="UInt16""#;
        assert_eq!(check("u16", &one(attrs, "", &raw)).unwrap().1, [1, 65535]);

        // Header longer than the initial read, data compressed.
        let filter = "x".repeat(400_000);
        let children =
            format!(r#"<Property id="Instrument:Filter:Name" type="String">{filter}</Property>"#);
        let packed = zstd::bulk::compress(&shuffle(&raw, 2), 1).unwrap();
        let attrs = format!(r#"{attrs} compression="zstd+sh:4:2""#);
        let file = one(&attrs, &children, &packed);
        assert_eq!(
            check("long", &file).unwrap(),
            (Some(filter), vec![1, 65535])
        );
        assert!(
            check("trunc", &file[..file.len() - 1])
                .unwrap_err()
                .contains("truncated")
        );
        assert!(
            check("header", &file[..1000])
                .unwrap_err()
                .contains("truncated")
        );
    }
}
