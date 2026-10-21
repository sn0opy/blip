use super::prefs::{BinningPref, MAX_PREFETCH};
use super::{App, MAX_SIDE, file_name};
use blip_core::process::{BOOST_STEP_PERCENT, MAX_BOOST};
use eframe::egui::{self, Color32, Key, Modifiers};

const SHORTCUTS: &[(&str, &str)] = &[
    ("↓ → Space J", "Next image"),
    ("↑ ← K", "Previous image"),
    ("PgDn / PgUp", "Jump 10 images"),
    ("Home / End", "First / last image"),
    ("Delete / Backspace", "Delete file permanently (asks first)"),
    ("Wheel / drag", "Zoom / pan (kept while blinking)"),
    ("Double-click, F, 0", "Reset zoom"),
    ("A", "Toggle auto stretch for this image"),
    ("+ / -", "Stretch harder / softer"),
    ("C", "Toggle flashing clipped pixels on this image"),
    ("S", "Toggle star sizes (HFR) on this image"),
    ("T", "Toggle sensor tilt (corner HFR) on this image"),
    ("P", "Toggle plate solve annotation on this image"),
    ("G", "Toggle corner mosaic (aberration inspection)"),
    ("B", "Toggle binned preview / full resolution"),
    ("N", "Toggle background neutralization (white balance)"),
    ("I", "Toggle info overlay"),
    ("D", "Toggle debug info (timings, stretch, cache, GPU)"),
    ("E, Ctrl+S", "Export as PNG / JPEG (full resolution)"),
    ("O, Ctrl+O", "Open folder"),
    ("F5, Ctrl+R", "Rescan folder"),
    ("Ctrl+,", "Settings"),
    ("?, F1", "This help"),
];

/// Without the value box, which would be a text field.
fn slider<'a, N: egui::emath::Numeric>(
    value: &'a mut N,
    range: std::ops::RangeInclusive<N>,
    label: String,
) -> egui::Slider<'a> {
    egui::Slider::new(value, range)
        .show_value(false)
        .text(label)
}

impl App {
    pub(super) fn help_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("help")).show(ctx, |ui| {
            ui.heading("Keyboard shortcuts");
            ui.add_space(6.0);
            egui::Grid::new("shortcuts")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    for (keys, action) in SHORTCUTS {
                        ui.label(egui::RichText::new(*keys).monospace().strong());
                        ui.label(*action);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Press ? or Esc to close").weak());
            ui.add_space(6.0);
            ui.label(egui::RichText::new(concat!("blip ", env!("CARGO_PKG_VERSION"))).weak());
        });
        let toggled = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Questionmark)
                || i.consume_key(Modifiers::NONE, Key::F1)
        });
        if toggled || modal.should_close() {
            self.show_help = false;
        }
    }

    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut prefs = self.prefs_draft.unwrap_or(self.prefs);
        let mut close = false;
        let mut dragging = false;
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Settings");
            ui.add_space(8.0);

            ui.label(egui::RichText::new("Preview").strong());
            ui.horizontal(|ui| {
                ui.label("Binning (B)");
                egui::ComboBox::from_id_salt("binning")
                    .selected_text(prefs.binning.label())
                    .show_ui(ui, |ui| {
                        for b in BinningPref::ALL {
                            ui.selectable_value(&mut prefs.binning, b, b.label());
                        }
                    });
            });
            let mode = match prefs.binning {
                BinningPref::Auto => format!("Bins 2x2, 3x3, … to fit {MAX_SIDE} px."),
                BinningPref::Bin1 => "Full resolution.".into(),
                BinningPref::Bin3 => {
                    "Color (Bayer) images bin 4x4: they bin whole 2x2 cells.".into()
                }
                // Keeps the line, so the GPU limit doesn't move.
                _ => " ".into(),
            };
            ui.label(egui::RichText::new(mode).weak());
            if let Some(limit) = self.loader.max_texture_side() {
                let gpu = format!("GPU limits to {limit} px; larger images are binned to fit.");
                ui.label(egui::RichText::new(gpu).weak());
            }
            ui.add_space(10.0);

            ui.label(egui::RichText::new("Images kept in memory").strong());
            let label = format!("{} ahead", prefs.ahead);
            dragging |= ui
                .add(slider(&mut prefs.ahead, 0..=MAX_PREFETCH, label))
                .dragged();
            let label = format!("{} behind", prefs.behind);
            dragging |= ui
                .add(slider(&mut prefs.behind, 0..=MAX_PREFETCH, label))
                .dragged();
            ui.label(
                egui::RichText::new(
                    "Counted in the direction you browse. Each image takes GPU memory, \
                     a few hundred MB at full resolution.",
                )
                .weak(),
            );
            ui.add_space(10.0);

            ui.label(egui::RichText::new("On start").strong());
            ui.checkbox(&mut prefs.show_info, "Info overlay (I)");
            ui.checkbox(&mut prefs.show_debug, "Debug info (D)");
            ui.add_space(10.0);

            ui.label(egui::RichText::new("Apply to every image").strong());
            ui.checkbox(&mut prefs.auto_stretch, "Auto stretch (A)");
            let label = match prefs.boost {
                0 => "Stretch boost off (+/-)".to_string(),
                n => format!("Stretch boost +{}% (+/-)", n * BOOST_STEP_PERCENT),
            };
            dragging |= ui
                .add(slider(&mut prefs.boost, 0..=MAX_BOOST, label))
                .dragged();
            ui.checkbox(&mut prefs.neutralize, "Background neutralization (N)");
            ui.checkbox(&mut prefs.flash_clip, "Flashing clipped pixels (C)");
            ui.checkbox(&mut prefs.star_view, "Star sizes (S)");
            ui.checkbox(&mut prefs.tilt_view, "Sensor tilt (T)");
            ui.checkbox(&mut prefs.sky_view, "Plate solve annotation (P)");
            ui.add_space(10.0);

            if ui.button("Close (Esc)").clicked() {
                close = true;
            }
        });
        let toggled = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Comma));
        let closing = close || toggled || modal.should_close();
        self.prefs_draft = (dragging && !closing).then_some(prefs);
        if self.prefs_draft.is_none() && prefs != self.prefs {
            self.apply_prefs(prefs);
        }
        if closing {
            self.show_settings = false;
        }
    }

    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_delete.clone() else {
            return;
        };
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm_delete")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Delete file?");
            ui.add_space(4.0);
            ui.label(egui::RichText::new(file_name(&path)).strong());
            ui.label("The file will be permanently removed from the drive.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let delete =
                    egui::Button::new(egui::RichText::new("Delete (Enter)").color(Color32::WHITE))
                        .fill(Color32::from_rgb(170, 40, 40));
                if ui.add(delete).clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel (Esc)").clicked() {
                    decision = Some(false);
                }
            });
        });
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Y)
            {
                decision = Some(true);
            }
            if i.consume_key(Modifiers::NONE, Key::N) {
                decision = Some(false);
            }
        });
        if modal.should_close() {
            decision.get_or_insert(false);
        }
        if let Some(confirmed) = decision {
            self.confirm_delete = None;
            if confirmed {
                self.delete(&path);
            }
        }
    }
}
