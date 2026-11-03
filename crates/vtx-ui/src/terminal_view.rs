//! The terminal screen widget.

use std::time::Duration;

use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, IMEPurpose, ImeEvent, Key,
    MouseWheelUnit, PointerButton, Rect, Response, Sense, Ui, Vec2, ViewportCommand, pos2,
};
use vtx_core::event::EventProxy;
use vtx_core::grid::{Dimensions, Scroll};
use vtx_core::mode::TermMode;
use vtx_core::mouse::{self, MouseButton, MouseEvent};
use vtx_core::selection::SelectionType;
use vtx_core::session::TerminalSession;
use vtx_core::term::Term;
use vtx_core::tty::TtySize;

use crate::font::TerminalFont;
use crate::input;
use crate::mouse as ui_mouse;
use crate::render::render_terminal;
use crate::theme::TerminalTheme;

/// Maximum time between presses counted as one multi-click, in seconds.
///
/// Matches upstream's click threshold.
const CLICK_TIMEOUT: f64 = 0.4;

/// Lines scrolled per wheel notch, like upstream's default multiplier.
///
/// Only applies to local scrollback: applications receiving wheel reports
/// see one event per line.
const SCROLL_MULTIPLIER: f32 = 3.0;

/// Time between blink phases, in seconds, like upstream's default.
const CURSOR_BLINK_INTERVAL: f64 = 0.75;

/// Seconds of input inactivity after which the cursor stops blinking.
const CURSOR_BLINK_TIMEOUT: f64 = 5.0;

/// State of the last primary press, for multi-click detection.
#[derive(Clone, Copy)]
struct Click {
    /// Time of the press.
    time: f64,
    /// Press count so far: 1 single, 2 double, 3 or more triple.
    count: usize,
}

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
    /// Last primary press, for double and triple click detection.
    last_click: Option<Click>,
    /// The focus state last reported to the child, if any.
    ///
    /// `None` means nothing was reported yet, either because the child
    /// never asked for focus reports or the view was never focused.
    focus_reported: Option<bool>,
    /// Wheel motion accumulated towards whole lines.
    ///
    /// Fractional deltas from trackpads only scroll once they add up to a
    /// full line.
    wheel_accum: f32,
    /// Whether the blinking cursor is in its visible phase.
    blink_visible: bool,
    /// When the blink last toggled.
    blink_toggled_at: f64,
    /// When the user last typed; blinking rests after a timeout.
    last_input: f64,
    /// The IME composition currently being edited at the cursor.
    preedit: String,
}

impl TerminalView {
    /// Creates a view with the default font and theme.
    pub fn new() -> Self {
        Self {
            font: TerminalFont::default(),
            theme: TerminalTheme::default(),
            size: None,
            last_click: None,
            focus_reported: None,
            wheel_accum: 0.0,
            blink_visible: true,
            blink_toggled_at: 0.0,
            last_input: 0.0,
            preedit: String::new(),
        }
    }

    /// Draws the terminal screen of `session`, handling resize and input.
    ///
    /// Unlike a plain [`egui::Widget`], this method borrows the whole
    /// session, which allows long-lived views (e.g. one per tab) to stay
    /// attached to it.
    pub fn show(&mut self, ui: &mut Ui, session: &mut TerminalSession) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

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

        // The terminal area reads like a text field.
        let response = response.on_hover_cursor(CursorIcon::Text);

        let exited = session.is_exited();
        self.resize(ui, rect, session);

        let events = ui.ctx().input(|i| i.events.clone());
        let cell_size = self.font.cell_size(ui.ctx());

        // Encode the input while the terminal is locked, but write it after
        // releasing the lock, so a slow PTY cannot stall the reader thread.
        let terminal = session.terminal().clone();
        let mut input = Vec::new();
        {
            let mut term = terminal.lock_unfair();

            // Pick up runtime color overrides, like OSC 11.
            self.theme = TerminalTheme::new(term.colors());

            if response.has_focus() && !exited {
                input = input::encode(&events, *term.mode());
                let (ime_input, composing) = self.ime(&events);
                input.extend_from_slice(&ime_input);

                if !input.is_empty() || composing {
                    // Typing, pasting, and starting an IME composition clear
                    // the selection, return the viewport to the bottom, and
                    // restart the cursor blink, like upstream. The
                    // composition start must count as input too: with an
                    // IME, keystrokes become preedit events instead of
                    // text, and skipping the scroll would leave the
                    // composition anchored outside the scrolled viewport.
                    term.clear_selection();
                    if term.grid().display_offset() != 0 {
                        term.scroll_display(Scroll::Bottom);
                    }
                    let now = ui.input(|i| i.time);
                    self.last_input = now;
                    self.blink_toggled_at = now;
                    self.blink_visible = true;
                }

                self.clipboard(&events, &term, ui.ctx());
                self.scroll_keys(&events, &mut term);
            }

            // Report focus changes to the child (mode 1004). The state is
            // tracked per view so enabling the mode while focused reports
            // immediately, like upstream.
            if !exited {
                let focused = response.has_focus() && term.mode().contains(TermMode::FOCUS_IN_OUT);
                if self.focus_reported != Some(focused) {
                    self.focus_reported = Some(focused);
                    input.extend_from_slice(input::focus(focused));
                }
            }

            if !exited && response.hovered() {
                self.wheel(&events, &response, rect, cell_size, &mut term, &mut input);
            }

            if !exited && cell_size.x > 0.0 && cell_size.y > 0.0 {
                self.selection(
                    &events,
                    &response,
                    rect,
                    cell_size,
                    &mut term,
                    ui.input(|i| i.time),
                );
            }

            // Advance the blink timer while the child asked for a blinking
            // cursor; after a timeout without input the cursor rests
            // visible, like upstream.
            let now = ui.input(|i| i.time);
            let blinking = term.cursor_style().is_some_and(|style| style.blinking)
                && response.has_focus()
                && now - self.last_input < CURSOR_BLINK_TIMEOUT;
            let cursor_visible = if blinking {
                if now - self.blink_toggled_at >= CURSOR_BLINK_INTERVAL {
                    self.blink_visible = !self.blink_visible;
                    self.blink_toggled_at = now;
                }

                // Keep the blink running even without other changes.
                ui.ctx().request_repaint_after(Duration::from_secs_f64(
                    (CURSOR_BLINK_INTERVAL - (now - self.blink_toggled_at)).max(0.0),
                ));

                self.blink_visible
            } else {
                true
            };

            // Position the IME candidate window at the terminal cursor.
            let cursor_pos = if cell_size.x > 0.0 && cell_size.y > 0.0 {
                let grid = term.grid();
                let point = term.cursor_point();
                // Clamp the anchor into the viewport: egui needs an IME rect
                // every focused frame to keep the IME enabled, but a cursor
                // scrolled out of view would push the candidate window and
                // the preedit outside the terminal.
                let row = (point.line.0 + grid.display_offset() as i32)
                    .clamp(0, grid.screen_lines() as i32 - 1);
                Some(pos2(
                    rect.left() + point.column.0 as f32 * cell_size.x,
                    rect.top() + row as f32 * cell_size.y,
                ))
            } else {
                None
            };

            // Track the hyperlink under the pointer for hover feedback.
            let hovered_link = if !exited && cell_size.x > 0.0 && cell_size.y > 0.0 {
                self.hyperlink_at(&response, rect, cell_size, &term)
            } else {
                None
            };
            if hovered_link.is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }

            // Clicking a hyperlink opens it; the selection a plain press
            // started at the link cell is dropped. While the child captures
            // the mouse, Shift keeps links reachable.
            let shift = response.ctx.input(|i| i.modifiers.shift);
            if response.clicked()
                && let Some(link) = hovered_link.as_ref()
                && (!term.mouse_mode() || shift)
            {
                term.clear_selection();
                let url = egui::OpenUrl::new_tab(link.uri().to_owned());
                ui.ctx().open_url(url);
            }

            let selection = term.selection_range();
            render_terminal(
                ui.painter(),
                rect,
                &term,
                &self.font,
                &self.theme,
                selection,
                cursor_visible,
                hovered_link.as_ref(),
            );

            if response.has_focus()
                && let Some(cursor_pos) = cursor_pos
            {
                self.draw_ime(ui, rect, cursor_pos, cell_size);
            }
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

    /// Handle IME composition events of the focused terminal.
    ///
    /// Returns the input bytes for commits, and whether a composition just
    /// started, i.e. the preedit went from empty to non-empty. Commits are
    /// written to the child like typed text; the preedit string is kept for
    /// drawing at the cursor. The events only arrive once the IME output
    /// anchors the candidate window, which [`Self::draw_ime`] takes care
    /// of.
    fn ime(&mut self, events: &[Event]) -> (Vec<u8>, bool) {
        let mut input = Vec::new();
        let mut started = false;
        for event in events {
            match event {
                Event::Ime(ImeEvent::Preedit { text, .. }) => {
                    started |= self.preedit.is_empty() && !text.is_empty();
                    self.preedit = text.clone();
                }
                Event::Ime(ImeEvent::Commit(text)) => {
                    self.preedit.clear();
                    input.extend_from_slice(text.as_bytes());
                }
                _ => (),
            }
        }
        (input, started)
    }

    /// Anchor the IME candidate window at the terminal cursor and draw the
    /// ongoing composition over the screen.
    fn draw_ime(&mut self, ui: &Ui, rect: Rect, cursor_pos: egui::Pos2, cell_size: Vec2) {
        // The platform expects screen coordinates for its candidate window.
        let transform = ui
            .ctx()
            .layer_transform_to_global(ui.layer_id())
            .unwrap_or_default();
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: IMEPurpose::Terminal,
                rect: transform * rect,
                cursor_rect: transform * Rect::from_min_size(cursor_pos, cell_size),
                should_interrupt_composition: false,
            });
        });

        if self.preedit.is_empty() {
            return;
        }

        // The composition is uncommitted: draw it at the cursor with an
        // underline, like a text field's inline preedit.
        let painter = ui.painter();
        let galley = painter.layout_no_wrap(
            self.preedit.clone(),
            self.font.font_id().clone(),
            self.theme.foreground(),
        );
        let thickness = (cell_size.y / 12.0).max(1.0);
        let underline = Rect::from_min_size(
            cursor_pos + Vec2::new(0.0, cell_size.y - thickness),
            Vec2::new(galley.size().x.max(cell_size.x), thickness),
        );
        painter.rect_filled(underline, egui::CornerRadius::ZERO, self.theme.foreground());
        painter.galley(cursor_pos, galley, self.theme.foreground());
    }

    /// The hyperlink below the pointer, if any.
    fn hyperlink_at(
        &self,
        response: &Response,
        rect: Rect,
        cell_size: Vec2,
        term: &Term<EventProxy>,
    ) -> Option<vtx_core::cell::Hyperlink> {
        let pos = response.hover_pos()?;

        let grid = term.grid();
        let (point, _) = ui_mouse::point_at(
            rect,
            pos,
            cell_size,
            grid.columns(),
            grid.screen_lines(),
            grid.display_offset(),
        );

        grid[point].hyperlink()
    }

    /// Handle the scroll key bindings of the focused terminal.
    ///
    /// Shift+PageUp/PageDown scrolls by a page, Shift+Home/End to the
    /// top/bottom of the scrollback, mirroring upstream's default
    /// bindings. The child never sees these keys.
    fn scroll_keys(&mut self, events: &[Event], term: &mut Term<EventProxy>) {
        for event in events {
            let Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                continue;
            };

            if !modifiers.shift {
                continue;
            }

            let scroll = match key {
                Key::PageUp => Scroll::PageUp,
                Key::PageDown => Scroll::PageDown,
                Key::Home => Scroll::Top,
                Key::End => Scroll::Bottom,
                _ => continue,
            };
            term.scroll_display(scroll);
        }
    }

    /// Handle mouse wheel scrolling over the terminal.
    ///
    /// The wheel follows an application-first waterfall, like upstream:
    /// while the child captures the mouse its wheel turns into reported
    /// mouse events, one per line; on the alternate screen with alternate
    /// scroll (mode 1007) it becomes arrow keys; and only otherwise does
    /// the viewport scroll the scrollback. Shift bypasses the first two.
    fn wheel(
        &mut self,
        events: &[Event],
        response: &Response,
        rect: egui::Rect,
        cell_size: Vec2,
        term: &mut Term<EventProxy>,
        input: &mut Vec<u8>,
    ) {
        if cell_size.x <= 0.0 || cell_size.y <= 0.0 {
            return;
        }

        let grid = term.grid();
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        let mode = *term.mode();
        let mouse_mode = mode.intersects(TermMode::MOUSE_MODE);
        let alt_scroll = mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL);

        for event in events {
            let Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                continue;
            };

            // Only vertical motion scrolls the terminal.
            let delta_lines = match unit {
                MouseWheelUnit::Line => delta.y,
                MouseWheelUnit::Point => delta.y / cell_size.y,
                MouseWheelUnit::Page => delta.y * screen_lines as f32,
            };

            // The multiplier applies to local scrolling only; applications
            // see one event per line.
            self.wheel_accum += if mouse_mode {
                delta_lines
            } else {
                delta_lines * SCROLL_MULTIPLIER
            };

            let lines = self.wheel_accum.trunc();
            if lines == 0.0 {
                continue;
            }
            self.wheel_accum -= lines;

            let up = lines > 0.0;
            let count = lines.abs() as usize;

            if mouse_mode && !modifiers.shift {
                // Report the wheel as one press per line, at the hovered
                // cell.
                if let Some(pos) = response.hover_pos() {
                    let (point, _) = ui_mouse::point_at(
                        rect,
                        pos,
                        cell_size,
                        columns,
                        screen_lines,
                        display_offset,
                    );
                    let line = (point.line.0 + display_offset as i32) as usize;
                    let event = MouseEvent {
                        button: if up {
                            MouseButton::WheelUp
                        } else {
                            MouseButton::WheelDown
                        },
                        motion: false,
                        pressed: true,
                        column: point.column.0,
                        line,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                        ctrl: modifiers.ctrl,
                    };
                    for _ in 0..count {
                        if let Some(report) = mouse::encode_mouse_event(&mode, event) {
                            input.extend_from_slice(&report);
                        }
                    }
                }
            } else if alt_scroll && !modifiers.shift {
                // Alternate scroll feeds arrow keys so full-screen apps
                // like pagers move their own cursor.
                let code: &[u8] = if up { b"\x1bOA" } else { b"\x1bOB" };
                for _ in 0..count {
                    input.extend_from_slice(code);
                }
            } else {
                term.scroll_display(Scroll::Delta(lines as i32));
            }
        }
    }

    /// Handle the clipboard shortcuts of the focused terminal.
    ///
    /// Ctrl+Shift+C copies the active selection to the system clipboard;
    /// Ctrl+Shift+V requests a paste, which arrives as an
    /// [`Event::Paste`] once the platform has read the clipboard.
    fn clipboard(&mut self, events: &[Event], term: &Term<EventProxy>, ctx: &egui::Context) {
        for event in events {
            let Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                continue;
            };

            if !modifiers.ctrl || !modifiers.shift {
                continue;
            }

            match key {
                Key::C => {
                    if let Some(text) = term.selection_to_string() {
                        ctx.copy_text(text);
                    }
                }
                Key::V => ctx.send_viewport_cmd(ViewportCommand::RequestPaste),
                _ => (),
            }
        }
    }

    /// Handle mouse selection on the terminal screen.
    ///
    /// Presses start a selection: a single press selects characters, a
    /// double press words, a triple press whole lines, and holding Ctrl
    /// switches to a rectangular block selection at any time, including
    /// mid-drag. While the application requested mouse reporting, Shift
    /// bypasses it so text can still be selected.
    fn selection(
        &mut self,
        events: &[Event],
        response: &Response,
        rect: egui::Rect,
        cell_size: Vec2,
        term: &mut Term<EventProxy>,
        now: f64,
    ) {
        let grid = term.grid();
        let columns = grid.columns();
        let screen_lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        let mouse_mode = term.mouse_mode();
        let ctrl = response.ctx.input(|i| i.modifiers.ctrl);

        // Presses start (or restart) the selection.
        for event in events {
            let Event::PointerButton {
                button: PointerButton::Primary,
                pressed: true,
                pos,
                modifiers,
            } = event
            else {
                continue;
            };
            if !rect.contains(*pos) {
                continue;
            }

            // The application owns the mouse; reporting its events to the
            // child arrives with the scrolling milestone.
            if mouse_mode && !modifiers.shift {
                continue;
            }

            let (point, side) =
                ui_mouse::point_at(rect, *pos, cell_size, columns, screen_lines, display_offset);

            // Count presses within the multi-click window.
            let count = match self.last_click {
                Some(click) if now - click.time <= CLICK_TIMEOUT => click.count + 1,
                _ => 1,
            };
            self.last_click = Some(Click { time: now, count });

            let ty = if modifiers.ctrl {
                SelectionType::Block
            } else {
                match count {
                    1 => SelectionType::Simple,
                    2 => SelectionType::Semantic,
                    _ => SelectionType::Lines,
                }
            };
            term.start_selection(ty, point, side);
        }

        // Drags extend the active selection.
        let shift = response.ctx.input(|i| i.modifiers.shift);
        let drag_active = response.dragged_by(PointerButton::Primary) && (!mouse_mode || shift);
        if drag_active && let Some(pos) = response.interact_pointer_pos() {
            let (point, side) =
                ui_mouse::point_at(rect, pos, cell_size, columns, screen_lines, display_offset);

            // The kind may change mid-drag: Ctrl switches to block
            // selection, mirroring upstream.
            let ty = if ctrl {
                SelectionType::Block
            } else {
                match self.last_click.map(|click| click.count) {
                    Some(2) => SelectionType::Semantic,
                    Some(count) if count >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                }
            };
            term.set_selection_ty(ty);
            term.update_selection(point, side);
        }
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

#[cfg(test)]
mod tests {
    use egui::{Event, ImeEvent};

    use super::TerminalView;

    /// A preedit event with the composition `text`.
    fn preedit(text: &str) -> Event {
        Event::Ime(ImeEvent::Preedit {
            text: text.to_owned(),
            active_range_chars: None,
        })
    }

    /// Starting a composition is reported once, so the view can scroll to
    /// the bottom like on any other input.
    #[test]
    fn ime_composition_start_is_detected() {
        let mut view = TerminalView::new();

        // The first non-empty preedit starts a composition.
        let (input, composing) = view.ime(&[preedit("你")]);
        assert!(input.is_empty());
        assert!(composing);

        // Extending the running composition is not a new start.
        let (input, composing) = view.ime(&[preedit("你好")]);
        assert!(input.is_empty());
        assert!(!composing);

        // Committing writes the text and ends the composition.
        let (input, composing) = view.ime(&[Event::Ime(ImeEvent::Commit("你好".to_owned()))]);
        assert_eq!(input, "你好".as_bytes());
        assert!(!composing);
        assert!(view.preedit.is_empty());

        // A fresh composition after the commit starts again.
        let (_, composing) = view.ime(&[preedit("好")]);
        assert!(composing);
    }

    /// Dismissing the IME (empty preedit) never counts as a start.
    #[test]
    fn ime_dismissal_is_no_composition_start() {
        let mut view = TerminalView::new();

        let (input, composing) = view.ime(&[preedit("")]);
        assert!(input.is_empty());
        assert!(!composing);
        assert!(view.preedit.is_empty());
    }
}
