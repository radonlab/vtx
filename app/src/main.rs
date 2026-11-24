mod app;

use eframe::egui;

use crate::app::VtxApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("vtx"),
        ..Default::default()
    };

    eframe::run_native(
        "vtx",
        options,
        Box::new(|cc| Ok(Box::new(VtxApp::new(cc.egui_ctx.clone())))),
    )
}
