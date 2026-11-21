#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analyze;
mod app;
mod collector;
mod model;
mod paths;
mod perf;
mod recorder;
mod store;
mod update;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // Игровой режим: отдельный процесс без окна и без графики
    if args.iter().any(|a| a == "--record") {
        recorder::run();
        return Ok(());
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 760.0]).with_title("GameLens"),
        ..Default::default()
    };
    eframe::run_native("GameLens", opts, Box::new(|_cc| Ok(Box::new(app::App::new()))))
}
