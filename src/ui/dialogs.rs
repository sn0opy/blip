use super::actions::{self, Action, SHORTCUTS, with_key};
use super::prefs::{
    BinningPref, MAX_PREFETCH, MAX_SUBFOLDER_DEPTH, Preferences, RejectHandling, SubjectOrder,
};
use super::rejects::REJECTED_FOLDER;
use super::{App, MAX_SIDE, file_count, file_name};
use blip_core::process::{BOOST_STEP_PERCENT, MAX_BOOST};
use eframe::egui::{self, Color32, Key, Modifiers};
use std::path::PathBuf;

pub(super) const DIALOG_WIDTH: f32 = 380.0;
/// The heading, Close button, frame and a gap to the window's edges.
const SETTINGS_FIXED_HEIGHT: f32 = 140.0;

pub(super) enum Deletion {
    File(PathBuf),
    Rejected,
}

impl App {
    pub(super) fn help_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let grid = egui::Grid::new("shortcuts")
            .num_columns(2)
            .spacing([24.0, 6.0]);
        let modal = egui::Modal::new(egui::Id::unique("help")).show(ctx, |ui| {
            ui.heading("Keyboard shortcuts");
            ui.add_space(6.0);
            grid.show(ui, |ui| {
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
        let gpu_limit = self.loader.max_texture_side();
        let mut frame = egui::Frame::popup(&ctx.global_style());
        let scrollbar_margin = egui::Margin {
            right: std::mem::take(&mut frame.inner_margin.right),
            ..egui::Margin::ZERO
        };
        let scroll = egui::ScrollArea::vertical()
            .max_height(ctx.content_rect().height() - SETTINGS_FIXED_HEIGHT)
            .auto_shrink([false, true])
            .content_margin(scrollbar_margin);
        let modal = egui::Modal::new(egui::Id::unique("settings")).frame(frame);
        let modal = modal.show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH + f32::from(scrollbar_margin.right));
            ui.heading("Settings");
            ui.add_space(8.0);
            let dragging = (scroll.show(ui, |ui| settings_form(ui, &mut prefs, gpu_limit))).inner;
            ui.add_space(10.0);
            let close = ui.button(with_key(ui.style(), "Close", "Esc")).clicked();
            (close, dragging)
        });
        let (close, dragging) = modal.inner;
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
        let Some(deletion) = &self.confirm_delete else {
            return;
        };
        let (heading, what, consequence) = match deletion {
            Deletion::File(path) => (
                "Delete file?",
                file_name(path),
                "The file will be permanently removed from the drive.",
            ),
            Deletion::Rejected => (
                "Delete rejected files?",
                file_count(self.list.rejected_count()),
                "They will be permanently removed from the drive.",
            ),
        };
        let delete_label = egui::RichText::new("Delete (Enter)").color(Color32::WHITE);
        let delete_button = egui::Button::new(delete_label).fill(Color32::from_rgb(170, 40, 40));
        let modal = egui::Modal::new(egui::Id::unique("confirm_delete")).show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH);
            ui.heading(heading);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(what).strong());
            ui.label(consequence);
            ui.add_space(8.0);
            let buttons = ui.horizontal(|ui| {
                let delete = ui.add(delete_button).clicked();
                (delete, ui.button("Cancel (Esc)").clicked())
            });
            buttons.inner
        });
        let (delete, cancel) = modal.inner;
        let (yes, no) = ctx.input_mut(|i| {
            let pressed = |i: &mut egui::InputState, key| i.consume_key(Modifiers::NONE, key);
            let yes = pressed(i, Key::Enter) || pressed(i, Key::Y);
            (yes, pressed(i, Key::N))
        });
        let decision = if no {
            Some(false)
        } else if yes || delete {
            Some(true)
        } else if cancel || modal.should_close() {
            Some(false)
        } else {
            None
        };
        if let Some(confirmed) = decision {
            let deletion = self.confirm_delete.take();
            match deletion.filter(|_| confirmed) {
                Some(Deletion::File(path)) => self.delete(&path),
                Some(Deletion::Rejected) => self.dispose_rejected(RejectHandling::Delete),
                None => {}
            }
        }
    }
}

/// Returns whether a slider is being dragged; the dialog applies its value on release.
fn settings_form(ui: &mut egui::Ui, prefs: &mut Preferences, gpu_limit: Option<usize>) -> bool {
    let mut dragging = folder_settings(ui, prefs);
    ui.add_space(10.0);
    rejected_settings(ui, prefs);
    ui.add_space(10.0);
    preview_settings(ui, prefs, gpu_limit);
    ui.add_space(10.0);
    dragging |= memory_settings(ui, prefs);
    ui.add_space(10.0);
    heading(ui, "Context menu");
    ui.horizontal(|ui| {
        ui.label("Sort subjects by");
        let order = &mut prefs.subject_order;
        choice(
            ui,
            "subject_order",
            order,
            &SubjectOrder::ALL,
            SubjectOrder::label,
        );
    });
    ui.add_space(10.0);
    heading(ui, "Toolbar");
    ui.checkbox(&mut prefs.show_toolbar, "Show toolbar");
    ui.add_space(10.0);
    heading(ui, "On start");
    toggle(ui, &mut prefs.show_info, Action::Info);
    toggle(ui, &mut prefs.show_debug, Action::Debug);
    toggle(ui, &mut prefs.actual_size, Action::ZoomActual);
    ui.add_space(10.0);
    dragging | every_image_settings(ui, prefs)
}

fn folder_settings(ui: &mut egui::Ui, prefs: &mut Preferences) -> bool {
    heading(ui, "Folder");
    let label = match prefs.subfolder_depth {
        0 => "Only the opened folder".to_string(),
        1 => "Subfolders 1 level deep".to_string(),
        n => format!("Subfolders up to {n} levels deep"),
    };
    let dragging = slider(
        ui,
        &mut prefs.subfolder_depth,
        0..=MAX_SUBFOLDER_DEPTH,
        label,
    );
    hint(ui, "How many folders deep to search for images.");
    ui.checkbox(&mut prefs.group_by_folder, "Group images by folder");
    ui.checkbox(
        &mut prefs.header_search,
        "Filter by header with KEYWORD:value",
    );
    let scan = egui::Checkbox::new(
        &mut prefs.scan_headers,
        "Read all headers when opening a folder",
    );
    ui.add_enabled(prefs.header_search, scan);
    dragging
}

fn rejected_settings(ui: &mut egui::Ui, prefs: &mut Preferences) {
    heading(ui, "Rejected files");
    ui.horizontal(|ui| {
        ui.label(actions::label(ui.style(), Action::HandleRejected));
        let handling = &mut prefs.reject_handling;
        choice(
            ui,
            "reject_handling",
            handling,
            &RejectHandling::ALL,
            RejectHandling::label,
        );
    });
    hint(
        ui,
        match prefs.reject_handling {
            RejectHandling::MoveToFolder => {
                format!("Into {REJECTED_FOLDER}/ in the opened folder, keeping subfolders.")
            }
            RejectHandling::MoveToTrash => "They can be restored from the trash.".into(),
            RejectHandling::Delete => "Asks first. This can't be undone.".into(),
        },
    );
}

fn preview_settings(ui: &mut egui::Ui, prefs: &mut Preferences, gpu_limit: Option<usize>) {
    heading(ui, "Preview");
    ui.horizontal(|ui| {
        ui.label(actions::label(ui.style(), Action::FullRes));
        let binning = &mut prefs.binning;
        choice(
            ui,
            "binning",
            binning,
            &BinningPref::ALL,
            BinningPref::label,
        );
    });
    let mode = match prefs.binning {
        BinningPref::Auto => format!("Bins 2x2, 3x3, … to fit {MAX_SIDE} px."),
        BinningPref::Bin1 => "Full resolution.".into(),
        // Keeps the line, so the GPU limit doesn't move.
        _ => " ".into(),
    };
    hint(ui, mode);
    if let Some(limit) = gpu_limit {
        hint(
            ui,
            format!("GPU limits to {limit} px; larger images are binned to fit."),
        );
    }
}

fn memory_settings(ui: &mut egui::Ui, prefs: &mut Preferences) -> bool {
    heading(ui, "Images kept in memory");
    let ahead = format!("{} ahead", prefs.ahead);
    let mut dragging = slider(ui, &mut prefs.ahead, 0..=MAX_PREFETCH, ahead);
    let behind = format!("{} behind", prefs.behind);
    dragging |= slider(ui, &mut prefs.behind, 0..=MAX_PREFETCH, behind);
    hint(
        ui,
        "Counted in the direction you browse. Each image takes GPU memory, a few hundred MB \
         at full resolution.",
    );
    dragging
}

fn every_image_settings(ui: &mut egui::Ui, prefs: &mut Preferences) -> bool {
    heading(ui, "Apply to every image");
    toggle(ui, &mut prefs.auto_stretch, Action::AutoStretch);
    let boost = match prefs.boost {
        0 => "Stretch boost off".to_string(),
        n => format!("Stretch boost +{}%", n * BOOST_STEP_PERCENT),
    };
    let boost = with_key(ui.style(), &boost, &actions::keys(Action::Boost(1)));
    let dragging = slider(ui, &mut prefs.boost, 0..=MAX_BOOST, boost);
    toggle(ui, &mut prefs.neutralize, Action::Neutralize);
    toggle(ui, &mut prefs.flash_clip, Action::ClipFlash);
    toggle(ui, &mut prefs.star_view, Action::Stars);
    toggle(ui, &mut prefs.tilt_view, Action::Tilt);
    toggle(ui, &mut prefs.sky_view, Action::Sky);
    dragging
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
}

fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text).weak());
}

/// A checkbox labeled with the action's name and key.
fn toggle(ui: &mut egui::Ui, value: &mut bool, action: Action) {
    ui.checkbox(value, actions::label(ui.style(), action));
}

/// Without the value box, which would be a text field. Returns whether it's dragged.
fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    label: impl Into<egui::WidgetText>,
) -> bool {
    let slider = egui::Slider::new(value, range)
        .show_value(false)
        .text(label);
    ui.add(slider).dragged()
}

fn choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    options: &[T],
    name: fn(T) -> &'static str,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(name(*value))
        .show_ui(ui, |ui| {
            for &option in options {
                ui.selectable_value(value, option, name(option));
            }
        });
}
