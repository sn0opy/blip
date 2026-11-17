use blip_core::solve::Solution;
use blip_core::wcs::Wcs;
use eframe::egui::{self, Color32, Pos2, Rect, TextureId, epaint::Vertex};

/// The mapping is nearly affine, so a coarse mesh suffices.
const MESH_CELLS: u32 = 8;

#[derive(Clone, Copy)]
pub struct Alignment {
    reference: Wcs,
    pub reference_size: [usize; 2],
    image: Wcs,
    image_size: [usize; 2],
}

impl Alignment {
    /// `None` unless the image's center lies within the reference.
    pub fn new(reference: &Solution, image: &Solution) -> Option<Self> {
        let alignment = Self {
            reference: reference.wcs,
            reference_size: reference.size,
            image: image.wcs,
            image_size: image.size,
        };
        let [w, h] = image.size.map(|s| s as f64);
        let [rw, rh] = reference.size.map(|s| s as f64);
        let (x, y) = alignment.reference_pixel(w / 2.0, h / 2.0)?;
        ((0.0..rw).contains(&x) && (0.0..rh).contains(&y)).then_some(alignment)
    }

    pub fn reference_wcs(&self) -> &Wcs {
        &self.reference
    }

    pub fn reference_pixel(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (ra, dec) = self.image.pixel_to_sky(x, y);
        self.reference.sky_to_pixel(ra, dec)
    }

    pub fn reference_pixels_per_pixel(&self) -> f64 {
        self.image.arcsec_per_px() / self.reference.arcsec_per_px()
    }

    pub fn screen_pos(&self, reference_rect: Rect, x: f64, y: f64) -> Option<Pos2> {
        let (x, y) = self.reference_pixel(x, y)?;
        let [rw, rh] = self.reference_size.map(|s| s as f64);
        Some(Pos2::new(
            reference_rect.min.x + ((x + 0.5) / rw) as f32 * reference_rect.width(),
            reference_rect.min.y + ((y + 0.5) / rh) as f32 * reference_rect.height(),
        ))
    }

    pub fn mesh(&self, texture: TextureId, uv: Rect, reference_rect: Rect) -> Option<egui::Mesh> {
        let [w, h] = self.image_size.map(|s| s as f64);
        let mut mesh = egui::Mesh::with_texture(texture);
        for j in 0..=MESH_CELLS {
            for i in 0..=MESH_CELLS {
                let (u, v) = (i as f64 / MESH_CELLS as f64, j as f64 / MESH_CELLS as f64);
                let pos = self.screen_pos(reference_rect, u * w - 0.5, v * h - 0.5)?;
                mesh.vertices.push(Vertex {
                    pos,
                    uv: uv.lerp_inside(egui::Vec2::new(u as f32, v as f32)),
                    color: Color32::WHITE,
                });
            }
        }
        let at = |i: u32, j: u32| j * (MESH_CELLS + 1) + i;
        for j in 0..MESH_CELLS {
            for i in 0..MESH_CELLS {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
                mesh.indices.extend([a, b, c, b, d, c]);
            }
        }
        Some(mesh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solution(crval: [f64; 2], crpix: [f64; 2], cd: [[f64; 2]; 2]) -> Solution {
        Solution {
            wcs: Wcs { crval, crpix, cd },
            size: [100, 80],
            matched_stars: 0,
            rms_arcsec: 0.0,
            hint_offset_deg: 0.0,
            tiles_tried: 0,
            ms: 0.0,
        }
    }

    #[test]
    fn maps_shifted_and_flipped_images_onto_the_reference() {
        let s = 1e-3;
        let at = [83.0, -5.0];
        let reference = solution(at, [50.0, 40.0], [[-s, 0.0], [0.0, s]]);
        let shifted = solution(at, [47.0, 42.0], [[-s, 0.0], [0.0, s]]);
        let flipped = solution(at, [50.0, 40.0], [[s, 0.0], [0.0, -s]]);
        let close = |(x, y): (f64, f64), (ex, ey): (f64, f64)| {
            assert!((x - ex).abs() < 1e-6 && (y - ey).abs() < 1e-6, "{x},{y}");
        };
        let a = Alignment::new(&reference, &shifted).unwrap();
        close(a.reference_pixel(47.0, 42.0).unwrap(), (50.0, 40.0));
        close(a.reference_pixel(57.0, 42.0).unwrap(), (60.0, 40.0));
        let a = Alignment::new(&reference, &flipped).unwrap();
        close(a.reference_pixel(60.0, 45.0).unwrap(), (40.0, 35.0));
    }

    #[test]
    fn other_targets_are_not_aligned() {
        let cd = [[-1e-3, 0.0], [0.0, 1e-3]];
        let reference = solution([83.0, -5.0], [50.0, 40.0], cd);
        let elsewhere = solution([84.0, -5.0], [50.0, 40.0], cd);
        assert!(Alignment::new(&reference, &elsewhere).is_none());
    }
}
