//! Deep-sky objects from OpenNGC (`data/dso.tsv`, built by `examples/build_dso.rs`).

use crate::wcs::{self, Wcs};
use std::sync::OnceLock;

static DATA: &str = include_str!("../data/dso.tsv");

#[derive(Clone, Debug, PartialEq)]
pub struct Dso {
    pub label: &'static str,
    pub common_name: &'static str,
    pub openngc_type: &'static str,
    pub ra: f64,
    pub dec: f64,
    pub major_arcmin: f64,
    pub minor_arcmin: f64,
    pub position_angle: f64,
}

impl Dso {
    /// "M 31" for "M 31 (NGC 224)".
    pub fn designation(&self) -> &'static str {
        self.label.split(" (").next().unwrap_or(self.label)
    }
}

pub fn all() -> &'static [Dso] {
    static ALL: OnceLock<Vec<Dso>> = OnceLock::new();
    ALL.get_or_init(|| {
        DATA.lines()
            .filter_map(|line| {
                let f: Vec<&'static str> = line.split('\t').collect();
                let num = |i: usize| f.get(i)?.parse::<f64>().ok();
                Some(Dso {
                    label: f.first()?,
                    ra: num(1)?,
                    dec: num(2)?,
                    major_arcmin: num(3).unwrap_or(0.0),
                    minor_arcmin: num(4).unwrap_or(0.0),
                    position_angle: num(5).unwrap_or(0.0),
                    openngc_type: f.get(6).copied().unwrap_or(""),
                    common_name: f.get(7).copied().unwrap_or(""),
                })
            })
            .collect()
    })
}

pub fn overlapping_cone(ra: f64, dec: f64, radius: f64) -> Vec<&'static Dso> {
    all()
        .iter()
        .filter(|d| {
            let half_major_deg = d.major_arcmin / 120.0;
            wcs::separation(ra, dec, d.ra, d.dec) <= radius + half_major_deg
        })
        .collect()
}

/// Objects centered in the image, and those it lies inside of; largest first.
pub fn in_field(wcs: &Wcs, size: [usize; 2]) -> Vec<&'static Dso> {
    let [w, h] = size.map(|s| s as f64);
    let (ra, dec) = wcs.pixel_to_sky(w / 2.0, h / 2.0);
    let half_diagonal = w.hypot(h) / 2.0 * wcs.arcsec_per_px() / 3600.0;
    let centered = |d: &Dso| {
        (wcs.sky_to_pixel(d.ra, d.dec))
            .is_some_and(|(x, y)| (0.0..w).contains(&x) && (0.0..h).contains(&y))
    };
    let surrounding = |d: &Dso| wcs::separation(ra, dec, d.ra, d.dec) <= d.major_arcmin / 120.0;
    let mut found: Vec<_> = (overlapping_cone(ra, dec, half_diagonal).into_iter())
        .filter(|d| centered(d) || surrounding(d))
        .collect();
    found.sort_by(|a, b| b.major_arcmin.total_cmp(&a.major_arcmin));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn north_up(ra: f64, dec: f64, arcsec_per_px: f64, size: [usize; 2]) -> Wcs {
        let s = arcsec_per_px / 3600.0;
        Wcs {
            crval: [ra, dec],
            crpix: size.map(|n| n as f64 / 2.0),
            cd: [[-s, 0.0], [0.0, -s]],
        }
    }

    #[test]
    fn finds_orion_nebula() {
        assert!(all().len() > 4000);
        let found = overlapping_cone(83.9, -5.1, 1.0);
        let m42 = found.iter().find(|d| d.label.starts_with("M 42 ")).unwrap();
        assert_eq!(m42.common_name, "Great Orion Nebula");
        assert!(m42.major_arcmin > 30.0);
    }

    #[test]
    fn finds_objects_centered_in_the_field_largest_first() {
        let size = [4000, 3000];
        let found = in_field(&north_up(83.82, -5.39, 1.0, size), size);
        let names: Vec<_> = found.iter().map(|d| d.designation()).collect();
        assert!(
            names.contains(&"M 42") && names.contains(&"M 43"),
            "{names:?}"
        );
        assert!(!names.contains(&"NGC 2024"), "{names:?}");
        assert!(
            found
                .windows(2)
                .all(|p| p[0].major_arcmin >= p[1].major_arcmin)
        );
    }

    #[test]
    fn finds_an_object_the_field_lies_inside_of() {
        let size = [1000, 1000];
        let found = in_field(&north_up(10.68, 41.77, 0.5, size), size);
        assert_eq!(found.first().map(|d| d.designation()), Some("M 31"));
    }

    #[test]
    fn designation_drops_the_alternative_name() {
        let found = overlapping_cone(10.68, 41.27, 0.1);
        let m31 = found
            .iter()
            .find(|d| d.common_name == "Andromeda Galaxy")
            .unwrap();
        assert_eq!(m31.designation(), "M 31");
    }
}
