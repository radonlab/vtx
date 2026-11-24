//! The eframe application: owns the dock state and drives the dock area.

use eframe::egui;
use egui_stack::{DockArea, DockState, OverlayType, Style, TabViewer};
use vtx_core::AppSettings;

/// One dockable panel: a terminal session or the settings page.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Tab {
    Terminal(String),
    Settings(AppSettings),
}

impl Tab {
    fn title_text(&self) -> &str {
        match self {
            Tab::Terminal(name) => name,
            Tab::Settings(_) => "Settings",
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
            Tab::Settings(settings) => {
                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Font size");
                        ui.add(egui::Slider::new(&mut settings.font_size, 6..=32));
                        ui.end_row();

                        ui.strong("Scrollback");
                        ui.add(
                            egui::DragValue::new(&mut settings.scrollback_lines)
                                .range(0..=1_000_000)
                                .suffix(" lines"),
                        );
                        ui.end_row();

                        ui.strong("Cursor blink");
                        ui.add(egui::Checkbox::new(&mut settings.cursor_blink, ""));
                        ui.end_row();
                    });
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
            Tab::Settings(AppSettings::default()),
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
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut VtxTabViewer);
        });
    }
}
