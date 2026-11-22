#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod advice;
mod analyze;
mod app;
mod collector;
mod config;
mod fps;
mod overlay;
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
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon128.rgba").to_vec(), width: 128, height: 128 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(test_size().unwrap_or([1280.0, 860.0]))
            .with_min_inner_size([900.0, 600.0])
            .with_title("GameLens")
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };
    eframe::run_native("GameLens", opts, Box::new(|_cc| Ok(Box::new(app::App::new()))))
}

/// Только для проверки внешнего вида: GAMELENS_SIZE=1280x2400
fn test_size() -> Option<[f32; 2]> {
    let s = std::env::var("GAMELENS_SIZE").ok()?;
    let (w, h) = s.split_once('x')?;
    Some([w.parse().ok()?, h.parse().ok()?])
}
