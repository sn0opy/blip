//! Painting helpers for overlays on top of the image. (Clipped pixels are a texture,
//! see `texture::clip_overlay`.)

use blip_core::stars::StarStats;
use blip_core::tilt::Tilt;
use blip_core::wcs::Wcs;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::cell::RefCell;

/// Circle every measured star and label it with its HFR.
pub fn stars(
    painter: &egui::Painter,
    clip: Rect,
    img_rect: Rect,
    src_size: [usize; 2],
    st: &StarStats,
) {
    let scale = img_rect.width() / src_size[0] as f32;
    let green = Color32::from_rgb(70, 230, 70);
    let font = egui::FontId::proportional(11.0);
    for star in &st.stars {
        let p = img_rect.min + Vec2::new(star.x + 0.5, star.y + 0.5) * scale;
        if !clip.expand(20.0).contains(p) {
            continue;
        }
        let r = (star.hfr * 2.0 * scale).max(5.0);
        painter.circle_stroke(p, r, egui::Stroke::new(1.0, green));
        painter.text(
            p - Vec2::new(0.0, r + 1.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.2}", star.hfr),
            font.clone(),
            green,
        );
    }
}

/// The tilt polygon with its diagonals, labeled with the corner and center HFRs.
pub fn tilt(painter: &egui::Painter, img_rect: Rect, src_size: [usize; 2], tilt: &Tilt) {
    let scale = img_rect.width() / src_size[0] as f32;
    let green = Color32::from_rgb(70, 230, 70);
    let stroke = egui::Stroke::new(1.0, green);
    let font = egui::FontId::proportional(13.0);
    let label = |pos: Pos2, align: egui::Align2, hfr: f32| {
        let text = format!("{hfr:.2}");
        let shadow = Color32::from_black_alpha(200);
        painter.text(pos + Vec2::splat(1.0), align, &text, font.clone(), shadow);
        painter.text(pos, align, &text, font.clone(), green);
    };

    let corners = (tilt.polygon(src_size[0], src_size[1]))
        .map(|(x, y)| img_rect.min + Vec2::new(x, y) * scale);
    painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
    painter.line_segment([corners[0], corners[2]], stroke);
    painter.line_segment([corners[1], corners[3]], stroke);
    // Each label goes on the outer side of its corner.
    let away = |d: f32| {
        if d < 0.0 {
            egui::Align::Max
        } else {
            egui::Align::Min
        }
    };
    for (p, hfr) in corners.into_iter().zip(tilt.corners) {
        let d = p - img_rect.center();
        let out = Vec2::new(d.x.signum(), d.y.signum());
        label(p + out * 4.0, egui::Align2([away(d.x), away(d.y)]), hfr);
    }
    let center = img_rect.center() + Vec2::new(8.0, 0.0);
    label(center, egui::Align2::LEFT_CENTER, tilt.center);
}

/// Monospace text on a translucent dark box, with its top-left corner at `pos`.
pub fn text_box(painter: &egui::Painter, pos: Pos2, text: &str, alpha: f32) {
    text_box_aligned(painter, pos, egui::Align2::LEFT_TOP, text, alpha);
}

/// Like [`text_box`], with `align` choosing which corner of the box is at `pos`.
pub fn text_box_aligned(
    painter: &egui::Painter,
    pos: Pos2,
    align: egui::Align2,
    text: &str,
    alpha: f32,
) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(12.0),
        Color32::from_gray(220).gamma_multiply(alpha),
    );
    let pos = align.anchor_size(pos, galley.size()).min;
    let bg = Rect::from_min_size(pos, galley.size()).expand(5.0);
    painter.rect_filled(bg, 4.0, Color32::from_black_alpha((160.0 * alpha) as u8));
    painter.galley(pos, galley, Color32::WHITE);
}

/// 1234567 -> "1,234,567"
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// RA / Dec grid, deep-sky objects and a north / east compass.
pub fn sky(painter: &egui::Painter, view: Rect, img_rect: Rect, src_size: [usize; 2], wcs: &Wcs) {
    let sky = SkyPainter {
        painter,
        clipped: painter.with_clip_rect(img_rect.intersect(view)),
        view,
        img_rect,
        screen_per_px: img_rect.width() / src_size[0] as f32,
        size: (src_size[0] as f64, src_size[1] as f64),
        wcs,
        taken_label_space: RefCell::new(Vec::new()),
    };
    let extent = sky.extent();
    sky.compass(&extent);
    sky.deep_sky_objects(&extent);
    sky.grid(&extent);
}

struct SkyPainter<'a> {
    painter: &'a egui::Painter,
    clipped: egui::Painter,
    view: Rect,
    img_rect: Rect,
    screen_per_px: f32,
    size: (f64, f64),
    wcs: &'a Wcs,
    taken_label_space: RefCell<Vec<Rect>>,
}

struct SkyExtent {
    center: (f64, f64),
    dec_range: (f64, f64),
    /// Relative to the center's RA, so it doesn't wrap at 0h.
    ra_offset_range: (f64, f64),
    diagonal_deg: f64,
}

const GRID_SAMPLES: usize = 96;
const DEC_STEPS_ARCMIN: [f64; 10] = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0];
const RA_STEPS_TIME_SECONDS: [f64; 12] = [
    5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
];
const LABEL_FONT_SIZE: f32 = 12.0;
const LABEL_MARGIN: f32 = 4.0;

impl SkyPainter<'_> {
    fn to_screen(&self, (x, y): (f64, f64)) -> Pos2 {
        self.img_rect.min + Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * self.screen_per_px
    }

    fn inside(&self, (x, y): (f64, f64)) -> bool {
        (0.0..self.size.0).contains(&x) && (0.0..self.size.1).contains(&y)
    }

    fn visible_image(&self) -> Rect {
        self.img_rect.intersect(self.view).shrink(LABEL_MARGIN)
    }

    /// Moved inside the visible image; skipped if it doesn't fit or would overlap
    /// another label.
    fn label(&self, anchor: Pos2, text: &str, color: Color32, align: egui::Align2) {
        let font = egui::FontId::proportional(LABEL_FONT_SIZE);
        let galley = self.painter.layout_no_wrap(text.into(), font, color);
        let bounds = self.visible_image();
        let rect = align.anchor_size(anchor, galley.size());
        if rect.width() > bounds.width() || rect.height() > bounds.height() {
            return;
        }
        let shift_into =
            |min: f32, max: f32, lo: f32, hi: f32| (lo - min).max(0.0) + (hi - max).min(0.0);
        let rect = rect.translate(Vec2::new(
            shift_into(rect.min.x, rect.max.x, bounds.min.x, bounds.max.x),
            shift_into(rect.min.y, rect.max.y, bounds.min.y, bounds.max.y),
        ));
        let mut taken = self.taken_label_space.borrow_mut();
        let space = rect.expand(2.0);
        if taken.iter().any(|r| r.intersects(space)) {
            return;
        }
        taken.push(space);
        let shadow = Color32::from_black_alpha(200);
        let shadow_galley = self.painter.layout_no_wrap(
            text.into(),
            egui::FontId::proportional(LABEL_FONT_SIZE),
            shadow,
        );
        (self.painter).galley(rect.min + Vec2::splat(1.0), shadow_galley, shadow);
        self.painter.galley(rect.min, galley, color);
    }

    fn extent(&self) -> SkyExtent {
        let (w, h) = self.size;
        let center = self.wcs.pixel_to_sky(w / 2.0, h / 2.0);
        let mut dec_range = (f64::MAX, f64::MIN);
        let mut ra_offset_range = (f64::MAX, f64::MIN);
        for i in 0..=64 {
            let t = i as f64 / 64.0;
            for (x, y) in [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)] {
                let (ra, dec) = self.wcs.pixel_to_sky(x, y);
                let ra_offset = (ra - center.0 + 540.0).rem_euclid(360.0) - 180.0;
                dec_range = (dec_range.0.min(dec), dec_range.1.max(dec));
                ra_offset_range = (
                    ra_offset_range.0.min(ra_offset),
                    ra_offset_range.1.max(ra_offset),
                );
            }
        }
        for pole in [90.0, -90.0] {
            if self
                .wcs
                .sky_to_pixel(0.0, pole)
                .is_some_and(|p| self.inside(p))
            {
                dec_range = (dec_range.0.min(pole), dec_range.1.max(pole));
                ra_offset_range = (-180.0, 180.0);
            }
        }
        SkyExtent {
            center,
            dec_range,
            ra_offset_range,
            diagonal_deg: (w.hypot(h) * self.wcs.arcsec_per_px() / 3600.0).max(1e-3),
        }
    }

    fn grid(&self, e: &SkyExtent) {
        let (ra_c, dec_c) = e.center;
        let (dec0, dec1) = e.dec_range;
        let (ra0, ra1) = (ra_c + e.ra_offset_range.0, ra_c + e.ra_offset_range.1);
        let lines_across = 5.0;
        let dec_step = smallest_step_at_least(
            e.diagonal_deg / lines_across,
            &DEC_STEPS_ARCMIN.map(|m| m / 60.0),
        );
        let ra_step = if ra1 - ra0 >= 359.0 {
            30.0
        } else {
            let cos_dec = dec_c.to_radians().cos().max(0.05);
            smallest_step_at_least(
                e.diagonal_deg / lines_across / cos_dec,
                &RA_STEPS_TIME_SECONDS.map(|s| s / 240.0),
            )
        };
        let along = |from: f64, to: f64| {
            (0..=GRID_SAMPLES).map(move |i| from + (to - from) * i as f64 / GRID_SAMPLES as f64)
        };
        let mut dec = (dec0 / dec_step).ceil() * dec_step;
        while dec <= dec1 {
            let line = along(ra0, ra1).map(|ra| (ra, dec)).collect();
            self.sky_line(line, &format_dec(dec, dec_step));
            dec += dec_step;
        }
        let mut ra = (ra0 / ra_step).ceil() * ra_step;
        while ra <= ra1 {
            let line = along(dec0, dec1).map(|dec| (ra, dec)).collect();
            self.sky_line(line, &format_ra(ra, ra_step));
            ra += ra_step;
        }
    }

    /// Labeled where it first enters the visible image.
    fn sky_line(&self, points: Vec<(f64, f64)>, text: &str) {
        let color = Color32::from_rgba_unmultiplied(120, 170, 255, 150);
        let visible = self.visible_image();
        let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
        let mut entry = None;
        for (ra, dec) in points {
            match self.wcs.sky_to_pixel(ra, dec) {
                Some(p) => {
                    let screen = self.to_screen(p);
                    if entry.is_none() && visible.contains(screen) {
                        entry = Some(screen);
                    }
                    runs.last_mut().unwrap().push(screen);
                }
                None => runs.push(Vec::new()),
            }
        }
        let stroke = egui::Stroke::new(1.0, color);
        for run in runs.into_iter().filter(|r| r.len() > 1) {
            self.clipped.add(egui::Shape::line(run, stroke));
        }
        if let Some(entry) = entry {
            let beside_line = entry + Vec2::new(3.0, 2.0);
            self.label(beside_line, text, color, egui::Align2::LEFT_TOP);
        }
    }

    /// Unit screen vectors.
    fn north_and_east(&self, ra: f64, dec: f64) -> Option<(Vec2, Vec2)> {
        let step = 1.0 / 60.0;
        let screen = |ra: f64, dec: f64| Some(self.to_screen(self.wcs.sky_to_pixel(ra, dec)?));
        let here = screen(ra, dec)?;
        let north = screen(ra, (dec + step).min(90.0))? - here;
        let east = screen(ra + step / dec.to_radians().cos().max(1e-3), dec)? - here;
        Some((north.normalized(), east.normalized()))
    }

    fn deep_sky_objects(&self, e: &SkyExtent) {
        let gold = Color32::from_rgb(255, 205, 90);
        let stroke = egui::Stroke::new(1.5, gold);
        let screen_per_arcmin = 60.0 / self.wcs.arcsec_per_px() as f32 * self.screen_per_px;
        let marker_radius = 6.0;
        let (ra_c, dec_c) = e.center;
        let mut largest_first = blip_core::dso::overlapping_cone(ra_c, dec_c, e.diagonal_deg / 2.0);
        largest_first.sort_by(|a, b| b.major_arcmin.total_cmp(&a.major_arcmin));
        let visible = self.visible_image();
        for d in largest_first {
            let Some(pixel) = self.wcs.sky_to_pixel(d.ra, d.dec) else {
                continue;
            };
            let Some((north, east)) = self.north_and_east(d.ra, d.dec) else {
                continue;
            };
            let center = self.to_screen(pixel);
            let min_visible_minor = d.major_arcmin * 0.15;
            let semi_major = d.major_arcmin as f32 / 2.0 * screen_per_arcmin;
            let semi_minor = d.minor_arcmin.max(min_visible_minor) as f32 / 2.0 * screen_per_arcmin;
            let bounds = if semi_major >= marker_radius {
                let (sin, cos) = (d.position_angle as f32).to_radians().sin_cos();
                let major_axis = north * cos + east * sin;
                let minor_axis = east * cos - north * sin;
                let outline: Vec<Pos2> = (0..64)
                    .map(|i| {
                        let t = i as f32 / 64.0 * std::f32::consts::TAU;
                        center
                            + major_axis * semi_major * t.cos()
                            + minor_axis * semi_minor * t.sin()
                    })
                    .collect();
                let bounds = Rect::from_points(&outline);
                self.clipped.add(egui::Shape::closed_line(outline, stroke));
                bounds
            } else {
                self.clipped.circle_stroke(center, marker_radius, stroke);
                Rect::from_center_size(center, Vec2::splat(2.0 * marker_radius))
            };
            if !bounds.intersects(visible) {
                continue;
            }
            let text = if d.common_name.is_empty() {
                d.label.to_string()
            } else {
                format!("{}\n{}", d.label, d.common_name)
            };
            let above_outline = bounds.top().max(visible.top()) - 2.0;
            let anchor = Pos2::new(center.x, above_outline.min(center.y));
            self.label(anchor, &text, gold, egui::Align2::CENTER_BOTTOM);
        }
    }

    fn compass(&self, e: &SkyExtent) {
        let Some((north, east)) = self.north_and_east(e.center.0, e.center.1) else {
            return;
        };
        let origin = self.img_rect.intersect(self.view).right_bottom() - Vec2::splat(60.0);
        let white = Color32::from_gray(230);
        for (direction, name) in [(north, "N"), (east, "E")] {
            (self.painter).arrow(origin, direction * 36.0, egui::Stroke::new(1.5, white));
            self.label(
                origin + direction * 46.0,
                name,
                white,
                egui::Align2::CENTER_CENTER,
            );
        }
    }
}

fn smallest_step_at_least(min: f64, ascending_steps: &[f64]) -> f64 {
    let largest = ascending_steps[ascending_steps.len() - 1];
    (ascending_steps.iter().copied())
        .find(|&s| s >= min)
        .unwrap_or(largest)
}

/// "−5°", "−5°30'" or "−5°30'20\"", depending on the step.
pub fn format_dec(dec: f64, step: f64) -> String {
    let sign = if dec < -1e-9 { "−" } else { "+" };
    let arcsec = (dec.abs() * 3600.0).round() as i64;
    let (d, m, s) = (arcsec / 3600, arcsec / 60 % 60, arcsec % 60);
    if step >= 1.0 {
        format!("{sign}{d}°")
    } else if step * 60.0 >= 1.0 - 1e-9 {
        format!("{sign}{d}°{m:02}'")
    } else {
        format!("{sign}{d}°{m:02}'{s:02}\"")
    }
}

/// "5h", "5h36m" or "5h35m30s", depending on the step.
pub fn format_ra(ra: f64, step: f64) -> String {
    let secs = (ra.rem_euclid(360.0) / 15.0 * 3600.0).round() as i64 % 86_400;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if step >= 15.0 {
        format!("{h}h")
    } else if step * 4.0 >= 1.0 - 1e-9 {
        format!("{h}h{m:02}m")
    } else {
        format!("{h}h{m:02}m{s:02}s")
    }
}
