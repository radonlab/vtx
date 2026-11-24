//! The eframe application: owns the dock state and drives the dock area.

use eframe::egui;

pub struct VtxApp {
    ctx: egui::Context,
}

impl VtxApp {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }
}

impl eframe::App for VtxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
}
