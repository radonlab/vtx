//! The terminal screen widget.

use egui::{Response, Sense, Ui};
use vtx_core::event::EventListener;
use vtx_core::term::Term;

use crate::font::TerminalFont;
use crate::render::render_terminal;
use crate::theme::TerminalTheme;

/// Renders the terminal screen of a single session.
///
/// The view itself only carries presentation settings (font and theme); the
/// terminal state is borrowed per frame, so one view can render whichever
/// terminal is currently attached to it. Keyboard input, resize, and repaint
/// scheduling land in the following milestones.
pub struct TerminalView {
    /// Monospace font and cell metrics.
    font: TerminalFont,
    /// Resolved rendering colors.
    theme: TerminalTheme,
}

impl TerminalView {
    /// Creates a view with the default font and theme.
    pub fn new() -> Self {
        Self {
            font: TerminalFont::default(),
            theme: TerminalTheme::default(),
        }
    }

    /// Draws the terminal screen, keeping its settings across frames.
    ///
    /// Unlike a plain [`egui::Widget`], this method borrows the terminal
    /// state, which allows long-lived views (e.g. one per session) to render
    /// the session attached to them.
    pub fn show<T: EventListener>(&mut self, ui: &mut Ui, term: &Term<T>) -> Response {
        // Pick up runtime color overrides, like OSC 11.
        self.theme = TerminalTheme::new(term.colors());

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        render_terminal(ui.painter(), rect, term, &self.font, &self.theme);
        response
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new()
    }
}
