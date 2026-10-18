//! The embedded star catalog: the brightest Gaia DR3 stars of every ~1 deg² cell, so
//! dense and sparse regions are equally solvable.
//!
//! Cells are 1° declination zones split into `round(360° · cos δ)` RA cells. The file
//! (`data/stars.bin`, built by `examples/build_catalog.rs`) is:
//!
//! - `MAGIC`
//! - one star count (u8) per cell, zone by zone from the south pole, RA ascending
//! - per cell, brightest first, one little-endian u32 per star: RA and Dec offset in
//!   the cell (12 bits each, ~0.9″ steps) and G magnitude (8 bits, 1/16 mag steps)

use std::sync::OnceLock;

pub const MAGIC: &[u8; 8] = b"BLIPSTR1";
pub const ZONES: usize = 180;
pub const EPOCH_YEAR: f64 = 2026.0;
const POSITION_STEPS: f64 = 4096.0;
const MAG_OFFSET: f64 = 1.5;
const MAG_STEPS: f64 = 16.0;

static DATA: &[u8] = include_bytes!("../data/stars.bin");

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CatStar {
    pub ra: f64,
    pub dec: f64,
    pub mag: f32,
}

pub fn cells_in_zone(zone: usize) -> usize {
    let dec_mid = zone as f64 + 0.5 - 90.0;
    ((360.0 * dec_mid.to_radians().cos()).round() as usize).max(1)
}

/// Index of every zone's first cell, plus the total cell count.
pub fn zone_starts() -> Vec<usize> {
    let mut starts = Vec::with_capacity(ZONES + 1);
    let mut n = 0;
    for zone in 0..ZONES {
        starts.push(n);
        n += cells_in_zone(zone);
    }
    starts.push(n);
    starts
}

/// (zone, RA index)
pub fn cell_of(ra: f64, dec: f64) -> (usize, usize) {
    let zone = ((dec + 90.0).floor().max(0.0) as usize).min(ZONES - 1);
    let n = cells_in_zone(zone);
    let i = ((ra.rem_euclid(360.0) / 360.0 * n as f64) as usize).min(n - 1);
    (zone, i)
}

pub fn encode(zone: usize, i: usize, ra: f64, dec: f64, mag: f64) -> u32 {
    let cell_width = 360.0 / cells_in_zone(zone) as f64;
    let fx = (ra.rem_euclid(360.0) - i as f64 * cell_width) / cell_width;
    let fy = dec + 90.0 - zone as f64;
    let quantize =
        |f: f64| ((f * POSITION_STEPS) as i64).clamp(0, POSITION_STEPS as i64 - 1) as u32;
    let m = ((mag + MAG_OFFSET) * MAG_STEPS).round().clamp(0.0, 255.0) as u32;
    quantize(fx) | (quantize(fy) << 12) | (m << 24)
}

fn decode(zone: usize, i: usize, v: u32) -> CatStar {
    let cell_width = 360.0 / cells_in_zone(zone) as f64;
    let fx = ((v & 0xfff) as f64 + 0.5) / POSITION_STEPS;
    let fy = (((v >> 12) & 0xfff) as f64 + 0.5) / POSITION_STEPS;
    CatStar {
        ra: (i as f64 + fx) * cell_width,
        dec: zone as f64 + fy - 90.0,
        mag: ((v >> 24) as f64 / MAG_STEPS - MAG_OFFSET) as f32,
    }
}

pub struct Catalog {
    zone_starts: Vec<usize>,
    counts: &'static [u8],
    cell_starts: Vec<u32>,
    stars: &'static [u8],
}

impl Catalog {
    pub fn get() -> &'static Catalog {
        static CATALOG: OnceLock<Catalog> = OnceLock::new();
        CATALOG.get_or_init(|| Self::parse(DATA).expect("embedded star catalog is corrupt"))
    }

    fn parse(data: &'static [u8]) -> Option<Self> {
        let zone_starts = zone_starts();
        let cells = zone_starts[ZONES];
        let rest = data.strip_prefix(MAGIC)?;
        let (counts, stars) = rest.split_at_checked(cells)?;
        let mut cell_starts = Vec::with_capacity(cells + 1);
        let mut n = 0u32;
        for &c in counts {
            cell_starts.push(n);
            n += c as u32;
        }
        cell_starts.push(n);
        (stars.len() == 4 * n as usize).then_some(Self {
            zone_starts,
            counts,
            cell_starts,
            stars,
        })
    }

    pub fn len(&self) -> usize {
        self.stars.len() / 4
    }

    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }

    pub fn bytes(&self) -> usize {
        MAGIC.len() + self.counts.len() + self.stars.len()
    }

    fn push_cell(&self, zone: usize, i: usize, out: &mut Vec<CatStar>) {
        let cell = self.zone_starts[zone] + i;
        let first = self.cell_starts[cell] as usize;
        let end = self.cell_starts[cell + 1] as usize;
        for v in self.stars[4 * first..4 * end].as_chunks::<4>().0 {
            out.push(decode(zone, i, u32::from_le_bytes(*v)));
        }
    }

    pub fn cone(&self, ra: f64, dec: f64, radius: f64) -> Vec<CatStar> {
        let mut out = Vec::new();
        let zone_of = |dec: f64| ((dec + 90.0).floor().max(0.0) as usize).min(ZONES - 1);
        let dec_nearest_pole = (dec.abs() + radius).min(90.0);
        let ra_half_width = if dec_nearest_pole >= 89.999 {
            180.0
        } else {
            (radius / dec_nearest_pole.to_radians().cos()).min(180.0)
        };
        for zone in zone_of(dec - radius)..=zone_of(dec + radius) {
            let n = cells_in_zone(zone);
            if ra_half_width >= 180.0 {
                (0..n).for_each(|i| self.push_cell(zone, i, &mut out));
                continue;
            }
            let cell_width = 360.0 / n as f64;
            let first = ((ra - ra_half_width) / cell_width).floor() as i64;
            let last = ((ra + ra_half_width) / cell_width).floor() as i64;
            let count = ((last - first + 1) as usize).min(n);
            for k in 0..count as i64 {
                let i = (first + k).rem_euclid(n as i64) as usize;
                self.push_cell(zone, i, &mut out);
            }
        }
        let (sin0, cos0) = dec.to_radians().sin_cos();
        let cos_radius = radius.to_radians().cos();
        out.retain(|s| {
            let (sin1, cos1) = s.dec.to_radians().sin_cos();
            sin0 * sin1 + cos0 * cos1 * (s.ra - ra).to_radians().cos() >= cos_radius
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_cover_the_sky_about_once_per_square_degree() {
        let cells = zone_starts()[ZONES];
        assert!((40_000..42_500).contains(&cells), "{cells}");
    }

    #[test]
    fn encoding_round_trips_within_a_cell_step() {
        for &(ra, dec, mag) in &[
            (0.0, -89.9, 3.0),
            (83.82, -5.39, 11.2),
            (359.999, 0.0, -1.4),
            (201.3, 89.95, 12.4),
        ] {
            let (zone, i) = cell_of(ra, dec);
            let s = decode(zone, i, encode(zone, i, ra, dec, mag));
            let d_ra = ((s.ra - ra + 180.0).rem_euclid(360.0) - 180.0) * dec.to_radians().cos();
            assert!(d_ra.abs() < 1.5 / 4096.0 * 360.0, "{ra} {dec}: {s:?}");
            assert!((s.dec - dec).abs() < 1.0 / 4096.0, "{ra} {dec}: {s:?}");
            assert!((s.mag as f64 - mag).abs() < 0.04, "{mag}: {s:?}");
        }
    }

    #[test]
    fn cone_finds_stars_in_orion_across_ra_zero_and_at_the_pole() {
        let cat = Catalog::get();
        assert!(cat.len() > 1_000_000);
        let orion_belt = cat.cone(83.0, -1.0, 1.0);
        assert!(orion_belt.len() > 50, "{}", orion_belt.len());
        assert!(cat.cone(0.1, 10.0, 1.0).iter().any(|s| s.ra > 359.0));
        assert!(!cat.cone(0.0, 90.0, 1.0).is_empty());
    }
}
