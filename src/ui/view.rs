use super::actions::{self, Action};
use super::gpu::MemoryBudget;
use super::headers::HeaderIndex;
use super::loader::{ClipOverlay, Loaded, LoaderStatus, MeasuredStars, Work};
use super::menu::MenuAction;
use super::tree::Row;
use super::{App, ImageToggle, draw, file_name};
use blip_core::process::StretchParams;
use eframe::egui::{self, AtomExt as _, Color32, Pos2, Rect, Vec2};
use std::path::{Path, PathBuf};

/// On/off period of the clipped-pixel flash, in seconds.
const FLASH_PERIOD: f64 = 0.8;
/// Space between the mosaic panels, in points.
const MOSAIC_GAP: f32 = 3.0;
const REJECTED_BORDER: f32 = 3.0;
const NO_MATCHES: &str = "No images match the filter";
const FILTER_TIP: &str =
    "Images whose path contains all words.\n-word leaves out, \"two words\" keeps the space.";

#[derive(PartialEq)]
pub(super) struct OverlayState {
    path: PathBuf,
    view: Rect,
    img_rect: Rect,
    toolbar_height: f32,
    pixels_per_point: f32,
    stars: bool,
    tilt: bool,
    sky: bool,
}

impl App {
    pub(super) fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if actions::tooltip(ui.button("Open…"), Action::Open).clicked() {
                self.pick_folder();
            }
            let has_files = !self.list.files().is_empty();
            let export = ui.add_enabled(has_files, egui::Button::new("Export…"));
            if actions::tooltip(export, Action::Export).clicked() {
                self.export_current(ui.ctx());
            }
            // Pushed right in the same row (not right-to-left), so the panel can't get
            // narrower than all three buttons.
            let gear_side = ui.spacing().interact_size.y;
            ui.add_space((ui.available_width() - gear_side).max(0.0));
            if actions::tooltip(gear_button(ui), Action::Settings).clicked() {
                self.show_settings = true;
            }
        });
        let label = (self.list.dir())
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "No folder".into());
        ui.add(egui::Label::new(egui::RichText::new(label).weak()).truncate());
        ui.horizontal(|ui| {
            let side = ui.spacing().interact_size.y;
            let width = ui.available_width() - side - ui.spacing().item_spacing.x;
            self.filter_field(ui, width.max(0.0));
            self.grouping_button(ui, side);
        });
        ui.separator();
        self.rejected_panel(ui);
        self.file_rows(ui);
    }

    fn file_rows(&mut self, ui: &mut egui::Ui) {
        let scroll_to_selected = self.list.take_scroll_to_selected();
        let scroll_rows = self.list.take_scroll_rows();
        let rows = self.list.tree().visible_rows();
        if rows.is_empty() && self.list.has_images() {
            ui.weak(format!("{NO_MATCHES}."));
        }
        let selected = self.list.selected();
        let selected_row = scroll_to_selected.then(|| {
            (rows.iter())
                .position(|row| matches!(row, Row::Image { index, .. } if *index == selected))
                .unwrap_or(0)
        });
        let row_h = ui.spacing().interact_size.y;
        let area = list_scroll_area(ui, row_h, scroll_rows, selected_row);
        let (mut clicked, mut menu, mut top) = (None, None, None);
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            top = (rows.get(range.start)).map(|row| self.list.row_path(row).to_path_buf());
            for row in &rows[range] {
                let button = self
                    .tree_row_button(ui, row)
                    .wrap_mode(egui::TextWrapMode::Extend)
                    .min_size(Vec2::new(ui.available_width(), row_h));
                let response = ui.add(button);
                if response.clicked() {
                    clicked = Some((*row).clone());
                }
                if let Some(open) = self.row_menu(&response, row) {
                    menu = Some(open);
                }
            }
        });
        self.list.set_top_row(top);
        let (menu_file, menu_action) = menu.unzip();
        self.handle_file_menu(ui.ctx(), menu_file, menu_action.flatten());
        match clicked {
            Some(Row::Image { index, .. }) => self.select(index),
            Some(Row::Folder { path, .. }) => self.list.toggle_folder(&path),
            None => {}
        }
    }

    /// The file whose context menu is open, and what was chosen in it.
    fn row_menu(
        &self,
        response: &egui::Response,
        row: &Row,
    ) -> Option<(PathBuf, Option<MenuAction>)> {
        let Row::Image { index, .. } = row else {
            return None;
        };
        let path = &self.list.files()[*index];
        let menu = egui::Popup::context_menu(response).show(|ui| self.file_menu(ui, path))?;
        Some((path.clone(), menu.inner))
    }

    /// Selects the text, so typing replaces it.
    pub(super) fn focus_filter(&self, ctx: &egui::Context) {
        let id = filter_id();
        ctx.memory_mut(|m| m.request_focus(id));
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(self.list.filter().chars().count()),
        );
        state.cursor.set_char_range(Some(all));
        egui::TextEdit::store_state(ctx, id, state);
    }

    fn filter_field(&mut self, ui: &mut egui::Ui, width: f32) {
        let clear_id = egui::IdSalt::new("clear_filter");
        let header_search = self.list.header_search();
        let (filter, headers) = self.list.filter_editor();
        let filtering = !filter.is_empty();
        let mut edit = egui::TextEdit::singleline(filter)
            .hint_text(format!("Filter ({})", actions::key_hint(Action::Filter)))
            .desired_width(width);
        if filtering {
            let size = Vec2::splat(ui.spacing().icon_width);
            edit = edit.suffix(egui::Atom::custom(clear_id, size));
        }
        let output = egui::CompletionPopup::new(filter_id())
            .align(egui::RectAlign::BOTTOM_START)
            .show(ui, edit, |query| {
                if header_search {
                    suggestions(headers, query.word)
                } else {
                    Vec::new()
                }
            });
        let clear_rect = output.text_edit.response.rect(clear_id);
        let tip = if header_search {
            format!("{FILTER_TIP}\nKEYWORD:value looks in the header.")
        } else {
            FILTER_TIP.to_string()
        };
        let response = output.text_edit.response.response.on_hover_text(tip);
        let mut changed = response.changed() || output.accepted.is_some();
        if let Some(rect) = clear_rect {
            let rect = Rect::from_x_y_ranges(rect.x_range(), response.rect.y_range());
            let clear = ui.place(rect, egui::Button::new("").frame(false));
            let color = ui.style().interact(&clear).fg_stroke.color;
            clear_icon(ui.painter(), clear.rect, color);
            let clear = clear.on_hover_cursor(egui::CursorIcon::PointingHand);
            if clear.on_hover_text("Clear filter").clicked() {
                self.list.filter_editor().0.clear();
                changed = true;
            }
        }
        if changed {
            self.list.filter_changed();
            self.files_changed();
        }
    }

    fn grouping_button(&mut self, ui: &mut egui::Ui, side: f32) {
        let has_folders = self.prefs.subfolder_depth > 0;
        let button = egui::Button::new("").min_size(Vec2::splat(side));
        let response = ui.add_enabled(has_folders, button);
        let color = ui.style().interact(&response).fg_stroke.color;
        list_icon(
            ui.painter(),
            response.rect,
            self.list.grouped() && has_folders,
            color,
        );
        let tip = if self.list.grouped() {
            "Grouped by folder; click for a flat list"
        } else {
            "Flat list; click to group by folder"
        };
        let response = response
            .on_hover_text(tip)
            .on_disabled_hover_text("Only the opened folder is listed (see Settings)");
        if response.clicked() {
            self.list.set_grouped(!self.list.grouped());
        }
    }

    /// Indented so images line up with the name of the folder they are in.
    fn tree_row_button(&self, ui: &egui::Ui, row: &Row) -> egui::Button<'static> {
        let triangle_side = ui.spacing().icon_width;
        let gap = ui.spacing().icon_spacing;
        let (mut atoms, selected) = match row {
            Row::Folder { path, .. } => {
                let mut atoms = egui::Atoms::new((file_name(path), egui::Atom::grow()));
                let rejected = self.list.rejected_in(path);
                if rejected > 0 {
                    atoms.push_right(egui::RichText::new(rejected.to_string()).color(draw::RED));
                }
                atoms.push_left(disclosure_triangle(
                    triangle_side,
                    !self.list.tree().is_collapsed(path),
                ));
                (atoms, false)
            }
            Row::Image { index, .. } => {
                let path = &self.list.files()[*index];
                let mut text = egui::RichText::new(file_name(path));
                if self.list.is_rejected(path) {
                    text = text.color(draw::RED).strikethrough();
                } else if self.cache.error(path).is_some() {
                    text = text.color(draw::RED);
                }
                (
                    egui::Atoms::new((text, egui::Atom::grow())),
                    *index == self.list.selected(),
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
        let toolbar_height = self.toolbar(ui.ctx(), rect);
        let bottom_left = rect.left_bottom() - Vec2::new(0.0, toolbar_height);

        let current = self.current().cloned();
        if let Some(cur) = &current
            && self.cache.contains(cur)
        {
            self.shown = Some(cur.clone());
            self.cache.discard_stale();
        }
        if !self.mosaic {
            self.zoom_and_pan(ui, &response, rect);
        }

        let shown = self.shown.clone().filter(|p| self.cache.touch(p));
        if let Some(path) = &shown {
            let is_current = current.as_ref() == Some(path);
            self.paint_shown(ui, &painter, rect, toolbar_height, path, is_current);
            if self.show_info {
                self.paint_info(&painter, rect, path);
            }
        } else if self.list.files().is_empty() {
            paint_placeholder(&painter, rect, &self.empty_list_text());
        }
        if self.show_debug {
            self.paint_debug(&painter, rect, shown.as_deref());
        }
        if let Some(cur) = &current {
            self.paint_error_or_spinner(ui, &painter, rect, bottom_left, cur);
        }
        if current.as_ref().is_some_and(|p| self.list.is_rejected(p)) {
            let border = egui::Stroke::new(REJECTED_BORDER, draw::RED);
            painter.rect_stroke(rect, 0.0, border, egui::StrokeKind::Inside);
        }
        let status_pos = bottom_left + Vec2::new(10.0, -26.0);
        self.status.paint(ui.ctx(), &painter, status_pos);
    }

    fn paint_shown(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        view: Rect,
        toolbar_height: f32,
        path: &Path,
        is_current: bool,
    ) {
        let Some(loaded) = self.cache.rendered(path) else {
            return;
        };
        let flash = (loaded.clip_overlay.as_ref())
            .filter(|_| is_current && self.flash_clip.is_on(path))
            .filter(|_| clip_flash_on(ui));
        let ppp = ui.ctx().pixels_per_point();
        if self.mosaic {
            paint_mosaic(painter, view, ppp, loaded, flash);
            return;
        }
        let size = Vec2::new(loaded.size[0] as f32, loaded.size[1] as f32);
        let fit = (view.width() / size.x).min(view.height() / size.y);
        if self.actual_size {
            self.zoom = loaded.src_size[0] as f32 / (ppp * size.x * fit);
        }
        let img_rect = Rect::from_center_size(view.center() + self.pan, size * fit * self.zoom);
        paint_image(painter, img_rect, loaded, flash);
        self.paint_overlays(painter, view, toolbar_height, img_rect, path, is_current);
    }

    fn paint_overlays(
        &mut self,
        painter: &egui::Painter,
        view: Rect,
        toolbar_height: f32,
        img_rect: Rect,
        path: &Path,
        is_current: bool,
    ) {
        let Some(src_size) = self.cache.rendered(path).map(|l| l.src_size) else {
            return;
        };
        let stars = self.stars.get(path);
        let shown_on = |toggle: &ImageToggle| is_current && toggle.is_on(path);
        let star_stats = stars
            .map(|s| &s.stats)
            .filter(|_| shown_on(&self.star_view));
        let tilt = (stars.and_then(|s| s.tilt.as_ref())).filter(|_| shown_on(&self.tilt_view));
        let wcs = (stars.and_then(|s| s.solution.as_ref().ok()))
            .map(|solution| &solution.wcs)
            .filter(|_| shown_on(&self.sky_view));
        let state = OverlayState {
            path: path.to_path_buf(),
            view,
            img_rect,
            toolbar_height,
            pixels_per_point: painter.pixels_per_point(),
            stars: star_stats.is_some(),
            tilt: tilt.is_some(),
            sky: wcs.is_some(),
        };
        let visible_image = img_rect.intersect(view);
        self.overlay
            .paint(painter, visible_image, state, |overlay| {
                if let Some(st) = star_stats {
                    draw::stars(overlay, view, img_rect, src_size, st);
                }
                if let Some(tilt) = tilt {
                    draw::tilt(overlay, img_rect, src_size, tilt);
                }
                if let Some(wcs) = wcs {
                    draw::sky(overlay, view, toolbar_height, img_rect, src_size, wcs);
                }
            });
    }

    fn paint_info(&self, painter: &egui::Painter, view: Rect, path: &Path) {
        let Some(loaded) = self.cache.rendered(path) else {
            return;
        };
        let text = info_text(
            path,
            loaded,
            self.stars.get(path),
            self.settings(path).stretch,
        );
        draw::text_box(painter, view.left_top() + Vec2::new(10.0, 8.0), &text, 1.0);
    }

    fn paint_debug(&self, painter: &egui::Painter, view: Rect, shown: Option<&Path>) {
        let general = self.debug_general();
        let image = shown.and_then(|path| {
            let loaded = self.cache.rendered(path)?;
            let stretch = self.settings(path).stretch;
            Some(debug_image(loaded, self.stars.get(path), stretch))
        });
        let text = match image {
            Some(image) => format!("{image}\n{general}"),
            None => general,
        };
        let pos = view.right_top() + Vec2::new(-44.0, 8.0);
        draw::text_box_aligned(painter, pos, egui::Align2::RIGHT_TOP, &text, 1.0);
    }

    fn empty_list_text(&self) -> String {
        if self.scanner.busy() {
            "Reading folder…".to_string()
        } else if self.list.has_images() {
            NO_MATCHES.to_string()
        } else {
            let key = actions::key_hint(Action::Open);
            format!("Open a folder ({key}) or drop a folder / FITS / XISF file here")
        }
    }

    fn paint_error_or_spinner(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        view: Rect,
        bottom_left: Pos2,
        current: &Path,
    ) {
        if let Some(err) = self.cache.error(current) {
            let text = format!("{}: {err}", file_name(current));
            draw::text_box(painter, bottom_left + Vec2::new(10.0, -48.0), &text, 1.0);
        } else if !self.cache.contains(current) {
            let spinner_rect =
                Rect::from_min_size(view.right_top() + Vec2::new(-34.0, 10.0), Vec2::splat(22.0));
            ui.put(spinner_rect, egui::Spinner::new().size(22.0));
        }
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
            self.zoom_to_fit();
        }
        if self.zoom <= 1.0 {
            self.pan = Vec2::ZERO;
        }
    }

    /// Debug lines that don't depend on the shown image.
    fn debug_general(&self) -> String {
        let status = self.loader.status();
        let mut lines = vec![
            format!(
                "cache {}/{} images · {} MB on GPU · {} star measurements · {} headers",
                self.cache.len(),
                self.cache_limit(),
                self.cache.gpu_bytes() / (1 << 20),
                self.stars.len(),
                self.list.header_count(),
            ),
            match self.cache.memory_budget() {
                Some(m) => format!("GPU {} · memory: {}", self.gpu_info, budget_text(m)),
                None => format!("GPU {}", self.gpu_info),
            },
            format!("prefetch {}", self.prefetch_states(&status)),
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

    /// The prefetched images after and before the current one, nearest first.
    fn prefetch_states(&self, status: &LoaderStatus) -> String {
        let loading = |p: &PathBuf| {
            (status.workers.iter()).any(|w| matches!(w, Some(Work::Image(j)) if j.path == *p))
        };
        let state = |p: &PathBuf| {
            if self.cache.contains(p) {
                '●'
            } else if self.cache.error(p).is_some() {
                '×'
            } else if loading(p) {
                '◐'
            } else {
                '○'
            }
        };
        let mut neighbors: Vec<(isize, char)> = (self.prefetch_window().into_iter())
            .filter(|(o, _)| *o != 0)
            .map(|(o, p)| (o, state(p)))
            .collect();
        neighbors.sort_by_key(|(o, _)| o.abs());
        let states_on_side = |below: bool| -> String {
            (neighbors.iter())
                .filter(|(o, _)| (*o > 0) == below)
                .map(|(_, state)| *state)
                .collect()
        };
        format!("+{} · -{}", states_on_side(true), states_on_side(false))
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

fn paint_mosaic(
    painter: &egui::Painter,
    view: Rect,
    ppp: f32,
    loaded: &Loaded,
    flash: Option<&ClipOverlay>,
) {
    for panel in mosaic_panels(view, ppp, loaded.size, loaded.src_size) {
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
}

fn paint_image(
    painter: &egui::Painter,
    img_rect: Rect,
    loaded: &Loaded,
    flash: Option<&ClipOverlay>,
) {
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(loaded.texture.id(), img_rect, uv, Color32::WHITE);
    if let Some(overlay) = flash {
        painter.image(overlay.texture.id(), img_rect, overlay.uv, Color32::WHITE);
    }
}

fn paint_placeholder(painter: &egui::Painter, view: Rect, text: &str) {
    painter.text(
        view.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(18.0),
        Color32::GRAY,
    );
}

/// Keeps the same files in view when rows above them come or go, and scrolls the selected
/// row into view.
fn list_scroll_area(
    ui: &egui::Ui,
    row_h: f32,
    scroll_rows: isize,
    selected_row: Option<usize>,
) -> egui::ScrollArea {
    let spacing = ui.spacing().item_spacing.y;
    let scroll_salt = "file_list_scroll";
    let mut area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt(scroll_salt);
    let id = ui.make_persistent_id(egui::IdSalt::new(scroll_salt));
    let offset = egui::scroll_area::State::load(ui.ctx(), id).map_or(0.0, |s| s.clamped_offset().y);
    if scroll_rows != 0 {
        let shifted = offset + scroll_rows as f32 * (row_h + spacing);
        area = area.vertical_scroll_offset(shifted.max(0.0));
    }
    if let Some(selected_row) = selected_row {
        let view_h = ui.available_height();
        let top = selected_row as f32 * (row_h + spacing);
        if top < offset {
            area = area.vertical_scroll_offset(top);
        } else if top + row_h > offset + view_h {
            area = area.vertical_scroll_offset(top + row_h - view_h + spacing);
        }
    }
    area
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

fn suggestions(headers: &mut HeaderIndex, word: &str) -> Vec<egui::Suggestion> {
    (headers.complete(word).iter())
        .map(|c| {
            egui::Suggestion::new(c.insert.clone())
                .content(c.label.clone())
                .description(c.file_count.to_string())
        })
        .collect()
}

fn filter_id() -> egui::Id {
    egui::Id::unique("file_filter")
}

fn clear_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let (center, r) = (rect.center(), rect.width() * 0.3);
    let stroke = egui::Stroke::new(1.5, color);
    for corner in [Vec2::new(r, r), Vec2::new(r, -r)] {
        painter.line_segment([center - corner, center + corner], stroke);
    }
}

fn list_icon(painter: &egui::Painter, rect: Rect, grouped: bool, color: Color32) {
    let (center, r) = (rect.center(), rect.height() * 0.3);
    let stroke = egui::Stroke::new(1.5, color);
    for (i, y) in [-0.7, 0.0, 0.7].into_iter().enumerate() {
        let indent = if grouped && i > 0 { 0.7 } else { 0.0 };
        let y = center.y + y * r;
        let left = center.x - r + indent * r;
        painter.line_segment([Pos2::new(left, y), Pos2::new(center.x + r, y)], stroke);
    }
}

/// The bundled fonts have no gear glyph.
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

fn budget_text(m: MemoryBudget) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    if m.budget >= 1 << 30 {
        format!("{:.1}/{:.1} GB", m.used as f64 / GB, m.budget as f64 / GB)
    } else {
        format!("{}/{} MB", m.used >> 20, m.budget >> 20)
    }
}

/// Debug lines for the shown image: geometry, timings, stretch, saturation level.
fn debug_image(l: &Loaded, stars: Option<&MeasuredStars>, params: StretchParams) -> String {
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
    l: &Loaded,
    stars: Option<&MeasuredStars>,
    params: StretchParams,
) -> String {
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
