//! Debayer + bin + auto stretch with the midtones transfer function.
//!
//! Previews use a "superpixel" debayer: every 2x2 Bayer cell becomes one RGB pixel
//! (no interpolation, no color artifacts) and larger even bin factors average more
//! cells. Full resolution uses bilinear interpolation.

use crate::image::{Bayer, Image};
use rayon::prelude::*;

/// Converts a median absolute deviation into the standard deviation of a normal
/// distribution.
pub const MAD_TO_SIGMA: f64 = 1.4826;

/// Full-resolution stretch statistics are taken from about this many pixels per channel.
const FULL_STATS_SAMPLES: usize = 4_000_000;
/// Clipped pixels collected for a preview's overlay.
const MAX_CLIP_POINTS: usize = 4_000_000;

/// Each stretch boost step raises the target background by this many percentage points.
pub const BOOST_STEP_PERCENT: u32 = 10;
pub const MAX_BOOST: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StretchParams {
    /// Shadows clipping point in units of normalized MAD (negative = below median).
    pub shadows_clip: f64,
    /// Target background brightness after stretching (0..1).
    pub target_bg: f64,
    /// Scale each channel so the background medians match (neutral gray sky).
    /// When false, the raw sensor color balance is kept.
    pub neutralize: bool,
    /// When false, samples are shown linearly (after the white balance).
    pub auto_stretch: bool,
}

impl StretchParams {
    const TARGET_BG_PERCENT: u32 = 15;

    /// The target background raised by `boost` steps (at most [`MAX_BOOST`]) of
    /// [`BOOST_STEP_PERCENT`] percentage points each, to stretch harder than the default.
    pub fn boosted(self, boost: u32) -> Self {
        let percent = Self::TARGET_BG_PERCENT + boost.min(MAX_BOOST) * BOOST_STEP_PERCENT;
        Self {
            target_bg: f64::from(percent) / 100.0,
            ..self
        }
    }
}

impl Default for StretchParams {
    fn default() -> Self {
        Self {
            shadows_clip: -2.8,
            target_bg: f64::from(Self::TARGET_BG_PERCENT) / 100.0,
            neutralize: true,
            auto_stretch: true,
        }
    }
}

/// Output pixel format, so results can be written straight into the consumer's
/// buffer type (GPU texture, image encoder) without another copy. `Zeroable` lets
/// large output buffers start as lazily zeroed pages that the workers fault in.
pub trait Pixel: Copy + Send + Sync + bytemuck::Zeroable {
    fn from_rgb(r: u8, g: u8, b: u8) -> Self;
}

impl Pixel for [u8; 3] {
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        [r, g, b]
    }
}

impl Pixel for [u8; 4] {
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        [r, g, b, 255]
    }
}

/// RGBA with 4-byte alignment, layout-compatible with GPU texture pixel types
/// (e.g. egui's `Color32`), so a `Vec<Rgba8>` can be reinterpreted without copying.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C, align(4))]
pub struct Rgba8(pub [u8; 4]);

impl Pixel for Rgba8 {
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 255])
    }
}

pub struct Rendered<P> {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<P>,
    pub bin: usize,
    pub stretch: StretchInfo,
}

/// The values the auto-stretch derived for an image.
#[derive(Clone, Debug, Default)]
pub struct StretchInfo {
    /// Per-channel background median (0..1), before white balance.
    pub medians: Vec<f64>,
    /// Per-channel white balance gains.
    pub gains: Vec<f64>,
    /// Shadows clipping point and midtones balance of the linked stretch (0..1).
    pub shadows: f64,
    pub midtones: f64,
}

#[inline]
pub fn mtf(m: f64, x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else if m == 0.5 {
        x
    } else {
        ((m - 1.0) * x) / ((2.0 * m - 1.0) * x - m)
    }
}

/// Binned preview with a long side of at most `max_side`.
pub fn render<P: Pixel>(img: &Image, max_side: usize, params: StretchParams) -> Rendered<P> {
    let (planes, width, height, bin) = bin_image(img, max_side);
    let planes: Vec<&[u16]> = planes.iter().map(Vec::as_slice).collect();
    let (pixels, stretch) = stretch(&planes, params);
    Rendered {
        width,
        height,
        pixels,
        bin,
        stretch,
    }
}

/// The preview's long side for `max_side` (`None` = full resolution), limited to `limit`,
/// the largest image the frontend can show. `None` means unbinned.
pub fn preview_side(img: &Image, max_side: Option<usize>, limit: usize) -> Option<usize> {
    match max_side {
        None if img.width.max(img.height) <= limit => None,
        None => Some(limit),
        Some(side) => Some(side.min(limit)),
    }
}

/// The preview for a long side from [`preview_side`], and its clipped pixels.
pub fn render_preview<P: Pixel>(
    img: &Image,
    clip: &ClipStats,
    side: Option<usize>,
    params: StretchParams,
) -> (Rendered<P>, Vec<(u32, u32, u8)>) {
    let r = match side {
        None => render_full(img, params),
        Some(side) => render(img, side, params),
    };
    let points = clip_points(img, clip, r.bin, r.width, r.height, MAX_CLIP_POINTS);
    (r, points)
}

pub fn render_full<P: Pixel>(img: &Image, params: StretchParams) -> Rendered<P> {
    let (pixels, stretch) = match img.bayer {
        Some(bayer) => debayer_stretch(img, bayer, params),
        None => {
            let n = img.width * img.height;
            let planes: Vec<&[u16]> = (0..img.planes)
                .map(|p| &img.data[p * n..(p + 1) * n])
                .collect();
            stretch(&planes, params)
        }
    };
    Rendered {
        width: img.width,
        height: img.height,
        pixels,
        bin: 1,
        stretch,
    }
}

fn stretch<P: Pixel>(planes: &[&[u16]], params: StretchParams) -> (Vec<P>, StretchInfo) {
    let stats: Vec<(f64, f64)> = planes.par_iter().map(|p| median_mad(p)).collect();
    let (luts, info) = stretch_luts(&stats, params);

    let pixels = if let [r, g, b] = planes {
        let (lr, lg, lb) = (&luts[0], &luts[1], &luts[2]);
        r.par_iter()
            .zip(g.par_iter())
            .zip(b.par_iter())
            .map(|((&r, &g), &b)| P::from_rgb(lr[r as usize], lg[g as usize], lb[b as usize]))
            .collect()
    } else {
        let l = &luts[0];
        planes[0]
            .par_iter()
            .map(|&v| {
                let v = l[v as usize];
                P::from_rgb(v, v, v)
            })
            .collect()
    };
    (pixels, info)
}

/// Full-resolution bilinear debayer fused with the stretch: rows are interpolated
/// into small per-thread buffers and mapped through the LUTs straight into the output,
/// so the three full-size color planes never exist.
fn debayer_stretch<P: Pixel>(
    img: &Image,
    bayer: Bayer,
    params: StretchParams,
) -> (Vec<P>, StretchInfo) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return (Vec::new(), stretch_luts(&[(0.0, 0.0); 3], params).1);
    }

    // Statistics from every `step`-th row pair (both Bayer row types). Interpolated
    // pixels are less noisy than raw ones, so the MAD is taken after debayering too.
    let step = (w * h / FULL_STATS_SAMPLES).max(1);
    let rows: Vec<usize> = (0..h).filter(|y| (y / 2) % step == 0).collect();
    let per_task = rows.len().div_ceil(rayon::current_num_threads()).max(1);
    let hist = rows
        .par_chunks(per_task)
        .map(|ys| {
            let mut hist = vec![0u32; 3 << 16];
            let mut buf = vec![0u16; 3 * w];
            for &y in ys {
                debayer_row(img, bayer, y, split3(&mut buf, w));
                for (c, plane) in buf.chunks_exact(w).enumerate() {
                    let hist = &mut hist[c << 16..(c + 1) << 16];
                    for &v in plane {
                        hist[v as usize] += 1;
                    }
                }
            }
            hist
        })
        .reduce_with(|mut a, b| {
            a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
            a
        })
        .unwrap_or_default();
    let stats: Vec<(f64, f64)> = hist
        .as_chunks::<65536>()
        .0
        .iter()
        .map(|h| hist_median_mad(h))
        .collect();
    let (luts, info) = stretch_luts(&stats, params);

    let (lr, lg, lb) = (&luts[0], &luts[1], &luts[2]);
    let mut pixels: Vec<P> = bytemuck::zeroed_vec(w * h);
    pixels.par_chunks_mut(w).enumerate().for_each_init(
        || vec![0u16; 3 * w],
        |buf, (y, out)| {
            debayer_row(img, bayer, y, split3(buf, w));
            let (r, gb) = buf.split_at(w);
            let (g, b) = gb.split_at(w);
            for (((o, &r), &g), &b) in out.iter_mut().zip(r).zip(g).zip(b) {
                *o = P::from_rgb(lr[r as usize], lg[g as usize], lb[b as usize]);
            }
        },
    );
    (pixels, info)
}

/// Split a `3 * w` buffer into R, G and B rows.
fn split3(buf: &mut [u16], w: usize) -> [&mut [u16]; 3] {
    let (r, gb) = buf.split_at_mut(w);
    let (g, b) = gb.split_at_mut(w);
    [r, g, b]
}

/// Lookup tables and the derived parameters of the linked stretch for channels with
/// the given (median, MAD).
fn stretch_luts(stats: &[(f64, f64)], params: StretchParams) -> (Vec<Vec<u8>>, StretchInfo) {
    // One linked stretch for all channels after an optional multiplicative background
    // neutralization. Channels are scaled *up* to the brightest median so saturated
    // star cores stay white instead of picking up a color cast.
    let max_med = stats.iter().map(|s| s.0).fold(0.0, f64::max);
    let gains: Vec<f64> = stats
        .iter()
        .map(|s| {
            if params.neutralize && s.0 > 0.0 {
                max_med / s.0
            } else {
                1.0
            }
        })
        .collect();
    let (shadows, midtones) = if params.auto_stretch {
        let n = stats.len() as f64;
        let median = stats.iter().zip(&gains).map(|(s, g)| s.0 * g).sum::<f64>() / n;
        let mad = stats.iter().zip(&gains).map(|(s, g)| s.1 * g).sum::<f64>() / n;
        let mad_n = mad * MAD_TO_SIGMA;
        let shadows = if mad_n > 0.0 {
            (median + params.shadows_clip * mad_n).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (shadows, mtf(params.target_bg, median - shadows))
    } else {
        (0.0, 0.5)
    };
    let luts: Vec<Vec<u8>> = gains
        .par_iter()
        .map(|&g| stf_lut(shadows, midtones, g))
        .collect();
    let info = StretchInfo {
        medians: stats.iter().map(|s| s.0).collect(),
        gains,
        shadows,
        midtones,
    };
    (luts, info)
}

/// Bin (and superpixel-debayer, if needed) into 1 or 3 planes so the long side is
/// at most `max_side`. Returns (planes, width, height, bin factor).
///
/// The bin factor never exceeds the short side, so very elongated images keep at
/// least one pixel per axis (and may exceed `max_side`). Bayer images smaller than one
/// 2x2 cell are interpolated at full resolution instead.
pub fn bin_image(img: &Image, max_side: usize) -> (Vec<Vec<u16>>, usize, usize, usize) {
    let (sw, sh) = (img.width, img.height);
    let long = sw.max(sh);
    let max_side = max_side.max(1);
    let src = &img.data;

    let Some(bayer) = img.bayer else {
        let f = long.div_ceil(max_side).min(sw.min(sh)).max(1);
        let (w, h) = (sw / f, sh / f);
        let plane_len = sw * sh;
        let div = (f * f) as u64;
        let planes = (0..img.planes)
            .map(|p| {
                let src = &src[p * plane_len..(p + 1) * plane_len];
                if f == 1 {
                    return src.to_vec();
                }
                let mut dst = vec![0u16; w * h];
                dst.par_chunks_mut(w).enumerate().for_each(|(oy, row)| {
                    for (ox, d) in row.iter_mut().enumerate() {
                        let mut sum = 0u64;
                        for sy in oy * f..oy * f + f {
                            let start = sy * sw + ox * f;
                            sum += src[start..start + f].iter().map(|&v| v as u64).sum::<u64>();
                        }
                        *d = (sum / div) as u16;
                    }
                });
                dst
            })
            .collect();
        return (planes, w, h, f);
    };

    if sw < 2 || sh < 2 {
        return (debayer_planes(img, bayer), sw, sh, 1);
    }

    // Even bin factor so each output pixel covers k x k whole Bayer cells.
    let k = (long / 2).div_ceil(max_side).min(sw / 2).min(sh / 2).max(1);
    let f = 2 * k;
    let (w, h) = (sw / f, sh / f);

    // Offsets of the R, G, G and B samples within a 2x2 cell.
    let mut offsets = [0usize; 4];
    let mut next_green = 1;
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let slot = match bayer.channel(dx, dy) {
            0 => 0,
            2 => 3,
            _ => {
                next_green += 1;
                next_green - 1
            }
        };
        offsets[slot] = dy * sw + dx;
    }
    let [o_r, o_g1, o_g2, o_b] = offsets;
    let cells = (k * k) as u64;

    let mut planes: Vec<Vec<u16>> = (0..3).map(|_| vec![0u16; w * h]).collect();
    let [r, g, b] = &mut planes[..] else {
        unreachable!()
    };
    r.par_chunks_mut(w)
        .zip(g.par_chunks_mut(w))
        .zip(b.par_chunks_mut(w))
        .enumerate()
        .for_each(|(oy, ((rr, gr), br))| {
            if k == 1 {
                let row = oy * f * sw;
                for ox in 0..w {
                    let base = row + 2 * ox;
                    rr[ox] = src[base + o_r];
                    gr[ox] = ((src[base + o_g1] as u32 + src[base + o_g2] as u32) >> 1) as u16;
                    br[ox] = src[base + o_b];
                }
                return;
            }
            for ox in 0..w {
                let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
                for cy in 0..k {
                    let row = (oy * f + 2 * cy) * sw + ox * f;
                    for cx in 0..k {
                        let base = row + 2 * cx;
                        sr += src[base + o_r] as u64;
                        sg += src[base + o_g1] as u64 + src[base + o_g2] as u64;
                        sb += src[base + o_b] as u64;
                    }
                }
                rr[ox] = (sr / cells) as u16;
                gr[ox] = (sg / (2 * cells)) as u16;
                br[ox] = (sb / cells) as u16;
            }
        });
    (planes, w, h, f)
}

/// Bilinear demosaic of a whole (small) image into three planes.
fn debayer_planes(img: &Image, bayer: Bayer) -> Vec<Vec<u16>> {
    let w = img.width;
    let mut planes: Vec<Vec<u16>> = (0..3).map(|_| vec![0u16; w * img.height]).collect();
    let [r, g, b] = &mut planes[..] else {
        unreachable!()
    };
    for y in 0..img.height {
        let row = y * w..(y + 1) * w;
        debayer_row(
            img,
            bayer,
            y,
            [&mut r[row.clone()], &mut g[row.clone()], &mut b[row]],
        );
    }
    planes
}

/// Bilinear demosaic of row `y` at full resolution: every missing color is the mean
/// of the same-color pixels in the 3x3 neighborhood.
fn debayer_row(img: &Image, bayer: Bayer, y: usize, [rr, gr, br]: [&mut [u16]; 3]) {
    #[derive(Clone, Copy, PartialEq)]
    enum Site {
        Red,
        Blue,
        /// Green pixel with red neighbors left and right.
        GreenRedRow,
        GreenBlueRow,
    }
    let site = |x: usize, y: usize| match bayer.channel(x, y) {
        0 => Site::Red,
        2 => Site::Blue,
        _ if bayer.channel(x + 1, y) == 0 => Site::GreenRedRow,
        _ => Site::GreenBlueRow,
    };

    let (w, h) = (img.width, img.height);
    let src = &img.data;
    if y == 0 || y + 1 == h || w < 3 {
        for x in 0..w {
            [rr[x], gr[x], br[x]] = debayer_edge_pixel(src, w, h, bayer, x, y);
        }
        return;
    }
    let up = &src[(y - 1) * w..y * w];
    let mid = &src[y * w..(y + 1) * w];
    let dn = &src[(y + 1) * w..(y + 2) * w];
    let sites = [site(0, y), site(1, y)];
    for x in 1..w - 1 {
        let c = mid[x];
        let (n, s) = (up[x] as u32, dn[x] as u32);
        let (e, west) = (mid[x + 1] as u32, mid[x - 1] as u32);
        let site = sites[x & 1];
        let (pr, pg, pb) = match site {
            Site::Red | Site::Blue => {
                let cross = ((n + s + e + west) >> 2) as u16;
                let diag =
                    ((up[x - 1] as u32 + up[x + 1] as u32 + dn[x - 1] as u32 + dn[x + 1] as u32)
                        >> 2) as u16;
                if site == Site::Red {
                    (c, cross, diag)
                } else {
                    (diag, cross, c)
                }
            }
            Site::GreenRedRow => (((e + west) >> 1) as u16, c, ((n + s) >> 1) as u16),
            Site::GreenBlueRow => (((n + s) >> 1) as u16, c, ((e + west) >> 1) as u16),
        };
        rr[x] = pr;
        gr[x] = pg;
        br[x] = pb;
    }
    for x in [0, w - 1] {
        [rr[x], gr[x], br[x]] = debayer_edge_pixel(src, w, h, bayer, x, y);
    }
}

/// Bilinear debayer of a single pixel with bounds checks, for the image border.
fn debayer_edge_pixel(
    src: &[u16],
    w: usize,
    h: usize,
    bayer: Bayer,
    x: usize,
    y: usize,
) -> [u16; 3] {
    let mut sum = [0u32; 3];
    let mut cnt = [0u32; 3];
    for sy in [y.wrapping_sub(1), y, y + 1] {
        for sx in [x.wrapping_sub(1), x, x + 1] {
            if sx < w && sy < h {
                let c = bayer.channel(sx, sy);
                sum[c] += src[sy * w + sx] as u32;
                cnt[c] += 1;
            }
        }
    }
    let center = src[y * w + x];
    let native = bayer.channel(x, y);
    std::array::from_fn(|c| {
        if c == native {
            center
        } else {
            sum[c].checked_div(cnt[c]).map_or(center, |v| v as u16)
        }
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClipStats {
    pub total: u64,
    /// Pixels at 0 (black clipping, e.g. offset too low).
    pub dark: u64,
    /// Pixels at the sensor's saturation level.
    pub bright: u64,
    /// Detected saturation level on the 0..65535 scale.
    pub bright_level: u16,
}

/// Count black-clipped and saturated pixels in the raw data.
///
/// The saturation level is derived from the data's bit depth: e.g. a 12-bit camera
/// scaled to 16 bit never sets the lowest 4 bits, so it saturates at 65520.
pub fn clip_stats(img: &Image) -> ClipStats {
    let unused_bits = img
        .data
        .par_iter()
        .copied()
        .reduce(|| 0, |a, b| a | b)
        .trailing_zeros()
        .min(15);
    let bright_level = !((1u16 << unused_bits) - 1);
    let (dark, bright) = img
        .data
        .par_chunks(1 << 16)
        .map(|chunk| {
            chunk.iter().fold((0u64, 0u64), |(d, b), &v| {
                (d + (v == 0) as u64, b + (v >= bright_level) as u64)
            })
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    ClipStats {
        total: img.data.len() as u64,
        dark,
        bright,
        bright_level,
    }
}

pub const CLIP_DARK: u8 = 1;
pub const CLIP_BRIGHT: u8 = 2;

/// Clipped pixels in a `w`x`h` image rendered with bin factor `bin`, as
/// `(x, y, kind)`; bright wins within a block. Capped at `limit` entries.
pub fn clip_points(
    img: &Image,
    stats: &ClipStats,
    bin: usize,
    w: usize,
    h: usize,
    limit: usize,
) -> Vec<(u32, u32, u8)> {
    if stats.dark == 0 && stats.bright == 0 {
        return Vec::new();
    }
    let sw = img.width;
    let plane_len = img.width * img.height;
    let level = stats.bright_level;
    let rows: Vec<Vec<(u32, u32, u8)>> = (0..h)
        .into_par_iter()
        .map(|oy| {
            let mut kinds = vec![0u8; w];
            for p in 0..img.planes {
                let plane = &img.data[p * plane_len..(p + 1) * plane_len];
                for sy in oy * bin..oy * bin + bin {
                    let line = &plane[sy * sw..sy * sw + w * bin];
                    for (sx, &v) in line.iter().enumerate() {
                        let k = if v >= level {
                            CLIP_BRIGHT
                        } else if v == 0 {
                            CLIP_DARK
                        } else {
                            continue;
                        };
                        let o = &mut kinds[sx / bin];
                        *o = (*o).max(k);
                    }
                }
            }
            kinds
                .iter()
                .enumerate()
                .filter(|(_, k)| **k != 0)
                .map(|(x, &k)| (x as u32, oy as u32, k))
                .collect()
        })
        .collect();
    rows.into_iter().flatten().take(limit).collect()
}

/// Exact median and MAD (both normalized to 0..1) via a 16-bit histogram.
pub fn median_mad(data: &[u16]) -> (f64, f64) {
    hist_median_mad(&histogram(data))
}

fn histogram(data: &[u16]) -> Vec<u32> {
    data.par_chunks(1 << 20)
        .map(|chunk| {
            let mut h = vec![0u32; 65536];
            for &v in chunk {
                h[v as usize] += 1;
            }
            h
        })
        .reduce(
            || vec![0u32; 65536],
            |mut a, b| {
                a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                a
            },
        )
}

/// Exact median and MAD (normalized to 0..1) of the values counted in a 16-bit histogram.
fn hist_median_mad(hist: &[u32]) -> (f64, f64) {
    let n = hist.iter().map(|&c| c as u64).sum::<u64>();
    if n == 0 {
        return (0.0, 0.0);
    }
    let half = n.div_ceil(2);

    let mut cum = 0u64;
    let mut med = 0usize;
    for (i, &c) in hist.iter().enumerate() {
        cum += c as u64;
        if cum >= half {
            med = i;
            break;
        }
    }

    // The histogram of |x - median| follows directly from the value histogram.
    let mut cum = hist[med] as u64;
    let mut mad = 0usize;
    if cum < half {
        for d in 1..65536usize {
            if med + d < 65536 {
                cum += hist[med + d] as u64;
            }
            if d <= med {
                cum += hist[med - d] as u64;
            }
            if cum >= half {
                mad = d;
                break;
            }
        }
    }
    (med as f64 / 65535.0, mad as f64 / 65535.0)
}

/// u16 -> u8 lookup table: scale by `gain`, clip at `shadows`, apply the MTF.
fn stf_lut(shadows: f64, midtones: f64, gain: f64) -> Vec<u8> {
    let range = (1.0 - shadows).max(f64::EPSILON);
    (0..65536u32)
        .map(|v| {
            let x = ((v as f64 / 65535.0 * gain).min(1.0) - shadows) / range;
            (mtf(midtones, x) * 255.0 + 0.5) as u8
        })
        .collect()
}

/// Synthetic images for unit tests.
#[cfg(test)]
pub(crate) mod testutil {
    use crate::fits;
    use crate::image::Image;

    /// Parse a 16-bit unsigned FITS image (row-major `data`) built in memory.
    pub fn image(w: usize, h: usize, bayer: Option<&str>, data: &[u16]) -> Image {
        assert_eq!(data.len(), w * h);
        let mut cards = vec![
            format!("SIMPLE  = {:>20}", "T"),
            format!("BITPIX  = {:>20}", 16),
            format!("NAXIS   = {:>20}", 2),
            format!("NAXIS1  = {w:>20}"),
            format!("NAXIS2  = {h:>20}"),
            format!("BZERO   = {:>20}", 32768),
            format!("BSCALE  = {:>20}", 1),
        ];
        if let Some(p) = bayer {
            cards.push(format!("BAYERPAT= '{p}'"));
        }
        cards.push("END".into());
        let mut bytes: Vec<u8> = cards
            .iter()
            .flat_map(|c| format!("{c:<80}").into_bytes())
            .collect();
        bytes.resize(bytes.len().next_multiple_of(2880), b' ');
        for &v in data {
            bytes.extend_from_slice(&(v ^ 0x8000).to_be_bytes());
        }
        bytes.resize(bytes.len().next_multiple_of(2880), 0);
        fits::parse(&bytes).unwrap()
    }

    /// Deterministic pseudo-random values in `0..n`.
    pub fn noise(i: usize, n: u32) -> u32 {
        let x = (i as u32).wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
        (x ^ (x >> 15)).wrapping_mul(0x2C1B_3C6D) % n
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{image, noise};
    use super::*;

    #[test]
    fn mtf_edge_cases() {
        for m in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert_eq!(mtf(m, 0.0), 0.0);
            assert_eq!(mtf(m, -1.0), 0.0);
            assert_eq!(mtf(m, 1.0), 1.0);
            assert_eq!(mtf(m, 2.0), 1.0);
        }
        assert_eq!(mtf(0.5, 0.3), 0.3);
        // The midtones balance maps to 0.5 by definition.
        for m in [0.01, 0.2, 0.7] {
            assert!((mtf(m, m) - 0.5).abs() < 1e-12);
        }
        // Monotonic.
        let ys: Vec<f64> = (0..=100).map(|i| mtf(0.05, i as f64 / 100.0)).collect();
        assert!(ys.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn median_mad_exact() {
        assert_eq!(median_mad(&[]), (0.0, 0.0));
        assert_eq!(median_mad(&[7; 9]), (7.0 / 65535.0, 0.0));
        // Median 3; deviations 2, 1, 0, 1, 97 -> MAD 1.
        assert_eq!(
            median_mad(&[1, 2, 3, 4, 100]),
            (3.0 / 65535.0, 1.0 / 65535.0)
        );
        assert_eq!(median_mad(&[0, 65535, 65535]), (1.0, 0.0));
    }

    #[test]
    fn without_auto_stretch_samples_map_linearly() {
        let params = StretchParams {
            auto_stretch: false,
            neutralize: false,
            ..Default::default()
        };
        let r = render_full::<[u8; 3]>(&image(3, 1, None, &[0, 32768, 65535]), params);
        assert_eq!(r.pixels, [[0; 3], [128; 3], [255; 3]]);
        // Bayer data is still debayered.
        let r = render::<[u8; 3]>(&channel_coded(12, 12, "RGGB"), 6, params);
        assert!(r.pixels.iter().all(|&p| p == [4, 12, 19]));
    }

    #[test]
    fn boost_raises_the_background_by_absolute_steps() {
        let data: Vec<u16> = (0..64 * 64)
            .map(|i| (1000 + noise(i, 200)) as u16)
            .collect();
        let img = image(64, 64, None, &data);
        let median = median_mad(&data).0;
        for (boost, target) in [(0, 0.15), (1, 0.25), (2, 0.35), (3, 0.45), (9, 0.45)] {
            let params = StretchParams::default().boosted(boost);
            assert_eq!(params.target_bg, target);
            let r = render_full::<[u8; 3]>(&img, params);
            let s = &r.stretch;
            let bg = mtf(s.midtones, (median - s.shadows) / (1.0 - s.shadows));
            assert!((bg - target).abs() < 0.01, "boost {boost}: {bg}");
        }
    }

    /// Bayer image whose samples encode their channel: R 1000, B 5000, and the
    /// greens 2000 (even rows) / 4000 (odd rows), so a correct superpixel is
    /// (1000, 3000, 5000).
    fn channel_coded(w: usize, h: usize, pattern: &str) -> Image {
        let p = pattern.as_bytes();
        let data: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match p[(y % 2) * 2 + x % 2] {
                    b'R' => 1000,
                    b'B' => 5000,
                    _ if y % 2 == 0 => 2000,
                    _ => 4000,
                }
            })
            .collect();
        image(w, h, Some(pattern), &data)
    }

    #[test]
    fn bayer_binning_uses_the_right_channels() {
        for pattern in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            // (max_side, expected bin): one cell per pixel, and 3x3 cells per pixel.
            for (max_side, bin) in [(6, 2), (2, 6)] {
                let img = channel_coded(12, 12, pattern);
                let (planes, w, h, f) = bin_image(&img, max_side);
                assert_eq!((w, h, f), (12 / bin, 12 / bin, bin), "{pattern}");
                for (plane, want) in planes.iter().zip([1000, 3000, 5000]) {
                    assert!(plane.iter().all(|&v| v == want), "{pattern} bin {bin}");
                }
            }
            // Full resolution: red and blue are flat, green mixes both rows' greens.
            let img = channel_coded(9, 7, pattern);
            let planes = debayer_planes(&img, img.bayer.unwrap());
            assert!(planes[0].iter().all(|&v| v == 1000), "{pattern}");
            assert!(planes[1].iter().all(|v| (2000..=4000).contains(v)));
            assert!(planes[2].iter().all(|&v| v == 5000), "{pattern}");
        }
    }

    #[test]
    fn mono_binning_averages_and_does_not_overflow() {
        let data: Vec<u16> = (0..16).map(|i| i as u16 * 100).collect();
        let (planes, w, h, f) = bin_image(&image(4, 4, None, &data), 2);
        assert_eq!((w, h, f), (2, 2, 2));
        assert_eq!(planes[0], [250, 450, 1050, 1250]);

        // Sums of more than 65537 saturated samples overflowed a u32.
        let img = image(600, 600, None, &vec![65535; 600 * 600]);
        let (planes, w, h, f) = bin_image(&img, 1);
        assert_eq!((w, h, f, planes[0][0]), (1, 1, 600, 65535));
        let img = image(1200, 1200, Some("RGGB"), &vec![65535; 1200 * 1200]);
        let (planes, w, h, f) = bin_image(&img, 1);
        assert_eq!((w, h, f), (1, 1, 1200));
        assert!(planes.iter().all(|p| p == &[65535]));
    }

    #[test]
    fn tiny_images_do_not_panic() {
        // Empty images are rejected by `fits::parse`, so 1 px is the smallest case.
        let sizes = [
            (1, 1),
            (1, 2),
            (2, 1),
            (2, 2),
            (1, 9),
            (9, 1),
            (3, 3),
            (5, 3),
            (3, 1000),
        ];
        for (w, h) in sizes {
            for bayer in [None, Some("RGGB"), Some("GBRG")] {
                let data: Vec<u16> = (0..w * h).map(|i| noise(i, 65536) as u16).collect();
                let img = image(w, h, bayer, &data);
                let clip = clip_stats(&img);
                for max_side in [1, 2, 4, 100] {
                    let r = render::<[u8; 4]>(&img, max_side, StretchParams::default());
                    assert_eq!(r.pixels.len(), r.width * r.height);
                    if w * h > 0 {
                        assert!(r.width >= 1 && r.height >= 1, "{w}x{h} {bayer:?}");
                    }
                    clip_points(&img, &clip, r.bin, r.width, r.height, 100);
                }
                let r = render_full::<Rgba8>(&img, StretchParams::default());
                assert_eq!((r.width, r.height, r.pixels.len()), (w, h, w * h));
                let channels = if bayer.is_some() { 3 } else { 1 };
                assert_eq!(r.stretch.medians.len(), channels);
                crate::stars::detect(&img, &clip);
            }
        }
    }

    /// The pre-fusion full-resolution path: three interpolated planes, then the
    /// stretch with exact statistics.
    fn render_full_reference(img: &Image, params: StretchParams) -> (Vec<[u8; 3]>, Vec<f64>) {
        let bayer = img.bayer.unwrap();
        let (w, h) = (img.width, img.height);
        let mut planes = vec![Vec::new(); 3];
        for y in 0..h {
            for x in 0..w {
                let px = debayer_edge_pixel(&img.data, w, h, bayer, x, y);
                planes.iter_mut().zip(px).for_each(|(p, v)| p.push(v));
            }
        }
        let planes: Vec<&[u16]> = planes.iter().map(Vec::as_slice).collect();
        let (pixels, info) = stretch(&planes, params);
        (pixels, info.medians)
    }

    #[test]
    fn fused_full_resolution_matches_reference() {
        for pattern in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            let (w, h) = (37, 23);
            // Sky background with per-channel offsets plus a few bright spots.
            let data: Vec<u16> = (0..w * h)
                .map(|i| {
                    let base = 900 + 300 * ((i % w) % 2 + (i / w) % 2) as u32;
                    let star = if noise(i, 40) == 0 { 30000 } else { 0 };
                    (base + noise(i, 200) + star) as u16
                })
                .collect();
            let img = image(w, h, Some(pattern), &data);
            let params = StretchParams::default();
            let fused = render_full::<[u8; 3]>(&img, params);
            let (pixels, medians) = render_full_reference(&img, params);
            assert_eq!(fused.stretch.medians, medians, "{pattern}");
            assert!(fused.pixels == pixels, "{pattern}");
        }
    }
}
