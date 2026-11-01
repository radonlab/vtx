//! The terminal screen widget.

use egui::{Align2, Color32, EventFilter, FontId, Response, Sense, Ui};
use vtx_core::session::TerminalSession;
use vtx_core::tty::TtySize;

use crate::font::TerminalFont;
use crate::input;
use crate::render::render_terminal;
use crate::theme::TerminalTheme;

/// Renders the terminal screen of one session and feeds it user input.
///
/// The view itself only carries presentation settings (font, theme, and the
/// size last requested from the session); the session owns the terminal
/// state. Every frame it resizes the session to the viewport, translates
/// keyboard input while it has focus, and renders the current screen.
pub struct TerminalView {
    /// Monospace font and cell metrics.
    font: TerminalFont,
    /// Resolved rendering colors.
    theme: TerminalTheme,
    /// Columns and rows last requested from the session.
    ///
    /// Sessions start at a fixed size and adopt the viewport on the first
    /// frame, since the layout is not known before it.
    size: Option<(u16, u16)>,
}

impl TerminalView {
    /// Creates a view with the default font and theme.
    pub fn new() -> Self {
        Self {
            font: TerminalFont::default(),
            theme: TerminalTheme::default(),
            size: None,
        }
    }

    /// Draws the terminal screen of `session`, handling resize and input.
    ///
    /// Unlike a plain [`egui::Widget`], this method borrows the whole
    /// session, which allows long-lived views (e.g. one per tab) to stay
    /// attached to it.
    pub fn show(&mut self, ui: &mut Ui, session: &mut TerminalSession) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());

        // Claim keyboard focus: on click, or when nothing else has it, so a
        // freshly opened tab is ready to type.
        if response.clicked() || ui.memory(|mem| mem.focused().is_none()) {
            response.request_focus();
        }

        if response.has_focus() {
            // Keep egui's focus navigation away from keys the terminal
            // needs: Tab, the arrows, and Escape must reach the child, not
            // move or surrender focus.
            ui.memory_mut(|mem| {
                mem.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }

        let exited = session.is_exited();
        self.resize(ui, rect, session);

        // Encode the input while the terminal is locked, but write it after
        // releasing the lock, so a slow PTY cannot stall the reader thread.
        let terminal = session.terminal().clone();
        let mut input = Vec::new();
        {
            let term = terminal.lock_unfair();

            // Pick up runtime color overrides, like OSC 11.
            self.theme = TerminalTheme::new(term.colors());

            if response.has_focus() && !exited {
                let events = ui.ctx().input(|i| i.events.clone());
                input = input::encode(&events, *term.mode());
            }

            render_terminal(ui.painter(), rect, &term, &self.font, &self.theme);
        }

        if !input.is_empty()
            && let Err(err) = session.write(&input)
        {
            log::warn!("writing input to the child failed: {err}");
        }

        if exited {
            self.draw_exit_overlay(ui, rect, session.exit_code());
        }

        response
    }

    /// Resize the session to the cell grid fitting into `rect`.
    ///
    /// The session starts at a default size and tracks the viewport from
    /// the first frame on; degenerate rects, like before the first layout,
    /// keep the current size.
    fn resize(&mut self, ui: &Ui, rect: egui::Rect, session: &mut TerminalSession) {
        let cell_size = self.font.cell_size(ui.ctx());
        if cell_size.x <= 0.0 || cell_size.y <= 0.0 {
            return;
        }

        let columns = (rect.width() / cell_size.x).floor() as u16;
        let rows = (rect.height() / cell_size.y).floor() as u16;
        if columns == 0 || rows == 0 || self.size == Some((columns, rows)) {
            return;
        }

        let size = TtySize {
            rows,
            cols: columns,
            pixel_width: cell_size.x as u16,
            pixel_height: cell_size.y as u16,
        };
        match session.resize(size) {
            Ok(()) => self.size = Some((columns, rows)),
            Err(err) => log::warn!("resizing the session failed: {err}"),
        }
    }

    /// Dim the screen of an exited session and report its exit code.
    fn draw_exit_overlay(&self, ui: &Ui, rect: egui::Rect, exit_code: Option<u32>) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(160));

        let message = match exit_code {
            Some(code) => format!("Process exited with code {code}"),
            None => "Process exited".to_owned(),
        };
        let font = FontId::monospace(self.font.font_id().size);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            message,
            font,
            Color32::WHITE,
        );
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new()
    }
}
