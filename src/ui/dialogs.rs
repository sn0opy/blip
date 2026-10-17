use super::{App, file_name};
use eframe::egui::{self, Color32, Key, Modifiers};

const SHORTCUTS: &[(&str, &str)] = &[
    ("↓ → Space J", "Next image"),
    ("↑ ← K", "Previous image"),
    ("PgDn / PgUp", "Jump 10 images"),
    ("Home / End", "First / last image"),
    ("Delete / Backspace", "Delete file (asks first)"),
    ("Wheel / drag", "Zoom / pan (kept while blinking)"),
    ("Double-click, F, 0", "Reset zoom"),
    ("A", "Toggle auto stretch for this image"),
    ("C", "Toggle flashing clipped pixels on this image"),
    ("S", "Toggle star sizes (HFR) on this image"),
    ("B", "Toggle binned preview / full resolution"),
    ("N", "Toggle background neutralization (white balance)"),
    ("I", "Toggle info overlay"),
    ("D", "Toggle debug info (timings, stretch, cache, GPU)"),
    ("E, Ctrl+S", "Export as PNG / JPEG (full resolution)"),
    ("O, Ctrl+O", "Open folder"),
    ("F5, Ctrl+R", "Rescan folder"),
    ("?, F1", "This help"),
];

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
