//! Textures built and uploaded on the loader threads.
//!
//! With wgpu (the normal case) a texture is created, filled with its full mip chain
//! and registered with egui-wgpu as a user texture without involving the UI thread,
//! so showing a new image never stalls a frame on a large upload. egui's own textures
//! have a single mip level; the mip chain is what keeps downscaled views smooth
//! instead of aliased. Without wgpu render state, textures go through egui's texture
//! manager instead (base level only, copied to the GPU by the UI thread).

use blip_core::overlay::{self, ClipMap};
use blip_core::process::Rgba8;
use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions, mutex::RwLock};
use eframe::egui_wgpu::{RenderState, Renderer};
use eframe::wgpu;
use rayon::prelude::*;
use std::sync::{Arc, OnceLock};

/// One mip level, RGBA in egui's gamma-encoded (sRGB) space.
pub struct Level {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba8>,
}

#[derive(Clone, Copy)]
pub enum Filter {
    /// Bilinear between texels and between mip levels, for the image itself.
    Smooth,
    /// Nearest everywhere, for crisp pixel markers.
    Nearest,
}

/// Where textures go: our own wgpu textures, or egui's texture manager.
#[derive(Clone)]
pub enum Uploader {
    Wgpu {
        device: wgpu::Device,
        queue: wgpu::Queue,
        renderer: Arc<RwLock<Renderer>>,
    },
    Egui(egui::Context),
}

impl Uploader {
    pub fn new(ctx: &egui::Context, render_state: Option<&RenderState>) -> Self {
        match render_state {
            Some(rs) => Self::Wgpu {
                device: rs.device.clone(),
                queue: rs.queue.clone(),
                renderer: rs.renderer.clone(),
            },
            None => Self::Egui(ctx.clone()),
        }
    }

    pub fn max_texture_side(&self) -> usize {
        match self {
            Self::Wgpu { device, .. } => device.limits().max_texture_dimension_2d as usize,
            Self::Egui(ctx) => ctx.input(|i| i.max_texture_side),
        }
    }

    /// Whether textures get levels beyond the base (egui's textures can't).
    pub fn mipmaps(&self) -> bool {
        matches!(self, Self::Wgpu { .. })
    }

    /// `levels` is the base level followed by its mip chain (egui's textures use the base only).
    pub fn upload(&self, name: &str, levels: Vec<Level>, filter: Filter) -> Texture {
        let size = |l: &Level| l.width * l.height * 4;
        let (bytes, owner) = match self {
            Self::Wgpu {
                device,
                queue,
                renderer,
            } => (
                levels.iter().map(size).sum(),
                Owner::Wgpu(upload_wgpu(device, queue, renderer, name, &levels, filter)),
            ),
            Self::Egui(ctx) => {
                let base = levels.into_iter().next().expect("no texture data");
                let bytes = size(&base);
                let pixels: Vec<egui::Color32> = bytemuck::allocation::cast_vec(base.pixels);
                let options = match filter {
                    Filter::Smooth => TextureOptions::LINEAR,
                    Filter::Nearest => TextureOptions::NEAREST,
                };
                let image = ColorImage::new([base.width, base.height], pixels);
                (bytes, Owner::Egui(ctx.load_texture(name, image, options)))
            }
        };
        Texture { bytes, owner }
    }
}

fn upload_wgpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &Arc<RwLock<Renderer>>,
    name: &str,
    levels: &[Level],
    filter: Filter,
) -> WgpuTexture {
    let extent = |l: &Level| wgpu::Extent3d {
        width: l.width as u32,
        height: l.height as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: extent(&levels[0]),
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // What egui uses for its own textures (gamma-encoded, filtered as stored),
        // so colors match the rest of the UI.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (mip, level) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&level.pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * level.width as u32),
                rows_per_image: Some(level.height as u32),
            },
            extent(level),
        );
    }
    // Start the copy now instead of with the next frame's submission.
    queue.submit(None);

    let (filter_mode, mipmap_filter) = match filter {
        Filter::Smooth => (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear),
        Filter::Nearest => (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest),
    };
    let sampler = wgpu::SamplerDescriptor {
        label: Some(name),
        mag_filter: filter_mode,
        min_filter: filter_mode,
        mipmap_filter,
        ..Default::default()
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let id = renderer
        .write()
        .register_native_texture_with_sampler_options(device, &view, sampler);
    WgpuTexture {
        id,
        renderer: renderer.clone(),
        _texture: texture,
    }
}

/// A texture egui can draw. Freed when dropped.
pub struct Texture {
    bytes: usize,
    owner: Owner,
}

enum Owner {
    Wgpu(WgpuTexture),
    Egui(TextureHandle),
}

/// A texture registered with egui-wgpu; unregistering drops its last reference.
struct WgpuTexture {
    id: TextureId,
    renderer: Arc<RwLock<Renderer>>,
    _texture: wgpu::Texture,
}

impl Drop for WgpuTexture {
    fn drop(&mut self) {
        self.renderer.write().free_texture(&self.id);
    }
}

impl Texture {
    pub fn id(&self) -> TextureId {
        match &self.owner {
            Owner::Wgpu(t) => t.id,
            Owner::Egui(handle) => handle.id(),
        }
    }

    /// GPU memory of all levels.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// `base` followed by its mip chain down to 1×1, box-filtered in linear light so
/// stars keep their brightness when the view is zoomed out.
pub fn smooth_mips(base: Level) -> Vec<Level> {
    let mut levels = vec![base];
    let (to_linear, to_srgb) = (to_linear_table(), to_srgb_table());
    let average = |quad: [Rgba8; 4]| {
        let mut out = [0u8; 4];
        for (c, o) in out[..3].iter_mut().enumerate() {
            let sum: f32 = quad.iter().map(|p| to_linear[p.0[c] as usize]).sum();
            *o = to_srgb[(sum * (65535.0 / 4.0) + 0.5) as usize];
        }
        let alpha: u32 = quad.iter().map(|p| p.0[3] as u32).sum();
        out[3] = ((alpha + 2) / 4) as u8;
        Rgba8(out)
    };
    while let Some(src) = levels.last().filter(|l| l.width > 1 || l.height > 1) {
        let (pixels, width, height) = downsample(&src.pixels, src.width, src.height, average);
        levels.push(Level {
            width,
            height,
            pixels,
        });
    }
    levels
}

/// Halve both sides (down to 1), combining each 2×2 block with `combine`. An odd
/// last row or column is dropped, matching the GPU's mip sizes.
fn downsample<T, F>(src: &[T], width: usize, height: usize, combine: F) -> (Vec<T>, usize, usize)
where
    T: Copy + Default + Send + Sync,
    F: Fn([T; 4]) -> T + Sync + Copy,
{
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = vec![T::default(); w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let r0 = &src[2 * y * width..][..width];
        let r1 = &src[(2 * y + 1).min(height - 1) * width..][..width];
        for (x, o) in row.iter_mut().enumerate() {
            let (x0, x1) = (2 * x, (2 * x + 1).min(width - 1));
            *o = combine([r0[x0], r0[x1], r1[x0], r1[x1]]);
        }
    });
    (out, w, h)
}

/// sRGB-encoded byte to linear light (0..1).
fn to_linear_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let v = i as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

/// Linear light in 1/65535 steps to an sRGB-encoded byte.
fn to_srgb_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=65535u32)
            .map(|i| {
                let v = i as f32 / 65535.0;
                let s = if v <= 0.003_130_8 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0 + 0.5) as u8
            })
            .collect()
    })
}

/// Clipped pixels as a transparent image with a colored texel per clipped cell
/// (red = saturated, blue = black). Each texel covers `cell`×`cell` texture pixels.
pub struct Overlay {
    pub levels: Vec<Level>,
    pub cell: usize,
}

/// Rasterize clip points (texture coordinates of a `width`×`height` image). With
/// `mipmaps`, every level marks a texel if any pixel under it is clipped (saturated
/// wins) and grows marks by one texel, so even single clipped pixels stay visible as
/// small squares when the image is shown downscaled. `None` if nothing is clipped.
pub fn clip_overlay(
    points: &[(u32, u32, u8)],
    width: usize,
    height: usize,
    mipmaps: bool,
) -> Option<Overlay> {
    let ClipMap {
        width: mut w,
        height: mut h,
        cell,
        mut kinds,
    } = overlay::rasterize(points, width, height)?;
    let mut levels = vec![clip_level(&kinds, w, h)];
    while mipmaps && (w > 1 || h > 1) {
        let (next, nw, nh) = downsample(&kinds, w, h, |q| q.into_iter().max().unwrap_or(0));
        (kinds, w, h) = (next, nw, nh);
        levels.push(clip_level(&overlay::dilate(&kinds, w, h), w, h));
    }
    Some(Overlay { levels, cell })
}

fn clip_level(kinds: &[u8], width: usize, height: usize) -> Level {
    Level {
        width,
        height,
        pixels: overlay::colorize(kinds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blip_core::overlay::{CLIP_BRIGHT_COLOR, CLIP_DARK_COLOR};
    use blip_core::process::{CLIP_BRIGHT, CLIP_DARK};

    fn level(width: usize, height: usize, v: u8) -> Level {
        let pixels = vec![Rgba8([v, v, v, 255]); width * height];
        Level {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn mip_chain_matches_gpu_sizes() {
        let sizes: Vec<_> = smooth_mips(level(5, 2, 0))
            .iter()
            .map(|l| (l.width, l.height, l.pixels.len()))
            .collect();
        assert_eq!(sizes, [(5, 2, 10), (2, 1, 2), (1, 1, 1)]);
    }

    #[test]
    fn mips_keep_flat_colors_and_average_in_linear_light() {
        for v in [0, 1, 37, 128, 254, 255] {
            let mips = smooth_mips(level(4, 4, v));
            assert!(
                mips.iter()
                    .all(|l| l.pixels.iter().all(|p| p.0 == [v, v, v, 255]))
            );
        }
        // Black and white average to 50% light, which is sRGB 188, not 128.
        let mut half = level(2, 1, 0);
        half.pixels[1] = Rgba8([255, 255, 255, 255]);
        assert_eq!(smooth_mips(half)[1].pixels[0].0, [188, 188, 188, 255]);
    }

    #[test]
    fn overlay_marks_survive_downscaling() {
        let points = [
            (10, 10, CLIP_DARK),
            (11, 10, CLIP_BRIGHT),
            (60, 60, CLIP_DARK),
        ];
        let o = clip_overlay(&points, 64, 64, true).unwrap();
        assert_eq!(o.levels.len(), 7);
        let at = |l: &Level, x: usize, y: usize| l.pixels[y * l.width + x];
        assert_eq!(at(&o.levels[0], 10, 10), CLIP_DARK_COLOR);
        assert_eq!(at(&o.levels[0], 12, 10), Rgba8::default());
        // Saturated wins when both fall into one texel; marks grow by one texel.
        assert_eq!(at(&o.levels[1], 5, 5), CLIP_BRIGHT_COLOR);
        assert_eq!(at(&o.levels[1], 6, 6), CLIP_BRIGHT_COLOR);
        assert_eq!(at(&o.levels[1], 30, 30), CLIP_DARK_COLOR);
        assert_eq!(at(&o.levels[1], 20, 20), Rgba8::default());
        assert!(o.levels[6].pixels[0] == CLIP_BRIGHT_COLOR);
        assert_eq!(
            clip_overlay(&points, 64, 64, false).unwrap().levels.len(),
            1
        );
    }
}
