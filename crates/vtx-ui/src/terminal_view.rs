//! The terminal screen widget.

use egui::{Color32, Response, Sense, Ui, Widget};

/// Renders the terminal screen of a single session.
///
/// The current implementation is a placeholder that only paints the
/// terminal background; screen contents and VT100 handling will be
/// layered on top later.
pub struct TerminalView {
    /// Background color of the terminal screen.
    background: Color32,
}

impl TerminalView {
    /// Creates a view with the default black background.
    pub fn new() -> Self {
        Self {
            background: Color32::BLACK,
        }
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TerminalView {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, self.background);
        response
    }
}
