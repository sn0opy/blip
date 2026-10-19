//! Gnomonic (TAN) world coordinate systems.
//!
//! Pixels are blip's: 0-based, row 0 = the first row in the file, so FITS pixel
//! `(x + 1, y + 1)`.

use crate::image::Image;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wcs {
    pub crval: [f64; 2],
    pub crpix: [f64; 2],
    pub cd: [[f64; 2]; 2],
}

/// `None` on the far hemisphere.
pub fn project(ra0: f64, dec0: f64, ra: f64, dec: f64) -> Option<[f64; 2]> {
    let (s0, c0) = dec0.to_radians().sin_cos();
    let (s, c) = dec.to_radians().sin_cos();
    let (sa, ca) = (ra - ra0).to_radians().sin_cos();
    let cos_c = s0 * s + c0 * c * ca;
    if cos_c <= 1e-6 {
        return None;
    }
    Some([
        (c * sa / cos_c).to_degrees(),
        ((c0 * s - s0 * c * ca) / cos_c).to_degrees(),
    ])
}

pub fn deproject(ra0: f64, dec0: f64, xi: f64, eta: f64) -> (f64, f64) {
    let (x, y) = (xi.to_radians(), eta.to_radians());
    let (s0, c0) = dec0.to_radians().sin_cos();
    let denom = c0 - y * s0;
    let ra = ra0 + x.atan2(denom).to_degrees();
    let dec = (s0 + y * c0).atan2((x * x + denom * denom).sqrt());
    (ra.rem_euclid(360.0), dec.to_degrees())
}

pub fn separation(ra0: f64, dec0: f64, ra1: f64, dec1: f64) -> f64 {
    let (s0, c0) = dec0.to_radians().sin_cos();
    let (s1, c1) = dec1.to_radians().sin_cos();
    let d = (ra1 - ra0).to_radians();
    // Vincenty: stable for tiny and antipodal separations.
    let a = c1 * d.sin();
    let b = c0 * s1 - s0 * c1 * d.cos();
    (a.hypot(b)).atan2(s0 * s1 + c0 * c1 * d.cos()).to_degrees()
}

impl Wcs {
    pub fn pixel_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.crpix[0], y - self.crpix[1]);
        let xi = self.cd[0][0] * dx + self.cd[0][1] * dy;
        let eta = self.cd[1][0] * dx + self.cd[1][1] * dy;
        deproject(self.crval[0], self.crval[1], xi, eta)
    }

    pub fn sky_to_pixel(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        let [xi, eta] = project(self.crval[0], self.crval[1], ra, dec)?;
        let [[a, b], [c, d]] = self.cd;
        let det = a * d - b * c;
        Some((
            self.crpix[0] + (d * xi - b * eta) / det,
            self.crpix[1] + (-c * xi + a * eta) / det,
        ))
    }

    fn det(&self) -> f64 {
        self.cd[0][0] * self.cd[1][1] - self.cd[0][1] * self.cd[1][0]
    }

    pub fn arcsec_per_px(&self) -> f64 {
        self.det().abs().sqrt() * 3600.0
    }

    pub fn mirrored(&self) -> bool {
        // North up and east left (ξ along -x, η along -y) has a positive determinant.
        self.det() < 0.0
    }

    pub fn north_clockwise_from_up(&self) -> f64 {
        let [[a, b], _] = self.cd;
        let sign = self.det().signum();
        let (north_x, north_y) = (-b * sign, a * sign);
        north_x.atan2(-north_y).to_degrees().rem_euclid(360.0)
    }

    /// SIP distortion terms are ignored.
    pub fn from_header(img: &Image) -> Option<Self> {
        if !img.get("CTYPE1")?.contains("TAN") {
            return None;
        }
        let f = |k: &str| img.get_f64(k);
        let crval = [f("CRVAL1")?, f("CRVAL2")?];
        let crpix = [f("CRPIX1")? - 1.0, f("CRPIX2")? - 1.0];
        let cd = if let Some(cd11) = f("CD1_1") {
            [
                [cd11, f("CD1_2").unwrap_or(0.0)],
                [f("CD2_1").unwrap_or(0.0), f("CD2_2")?],
            ]
        } else {
            let (c1, c2) = (f("CDELT1")?, f("CDELT2")?);
            let pc = match f("PC1_1") {
                Some(p11) => [
                    [p11, f("PC1_2").unwrap_or(0.0)],
                    [f("PC2_1").unwrap_or(0.0), f("PC2_2").unwrap_or(1.0)],
                ],
                None => {
                    let (s, c) = f("CROTA2").unwrap_or(0.0).to_radians().sin_cos();
                    [[c, -s * c2 / c1], [s * c1 / c2, c]]
                }
            };
            [
                [c1 * pc[0][0], c1 * pc[0][1]],
                [c2 * pc[1][0], c2 * pc[1][1]],
            ]
        };
        let w = Self { crval, crpix, cd };
        (w.det() != 0.0 && w.det().is_finite()).then_some(w)
    }
}

pub fn parse_sexagesimal(s: &str) -> Option<f64> {
    let s = s.trim();
    let negative = s.starts_with('-');
    let parts: Vec<f64> = s
        .trim_start_matches(['+', '-'])
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (&first, rest) = parts.split_first()?;
    let v =
        first + rest.first().map_or(0.0, |m| m / 60.0) + rest.get(1).map_or(0.0, |s| s / 3600.0);
    Some(if negative { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips() {
        for &(ra0, dec0, ra, dec) in &[
            (83.8, -5.4, 84.5, -4.9),
            (359.5, 30.0, 0.7, 31.0),
            (10.0, 88.0, 190.0, 89.0),
        ] {
            let [xi, eta] = project(ra0, dec0, ra, dec).unwrap();
            let (r, d) = deproject(ra0, dec0, xi, eta);
            assert!(separation(r, d, ra, dec) < 1e-9, "{ra} {dec} → {r} {d}");
        }
    }

    #[test]
    fn north_up_east_left_round_trips() {
        let s = 1.0 / 3600.0;
        let wcs = Wcs {
            crval: [83.8, -5.4],
            crpix: [3000.0, 2000.0],
            cd: [[-s, 0.0], [0.0, -s]],
        };
        let (ra, dec) = wcs.pixel_to_sky(100.0, 4000.0);
        let (x, y) = wcs.sky_to_pixel(ra, dec).unwrap();
        assert!((x - 100.0).abs() < 1e-6 && (y - 4000.0).abs() < 1e-6);
        assert!((wcs.arcsec_per_px() - 1.0).abs() < 1e-9);
        let north = wcs.north_clockwise_from_up();
        assert!(north.abs() < 1e-9 || (north - 360.0).abs() < 1e-9);
        assert!(!wcs.mirrored());
    }

    #[test]
    fn parses_sexagesimal() {
        assert_eq!(parse_sexagesimal("05 30 00"), Some(5.5));
        assert_eq!(parse_sexagesimal("-05:30:00"), Some(-5.5));
        assert_eq!(
            parse_sexagesimal("+68 10 05").map(|v| (v * 1e4).round()),
            Some(681681.0)
        );
        assert_eq!(parse_sexagesimal("05h35m"), Some(5.0 + 35.0 / 60.0));
        assert_eq!(parse_sexagesimal("83.25"), Some(83.25));
    }
}
