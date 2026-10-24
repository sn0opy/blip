//! The file list's context menu: the plate solve's center and subjects, searches on
//! AstroBin, Aladin and SIMBAD, and file actions.

use super::App;
use super::prefs::SubjectOrder;
use blip_core::astrobin::{self, FieldOfView, Search};
use blip_core::dso::{self, Dso};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_SUBJECT_LIST_HEIGHT: f32 = 400.0;

#[cfg(target_os = "macos")]
const REVEAL: &str = "Reveal in Finder";
#[cfg(windows)]
const REVEAL: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", windows)))]
const REVEAL: &str = "Open containing folder";

/// What the app has to do after the menu closes.
pub(super) enum MenuAction {
    Status(String),
    Export(PathBuf),
    Delete(PathBuf),
}

impl App {
    /// Called every frame with the file whose menu is open, if any, and what was chosen in it.
    pub(super) fn handle_file_menu(
        &mut self,
        ctx: &egui::Context,
        file: Option<PathBuf>,
        action: Option<MenuAction>,
    ) {
        if file != self.menu_file {
            self.menu_file = file;
            self.request_loads();
        }
        match action {
            Some(MenuAction::Status(text)) => self.set_status(text, Some(2.5)),
            Some(MenuAction::Export(path)) => self.export(ctx, path),
            Some(MenuAction::Delete(path)) => self.confirm_delete = Some(path),
            None => {}
        }
    }

    pub(super) fn file_menu(&self, ui: &mut egui::Ui, path: &Path) -> Option<MenuAction> {
        point_at_entries(ui);
        let measured = self.stars.get(path);
        let solution = match measured.map(|m| &m.solution) {
            Some(Ok(solution)) => Some(solution),
            Some(Err(e)) => {
                ui.weak(format!("Plate solve failed: {e}"));
                None
            }
            None => {
                ui.weak("Plate solving…");
                None
            }
        };
        let solved = solution.is_some();
        let field = solution.map(|s| FieldOfView::of(&s.wcs, s.size));
        let mut subjects = solution.map_or_else(Vec::new, |s| dso::in_field(&s.wcs, s.size));
        if self.prefs.subject_order == SubjectOrder::Name {
            subjects.sort_by_cached_key(|d| natural_key(d.label));
        }
        let status = |text: &str| Some(MenuAction::Status(text.into()));
        let mut action = None;

        let copy_center = egui::Button::new("Copy center coordinates");
        if ui.add_enabled(solved, copy_center).clicked()
            && let Some(field) = &field
        {
            let (ra, dec) = field.center;
            ui.ctx().copy_text(coordinates(ra, dec));
            action = status("Copied center coordinates");
        }
        let copy_subjects = egui::Button::new("Copy list of subjects in FOV");
        if ui
            .add_enabled(!subjects.is_empty(), copy_subjects)
            .clicked()
        {
            let names: Vec<String> = subjects.iter().map(|d| subject_name(d)).collect();
            ui.ctx().copy_text(names.join("\n"));
            let s = if subjects.len() == 1 { "" } else { "s" };
            action = status(&format!("Copied {} subject{s}", subjects.len()));
        }
        let search_field = egui::Button::new("Search this FOV on AstroBin");
        if ui.add_enabled(solved, search_field).clicked() {
            let search = Search {
                field,
                focal_length_mm: measured.and_then(|m| m.focal_length_mm),
                ..Search::default()
            };
            action = open_search(ui, &search);
        }
        if let Some(d) = subject_menu(ui, "Search subject on AstroBin", &subjects) {
            let search = Search {
                subject: Some(d.designation().into()),
                ..Search::default()
            };
            action = open_search(ui, &search);
        }
        let open_aladin = egui::Button::new("Open this FOV in Aladin Lite");
        if ui.add_enabled(solved, open_aladin).clicked()
            && let Some(field) = &field
        {
            open_url(ui, aladin_url(field));
        }
        if let Some(d) = subject_menu(ui, "Look up subject in SIMBAD", &subjects) {
            open_url(ui, simbad_url(d));
        }

        ui.separator();
        if ui.button(REVEAL).clicked()
            && let Err(e) = reveal(path)
        {
            action = status(&format!("Cannot open the file manager: {e}"));
        }
        if ui.button("Copy file path").clicked() {
            ui.ctx().copy_text(path.display().to_string());
            action = status("Copied file path");
        }
        if ui.button("Export…").clicked() {
            action = Some(MenuAction::Export(path.to_path_buf()));
        }

        ui.separator();
        if ui.button("Delete…").clicked() {
            action = Some(MenuAction::Delete(path.to_path_buf()));
        }
        action
    }
}

fn point_at_entries(ui: &mut egui::Ui) {
    ui.visuals_mut().interact_cursor = Some(egui::CursorIcon::PointingHand);
}

/// Returns the subject clicked.
fn subject_menu(ui: &mut egui::Ui, label: &str, subjects: &[&'static Dso]) -> Option<&'static Dso> {
    let mut clicked = None;
    ui.add_enabled_ui(!subjects.is_empty(), |ui| {
        ui.menu_button(label, |ui| {
            point_at_entries(ui);
            egui::ScrollArea::vertical()
                .max_height(MAX_SUBJECT_LIST_HEIGHT)
                .show(ui, |ui| {
                    for d in subjects {
                        if ui.button(subject_name(d)).clicked() {
                            clicked = Some(*d);
                        }
                    }
                });
        });
    });
    clicked
}

fn open_url(ui: &egui::Ui, url: String) {
    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
}

fn open_search(ui: &egui::Ui, search: &Search) -> Option<MenuAction> {
    let Some(url) = astrobin::search_url(search) else {
        return Some(MenuAction::Status(
            "AstroBin can't search this field".into(),
        ));
    };
    open_url(ui, url);
    None
}

fn aladin_url(field: &FieldOfView) -> String {
    let (ra, dec) = field.center;
    let fov = field.width.max(field.height);
    format!("https://aladin.cds.unistra.fr/AladinLite/?target={ra:.5}%20{dec:.5}&fov={fov:.3}")
}

fn simbad_url(d: &Dso) -> String {
    let ident = d.designation().replace(' ', "%20");
    format!("https://simbad.cds.unistra.fr/simbad/sim-id?Ident={ident}")
}

/// Shows the file in the system's file manager.
fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        // Explorer only accepts the path quoted after the comma, which `arg` can't produce.
        let mut c = Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    let mut child = command.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn subject_name(d: &Dso) -> String {
    if d.common_name.is_empty() {
        d.label.to_string()
    } else {
        format!("{}, {}", d.label, d.common_name)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Chunk {
    Number(u64),
    Text(String),
}

/// Sorts "M 2" before "M 10", ignoring case and spaces ("NGC0704B" next to "NGC 705").
fn natural_key(s: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut rest = s;
    while let Some(first) = rest.chars().next() {
        let digits = first.is_ascii_digit();
        let end = (rest.find(|c: char| c.is_ascii_digit() != digits)).unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        chunks.push(if digits {
            Chunk::Number(run.parse().unwrap_or(u64::MAX))
        } else {
            Chunk::Text(run.replace(' ', "").to_lowercase())
        });
        rest = tail;
    }
    chunks
}

/// "05h35m16.8s -05°23'24\"", a form most astronomy software accepts.
fn coordinates(ra: f64, dec: f64) -> String {
    let tenths = (ra.rem_euclid(360.0) / 15.0 * 36_000.0).round() as i64 % 864_000;
    let (h, m, s) = (tenths / 36_000, tenths / 600 % 60, tenths % 600);
    let arcsec = (dec * 3600.0).round() as i64;
    let sign = if arcsec < 0 { '-' } else { '+' };
    let arcsec = arcsec.abs();
    format!(
        "{h:02}h{m:02}m{:02}.{}s {sign}{:02}°{:02}'{:02}\"",
        s / 10,
        s % 10,
        arcsec / 3600,
        arcsec / 60 % 60,
        arcsec % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_coordinates_sexagesimally() {
        assert_eq!(coordinates(83.82, -5.39), "05h35m16.8s -05°23'24\"");
        assert_eq!(coordinates(359.99999, 89.99999), "00h00m00.0s +90°00'00\"");
        assert_eq!(coordinates(10.68, 0.0), "00h42m43.2s +00°00'00\"");
    }

    #[test]
    fn sorts_designations_naturally() {
        let mut labels = [
            "NGC 705",
            "M 101",
            "IC 434",
            "NGC0704B",
            "M 31 (NGC 224)",
            "M 2",
        ];
        labels.sort_by_cached_key(|l| natural_key(l));
        assert_eq!(
            labels,
            [
                "IC 434",
                "M 2",
                "M 31 (NGC 224)",
                "M 101",
                "NGC0704B",
                "NGC 705"
            ]
        );
    }

    #[test]
    fn links_to_aladin_and_simbad() {
        let field = FieldOfView {
            center: (83.82, -5.39),
            width: 1.2,
            height: 1.85,
        };
        assert_eq!(
            aladin_url(&field),
            "https://aladin.cds.unistra.fr/AladinLite/?target=83.82000%20-5.39000&fov=1.850"
        );
        let found = dso::overlapping_cone(83.82, -5.39, 0.1);
        let m42 = found.iter().find(|d| d.designation() == "M 42").unwrap();
        assert_eq!(
            simbad_url(m42),
            "https://simbad.cds.unistra.fr/simbad/sim-id?Ident=M%2042"
        );
    }
}
