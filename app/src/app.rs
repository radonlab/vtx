//! The eframe application: owns the dock state and drives the dock area.

use eframe::egui;
use egui_stack::{DockArea, DockState, OverlayType, Style, TabViewer};

/// One dockable panel. For now a plain label; will become a terminal session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Tab {
    Terminal(String),
}

impl Tab {
    fn title_text(&self) -> &str {
        match self {
            Tab::Terminal(name) => name,
        }
    }
}

/// Tells egui_stack how to render and name each tab.
struct VtxTabViewer;

impl TabViewer for VtxTabViewer {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::unique(tab)
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab.title_text().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            Tab::Terminal(name) => {
                ui.label(format!("terminal placeholder: {name}"));
            }
        }
    }
}

pub struct VtxApp {
    dock_state: DockState<Tab>,
}

impl VtxApp {
    pub fn new(_ctx: egui::Context) -> Self {
        let tabs = vec![
            Tab::Terminal("term 1".into()),
            Tab::Terminal("term 2".into()),
        ];
        Self {
            dock_state: DockState::new(tabs),
        }
    }
}

impl eframe::App for VtxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            let mut style = Style::from_egui(ui.style().as_ref());
            style.overlay.overlay_type = OverlayType::Widgets;
            DockArea::new(&mut self.dock_state)
                .style(style)
                .show_inside(ui, &mut VtxTabViewer);
        });
    }
}
