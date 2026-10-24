use super::gpu::MemoryBudget;
use super::loader::Work;
use super::tree::Row;
use super::{App, CacheEntry, MeasuredStars, STATUS_FADE, draw, file_name};
use blip_core::process::StretchParams;
use eframe::egui::{self, AtomExt as _, Color32, Pos2, Rect, Vec2};
use std::path::{Path, PathBuf};

/// On/off period of the clipped-pixel flash, in seconds.
const FLASH_PERIOD: f64 = 0.8;
/// Space between the mosaic panels, in points.
const MOSAIC_GAP: f32 = 3.0;

impl App {
    pub(super) fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
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
                self.export_current(ui.ctx());
            }
            // Pushed right in the same row (not right-to-left), so the panel can't get
            // narrower than all three buttons.
            let gear_side = ui.spacing().interact_size.y;
            ui.add_space((ui.available_width() - gear_side).max(0.0));
            if gear_button(ui).on_hover_text("Settings (Ctrl+,)").clicked() {
                self.show_settings = true;
            }
        });
        let label = self
            .dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "No folder".into());
        ui.add(egui::Label::new(egui::RichText::new(label).weak()).truncate());
        ui.separator();

        if self.scroll_to_selected
            && let Some(current) = self.files.get(self.selected)
        {
            self.tree.reveal(current);
        }
        let rows = self.tree.visible_rows();
        let row_h = ui.spacing().interact_size.y;
        let spacing = ui.spacing().item_spacing.y;
        let scroll_salt = "file_list_scroll";
        let mut area = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(scroll_salt);
        if self.scroll_to_selected {
            let id = ui.make_persistent_id(egui::IdSalt::new(scroll_salt));
            let offset =
                egui::scroll_area::State::load(ui.ctx(), id).map_or(0.0, |s| s.clamped_offset().y);
            let view_h = ui.available_height();
            let selected_row = (rows.iter())
                .position(|row| matches!(row, Row::Image { index, .. } if *index == self.selected))
                .unwrap_or(0);
            let top = selected_row as f32 * (row_h + spacing);
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if top + row_h > offset + view_h {
                area = area.vertical_scroll_offset(top + row_h - view_h + spacing);
            }
            self.scroll_to_selected = false;
        }
        let mut clicked = None;
        let mut menu_file = None;
        let mut menu_action = None;
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for row in &rows[range] {
                let button = self
                    .tree_row_button(ui, row)
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .min_size(Vec2::new(ui.available_width(), row_h));
                let response = ui.add(button);
                if response.clicked() {
                    clicked = Some((*row).clone());
                }
                if let Row::Image { index, .. } = row {
                    let path = &self.files[*index];
                    response.context_menu(|ui| {
                        menu_action = self.file_menu(ui, path);
                        menu_file = Some(path.clone());
                    });
                }
            }
        });
        self.handle_file_menu(ui.ctx(), menu_file, menu_action);
        match clicked {
            Some(Row::Image { index, .. }) => self.select(index),
            Some(Row::Folder { path, .. }) => self.tree.toggle(&path),
            None => {}
        }
    }

    /// Indented so images line up with the name of the folder they are in.
    fn tree_row_button(&self, ui: &egui::Ui, row: &Row) -> egui::Button<'static> {
        let triangle_side = ui.spacing().icon_width;
        let gap = ui.spacing().icon_spacing;
        let (mut atoms, selected) = match row {
            Row::Folder { path, .. } => {
                let mut atoms = egui::Atoms::new((file_name(path), egui::Atom::grow()));
                atoms.push_left(disclosure_triangle(
                    triangle_side,
                    !self.tree.is_collapsed(path),
                ));
                (atoms, false)
            }
            Row::Image { index, .. } => {
                let path = &self.files[*index];
                let mut text = egui::RichText::new(file_name(path));
                if self.errors.contains_key(path) {
                    text = text.color(Color32::from_rgb(220, 90, 90));
                }
                (
                    egui::Atoms::new((text, egui::Atom::grow())),
                    *index == self.selected,
                )
            }
        };
        if row.depth() > 0 {
            let indent = row.depth() as f32 * (triangle_side + gap) - gap;
            atoms.push_left(egui::Atom::default().atom_size(Vec2::new(indent, 0.0)));
        }
        egui::Button::selectable(selected, atoms)
    }

    pub(super) fn image_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let response = ui.interact(
            rect,
            ui.scope_id().with("image"),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);

        let current = self.current().cloned();
        if let Some(cur) = &current
            && self.cache.contains_key(cur)
        {
            self.shown = Some(cur.clone());
            self.stale = None;
        }
        if !self.mosaic {
            self.zoom_and_pan(ui, &response, rect);
        }

        let debug_general = self.show_debug.then(|| self.debug_general());
        let debug_pos = rect.right_top() + Vec2::new(-44.0, 8.0);
        let mut debug_drawn = false;

        let params = (self.shown.as_ref()).map_or(self.params, |p| self.settings(p).stretch);
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
            let is_current = current.as_ref() == Some(path);
            let flash = loaded
                .clip_overlay
                .as_ref()
                .filter(|_| is_current && self.flash_clip.is_on(path))
                .filter(|_| clip_flash_on(ui));
            let stars = self.stars.get(path);

            if self.mosaic {
                let ppp = ui.ctx().pixels_per_point();
                for panel in mosaic_panels(rect, ppp, loaded.size, loaded.src_size) {
                    painter.image(loaded.texture.id(), panel.screen, panel.uv, Color32::WHITE);
                    if let Some(overlay) = flash {
                        let overlay_uv = Rect::from_min_max(
                            overlay.uv.lerp_inside(panel.uv.min.to_vec2()),
                            overlay.uv.lerp_inside(panel.uv.max.to_vec2()),
                        );
                        painter.image(
                            overlay.texture.id(),
                            panel.screen,
                            overlay_uv,
                            Color32::WHITE,
                        );
                    }
                }
            } else {
                let size = Vec2::new(loaded.size[0] as f32, loaded.size[1] as f32);
                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                if self.actual_size {
                    let ppp = ui.ctx().pixels_per_point();
                    self.zoom = loaded.src_size[0] as f32 / (ppp * size.x * fit);
                }
                let img_rect =
                    Rect::from_center_size(rect.center() + self.pan, size * fit * self.zoom);
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(loaded.texture.id(), img_rect, uv, Color32::WHITE);
                if let Some(overlay) = flash {
                    painter.image(overlay.texture.id(), img_rect, overlay.uv, Color32::WHITE);
                }

                if is_current
                    && self.star_view.is_on(path)
                    && let Some(st) = stars
                {
                    draw::stars(&painter, rect, img_rect, loaded.src_size, &st.stats);
                }
                if is_current
                    && self.tilt_view.is_on(path)
                    && let Some(tilt) = stars.and_then(|s| s.tilt.as_ref())
                {
                    draw::tilt(&painter, img_rect, loaded.src_size, tilt);
                }
                if is_current
                    && self.sky_view.is_on(path)
                    && let Some(Ok(sol)) = stars.map(|s| &s.solution)
                {
                    draw::sky(&painter, rect, img_rect, loaded.src_size, &sol.wcs);
                }
            }

            if self.show_info {
                let text = info_text(path, entry, stars, params);
                draw::text_box(&painter, rect.left_top() + Vec2::new(10.0, 8.0), &text, 1.0);
            }
            if let Some(general) = &debug_general {
                let text = format!("{}\n{general}", debug_image(entry, stars, params));
                draw::text_box_aligned(&painter, debug_pos, egui::Align2::RIGHT_TOP, &text, 1.0);
                debug_drawn = true;
            }
        } else if self.files.is_empty() {
            let text = if self.scanner.busy() {
                "Reading folder…"
            } else {
                "Open a folder (O) or drop a folder / FITS / XISF file here"
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
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
                self.actual_size = false;
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
            self.set_actual_size(false);
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
        let status = self.loader.status();
        let loading = |p: &PathBuf| {
            (status.workers.iter()).any(|w| matches!(w, Some(Work::Image(j)) if j.path == *p))
        };
        let mut neighbors: Vec<(isize, char)> = (self.prefetch_window().into_iter())
            .filter(|(o, _)| *o != 0)
            .map(|(o, p)| {
                let state = if self.cache.contains_key(p) {
                    '●'
                } else if self.errors.contains_key(p) {
                    '×'
                } else if loading(p) {
                    '◐'
                } else {
                    '○'
                };
                (o, state)
            })
            .collect();
        neighbors.sort_by_key(|(o, _)| o.abs());
        let states_on_side = |below: bool| -> String {
            (neighbors.iter())
                .filter(|(o, _)| (*o > 0) == below)
                .map(|(_, state)| *state)
                .collect()
        };
        let prefetch = format!("+{} · -{}", states_on_side(true), states_on_side(false));
        let mut lines = vec![
            format!(
                "cache {}/{} images · {} MB on GPU · {} star measurements",
                self.cache.len(),
                self.cache_limit(),
                gpu_bytes / (1 << 20),
                self.stars.len(),
            ),
            match self.memory_budget {
                Some(m) => format!("GPU {} · memory: {}", self.gpu_info, memory(m)),
                None => format!("GPU {}", self.gpu_info),
            },
            format!("prefetch {prefetch}"),
            format!(
                "queue: {} images · {} star measurements waiting",
                status.queued_images, status.queued_stars
            ),
            format!(
                "raw data kept for stars: {} images · {} MB",
                status.raw_images,
                status.raw_bytes / (1 << 20)
            ),
        ];
        for (i, work) in status.workers.iter().enumerate() {
            let work = match work {
                None => "idle".to_string(),
                Some(Work::Image(job)) => format!("image {}", file_name(&job.path)),
                Some(Work::Stars(path)) => format!("stars {}", file_name(path)),
            };
            lines.push(format!("worker {}: {work}", i + 1));
        }
        lines.join("\n")
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

#[derive(Clone, Copy, Debug)]
struct MosaicPanel {
    screen: Rect,
    uv: Rect,
}

/// 3×3 square crops from the corners, edge midpoints and center, row-major, at one
/// sensor pixel per physical screen pixel.
fn mosaic_panels(
    view: Rect,
    ppp: f32,
    texture_size: [usize; 2],
    sensor_size: [usize; 2],
) -> [MosaicPanel; 9] {
    let snap = |points: f32| (points * ppp).round() / ppp;
    let gap = snap(MOSAIC_GAP);
    let side = snap((view.width().min(view.height()) - 2.0 * gap) / 3.0).max(1.0 / ppp);
    let sensor_crop = (side * ppp).min(sensor_size[0].min(sensor_size[1]) as f32 / 3.0);

    // Along one axis, for panel 0 (start), 1 (middle) or 2 (end): its screen position
    // and UV range.
    let axis = |i: usize, axis: usize| {
        let margin = (view.size()[axis] - 3.0 * side - 2.0 * gap) / 2.0;
        let texels = texture_size[axis] as f32;
        let crop = sensor_crop * texels / sensor_size[axis] as f32;
        let start = (i as f32 * (texels - crop) / 2.0)
            .round()
            .min(texels - crop);
        let screen = snap(view.min[axis] + margin) + i as f32 * (side + gap);
        (screen, start / texels, (start + crop) / texels)
    };
    std::array::from_fn(|i| {
        let (x, u0, u1) = axis(i % 3, 0);
        let (y, v0, v1) = axis(i / 3, 1);
        MosaicPanel {
            screen: Rect::from_min_size(Pos2::new(x, y), Vec2::splat(side)),
            uv: Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
        }
    })
}

/// Whether the clipped-pixel flash is in its visible half. Repaints at the next switch.
fn clip_flash_on(ui: &egui::Ui) -> bool {
    let phase = (ui.input(|i| i.time) / FLASH_PERIOD).fract();
    let next_edge = if phase < 0.5 {
        0.5 - phase
    } else {
        1.0 - phase
    };
    ui.ctx()
        .request_repaint_after_secs((next_edge * FLASH_PERIOD) as f32 + 0.001);
    phase < 0.5
}

/// The bundled fonts have no gear glyph.
fn disclosure_triangle(side: f32, open: bool) -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(side), move |ui, args| {
        let pointing = if open {
            0.0
        } else {
            -std::f32::consts::TAU / 4.0
        };
        let rect = Rect::from_center_size(args.rect.center(), args.rect.size() * 0.6);
        let triangle = egui::Shape::rotated_triangle(rect, pointing, args.fallback_text_color);
        ui.painter().add(triangle);
    })
}

fn gear_button(ui: &mut egui::Ui) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let response = ui.add(egui::Button::new("").min_size(Vec2::splat(side)));
    let color = ui.style().interact(&response).fg_stroke.color;
    let (center, r) = (response.rect.center(), side * 0.3);
    let painter = ui.painter();
    for i in 0..8 {
        let dir = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.0);
        let stroke = egui::Stroke::new(r * 0.4, color);
        painter.line_segment([center + dir * r * 0.6, center + dir * r], stroke);
    }
    painter.circle_stroke(center, r * 0.55, egui::Stroke::new(r * 0.4, color));
    response
}

fn memory(m: MemoryBudget) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    if m.budget >= 1 << 30 {
        format!("{:.1}/{:.1} GB", m.used as f64 / GB, m.budget as f64 / GB)
    } else {
        format!("{}/{} MB", m.used >> 20, m.budget >> 20)
    }
}

/// Debug lines for the shown image: geometry, timings, stretch, saturation level.
fn debug_image(entry: &CacheEntry, stars: Option<&MeasuredStars>, params: StretchParams) -> String {
    let l = &entry.loaded;
    let mode = if l.bin == 1 {
        "full res".to_string()
    } else {
        format!("bin {0}x{0}", l.bin)
    };
    let pattern = l.bayer.map(|b| format!(" {b}")).unwrap_or_default();
    let star_ms = stars.map_or("…".to_string(), |s| format!("{:.0} ms", s.detect_ms));
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
    let mb = l.file_bytes as f64 / f64::from(1 << 20);
    [
        format!(
            "sensor {}×{}{pattern} → texture {}×{} ({mode})",
            l.src_size[0], l.src_size[1], l.size[0], l.size[1]
        ),
        format!(
            "read {:.0} ms ({:.0} MB, {:.0} MB/s) · decode {:.0} ms",
            l.read_ms,
            mb,
            mb / (l.read_ms / 1000.0).max(1e-3),
            l.decode_ms
        ),
        format!(
            "process {:.0} ms · texture {:.0} ms · stars {star_ms}",
            l.process_ms, l.texture_ms
        ),
        format!(
            "background {} ADU · gain {}",
            per_channel(&s.medians, &|v| format!("{:.0}", v * 65535.0)),
            per_channel(&s.gains, &|v| format!("{v:.2}"))
        ),
        if params.auto_stretch {
            format!(
                "stretch shadows {:.4} · midtones {:.4} · target {:.0}% · neutralize {}",
                s.shadows,
                s.midtones,
                params.target_bg * 100.0,
                if params.neutralize { "on" } else { "off" }
            )
        } else {
            format!(
                "stretch off · neutralize {}",
                if params.neutralize { "on" } else { "off" }
            )
        },
        format!("saturation level {} ADU", l.clip.bright_level),
        stars.map_or("solve …".to_string(), debug_solve),
    ]
    .join("\n")
}

fn debug_solve(s: &MeasuredStars) -> String {
    let Ok(sol) = &s.solution else {
        return "solve failed".into();
    };
    format!(
        "solve {:.0} ms · {}/{} matched · {:.2}° off hint",
        sol.ms,
        sol.matched_stars,
        s.stats.points_brightest_first.len(),
        sol.hint_offset_deg
    )
}

fn info_text(
    path: &Path,
    entry: &CacheEntry,
    stars: Option<&MeasuredStars>,
    params: StretchParams,
) -> String {
    let l = &entry.loaded;
    let mut lines = vec![file_name(path)];
    if !l.info.is_empty() {
        lines.push(l.info.clone());
    }
    if !params.auto_stretch {
        lines.push("auto stretch off".into());
    } else if params.target_bg != StretchParams::default().target_bg {
        lines.push(format!(
            "stretch boosted: background {:.0}%",
            params.target_bg * 100.0
        ));
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
    match stars.map(|s| &s.stats) {
        Some(st) if !st.stars.is_empty() => lines.push(format!(
            "stars: {} · HFR median {:.2} px · mean {:.2} px",
            draw::thousands(st.stars.len() as u64),
            st.median_hfr,
            st.mean_hfr
        )),
        Some(_) => lines.push("stars: none found".into()),
        None => lines.push("stars: measuring…".into()),
    }
    match stars.map(|s| &s.solution) {
        Some(Ok(sol)) => {
            let w = &sol.wcs;
            let [sw, sh] = l.src_size;
            let (ra, dec) = w.pixel_to_sky(sw as f64 / 2.0, sh as f64 / 2.0);
            let fov = |px: usize| px as f64 * w.arcsec_per_px() / 3600.0;
            lines.push(format!(
                "solved: {} {} · {:.2}°×{:.2}° · north {:.0}° CW{}",
                draw::format_ra(ra, 0.0),
                draw::format_dec(dec, 0.0),
                fov(sw),
                fov(sh),
                w.north_clockwise_from_up(),
                if w.mirrored() { " · mirrored" } else { "" },
            ));
        }
        Some(Err(e)) => lines.push(format!("plate solve failed: {e}")),
        None => lines.push("plate solving…".into()),
    }
    if let Some(t) = stars.and_then(|s| s.tilt.as_ref()) {
        lines.push(format!(
            "tilt: {:.2} px ({:.0}%) · corners {:.2} px · center {:.2} px",
            t.spread(),
            t.percent(),
            t.corner_mean(),
            t.center
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: Rect = Rect::from_min_max(Pos2::new(10.0, 0.0), Pos2::new(910.0, 600.0));
    const PPP: f32 = 2.0;
    const SENSOR: [usize; 2] = [6000, 4000];

    #[test]
    fn mosaic_crops_corners_edges_and_center() {
        let panels = mosaic_panels(VIEW, PPP, SENSOR, SENSOR);
        assert_eq!(panels[0].uv.min, Pos2::ZERO);
        assert_eq!(panels[8].uv.max, Pos2::new(1.0, 1.0));
        assert!((panels[4].uv.center() - Pos2::new(0.5, 0.5)).length() < 1e-3);
    }

    #[test]
    fn mosaic_shows_one_texel_per_physical_pixel() {
        let MosaicPanel { screen, uv } = mosaic_panels(VIEW, PPP, SENSOR, SENSOR)[0];
        assert_eq!(screen.width() * PPP, uv.width() * SENSOR[0] as f32);
        assert_eq!(screen.height() * PPP, uv.height() * SENSOR[1] as f32);
    }

    #[test]
    fn mosaic_panels_are_centered_squares_inside_the_view() {
        let panels = mosaic_panels(VIEW, PPP, SENSOR, SENSOR);
        let (first, last) = (panels[0].screen, panels[8].screen);
        assert!(VIEW.contains_rect(first.union(last)), "{first:?} {last:?}");
        assert_eq!(first.width(), first.height());
        assert!((panels[4].screen.center() - VIEW.center()).length() < 1.0);
    }

    #[test]
    fn mosaic_of_binned_texture_crops_the_same_area() {
        let binned_size = [1500, 1000];
        let full = mosaic_panels(VIEW, PPP, SENSOR, SENSOR);
        let binned = mosaic_panels(VIEW, PPP, binned_size, SENSOR);
        let binned_texel = Vec2::new(1.0 / binned_size[0] as f32, 1.0 / binned_size[1] as f32);
        for (b, f) in binned.iter().zip(&full) {
            let offset = (b.uv.min - f.uv.min).abs();
            assert!(
                offset.x <= binned_texel.x && offset.y <= binned_texel.y,
                "{b:?} {f:?}"
            );
            assert!((b.uv.size() - f.uv.size()).length() < 1e-6, "{b:?} {f:?}");
        }
    }
}
