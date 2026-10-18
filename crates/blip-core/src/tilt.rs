//! Sensor tilt: the median HFR in the corners and the center of the 3x3 grid. One
//! side larger than the opposite one means tilt; all corners larger than the center
//! means field curvature.

use crate::grid::Grid;
use crate::stars::Star;

/// Grid cells (column, row) of the corners, clockwise from top left.
pub const CORNER_CELLS: [(usize, usize); 4] = [(0, 0), (2, 0), (2, 2), (0, 2)];
/// Distance of the drawn corners from the center, as a fraction of the image size.
const EXTENT: f32 = 0.25;
/// Exaggerates the corner HFR differences in the drawing.
const GAIN: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tilt {
    pub center: f32,
    /// In `CORNER_CELLS` order.
    pub corners: [f32; 4],
}

impl Tilt {
    /// `None` unless the corners and the center have enough stars.
    pub fn measure(stars: &[Star], width: usize, height: usize) -> Option<Self> {
        Self::from_grid(&Grid::measure(stars, width, height))
    }

    pub fn from_grid(grid: &Grid) -> Option<Self> {
        let hfr = |(col, row)| grid.cell(col, row).median_hfr;
        Some(Self {
            center: hfr((1, 1))?,
            corners: [
                hfr(CORNER_CELLS[0])?,
                hfr(CORNER_CELLS[1])?,
                hfr(CORNER_CELLS[2])?,
                hfr(CORNER_CELLS[3])?,
            ],
        })
    }

    /// Largest minus smallest corner HFR.
    pub fn spread(&self) -> f32 {
        let max = self.corners.into_iter().fold(f32::MIN, f32::max);
        let min = self.corners.into_iter().fold(f32::MAX, f32::min);
        max - min
    }

    pub fn corner_mean(&self) -> f32 {
        self.corners.iter().sum::<f32>() / 4.0
    }

    pub fn percent(&self) -> f32 {
        self.spread() / self.corner_mean() * 100.0
    }

    /// The corners to draw, in image pixels: a rectangle around the center, each
    /// corner pushed out by how much larger than average its HFR is.
    pub fn polygon(&self, width: usize, height: usize) -> [(f32, f32); 4] {
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let (dx, dy) = (width as f32 * EXTENT, height as f32 * EXTENT);
        let mean = self.corner_mean();
        std::array::from_fn(|i| {
            let f = (1.0 + GAIN * (self.corners[i] / mean - 1.0)).clamp(0.5, 1.5);
            let (col, row) = CORNER_CELLS[i];
            let (sx, sy) = (col as f32 - 1.0, row as f32 - 1.0);
            (cx + sx * dx * f, cy + sy * dy * f)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(width: usize, height: usize, hfr: impl Fn(f32, f32) -> f32) -> Vec<Star> {
        let mut stars = Vec::new();
        for y in (5..height).step_by(10) {
            for x in (5..width).step_by(10) {
                let (x, y) = (x as f32, y as f32);
                stars.push(Star {
                    x,
                    y,
                    hfr: hfr(x, y),
                });
            }
        }
        stars
    }

    #[test]
    fn measures_corners_and_center() {
        // HFR grows to the right: tilt about the vertical axis.
        let stars = grid(300, 200, |x, _| 2.0 + x / 300.0);
        let t = Tilt::measure(&stars, 300, 200).unwrap();
        let [tl, tr, br, bl] = t.corners;
        assert!(tr > tl + 0.5 && br > bl + 0.5, "{t:?}");
        assert!((tl - bl).abs() < 0.01 && (tr - br).abs() < 0.01, "{t:?}");
        assert!((t.center - 2.5).abs() < 0.05, "{t:?}");
        assert!((t.spread() - (tr - tl)).abs() < 1e-6);

        // Larger corners are drawn further out.
        let p = t.polygon(300, 200);
        assert!(p[1].0 - 150.0 > 150.0 - p[0].0, "{p:?}");
        assert!(p[2].1 - 100.0 > 100.0 - p[0].1, "{p:?}");
    }

    #[test]
    fn needs_stars_in_every_region() {
        let stars: Vec<Star> = grid(300, 200, |_, _| 2.0)
            .into_iter()
            .filter(|s| !(s.x < 100.0 && s.y < 66.0))
            .collect();
        assert!(Tilt::measure(&stars, 300, 200).is_none());
        assert!(Tilt::measure(&[], 0, 0).is_none());
    }

    #[test]
    fn flat_field_draws_a_rectangle() {
        let t = Tilt::measure(&grid(300, 200, |_, _| 2.0), 300, 200).unwrap();
        assert_eq!(t.spread(), 0.0);
        assert_eq!(
            t.polygon(300, 200),
            [(75.0, 50.0), (225.0, 50.0), (225.0, 150.0), (75.0, 150.0)]
        );
    }
}
