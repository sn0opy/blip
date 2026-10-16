//! The clipped-pixel overlay: clip points rasterized onto a grid of at most
//! `MAX_SIDE` cells per side, one colored texel per clipped cell (red = saturated,
//! blue = black).

use crate::process::{CLIP_BRIGHT, Rgba8};
use rayon::prelude::*;

/// Long side limit of the overlay. Markers cover a whole cell of image pixels when
/// the image is larger, which keeps the overlay's memory small next to the image's.
pub const MAX_SIDE: usize = 2048;
pub const CLIP_BRIGHT_COLOR: Rgba8 = Rgba8([255, 40, 40, 255]);
pub const CLIP_DARK_COLOR: Rgba8 = Rgba8([40, 140, 255, 255]);

/// The strongest clip kind (`0`, `CLIP_DARK` or `CLIP_BRIGHT`) per cell; each cell
/// covers `cell`×`cell` image pixels.
pub struct ClipMap {
    pub width: usize,
    pub height: usize,
    pub cell: usize,
    pub kinds: Vec<u8>,
}

/// Rasterize clip points (coordinates in a `width`×`height` image). `None` if
/// nothing is clipped.
pub fn rasterize(points: &[(u32, u32, u8)], width: usize, height: usize) -> Option<ClipMap> {
    if points.is_empty() {
        return None;
    }
    let cell = width.max(height).div_ceil(MAX_SIDE);
    let (w, h) = (width.div_ceil(cell), height.div_ceil(cell));
    let mut kinds = vec![0u8; w * h];
    for &(x, y, kind) in points {
        let k = &mut kinds[y as usize / cell * w + x as usize / cell];
        *k = (*k).max(kind);
    }
    Some(ClipMap {
        width: w,
        height: h,
        cell,
        kinds,
    })
}

/// Grow marks by one texel in every direction (stronger kind wins), so they stay
/// visible when drawn downscaled.
pub fn dilate(kinds: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    *o = (*o).max(kinds[ny * w + nx]);
                }
            }
        }
    });
    out
}

/// Transparent RGBA with the clip colors.
pub fn colorize(kinds: &[u8]) -> Vec<Rgba8> {
    kinds
        .par_iter()
        .map(|&k| match k {
            0 => Rgba8::default(),
            CLIP_BRIGHT => CLIP_BRIGHT_COLOR,
            _ => CLIP_DARK_COLOR,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::CLIP_DARK;

    #[test]
    fn rasterize_marks_cells() {
        assert!(rasterize(&[], 100, 100).is_none());
        let m = rasterize(&[(10, 10, CLIP_DARK), (60, 60, CLIP_BRIGHT)], 64, 64).unwrap();
        assert_eq!((m.width, m.height, m.cell), (64, 64, 1));
        assert_eq!(m.kinds[10 * 64 + 10], CLIP_DARK);
        assert_eq!(m.kinds[10 * 64 + 11], 0);

        // Large images share cells; saturated wins within one.
        let m = rasterize(&[(4998, 0, CLIP_DARK), (4999, 0, CLIP_BRIGHT)], 5000, 10).unwrap();
        assert_eq!((m.width, m.height, m.cell), (1667, 4, 3));
        assert_eq!(m.kinds[1666], CLIP_BRIGHT);
    }

    #[test]
    fn dilate_grows_marks_and_keeps_the_stronger_kind() {
        let mut kinds = vec![0; 25];
        kinds[5 + 1] = CLIP_DARK;
        kinds[5 + 3] = CLIP_BRIGHT;
        let grown = dilate(&kinds, 5, 5);
        assert_eq!(grown[5 + 2], CLIP_BRIGHT);
        assert_eq!(grown[4 * 5], 0);
        let first_row = [CLIP_DARK_COLOR, CLIP_DARK_COLOR, CLIP_BRIGHT_COLOR];
        assert_eq!(colorize(&grown[..3]), first_row);
    }
}
