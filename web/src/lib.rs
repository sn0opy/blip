//! Experimental WebAssembly build of blip: the JavaScript-facing API over the desktop
//! app's decoding and processing (`crates/blip-core`).

use blip_core::overlay::{self, ClipMap};
use blip_core::process::{self, ClipStats, Rgba8, StretchParams};
use blip_core::{export, image, stars, tilt::Tilt};
use wasm_bindgen::prelude::*;

/// "Full resolution" falls back to binning above this long side, like the desktop app
/// does at the GPU's texture limit. Browsers cap canvas and bitmap sizes around here.
const FULL_RES_MAX_SIDE: usize = 8192;

/// A decoded image, kept on the wasm side so setting changes, star measurement and
/// export don't decode again. Holds the full 16-bit data: free it before decoding the
/// next file.
#[wasm_bindgen]
pub struct Decoded {
    img: image::Image,
    clip: ClipStats,
}

/// Decode a FITS or XISF file.
#[wasm_bindgen]
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let img = image::parse(bytes)?;
    let clip = process::clip_stats(&img);
    Ok(Decoded { img, clip })
}

fn params(neutralize: bool, auto_stretch: bool, boost: u32) -> StretchParams {
    StretchParams {
        neutralize,
        auto_stretch,
        ..Default::default()
    }
    .boosted(boost)
}

#[wasm_bindgen]
impl Decoded {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> usize {
        self.img.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> usize {
        self.img.height
    }

    /// Bayer pattern ("RGGB", …), if any.
    #[wasm_bindgen(getter)]
    pub fn bayer(&self) -> Option<String> {
        self.img.bayer.map(|b| b.to_string())
    }

    /// One-line summary of the capture from the header.
    #[wasm_bindgen(getter)]
    pub fn info(&self) -> String {
        self.img.describe()
    }

    #[wasm_bindgen(getter)]
    pub fn clip(&self) -> Clip {
        let c = &self.clip;
        Clip {
            total: c.total as f64,
            dark: c.dark as f64,
            bright: c.bright as f64,
            level: f64::from(c.bright_level),
        }
    }

    /// Stretched RGBA image binned to a long side of at most `max_side`, or at full
    /// resolution without it, plus its clipped-pixel overlay. `boost` raises the target
    /// background (`StretchParams::boosted`).
    pub fn render(
        &self,
        max_side: Option<usize>,
        neutralize: bool,
        auto_stretch: bool,
        boost: u32,
    ) -> Rendered {
        let params = params(neutralize, auto_stretch, boost);
        let side = process::preview_side(&self.img, max_side, FULL_RES_MAX_SIDE);
        let (r, points) = process::render_preview::<Rgba8>(&self.img, &self.clip, side, params);
        let overlay = overlay::rasterize(&points, r.width, r.height).map(|m| Overlay {
            grown: overlay::colorize(&overlay::dilate(&m.kinds, m.width, m.height)),
            plain: overlay::colorize(&m.kinds),
            map: m,
        });
        Rendered {
            width: r.width,
            height: r.height,
            bin: r.bin,
            pixels: r.pixels,
            stretch: r.stretch,
            target: params.target_bg,
            overlay,
        }
    }

    /// Star centroids and half-flux radii in sensor pixels.
    pub fn stars(&self) -> Stars {
        let s = stars::detect(&self.img, &self.clip);
        let (w, h) = (self.img.width, self.img.height);
        let tilt = Tilt::measure(&s.stars, w, h).map(|t| TiltInfo {
            center: t.center,
            spread: t.spread(),
            percent: t.percent(),
            mean: t.corner_mean(),
            corners: (t.polygon(w, h).into_iter().zip(t.corners))
                .flat_map(|((x, y), hfr)| [x, y, hfr])
                .collect(),
        });
        Stars {
            median_hfr: s.median_hfr,
            mean_hfr: s.mean_hfr,
            list: s.stars.iter().flat_map(|s| [s.x, s.y, s.hfr]).collect(),
            tilt,
        }
    }

    /// The stretched image at full resolution as PNG or JPEG.
    pub fn export(
        &self,
        neutralize: bool,
        auto_stretch: bool,
        boost: u32,
        jpeg: bool,
    ) -> Result<Vec<u8>, String> {
        let params = params(neutralize, auto_stretch, boost);
        let r = process::render_full::<[u8; 3]>(&self.img, params);
        export::encode(r.pixels.as_flattened(), r.width, r.height, jpeg)
    }
}

/// Clipping counts and the saturation level.
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct Clip {
    #[wasm_bindgen(readonly)]
    pub total: f64,
    #[wasm_bindgen(readonly)]
    pub dark: f64,
    #[wasm_bindgen(readonly)]
    pub bright: f64,
    #[wasm_bindgen(readonly)]
    pub level: f64,
}

/// A rendered image. Pixels are read straight from wasm memory (`*_ptr` / `*_len`);
/// the views are only valid until the next call into wasm.
#[wasm_bindgen]
pub struct Rendered {
    #[wasm_bindgen(readonly)]
    pub width: usize,
    #[wasm_bindgen(readonly)]
    pub height: usize,
    #[wasm_bindgen(readonly)]
    pub bin: usize,
    /// Target background brightness of the stretch.
    #[wasm_bindgen(readonly)]
    pub target: f64,
    pixels: Vec<Rgba8>,
    stretch: process::StretchInfo,
    overlay: Option<Overlay>,
}

#[wasm_bindgen]
impl Rendered {
    pub fn pixels_ptr(&self) -> *const u8 {
        self.pixels.as_ptr().cast()
    }

    pub fn pixels_len(&self) -> usize {
        4 * self.pixels.len()
    }

    /// Per-channel background median (0..1), before white balance.
    #[wasm_bindgen(getter)]
    pub fn medians(&self) -> Vec<f64> {
        self.stretch.medians.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn gains(&self) -> Vec<f64> {
        self.stretch.gains.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn shadows(&self) -> f64 {
        self.stretch.shadows
    }

    #[wasm_bindgen(getter)]
    pub fn midtones(&self) -> f64 {
        self.stretch.midtones
    }

    /// The clipped-pixel overlay, `None` if nothing is clipped.
    pub fn take_overlay(&mut self) -> Option<Overlay> {
        self.overlay.take()
    }
}

/// Clipped pixels as transparent RGBA: plain marks for zoomed-in views, and marks grown
/// by one texel for downscaled ones. One texel covers `cell`×`cell` image pixels.
#[wasm_bindgen]
pub struct Overlay {
    map: ClipMap,
    plain: Vec<Rgba8>,
    grown: Vec<Rgba8>,
}

#[wasm_bindgen]
impl Overlay {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> usize {
        self.map.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> usize {
        self.map.height
    }

    #[wasm_bindgen(getter)]
    pub fn cell(&self) -> usize {
        self.map.cell
    }

    pub fn pixels_ptr(&self, grown: bool) -> *const u8 {
        let pixels = if grown { &self.grown } else { &self.plain };
        pixels.as_ptr().cast()
    }

    pub fn pixels_len(&self) -> usize {
        4 * self.plain.len()
    }
}

#[wasm_bindgen]
pub struct Stars {
    #[wasm_bindgen(readonly)]
    pub median_hfr: f32,
    #[wasm_bindgen(readonly)]
    pub mean_hfr: f32,
    list: Vec<f32>,
    tilt: Option<TiltInfo>,
}

#[wasm_bindgen]
impl Stars {
    /// `[x, y, hfr, x, y, hfr, …]`.
    #[wasm_bindgen(getter)]
    pub fn list(&self) -> Vec<f32> {
        self.list.clone()
    }

    /// `undefined` with too few stars.
    #[wasm_bindgen(getter)]
    pub fn tilt(&self) -> Option<TiltInfo> {
        self.tilt.clone()
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct TiltInfo {
    #[wasm_bindgen(readonly)]
    pub center: f32,
    #[wasm_bindgen(readonly)]
    pub spread: f32,
    #[wasm_bindgen(readonly)]
    pub percent: f32,
    #[wasm_bindgen(readonly)]
    pub mean: f32,
    corners: Vec<f32>,
}

#[wasm_bindgen]
impl TiltInfo {
    /// The drawn corners as `[x, y, hfr, …]`, clockwise from top left.
    #[wasm_bindgen(getter)]
    pub fn corners(&self) -> Vec<f32> {
        self.corners.clone()
    }
}
