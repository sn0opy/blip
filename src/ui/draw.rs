//! Painting helpers for overlays on top of the image. (Clipped pixels are a texture,
//! see `texture::clip_overlay`.)

use blip_core::stars::StarStats;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

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
