//! Star detection and half-flux radius (HFR) measurement.
//!
//! Stars are detected on a 2x2-binned, white-balanced luminance image (no Bayer
//! checkerboard, less noise) and measured on the full-resolution data, so HFR is
//! reported in sensor pixels.

use crate::image::Image;
use crate::process::{ClipStats, MAD_TO_SIGMA};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Star {
    /// Centroid in full-resolution sensor pixels.
    pub x: f32,
    pub y: f32,
    /// Half-flux radius in sensor pixels.
    pub hfr: f32,
}

/// A bright point for plate solving, including the saturated stars and blends HFR
/// measurement rejects.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub flux: f32,
}

#[derive(Clone, Debug, Default)]
pub struct StarStats {
    pub stars: Vec<Star>,
    pub points_brightest_first: Vec<Point>,
    pub mean_hfr: f32,
    pub median_hfr: f32,
}

/// Detection threshold in units of background noise of the smoothed binned image.
/// High enough to keep only stars whose HFR can be measured reliably.
const DETECT_SIGMA: f32 = 30.0;
/// Measurement aperture radius in units of HFR.
const APERTURE: f32 = 4.0;
/// Measurement aperture limits (sensor pixels).
const MIN_RADIUS: f32 = 8.0;
const MAX_RADIUS: f32 = 30.0;
const MAX_POINTS: usize = 400;

pub fn detect(img: &Image, clip: &ClipStats) -> StarStats {
    const MARGIN: usize = 16;
    let (bw, bh) = (img.width / 2, img.height / 2);
    // No room for a peak away from the border (this also covers empty images).
    if bw <= 2 * MARGIN || bh <= 2 * MARGIN {
        return StarStats::default();
    }
    let lum = Luminance::new(img);
    let smooth = smoothed_binned(&lum, bw, bh);

    let (bg, sigma) = robust_stats(&smooth);
    let threshold = bg + DETECT_SIGMA * sigma.max(f32::EPSILON);
    // Stars on a raised background (cluster cores, nebulae, other stars' halos) are blends
    // more often than not. Smoothing + binning cut the noise ~6x, so 20 sigma here is
    // roughly 3 sigma of single-pixel noise.
    let max_local_bg = bg + 20.0 * sigma;

    let mut peaks: Vec<(usize, usize, f32)> = (MARGIN..bh.saturating_sub(MARGIN))
        .into_par_iter()
        .flat_map_iter(|y| {
            let smooth = &smooth;
            (MARGIN..bw.saturating_sub(MARGIN)).filter_map(move |x| {
                let v = smooth[y * bw + x];
                if v <= threshold {
                    return None;
                }
                for dy in -3isize..=3 {
                    let row = (y as isize + dy) as usize * bw;
                    for dx in -3isize..=3 {
                        if (dx, dy) == (0, 0) {
                            continue;
                        }
                        let n = smooth[row + (x as isize + dx) as usize];
                        // Ties: keep only the first pixel of a flat top.
                        if n > v || (n == v && (dy < 0 || (dy == 0 && dx < 0))) {
                            return None;
                        }
                    }
                }
                Some((x, y, v))
            })
        })
        .collect();
    // Brightest first, so blends are resolved in favor of the dominant star.
    peaks.sort_by(|a, b| b.2.total_cmp(&a.2));
    let points_brightest_first = bright_points(&smooth, bw, &peaks, threshold - bg);

    let measured: Vec<Option<Star>> = peaks
        .par_iter()
        .map(|&(bx, by, _)| {
            measure(
                img,
                &lum,
                clip,
                2.0 * bx as f32 + 1.0,
                2.0 * by as f32 + 1.0,
                max_local_bg,
            )
        })
        .collect();

    // Drop stars overlapping a brighter, already accepted star (blends / double peaks).
    let mut stars: Vec<Star> = Vec::new();
    let cell = 2.0 * MAX_RADIUS;
    let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = Default::default();
    for star in measured.into_iter().flatten() {
        let (gx, gy) = ((star.x / cell) as i32, (star.y / cell) as i32);
        let overlaps = (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                grid.get(&(gx + dx, gy + dy)).is_some_and(|ids| {
                    ids.iter().any(|&i| {
                        let o = &stars[i];
                        let d = ((o.x - star.x).powi(2) + (o.y - star.y).powi(2)).sqrt();
                        d < 3.0 * (o.hfr + star.hfr)
                    })
                })
            })
        });
        if !overlaps {
            grid.entry((gx, gy)).or_default().push(stars.len());
            stars.push(star);
        }
    }

    // Drop HFR outliers (unresolved doubles, galaxies): more than 4 MAD above the median.
    let mut hfrs: Vec<f32> = stars.iter().map(|s| s.hfr).collect();
    if hfrs.len() >= 5 {
        hfrs.sort_by(f32::total_cmp);
        let med = hfrs[hfrs.len() / 2];
        let mut dev: Vec<f32> = hfrs.iter().map(|h| (h - med).abs()).collect();
        dev.sort_by(f32::total_cmp);
        let limit = med + 4.0 * MAD_TO_SIGMA as f32 * dev[dev.len() / 2];
        stars.retain(|s| s.hfr <= limit);
    }
    let mut hfrs: Vec<f32> = stars.iter().map(|s| s.hfr).collect();
    hfrs.sort_by(f32::total_cmp);
    let mean_hfr = if hfrs.is_empty() {
        0.0
    } else {
        hfrs.iter().sum::<f32>() / hfrs.len() as f32
    };
    let median_hfr = hfrs.get(hfrs.len() / 2).copied().unwrap_or(0.0);
    StarStats {
        stars,
        points_brightest_first,
        mean_hfr,
        median_hfr,
    }
}

/// Peaks that mostly sit on a raised local background (nebula knots, halos) are
/// dropped; saturated stars are kept, as they are the ones a star catalog lists.
fn bright_points(
    smooth: &[f32],
    bw: usize,
    peaks_brightest_first: &[(usize, usize, f32)],
    min_local_contrast: f32,
) -> Vec<Point> {
    const CENTROID_HALF_WIDTH: isize = 3;
    const BACKGROUND_RING: (isize, isize) = (6, 9);
    const MAX_PEAKS: usize = 4 * MAX_POINTS;
    const MIN_SEPARATION_PX: f32 = 24.0;
    let (ring_inner, ring_outer) = BACKGROUND_RING;
    let at = |x: isize, y: isize| smooth[y as usize * bw + x as usize];
    let bh = smooth.len() / bw;
    let mut points: Vec<Point> = peaks_brightest_first
        .par_iter()
        .take(MAX_PEAKS)
        .filter_map(|&(bx, by, peak)| {
            let (bx, by) = (bx as isize, by as isize);
            let ring_fits = (ring_outer..bw as isize - ring_outer).contains(&bx)
                && (ring_outer..bh as isize - ring_outer).contains(&by);
            if !ring_fits {
                return None;
            }
            let mut ring = Vec::with_capacity(160);
            for dy in -ring_outer..=ring_outer {
                for dx in -ring_outer..=ring_outer {
                    let r2 = dx * dx + dy * dy;
                    if (ring_inner * ring_inner..=ring_outer * ring_outer).contains(&r2) {
                        ring.push(at(bx + dx, by + dy));
                    }
                }
            }
            let mid = ring.len() / 2;
            let local_background = *ring.select_nth_unstable_by(mid, f32::total_cmp).1;
            if peak - local_background < min_local_contrast.max(0.0) {
                return None;
            }
            let (mut sx, mut sy, mut flux) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -CENTROID_HALF_WIDTH..=CENTROID_HALF_WIDTH {
                for dx in -CENTROID_HALF_WIDTH..=CENTROID_HALF_WIDTH {
                    let f = (at(bx + dx, by + dy) - local_background).max(0.0);
                    sx += f * dx as f32;
                    sy += f * dy as f32;
                    flux += f;
                }
            }
            let binned_to_sensor = |b: f32| 2.0 * b + 0.5;
            (flux > 0.0).then(|| Point {
                x: binned_to_sensor(bx as f32 + sx / flux),
                y: binned_to_sensor(by as f32 + sy / flux),
                flux,
            })
        })
        .collect();
    points.sort_by(|a, b| b.flux.total_cmp(&a.flux));
    let mut kept: Vec<Point> = Vec::with_capacity(MAX_POINTS);
    for p in points {
        let next_to_brighter = |k: &Point| (k.x - p.x).hypot(k.y - p.y) < MIN_SEPARATION_PX;
        if !kept.iter().any(next_to_brighter) {
            kept.push(p);
            if kept.len() == MAX_POINTS {
                break;
            }
        }
    }
    kept
}

/// Full-resolution luminance view: Bayer channels are scaled to a common background
/// level (white balance), RGB planes averaged, mono used as is.
struct Luminance<'a> {
    img: &'a Image,
    gains: [f32; 3],
}

impl<'a> Luminance<'a> {
    fn new(img: &'a Image) -> Self {
        let mut gains = [1.0; 3];
        if let Some(bayer) = img.bayer {
            // Per-channel medians of a sparse grid, via exact 16-bit histograms.
            const STEP: usize = 7;
            let w = img.width;
            let rows: Vec<usize> = (0..img.height).step_by(STEP).collect();
            // Few tasks: the sample is small next to the histograms.
            let per_task = rows.len().div_ceil(4).max(1);
            let hist = rows
                .par_chunks(per_task)
                .map(|ys| {
                    let mut hist = vec![0u32; 3 << 16];
                    for &y in ys {
                        for x in (0..w).step_by(STEP) {
                            hist[(bayer.channel(x, y) << 16) | img.data[y * w + x] as usize] += 1;
                        }
                    }
                    hist
                })
                .reduce_with(|mut a, b| {
                    a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                    a
                })
                .unwrap_or_default();
            let mut medians = [1.0f32; 3];
            for (m, h) in medians.iter_mut().zip(hist.as_chunks::<65536>().0) {
                let n = h.iter().map(|&c| c as u64).sum::<u64>();
                // The element at sorted index n / 2, like `select_nth_unstable(n / 2)`.
                let mut cum = 0u64;
                if let Some(v) = h.iter().position(|&c| {
                    cum += c as u64;
                    cum > n / 2
                }) {
                    *m = v as f32;
                }
            }
            let max = medians.iter().copied().fold(1.0, f32::max);
            gains = medians.map(|m| if m > 0.0 { max / m } else { 1.0 });
        }
        Self { img, gains }
    }

    /// Row `by` of the 2x2-binned luminance (same arithmetic as summing `at`).
    fn binned_row(&self, by: usize, out: &mut [f32]) {
        let img = self.img;
        let w = img.width;
        let (y0, y1) = (2 * by, 2 * by + 1);
        match img.bayer {
            Some(b) => {
                // Binned cells start at even coordinates, so the channels are fixed.
                let g = |dx, dy| self.gains[b.channel(dx, dy)];
                let (g00, g10, g01, g11) = (g(0, 0), g(1, 0), g(0, 1), g(1, 1));
                let n = out.len();
                let top = &img.data[y0 * w..y0 * w + 2 * n];
                let bot = &img.data[y1 * w..y1 * w + 2 * n];
                for (x, o) in out.iter_mut().enumerate() {
                    *o = (top[2 * x] as f32 * g00
                        + top[2 * x + 1] as f32 * g10
                        + bot[2 * x] as f32 * g01
                        + bot[2 * x + 1] as f32 * g11)
                        * 0.25;
                }
            }
            None => {
                for (x, o) in out.iter_mut().enumerate() {
                    let x = 2 * x;
                    *o =
                        (self.at(x, y0) + self.at(x + 1, y0) + self.at(x, y1) + self.at(x + 1, y1))
                            * 0.25;
                }
            }
        }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        let img = self.img;
        let i = y * img.width + x;
        match (img.bayer, img.planes) {
            (Some(b), _) => img.data[i] as f32 * self.gains[b.channel(x, y)],
            (None, 3) => {
                let n = img.width * img.height;
                (img.data[i] as f32 + img.data[n + i] as f32 + img.data[2 * n + i] as f32) / 3.0
            }
            _ => img.data[i] as f32,
        }
    }
}

/// The 2x2-binned luminance with a 3x3 box blur on top (border pixels unblurred),
/// which makes peak finding robust against noise. The blur is separable: each block
/// of rows keeps a ring of the horizontal sums of three consecutive binned rows.
fn smoothed_binned(lum: &Luminance, bw: usize, bh: usize) -> Vec<f32> {
    const BLOCK: usize = 32;
    let mut smooth = vec![0f32; bw * bh];
    smooth.par_chunks_mut(BLOCK * bw).enumerate().for_each_init(
        || (vec![0f32; bw], vec![0f32; 3 * bw]),
        |(binned, ring), (block, out)| {
            let y0 = block * BLOCK;
            let mut next = y0.saturating_sub(1);
            for (y, row) in (y0..).zip(out.chunks_exact_mut(bw)) {
                // Horizontal 3-sums; the border columns keep the plain binned value.
                while next <= (y + 1).min(bh - 1) {
                    lum.binned_row(next, binned);
                    let hs = &mut ring[(next % 3) * bw..(next % 3 + 1) * bw];
                    hs[0] = binned[0];
                    hs[bw - 1] = binned[bw - 1];
                    for (h, b) in hs[1..bw - 1].iter_mut().zip(binned.windows(3)) {
                        *h = b[0] + b[1] + b[2];
                    }
                    next += 1;
                }
                if y == 0 || y + 1 == bh {
                    lum.binned_row(y, row);
                    continue;
                }
                let hs = |y: usize| &ring[(y % 3) * bw..(y % 3 + 1) * bw];
                let (up, mid, dn) = (hs(y - 1), hs(y), hs(y + 1));
                row[0] = mid[0];
                row[bw - 1] = mid[bw - 1];
                for x in 1..bw - 1 {
                    row[x] = (up[x] + mid[x] + dn[x]) / 9.0;
                }
            }
        },
    );
    smooth
}

/// Measure one star around (cx, cy): reject saturated ones, refine the centroid,
/// subtract the local background (annulus median) and compute the HFR.
fn measure(
    img: &Image,
    lum: &Luminance,
    clip: &ClipStats,
    cx: f32,
    cy: f32,
    max_local_bg: f32,
) -> Option<Star> {
    let (w, h) = (img.width as f32, img.height as f32);
    let in_bounds =
        |x: f32, y: f32, r: f32| x - r >= 0.0 && y - r >= 0.0 && x + r < w - 1.0 && y + r < h - 1.0;

    // Saturated cores make the profile flat-topped: HFR would be meaningless.
    let (ix, iy) = (cx as usize, cy as usize);
    for y in iy.saturating_sub(3)..=(iy + 3).min(img.height - 1) {
        for x in ix.saturating_sub(3)..=(ix + 3).min(img.width - 1) {
            for p in 0..img.planes {
                if img.data[p * img.width * img.height + y * img.width + x] >= clip.bright_level {
                    return None;
                }
            }
        }
    }

    let mut radius = MIN_RADIUS * 1.5;
    let (mut x0, mut y0) = (cx, cy);
    let mut hfr = 0.0;
    // The first pass estimates the size, the second uses an aperture of APERTURE x HFR.
    for _ in 0..2 {
        if !in_bounds(x0, y0, radius + 5.0) {
            return None;
        }
        let bg = annulus_median(lum, x0, y0, radius + 1.0, radius + 5.0);
        if bg > max_local_bg {
            return None;
        }

        let (mut sx, mut sy, mut sf) = (0.0f64, 0.0f64, 0.0f64);
        each_in_circle(x0, y0, radius, |x, y, _| {
            let f = (lum.at(x, y) - bg).max(0.0) as f64;
            sx += f * x as f64;
            sy += f * y as f64;
            sf += f;
        });
        if sf <= 0.0 {
            return None;
        }
        x0 = (sx / sf) as f32;
        y0 = (sy / sf) as f32;
        // The centroid can move by up to the radius: stay inside the image.
        if !in_bounds(x0, y0, radius) {
            return None;
        }

        // HFR = flux-weighted mean distance from the centroid.
        let (mut sr, mut sf) = (0.0f64, 0.0f64);
        let mut lit = 0;
        each_in_circle(x0, y0, radius, |x, y, _| {
            // Signed flux: background noise averages out instead of inflating the HFR.
            let f = (lum.at(x, y) - bg) as f64;
            if f > 0.0 {
                lit += 1;
            }
            let r = ((x as f32 - x0).powi(2) + (y as f32 - y0).powi(2)).sqrt() as f64;
            sr += f * r;
            sf += f;
        });
        if sf <= 0.0 || lit < 6 {
            return None;
        }
        hfr = (sr / sf) as f32;
        radius = (APERTURE * hfr).clamp(MIN_RADIUS, MAX_RADIUS);
    }

    // Hot pixels / cosmic rays are tiny; huge blobs are galaxies, nebulae or blends.
    if !(0.8..=MAX_RADIUS / 2.5).contains(&hfr) {
        return None;
    }
    Some(Star { x: x0, y: y0, hfr })
}

fn each_in_circle(cx: f32, cy: f32, r: f32, mut f: impl FnMut(usize, usize, f32)) {
    let r2 = r * r;
    let (x0, x1) = ((cx - r).floor() as usize, (cx + r).ceil() as usize);
    let (y0, y1) = ((cy - r).floor() as usize, (cy + r).ceil() as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
            if d2 <= r2 {
                f(x, y, d2);
            }
        }
    }
}

fn annulus_median(lum: &Luminance, cx: f32, cy: f32, r_in: f32, r_out: f32) -> f32 {
    let mut vals = Vec::with_capacity(256);
    let r_in2 = r_in * r_in;
    each_in_circle(cx, cy, r_out, |x, y, d2| {
        if d2 >= r_in2 {
            vals.push(lum.at(x, y));
        }
    });
    if vals.is_empty() {
        return 0.0;
    }
    let mid = vals.len() / 2;
    *vals.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// Median and MAD-based sigma from a sparse sample (every 5th value).
fn robust_stats(data: &[f32]) -> (f32, f32) {
    const STEP: usize = 5;
    let n = data.len().div_ceil(STEP);
    if n == 0 {
        return (0.0, 0.0);
    }
    let med = select_nth(data, STEP, n / 2, |v| v);
    let mad = select_nth(data, STEP, n / 2, |v| (v - med).abs());
    (med, mad * MAD_TO_SIGMA as f32)
}

/// The `k`-th smallest (by `total_cmp`) of `f(v)` over every `step`-th value of
/// `vals`, without sorting or copying: an exact radix select on an order-preserving
/// u32 key. Each pass histograms the next digit of the keys that share the digits
/// found so far, and picks the digit whose bucket holds rank `k`. Small digits keep
/// the per-task histograms in L1.
fn select_nth(vals: &[f32], step: usize, mut k: usize, f: impl Fn(f32) -> f32 + Sync) -> f32 {
    // Integer order of the key equals `f32::total_cmp`.
    let key = |v: f32| {
        let b = f(v).to_bits();
        b ^ ((((b as i32) >> 31) as u32) | 0x8000_0000)
    };
    // A multiple of `step`, so striding within chunks keeps the global phase.
    let chunk = vals
        .len()
        .div_ceil(rayon::current_num_threads())
        .max(1 << 16)
        .next_multiple_of(step);
    let mut prefix = 0u32;
    for (shift, bits) in [(20, 12), (8, 12), (0, 8)] {
        let top = shift + bits;
        let mask = (1u32 << bits) - 1;
        let hist = vals
            .par_chunks(chunk)
            .map(|c| {
                let mut hist = vec![0u32; 1 << bits];
                for &v in c.iter().step_by(step) {
                    let key = key(v);
                    if (key as u64 >> top) as u32 == prefix {
                        hist[((key >> shift) & mask) as usize] += 1;
                    }
                }
                hist
            })
            .reduce_with(|mut a, b| {
                a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                a
            })
            .unwrap_or_default();
        let mut digit = 0;
        for (i, &c) in hist.iter().enumerate() {
            if c as usize > k {
                digit = i as u32;
                break;
            }
            k -= c as usize;
        }
        prefix = (prefix << bits) | digit;
    }
    f32::from_bits(if prefix >> 31 == 1 {
        prefix ^ 0x8000_0000
    } else {
        !prefix
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::clip_stats;
    use crate::process::testutil::{image, noise};

    #[test]
    fn select_nth_matches_sorting() {
        let vals: Vec<f32> = (0..10_001)
            .map(|i| (noise(i, 20_000) as f32 - 5_000.0) / 7.0)
            .collect();
        for step in [1, 5] {
            let mut s: Vec<f32> = vals.iter().step_by(step).copied().collect();
            s.sort_by(f32::total_cmp);
            for k in [0, 1, s.len() / 3, s.len() / 2, s.len() - 1] {
                assert_eq!(select_nth(&vals, step, k, |v| v), s[k]);
            }
            let med = s[s.len() / 2];
            let mut dev: Vec<f32> = s.iter().map(|v| (v - med).abs()).collect();
            dev.sort_by(f32::total_cmp);
            let k = dev.len() / 2;
            assert_eq!(select_nth(&vals, step, k, |v| (v - med).abs()), dev[k]);
        }
    }

    #[test]
    fn smoothing_matches_direct_computation() {
        let (w, h) = (2 * 41 + 1, 2 * 37);
        let data: Vec<u16> = (0..w * h).map(|i| 1000 + noise(i, 3000) as u16).collect();
        let img = image(w, h, Some("GRBG"), &data);
        let lum = Luminance::new(&img);
        let (bw, bh) = (w / 2, h / 2);
        let binned: Vec<f32> = (0..bw * bh)
            .map(|i| {
                let (x, y) = (2 * (i % bw), 2 * (i / bw));
                (lum.at(x, y) + lum.at(x + 1, y) + lum.at(x, y + 1) + lum.at(x + 1, y + 1)) * 0.25
            })
            .collect();
        let smooth = smoothed_binned(&lum, bw, bh);
        for (i, &s) in smooth.iter().enumerate() {
            let (x, y) = (i % bw, i / bw);
            let want = if x == 0 || y == 0 || x + 1 == bw || y + 1 == bh {
                binned[i]
            } else {
                let mut s = 0.0;
                for dy in 0..3 {
                    let row = (y + dy - 1) * bw;
                    s += binned[row + x - 1] + binned[row + x] + binned[row + x + 1];
                }
                s / 9.0
            };
            assert_eq!(s, want, "({x}, {y})");
        }
    }

    #[test]
    fn finds_synthetic_stars() {
        let (w, h) = (256, 256);
        let stars = [(70.3, 80.6), (180.0, 75.2), (90.8, 190.4), (175.5, 170.1)];
        let sigma: f32 = 2.0;
        let data: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let star: f32 = stars
                    .iter()
                    .map(|&(sx, sy)| {
                        let d2 = (x - sx).powi(2) + (y - sy).powi(2);
                        20_000.0 * (-d2 / (2.0 * sigma * sigma)).exp()
                    })
                    .sum();
                (1000.0 + noise(i, 40) as f32 + star) as u16
            })
            .collect();
        let img = image(w, h, None, &data);
        let found = detect(&img, &clip_stats(&img));
        assert_eq!(found.stars.len(), stars.len());
        for &(sx, sy) in &stars {
            assert!(
                found
                    .stars
                    .iter()
                    .any(|s| (s.x - sx).abs() < 0.3 && (s.y - sy).abs() < 0.3),
                "no star at ({sx}, {sy}): {:?}",
                found.stars
            );
        }
        // Flux-weighted mean radius of a Gaussian: sigma * sqrt(pi / 2).
        let want = sigma * (std::f32::consts::PI / 2.0).sqrt();
        assert!(
            (found.median_hfr - want).abs() < 0.4,
            "{}",
            found.median_hfr
        );
    }
}
