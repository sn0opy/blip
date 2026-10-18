//! Deep-sky objects from OpenNGC (`data/dso.tsv`, built by `examples/build_dso.rs`).

use crate::wcs;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_orion_nebula() {
        assert!(all().len() > 4000);
        let found = overlapping_cone(83.9, -5.1, 1.0);
        let m42 = found.iter().find(|d| d.label.starts_with("M 42 ")).unwrap();
        assert_eq!(m42.common_name, "Great Orion Nebula");
        assert!(m42.major_arcmin > 30.0);
    }
}
