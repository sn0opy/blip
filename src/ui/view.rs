use super::{App, CacheEntry, MeasuredStars, STATUS_FADE, draw, file_name};
use blip::process::StretchParams;
use blip::stars::StarStats;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::path::Path;

/// On/off period of the clipped-pixel flash, in seconds.
const FLASH_PERIOD: f64 = 0.8;

impl App {
    pub(super) fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Open…")
                .on_hover_text("Open folder (O)")
                .clicked()
            {
                self.pick_folder();
            }
            if ui
                .add_enabled(!self.files.is_empty(), egui::Button::new("Export…"))
                .on_hover_text("Export stretched image as PNG/JPEG at full resolution (E)")
                .clicked()
            {
                self.export(ui.ctx());
            }
            let label = self
                .dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "No folder".into());
            ui.add(egui::Label::new(egui::RichText::new(label).weak()).truncate());
        });
        ui.separator();

        let row_h = ui.spacing().interact_size.y;
        let spacing = ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.scroll_to_selected {
            let id = ui.make_persistent_id("file_list_scroll");
            let offset = egui::scroll_area::State::load(ui.ctx(), id).map_or(0.0, |s| s.offset.y);
            let view_h = ui.available_height();
            let top = self.selected as f32 * (row_h + spacing);
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if top + row_h > offset + view_h {
                area = area.vertical_scroll_offset(top + row_h - view_h + spacing);
            }
            self.scroll_to_selected = false;
        }
        let mut clicked = None;
        area.id_salt("file_list_scroll")
            .show_rows(ui, row_h, self.files.len(), |ui, range| {
                let text_width = ui.available_width() - 2.0 * ui.spacing().button_padding.x;
                for i in range {
                    let path = &self.files[i];
                    let name = file_name(path);
                    let name = if self.show_name_end {
                        elide_start(ui, &name, text_width)
                    } else {
                        name
                    };
                    let mut text = egui::RichText::new(name);
                    if self.errors.contains_key(path) {
                        text = text.color(Color32::from_rgb(220, 90, 90));
                    }
                    let button =
                        egui::Button::selectable(i == self.selected, (text, egui::Atom::grow()))
                            .truncate()
                            .min_size(Vec2::new(ui.available_width(), row_h));
                    if ui.add(button).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        if let Some(i) = clicked {
            self.select(i);
        }
    }

    pub(super) fn image_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("image"), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);

        let current = self.current().cloned();
        if let Some(cur) = &current
            && self.cache.contains_key(cur)
        {
            self.shown = Some(cur.clone());
            self.stale = None;
        }
        self.zoom_and_pan(ui, &response, rect);

        let debug_general = self.show_debug.then(|| self.debug_general());
        let debug_pos = rect.right_top() + Vec2::new(-44.0, 8.0);
        let mut debug_drawn = false;

        let shown = self.shown.as_ref().and_then(|p| {
            let stale = self
                .stale
                .as_mut()
                .filter(|(sp, _)| sp == p)
                .map(|(_, e)| e);
            self.cache.get_mut(p).or(stale).map(|e| (p, e))
        });
        if let Some((path, entry)) = shown {
            self.tick += 1;
            entry.last_used = self.tick;
            let loaded = &entry.loaded;
            let size = Vec2::new(loaded.size[0] as f32, loaded.size[1] as f32);
            let fit = (rect.width() / size.x).min(rect.height() / size.y);
            let img_rect = Rect::from_center_size(rect.center() + self.pan, size * fit * self.zoom);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(loaded.texture.id(), img_rect, uv, Color32::WHITE);

            let is_current = current.as_ref() == Some(path);
            if is_current
                && self.flash_clip.as_ref() == Some(path)
                && let Some(overlay) = &loaded.clip_overlay
            {
                let phase = (ui.input(|i| i.time) / FLASH_PERIOD).fract();
                let next_edge = if phase < 0.5 {
                    0.5 - phase
                } else {
                    1.0 - phase
                };
                ui.ctx()
                    .request_repaint_after_secs((next_edge * FLASH_PERIOD) as f32 + 0.001);
                if phase < 0.5 {
                    painter.image(overlay.texture.id(), img_rect, overlay.uv, Color32::WHITE);
                }
            }

            let stars = self.stars.get(path);
            if is_current
                && self.star_view.as_ref() == Some(path)
                && let Some(st) = stars
            {
                draw::stars(&painter, rect, img_rect, loaded.src_size, &st.stats);
            }

            if self.show_info {
                let text = info_text(path, entry, stars.map(|s| &s.stats));
                draw::text_box(&painter, rect.left_top() + Vec2::new(10.0, 8.0), &text, 1.0);
            }
            if let Some(general) = &debug_general {
                let text = format!("{}\n{general}", debug_image(entry, stars, self.params));
                draw::text_box_aligned(&painter, debug_pos, egui::Align2::RIGHT_TOP, &text, 1.0);
                debug_drawn = true;
            }
        } else if self.files.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Open a folder (O) or drop a folder / FITS / XISF file here",
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
        }

        if let Some(general) = &debug_general
            && !debug_drawn
        {
            draw::text_box_aligned(&painter, debug_pos, egui::Align2::RIGHT_TOP, general, 1.0);
        }

        if let Some(cur) = &current {
            if let Some(err) = self.errors.get(cur) {
                let text = format!("{}: {err}", file_name(cur));
                draw::text_box(
                    &painter,
                    rect.left_bottom() + Vec2::new(10.0, -48.0),
                    &text,
                    1.0,
                );
            } else if !self.cache.contains_key(cur) {
                let spinner_rect = Rect::from_min_size(
                    rect.right_top() + Vec2::new(-34.0, 10.0),
                    Vec2::splat(22.0),
                );
                ui.put(spinner_rect, egui::Spinner::new().size(22.0));
            }
        }
        self.status_line(ui, &painter, rect);
    }

    /// Mouse wheel zooms around the cursor, dragging pans, double-click resets.
    fn zoom_and_pan(&mut self, ui: &egui::Ui, response: &egui::Response, rect: Rect) {
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
            if scroll != 0.0 {
                let new_zoom = (self.zoom * (scroll / 200.0).exp()).clamp(1.0, 32.0);
                if let Some(pointer) = response.hover_pos() {
                    let center = rect.center() + self.pan;
                    self.pan += (pointer - center) * (1.0 - new_zoom / self.zoom);
                }
                self.zoom = new_zoom;
            }
        }
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.double_clicked() {
            self.zoom = 1.0;
        }
        if self.zoom <= 1.0 {
            self.pan = Vec2::ZERO;
        }
    }

    /// Debug lines that don't depend on the shown image.
    fn debug_general(&self) -> String {
        let gpu_bytes: usize = self
            .cache
            .values()
            .chain(self.stale.as_ref().map(|(_, e)| e))
            .map(|e| e.loaded.gpu_bytes())
            .sum();
        format!(
            "cache {}/{} images · {} MB on GPU · {} star measurements\nGPU {}",
            self.cache.len(),
            self.cache_limit(),
            gpu_bytes / (1 << 20),
            self.stars.len(),
            self.gpu_info
        )
    }

    fn status_line(&mut self, ui: &egui::Ui, painter: &egui::Painter, rect: Rect) {
        let Some(status) = &self.status else { return };
        let age = status.since.elapsed().as_secs_f32();
        let alpha = match status.ttl {
            Some(ttl) if age >= ttl => 1.0 - ((age - ttl) / STATUS_FADE).min(1.0),
            _ => 1.0,
        };
        if alpha <= 0.0 {
            self.status = None;
            return;
        }
        let pos = rect.left_bottom() + Vec2::new(10.0, -26.0);
        draw::text_box(painter, pos, &status.text, alpha);
        match status.ttl {
            Some(ttl) if age < ttl => ui.ctx().request_repaint_after_secs(ttl - age),
            Some(_) => ui.ctx().request_repaint(),
            None => {}
        }
    }
}

/// Shorten `name` from the front ("…_0001.fit") so it fits into `max_width`.
fn elide_start(ui: &egui::Ui, name: &str, max_width: f32) -> String {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let fits = |s: &str| {
        let galley = ui
            .painter()
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE);
        galley.size().x <= max_width
    };
    if fits(name) {
        return name.to_owned();
    }
    let chars: Vec<char> = name.chars().collect();
    let with_ellipsis = |skip: usize| -> String {
        std::iter::once('…')
            .chain(chars[skip..].iter().copied())
            .collect()
    };
    // Smallest number of leading characters to drop.
    let (mut lo, mut hi) = (1, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(&with_ellipsis(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    with_ellipsis(lo)
}

/// Debug lines for the shown image: geometry, timings, stretch, saturation level.
fn debug_image(entry: &CacheEntry, stars: Option<&MeasuredStars>, params: StretchParams) -> String {
    let l = &entry.loaded;
    let mode = if l.bin == 1 {
        "full res".to_string()
    } else {
        format!("bin {}", l.bin)
    };
    let pattern = l
        .bayer
        .map(|b| format!(" {b:?}").to_uppercase())
        .unwrap_or_default();
    let star_ms = stars.map_or("…".to_string(), |s| format!("{:.0} ms", s.ms));
    let channels: &[&str] = if l.stretch.medians.len() == 3 {
        &["R", "G", "B"]
    } else {
        &["L"]
    };
    let per_channel = |values: &[f64], fmt: &dyn Fn(f64) -> String| {
        channels
            .iter()
            .zip(values)
            .map(|(c, v)| format!("{c} {}", fmt(*v)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let s = &l.stretch;
    [
        format!(
            "sensor {}×{}{pattern} → texture {}×{} ({mode})",
            l.src_size[0], l.src_size[1], l.size[0], l.size[1]
        ),
        format!(
            "read {:.0} ms · process {:.0} ms · texture {:.0} ms · stars {star_ms}",
            l.read_ms, l.process_ms, l.texture_ms
        ),
        format!(
            "background {} ADU · gain {}",
            per_channel(&s.medians, &|v| format!("{:.0}", v * 65535.0)),
            per_channel(&s.gains, &|v| format!("{v:.2}"))
        ),
        format!(
            "stretch shadows {:.4} · midtones {:.4} · target {} · neutralize {}",
            s.shadows,
            s.midtones,
            params.target_bg,
            if params.neutralize { "on" } else { "off" }
        ),
        format!("saturation level {} ADU", l.clip.bright_level),
    ]
    .join("\n")
}

fn info_text(path: &Path, entry: &CacheEntry, stars: Option<&StarStats>) -> String {
    let l = &entry.loaded;
    let mut lines = vec![file_name(path)];
    if !l.info.is_empty() {
        lines.push(l.info.clone());
    }
    let c = &l.clip;
    if c.total > 0 {
        let pct = |n: u64| n as f64 * 100.0 / c.total as f64;
        lines.push(format!(
            "clipped: dark {} ({:.3}%) · bright {} ({:.3}%)",
            draw::thousands(c.dark),
            pct(c.dark),
            draw::thousands(c.bright),
            pct(c.bright)
        ));
    }
    match stars {
        Some(st) if !st.stars.is_empty() => lines.push(format!(
            "stars: {} · HFR median {:.2} px · mean {:.2} px",
            draw::thousands(st.stars.len() as u64),
            st.median_hfr,
            st.mean_hfr
        )),
        Some(_) => lines.push("stars: none found".into()),
        None => lines.push("stars: measuring…".into()),
    }
    lines.join("\n")
}
