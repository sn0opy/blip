//! A 3x3 grid over the image with star statistics per cell, for comparing the
//! corners, edges and center (tilt, off-axis aberrations).

use crate::stars::Star;

pub const SIDE: usize = 3;
/// Fewer stars than this in a cell make its median too noisy to compare.
const MIN_STARS: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub stars: usize,
    /// Median HFR (sensor pixels); `None` with fewer than `MIN_STARS` stars.
    pub median_hfr: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Row-major, top left first.
    pub cells: [Cell; SIDE * SIDE],
}

impl Grid {
    pub fn measure(stars: &[Star], width: usize, height: usize) -> Self {
        let mut hfrs: [Vec<f32>; SIDE * SIDE] = Default::default();
        for s in stars {
            let (col, row) = cell_of(s.x, s.y, width, height);
            hfrs[row * SIDE + col].push(s.hfr);
        }
        Self {
            cells: hfrs.map(|mut h| Cell {
                stars: h.len(),
                median_hfr: (h.len() >= MIN_STARS).then(|| {
                    let mid = h.len() / 2;
                    *h.select_nth_unstable_by(mid, f32::total_cmp).1
                }),
            }),
        }
    }

    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        &self.cells[row * SIDE + col]
    }
}

/// The (column, row) of the cell containing image position (x, y).
pub fn cell_of(x: f32, y: f32, width: usize, height: usize) -> (usize, usize) {
    let index = |v: f32, len: usize| ((v * SIDE as f32 / len as f32) as usize).min(SIDE - 1);
    (index(x, width), index(y, height))
}

/// Pixel bounds `[x0, y0, x1, y1)` of a cell; together the cells tile the image.
pub fn cell_rect(col: usize, row: usize, width: usize, height: usize) -> [usize; 4] {
    let start = |i: usize, len: usize| (i * len).div_ceil(SIDE);
    [
        start(col, width),
        start(row, height),
        start(col + 1, width),
        start(row + 1, height),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_count_stars_and_need_enough_of_them() {
        let star = |x, y, hfr| Star { x, y, hfr };
        let mut stars: Vec<Star> = (0..5).map(|i| star(10.0, 10.0, i as f32)).collect();
        stars.extend((0..4).map(|_| star(150.0, 199.9, 1.0)));
        stars.push(star(299.9, 0.0, 1.0));
        let g = Grid::measure(&stars, 300, 200);
        assert_eq!(g.cell(0, 0).median_hfr, Some(2.0));
        assert_eq!(g.cell(1, 2).stars, 4);
        assert_eq!(g.cell(1, 2).median_hfr, None);
        assert_eq!(g.cell(2, 0).stars, 1);
        assert_eq!(g.cells.iter().map(|c| c.stars).sum::<usize>(), stars.len());
    }

    #[test]
    fn cell_rects_match_cell_of() {
        for (w, h) in [(300, 200), (625, 418), (7, 5)] {
            for y in 0..h {
                for x in 0..w {
                    let (col, row) = cell_of(x as f32, y as f32, w, h);
                    let [x0, y0, x1, y1] = cell_rect(col, row, w, h);
                    assert!(
                        (x0..x1).contains(&x) && (y0..y1).contains(&y),
                        "{w}x{h} ({x}, {y})"
                    );
                }
            }
            assert_eq!(cell_rect(2, 2, w, h)[2..], [w, h]);
        }
    }
}
