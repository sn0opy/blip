#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use eframe::egui;
use std::path::PathBuf;

/// `blip [folder-or-file] [--backend dx12|vulkan|gl] [--present mailbox|fifo|immediate|auto]`
#[derive(Default)]
struct Args {
    start: Option<PathBuf>,
    backend: Option<String>,
    present: Option<String>,
}

impl Args {
    fn parse() -> Self {
        let mut args = Args::default();
        let mut it = std::env::args_os().skip(1);
        while let Some(a) = it.next() {
            match a.to_str() {
                Some("--backend") => args.backend = it.next().and_then(|v| v.into_string().ok()),
                Some("--present") => args.present = it.next().and_then(|v| v.into_string().ok()),
                _ => args.start = Some(PathBuf::from(a)),
            }
        }
        args.present = args.present.or_else(|| std::env::var("BLIP_PRESENT").ok());
        args
    }
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let wgpu_options = ui::wgpu_options(args.backend.as_deref(), args.present.as_deref());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("blip")
        .with_app_id("sh.sascha.blip")
        .with_inner_size([1400.0, 900.0])
        .with_drag_and_drop(true);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/icon-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        wgpu_options,
        ..Default::default()
    };
    let start = args.start;
    eframe::run_native(
        "blip",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, start)))),
    )
}
