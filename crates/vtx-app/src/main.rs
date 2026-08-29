//! VTX application entry point.

use eframe::egui;
use vtx_core::AppConfig;
use vtx_ui::GridCanvas;

/// Application state.
struct VtxApp {}

impl VtxApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {}
    }
}

impl eframe::App for VtxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let size = ui.available_size();
        ui.add(GridCanvas::new(size));
    }
}

fn main() -> eframe::Result<()> {
    let config = AppConfig::default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(config.title.clone())
            .with_inner_size(egui::vec2(config.window_size[0], config.window_size[1])),
        ..Default::default()
    };
    eframe::run_native(
        &config.title,
        options,
        Box::new(|cc| Ok(Box::new(VtxApp::new(cc)))),
    )
}
