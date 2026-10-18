//! Plate solving near the pointing and pixel scale from the header, by matching
//! triangles of bright image points against triangles of catalog stars.

use crate::catalog::{CatStar, Catalog};
use crate::image::Image;
use crate::stars::Point;
use crate::wcs::{self, Wcs};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

const PATTERN_POINTS: usize = 40;
/// Saturation, color and nebulosity reshuffle the brightness order, so the catalog side
/// gets more stars than the image side.
const CATALOG_SURPLUS: f64 = 2.0;
const MAX_CATALOG_STARS: usize = 400;
const NEAREST_NEIGHBORS: usize = 6;
const SHAPE_TOLERANCE: f64 = 0.012;
const SCALE_TOLERANCE: f64 = 0.15;
const MIN_TRIANGLE_FRACTION_OF_FIELD: f64 = 0.06;
const MIN_MATCHES: usize = 8;
const TEST_RADIUS_PX: f64 = 6.0;
const FIT_RADIUS_PX: f64 = 2.5;
const SEARCH_RADIUS_DEG: f64 = 3.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hint {
    /// (RA, Dec) in degrees, most trusted first.
    pub pointings: Vec<(f64, f64)>,
    pub arcsec_per_px: Option<f64>,
}

impl Hint {
    pub fn from_header(img: &Image) -> Self {
        let mut pointings: Vec<(f64, f64)> = Vec::new();
        let mut add = |pointing: Option<(f64, f64)>| {
            if let Some((ra, dec)) = pointing
                && (-90.0..=90.0).contains(&dec)
                && !pointings
                    .iter()
                    .any(|&(r, d)| wcs::separation(r, d, ra, dec) < 0.25)
            {
                pointings.push((ra.rem_euclid(360.0), dec));
            }
        };
        add(img.get_f64("RA").zip(img.get_f64("DEC")));
        let sexagesimal = |key: &str| img.get(key).and_then(wcs::parse_sexagesimal);
        let ra_hours = sexagesimal("OBJCTRA");
        add(ra_hours.map(|h| h * 15.0).zip(sexagesimal("OBJCTDEC")));

        const ARCSEC_PER_MILLIRADIAN: f64 = 206.264_806;
        let arcsec_per_px = img
            .get_f64("SCALE")
            .or(img.get_f64("PIXSCALE"))
            .or_else(|| {
                let pixel_um = img.get_f64("XPIXSZ").or(img.get_f64("PIXSIZE1"))?;
                let focal_mm = img.get_f64("FOCALLEN")?;
                (focal_mm > 0.0).then(|| ARCSEC_PER_MILLIRADIAN * pixel_um / focal_mm)
            })
            .filter(|s| *s > 0.01 && *s < 600.0);
        Self {
            pointings,
            arcsec_per_px,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    pub wcs: Wcs,
    pub matched_stars: usize,
    pub rms_arcsec: f64,
    pub hint_offset_deg: f64,
    pub tiles_tried: usize,
    pub ms: f64,
}

pub fn solve(
    points_brightest_first: &[Point],
    width: usize,
    height: usize,
    hint: &Hint,
) -> Result<Solution, String> {
    let started = Instant::now();
    let arcsec_per_px = hint
        .arcsec_per_px
        .ok_or("no pixel scale in the header (FOCALLEN / XPIXSZ)")?;
    if hint.pointings.is_empty() {
        return Err("no pointing in the header (RA / DEC)".into());
    }
    let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
    let centered: Vec<[f64; 2]> = points_brightest_first
        .iter()
        .map(|p| [p.x as f64 - cx, p.y as f64 - cy])
        .collect();
    let pattern = &centered[..centered.len().min(PATTERN_POINTS)];
    if pattern.len() < MIN_MATCHES {
        return Err(format!("too few stars ({})", pattern.len()));
    }

    let deg_per_px = arcsec_per_px / 3600.0;
    let (field_w, field_h) = (width as f64 * deg_per_px, height as f64 * deg_per_px);
    let half_diagonal = field_w.hypot(field_h) / 2.0;
    let short_side_px = width.min(height) as f64;
    let image_triangles =
        nearest_neighbor_triangles(pattern, MIN_TRIANGLE_FRACTION_OF_FIELD * short_side_px);
    let index = ShapeIndex::new(&image_triangles);
    let tile_step = 0.6 * field_w.min(field_h);
    let tile_radius = half_diagonal + 0.75 * tile_step;
    let tile_area = std::f64::consts::PI * tile_radius * tile_radius;
    let catalog_stars_at_image_density =
        pattern.len() as f64 * CATALOG_SURPLUS * tile_area / (field_w * field_h);
    let tiles = Tiles {
        catalog: Catalog::get(),
        pattern,
        image_triangles: &image_triangles,
        index: &index,
        deg_per_px,
        radius: tile_radius,
        catalog_stars: (catalog_stars_at_image_density.round() as usize)
            .clamp(pattern.len(), MAX_CATALOG_STARS),
        min_triangle_deg: MIN_TRIANGLE_FRACTION_OF_FIELD
            * field_w.min(field_h)
            * (1.0 - SCALE_TOLERANCE),
    };

    let rings = (SEARCH_RADIUS_DEG / tile_step).ceil() as i64;
    let mut tiles_tried = 0;
    // Ring by ring across all pointings, so a wrong RA / DEC doesn't delay a right
    // OBJCTRA / OBJCTDEC.
    for ring in 0..=rings {
        for &(ra0, dec0) in &hint.pointings {
            let offsets = square_ring(ring);
            tiles_tried += offsets.len();
            let best = offsets
                .par_iter()
                .filter_map(|&(i, j)| {
                    let (xi, eta) = (i as f64 * tile_step, j as f64 * tile_step);
                    let (ra, dec) = wcs::deproject(ra0, dec0, xi, eta);
                    tiles.try_match(ra, dec)
                })
                .max_by_key(|m| m.matches);
            if let Some(m) = best {
                let mut solution = refine(
                    tiles.catalog,
                    &centered,
                    cx,
                    cy,
                    deg_per_px,
                    half_diagonal,
                    m,
                )?;
                let (ra, dec) = solution.wcs.pixel_to_sky(cx, cy);
                solution.hint_offset_deg = wcs::separation(ra0, dec0, ra, dec);
                solution.tiles_tried = tiles_tried;
                solution.ms = started.elapsed().as_secs_f64() * 1000.0;
                return Ok(solution);
            }
        }
    }
    Err(format!(
        "no match within {SEARCH_RADIUS_DEG}° of the header position ({tiles_tried} tiles)"
    ))
}

fn square_ring(ring: i64) -> Vec<(i64, i64)> {
    (-ring..=ring)
        .flat_map(|i| (-ring..=ring).map(move |j| (i, j)))
        .filter(|&(i, j)| i.abs().max(j.abs()) == ring)
        .collect()
}

struct Tiles<'a> {
    catalog: &'static Catalog,
    pattern: &'a [[f64; 2]],
    image_triangles: &'a [Triangle],
    index: &'a ShapeIndex,
    deg_per_px: f64,
    radius: f64,
    catalog_stars: usize,
    min_triangle_deg: f64,
}

struct TileMatch {
    tangent_point: (f64, f64),
    pixel_to_tangent: Affine,
    matches: usize,
}

impl Tiles<'_> {
    fn try_match(&self, ra: f64, dec: f64) -> Option<TileMatch> {
        let mut stars = self.catalog.cone(ra, dec, self.radius);
        stars.sort_by(|a, b| a.mag.total_cmp(&b.mag));
        let projected: Vec<[f64; 2]> = stars
            .iter()
            .filter_map(|s| wcs::project(ra, dec, s.ra, s.dec))
            .collect();
        let brightest = &projected[..projected.len().min(self.catalog_stars)];
        if brightest.len() < MIN_MATCHES {
            return None;
        }
        let catalog_grid = PointGrid::new(&projected, TEST_RADIUS_PX * self.deg_per_px);

        let mut best: Option<(usize, Affine)> = None;
        for sky in nearest_neighbor_triangles(brightest, self.min_triangle_deg) {
            for k in self.index.candidates(sky.side_ratios) {
                let image = &self.image_triangles[k as usize];
                if !image.same_shape(&sky) {
                    continue;
                }
                let scale = sky.longest_side / image.longest_side / self.deg_per_px;
                if (scale - 1.0).abs() > SCALE_TOLERANCE {
                    continue;
                }
                let Some(transform) = Affine::least_squares(&[0, 1, 2].map(|v| {
                    (
                        self.pattern[image.vertices_by_opposite_side[v]],
                        brightest[sky.vertices_by_opposite_side[v]],
                    )
                })) else {
                    continue;
                };
                if !transform.is_similarity(self.deg_per_px) {
                    continue;
                }
                let matches = self
                    .pattern
                    .iter()
                    .filter(|&&p| {
                        catalog_grid
                            .nearest_within_radius(transform.apply(p))
                            .is_some()
                    })
                    .count();
                if best.as_ref().is_none_or(|(m, _)| matches > *m) {
                    best = Some((matches, transform));
                }
            }
        }
        let (matches, pixel_to_tangent) = best?;
        // The triangle's own three points always match.
        (matches >= MIN_MATCHES + 3).then_some(TileMatch {
            tangent_point: (ra, dec),
            pixel_to_tangent,
            matches,
        })
    }
}

/// Least-squares fits over all points near a catalog star, with a shrinking match
/// radius and the tangent point moved to the image center.
fn refine(
    catalog: &Catalog,
    points: &[[f64; 2]],
    cx: f64,
    cy: f64,
    deg_per_px: f64,
    half_diagonal: f64,
    m: TileMatch,
) -> Result<Solution, String> {
    let (mut ra0, mut dec0) = m.tangent_point;
    let mut transform = m.pixel_to_tangent;
    let mut stars: Vec<CatStar> = Vec::new();
    let mut fit = None;
    for (iteration, radius_px) in [TEST_RADIUS_PX, 4.0, FIT_RADIUS_PX, FIT_RADIUS_PX]
        .into_iter()
        .enumerate()
    {
        let [xi, eta] = transform.apply([0.0, 0.0]);
        let (ra, dec) = wcs::deproject(ra0, dec0, xi, eta);
        if iteration == 0 || wcs::separation(ra, dec, ra0, dec0) > 0.2 * half_diagonal {
            stars = catalog.cone(ra, dec, half_diagonal * 1.05);
        }
        transform = transform.retangent((ra0, dec0), (ra, dec))?;
        (ra0, dec0) = (ra, dec);

        let projected: Vec<[f64; 2]> = stars
            .iter()
            .filter_map(|s| wcs::project(ra0, dec0, s.ra, s.dec))
            .collect();
        let grid = PointGrid::new(&projected, radius_px * deg_per_px);
        let pairs: Vec<([f64; 2], [f64; 2])> = points
            .iter()
            .filter_map(|&p| {
                let star = grid.nearest_within_radius(transform.apply(p))?;
                Some((p, projected[star]))
            })
            .collect();
        if pairs.len() < MIN_MATCHES {
            return Err(format!("fit lost its matches ({})", pairs.len()));
        }
        transform = Affine::least_squares(&pairs).ok_or("degenerate fit")?;
        let squared_error: f64 = pairs
            .iter()
            .map(|&(p, q)| {
                let [x, y] = transform.apply(p);
                (x - q[0]).powi(2) + (y - q[1]).powi(2)
            })
            .sum();
        let rms_deg = (squared_error / pairs.len() as f64).sqrt();
        fit = Some((pairs.len(), rms_deg * 3600.0));
    }
    let (matched_stars, rms_arcsec) = fit.ok_or("no fit")?;
    let [px, py] = transform.preimage_of_origin().ok_or("degenerate fit")?;
    Ok(Solution {
        wcs: Wcs {
            crval: [ra0, dec0],
            crpix: [cx + px, cy + py],
            cd: transform.linear,
        },
        matched_stars,
        rms_arcsec,
        hint_offset_deg: 0.0,
        tiles_tried: 0,
        ms: 0.0,
    })
}

#[derive(Clone, Copy, Debug)]
struct Affine {
    linear: [[f64; 2]; 2],
    offset: [f64; 2],
}

impl Affine {
    fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [[a, b], [c, d]] = self.linear;
        [
            a * x + b * y + self.offset[0],
            c * x + d * y + self.offset[1],
        ]
    }

    fn preimage_of_origin(&self) -> Option<[f64; 2]> {
        let [[a, b], [c, d]] = self.linear;
        let det = a * d - b * c;
        let [tx, ty] = self.offset;
        (det != 0.0).then(|| [(b * ty - d * tx) / det, (c * tx - a * ty) / det])
    }

    /// The same pixel → sky mapping, expressed in the tangent plane at `to`.
    fn retangent(&self, from: (f64, f64), to: (f64, f64)) -> Result<Self, String> {
        let moved = |p: [f64; 2]| {
            let [xi, eta] = self.apply(p);
            let (ra, dec) = wcs::deproject(from.0, from.1, xi, eta);
            wcs::project(to.0, to.1, ra, dec).unwrap_or([f64::NAN; 2])
        };
        let corners = [[0.0, 0.0], [1000.0, 0.0], [0.0, 1000.0]];
        Self::least_squares(&corners.map(|p| (p, moved(p)))).ok_or("degenerate fit".into())
    }

    fn least_squares(pairs: &[([f64; 2], [f64; 2])]) -> Option<Self> {
        let n = pairs.len() as f64;
        let sum = pairs.iter().fold([0.0; 4], |s, (p, q)| {
            [s[0] + p[0], s[1] + p[1], s[2] + q[0], s[3] + q[1]]
        });
        let [mx, my, mu, mv] = sum.map(|s| s / n);
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        let (mut sxu, mut syu, mut sxv, mut syv) = (0.0, 0.0, 0.0, 0.0);
        for &([x, y], [u, v]) in pairs {
            let (x, y, u, v) = (x - mx, y - my, u - mu, v - mv);
            sxx += x * x;
            sxy += x * y;
            syy += y * y;
            sxu += x * u;
            syu += y * u;
            sxv += x * v;
            syv += y * v;
        }
        let det = sxx * syy - sxy * sxy;
        if det.abs() < 1e-12 * (sxx * syy).max(1e-300) || !det.is_finite() {
            return None;
        }
        let solve = |su: f64, tu: f64| ((syy * su - sxy * tu) / det, (sxx * tu - sxy * su) / det);
        let (a, b) = solve(sxu, syu);
        let (c, d) = solve(sxv, syv);
        Some(Self {
            linear: [[a, b], [c, d]],
            offset: [mu - a * mx - b * my, mv - c * mx - d * my],
        })
    }

    /// A rotation, possibly mirrored, at the expected scale.
    fn is_similarity(&self, expected_scale: f64) -> bool {
        let [[a, b], [c, d]] = self.linear;
        let (scale_x, scale_y) = (a.hypot(c), b.hypot(d));
        let skew = (a * b + c * d).abs() / (scale_x * scale_y);
        let scale = (scale_x * scale_y).sqrt();
        (scale_x / scale_y - 1.0).abs() < 0.05
            && skew < 0.05
            && (scale / expected_scale - 1.0).abs() < SCALE_TOLERANCE
    }
}

struct Triangle {
    /// Shortest and middle side over the longest.
    side_ratios: [f64; 2],
    vertices_by_opposite_side: [usize; 3],
    longest_side: f64,
}

impl Triangle {
    fn same_shape(&self, other: &Triangle) -> bool {
        let [a0, a1] = self.side_ratios;
        let [b0, b1] = other.side_ratios;
        (a0 - b0).abs() <= SHAPE_TOLERANCE && (a1 - b1).abs() <= SHAPE_TOLERANCE
    }
}

fn nearest_neighbor_triangles(pts: &[[f64; 2]], min_side: f64) -> Vec<Triangle> {
    let dist = |a: usize, b: usize| (pts[a][0] - pts[b][0]).hypot(pts[a][1] - pts[b][1]);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in 0..pts.len() {
        let mut neighbors: Vec<usize> = (0..pts.len()).filter(|&j| j != i).collect();
        neighbors.sort_by(|&a, &b| dist(i, a).total_cmp(&dist(i, b)));
        neighbors.truncate(NEAREST_NEIGHBORS);
        for (n, &j) in neighbors.iter().enumerate() {
            for &k in &neighbors[n + 1..] {
                let mut vertices = [i, j, k];
                vertices.sort_unstable();
                if !seen.insert(vertices) {
                    continue;
                }
                let [p, q, r] = vertices;
                let mut sides_with_opposite_vertex =
                    [(dist(q, r), p), (dist(p, r), q), (dist(p, q), r)];
                sides_with_opposite_vertex.sort_by(|a, b| a.0.total_cmp(&b.0));
                let [(short, v1), (middle, v2), (long, v3)] = sides_with_opposite_vertex;
                let too_small = long < min_side;
                let too_thin = short < 0.1 * long;
                let vertices_ambiguous =
                    middle - short < 0.03 * long || long - middle < 0.03 * long;
                if too_small || too_thin || vertices_ambiguous {
                    continue;
                }
                out.push(Triangle {
                    side_ratios: [short / long, middle / long],
                    vertices_by_opposite_side: [v1, v2, v3],
                    longest_side: long,
                });
            }
        }
    }
    out
}

struct ShapeIndex(HashMap<(i32, i32), Vec<u32>>);

impl ShapeIndex {
    fn bin(side_ratios: [f64; 2]) -> (i32, i32) {
        let [a, b] = side_ratios.map(|r| (r / SHAPE_TOLERANCE).floor() as i32);
        (a, b)
    }

    fn new(triangles: &[Triangle]) -> Self {
        let mut bins: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, t) in triangles.iter().enumerate() {
            bins.entry(Self::bin(t.side_ratios))
                .or_default()
                .push(i as u32);
        }
        Self(bins)
    }

    fn candidates(&self, side_ratios: [f64; 2]) -> Vec<u32> {
        let (bx, by) = Self::bin(side_ratios);
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = self.0.get(&(bx + dx, by + dy)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out
    }
}

struct PointGrid<'a> {
    pts: &'a [[f64; 2]],
    radius: f64,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

impl<'a> PointGrid<'a> {
    fn new(pts: &'a [[f64; 2]], radius: f64) -> Self {
        let mut grid = Self {
            pts,
            radius,
            cells: HashMap::new(),
        };
        for (i, &p) in pts.iter().enumerate() {
            let cell = grid.cell(p);
            grid.cells.entry(cell).or_default().push(i as u32);
        }
        grid
    }

    fn cell(&self, p: [f64; 2]) -> (i64, i64) {
        let [x, y] = p.map(|v| (v / self.radius).floor() as i64);
        (x, y)
    }

    fn nearest_within_radius(&self, p: [f64; 2]) -> Option<usize> {
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let (cx, cy) = self.cell(p);
        let mut best = None;
        let mut best_d2 = self.radius * self.radius;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &i in self.cells.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let q = self.pts[i as usize];
                    let d2 = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                    if d2 <= best_d2 {
                        best_d2 = d2;
                        best = Some(i as usize);
                    }
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_rotated_mirrored_field_with_missing_stars() {
        let (w, h) = (4000usize, 3000usize);
        let truth = Wcs {
            crval: [83.9, -5.2],
            crpix: [w as f64 / 2.0 + 13.0, h as f64 / 2.0 - 7.0],
            cd: {
                let s = 1.2 / 3600.0;
                let (sin, cos) = 25f64.to_radians().sin_cos();
                [[s * cos, -s * sin], [-s * sin, -s * cos]]
            },
        };
        let mut stars = Catalog::get().cone(83.9, -5.2, 1.5);
        stars.sort_by(|a, b| a.mag.total_cmp(&b.mag));
        let saturated = |i: usize| i % 5 == 2;
        let points: Vec<Point> = stars
            .iter()
            .enumerate()
            .filter(|&(i, _)| !saturated(i))
            .filter_map(|(i, s)| {
                let (x, y) = truth.sky_to_pixel(s.ra, s.dec)?;
                let inside = (0.0..w as f64).contains(&x) && (0.0..h as f64).contains(&y);
                inside.then_some(Point {
                    x: x as f32,
                    y: y as f32,
                    flux: 1000.0 - i as f32,
                })
            })
            .collect();
        let hint = Hint {
            pointings: vec![(84.4, -5.0)],
            arcsec_per_px: Some(1.25),
        };
        let solution = solve(&points, w, h, &hint).unwrap();
        let (ra, dec) = solution.wcs.pixel_to_sky(100.0, 2900.0);
        let (true_ra, true_dec) = truth.pixel_to_sky(100.0, 2900.0);
        let error_arcsec = wcs::separation(ra, dec, true_ra, true_dec) * 3600.0;
        assert!(error_arcsec < 1.0, "{solution:?}");
        assert!(solution.rms_arcsec < 1.0, "{solution:?}");
    }
}
