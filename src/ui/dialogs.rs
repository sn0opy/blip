use super::actions::{self, Action, SHORTCUTS, with_key};
use super::prefs::{BinningPref, MAX_PREFETCH, MAX_SUBFOLDER_DEPTH, SubjectOrder};
use super::{App, MAX_SIDE, file_name};
use blip_core::process::{BOOST_STEP_PERCENT, MAX_BOOST};
use eframe::egui::{self, Color32, Key, Modifiers};

pub(super) const DIALOG_WIDTH: f32 = 380.0;
/// The heading, Close button, frame and a gap to the window's edges.
const SETTINGS_FIXED_HEIGHT: f32 = 140.0;

/// Without the value box, which would be a text field.
fn slider<'a, N: egui::emath::Numeric>(
    value: &'a mut N,
    range: std::ops::RangeInclusive<N>,
    label: impl Into<egui::WidgetText>,
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
                    for shortcut in SHORTCUTS {
                        let keys = actions::platform_keys(shortcut.keys);
                        ui.label(egui::RichText::new(keys).monospace().strong());
                        ui.label(shortcut.description);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            let close = format!("Press {} or Esc to close", actions::key_hint(Action::Help));
            ui.label(egui::RichText::new(close).weak());
            ui.add_space(6.0);
            ui.label(egui::RichText::new(concat!("blip ", env!("CARGO_PKG_VERSION"))).weak());
        });
        if actions::pressed(ctx, Action::Help) || modal.should_close() {
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
        let mut frame = egui::Frame::popup(&ctx.global_style());
        let scrollbar_margin = egui::Margin {
            right: std::mem::take(&mut frame.inner_margin.right),
            ..egui::Margin::ZERO
        };
        let modal = egui::Modal::new(egui::Id::unique("settings"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(DIALOG_WIDTH + f32::from(scrollbar_margin.right));
                let style = ui.style().clone();
                let with_action_key = |action: Action| actions::label(&style, action);
                ui.heading("Settings");
                ui.add_space(8.0);

                let max_height = ctx.content_rect().height() - SETTINGS_FIXED_HEIGHT;
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .content_margin(scrollbar_margin)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Folder").strong());
                        let label = match prefs.subfolder_depth {
                            0 => "Only the opened folder".to_string(),
                            1 => "Subfolders 1 level deep".to_string(),
                            n => format!("Subfolders up to {n} levels deep"),
                        };
                        dragging |= ui
                            .add(slider(
                                &mut prefs.subfolder_depth,
                                0..=MAX_SUBFOLDER_DEPTH,
                                label,
                            ))
                            .dragged();
                        ui.label(
                            egui::RichText::new("How many folders deep to search for images.")
                                .weak(),
                        );
                        ui.add_space(10.0);

                        ui.label(egui::RichText::new("Preview").strong());
                        ui.horizontal(|ui| {
                            ui.label(with_action_key(Action::FullRes));
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
                            // Keeps the line, so the GPU limit doesn't move.
                            _ => " ".into(),
                        };
                        ui.label(egui::RichText::new(mode).weak());
                        if let Some(limit) = self.loader.max_texture_side() {
                            let gpu = format!(
                                "GPU limits to {limit} px; larger images are binned to fit."
                            );
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

                        ui.label(egui::RichText::new("Context menu").strong());
                        ui.horizontal(|ui| {
                            ui.label("Sort subjects by");
                            egui::ComboBox::from_id_salt("subject_order")
                                .selected_text(prefs.subject_order.label())
                                .show_ui(ui, |ui| {
                                    for o in SubjectOrder::ALL {
                                        ui.selectable_value(&mut prefs.subject_order, o, o.label());
                                    }
                                });
                        });
                        ui.add_space(10.0);

                        ui.label(egui::RichText::new("Toolbar").strong());
                        ui.checkbox(&mut prefs.show_toolbar, "Show toolbar");
                        ui.add_space(10.0);

                        ui.label(egui::RichText::new("On start").strong());
                        ui.checkbox(&mut prefs.show_info, with_action_key(Action::Info));
                        ui.checkbox(&mut prefs.show_debug, with_action_key(Action::Debug));
                        ui.checkbox(&mut prefs.actual_size, with_action_key(Action::ZoomActual));
                        ui.add_space(10.0);

                        ui.label(egui::RichText::new("Apply to every image").strong());
                        ui.checkbox(
                            &mut prefs.auto_stretch,
                            with_action_key(Action::AutoStretch),
                        );
                        let boost = match prefs.boost {
                            0 => "Stretch boost off".to_string(),
                            n => format!("Stretch boost +{}%", n * BOOST_STEP_PERCENT),
                        };
                        let boost = with_key(&style, &boost, &actions::keys(Action::Boost(1)));
                        dragging |= ui
                            .add(slider(&mut prefs.boost, 0..=MAX_BOOST, boost))
                            .dragged();
                        ui.checkbox(&mut prefs.neutralize, with_action_key(Action::Neutralize));
                        ui.checkbox(&mut prefs.flash_clip, with_action_key(Action::ClipFlash));
                        ui.checkbox(&mut prefs.star_view, with_action_key(Action::Stars));
                        ui.checkbox(&mut prefs.tilt_view, with_action_key(Action::Tilt));
                        ui.checkbox(&mut prefs.sky_view, with_action_key(Action::Sky));
                    });
                ui.add_space(10.0);

                if ui.button(with_key(&style, "Close", "Esc")).clicked() {
                    close = true;
                }
            });
        let closing = close || actions::pressed(ctx, Action::Settings) || modal.should_close();
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
            ui.set_width(DIALOG_WIDTH);
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
