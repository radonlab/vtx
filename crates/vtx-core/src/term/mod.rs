//! Terminal state machine.
//!
//! [`Term`] implements the `vte` ANSI handler trait, translating parsed
//! escape sequences into grid mutations. The current milestone covers
//! printable input, basic C0 controls (CR/LF/BS/TAB/BEL), fundamental CSI
//! sequences (CUP/CUU/CUD/CUF/CUB/ED/EL), and SGR attributes.

use std::cmp;
use std::ops::{Index, IndexMut, Range};

use unicode_width::UnicodeWidthChar;
use vte::ansi::{Attr, CharsetIndex, Color, Handler, NamedColor};

use crate::cell::{Cell, Flags};
use crate::color::Colors;
use crate::event::{Event, EventListener};
use crate::grid::{Dimensions, Grid};
use crate::index::{Column, Line, Point};
use crate::mode::TermMode;

#[cfg(test)]
mod tests;

/// Number of columns between automatically created tabstops.
const INITIAL_TABSTOPS: usize = 8;

/// Minimum number of columns.
///
/// A minimum of 2 is necessary to hold fullwidth unicode characters.
pub const MIN_COLUMNS: usize = 2;

/// Minimum number of visible lines.
pub const MIN_SCREEN_LINES: usize = 1;

/// Terminal state: grid, cursor, modes, and the ANSI handler implementation.
pub struct Term<T> {
    /// Event proxy for emitting [`Event`]s, like PTY replies.
    event_proxy: T,

    /// Currently active grid.
    grid: Grid<Cell>,

    /// Index into `charsets`, pointing to what ASCII is currently being
    /// mapped to.
    active_charset: CharsetIndex,

    /// Tabstops.
    tabs: TabStops,

    /// Mode flags.
    mode: TermMode,

    /// Scroll region.
    ///
    /// Range going from top to bottom of the terminal, indexed from the top
    /// of the viewport. The scroll region always covers the full viewport
    /// until DECSTBM support lands.
    scroll_region: Range<Line>,

    /// Modified terminal colors.
    colors: Colors,
}

impl<T: EventListener> Term<T> {
    /// Create a new terminal with the given dimensions and scrollback limit.
    pub fn new(
        columns: usize,
        screen_lines: usize,
        history_size: usize,
        event_proxy: T,
    ) -> Term<T> {
        Term {
            event_proxy,
            grid: Grid::new(screen_lines, columns, history_size),
            active_charset: CharsetIndex::default(),
            tabs: TabStops::new(columns),
            mode: TermMode::default(),
            scroll_region: Line(0)..Line(screen_lines as i32),
            colors: Colors::default(),
        }
    }

    /// Immutable access to the active grid.
    pub fn grid(&self) -> &Grid<Cell> {
        &self.grid
    }

    /// The currently active cursor position.
    pub fn cursor_point(&self) -> Point {
        self.grid.cursor.point
    }

    /// The terminal's mode flags.
    pub fn mode(&self) -> &TermMode {
        &self.mode
    }

    /// Mutable access to the terminal's mode flags.
    pub fn mode_mut(&mut self) -> &mut TermMode {
        &mut self.mode
    }

    /// The terminal's runtime color overrides.
    pub fn colors(&self) -> &Colors {
        &self.colors
    }

    /// Resize the terminal to the given dimensions.
    ///
    /// Minimal port of upstream `Term::resize`: vi mode cursor tracking,
    /// selection rotation, damage bookkeeping, and the alternate screen grid
    /// land together with their features in later milestones.
    pub fn resize(&mut self, columns: usize, screen_lines: usize) {
        let old_cols = self.grid.columns();
        let old_lines = self.grid.screen_lines();

        let num_cols = columns.max(MIN_COLUMNS);
        let num_lines = screen_lines.max(MIN_SCREEN_LINES);

        if old_cols == num_cols && old_lines == num_lines {
            return;
        }

        // The primary grid reflows into the new dimensions.
        self.grid.resize(true, num_lines, num_cols);

        // Recreate the tabstop list when the width changed.
        if old_cols != num_cols {
            self.tabs.resize(num_cols);
        }

        // Reset the scrolling region.
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
    }

    /// Insert a linebreak at the current cursor position.
    fn wrapline(&mut self) {
        if !self.mode.contains(TermMode::LINE_WRAP) {
            return;
        }

        self.grid.cursor_cell().flags.insert(Flags::WRAPLINE);

        if self.grid.cursor.point.line + 1 >= self.scroll_region.end {
            self.linefeed();
        } else {
            self.grid.cursor.point.line += 1;
        }

        self.grid.cursor.point.column = Column(0);
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Write `c` to the cell at the cursor position.
    #[inline(always)]
    fn write_at_cursor(&mut self, c: char) {
        let c = self.grid.cursor.charsets[self.active_charset].map(c);
        let fg = self.grid.cursor.template.fg;
        let bg = self.grid.cursor.template.bg;
        let flags = self.grid.cursor.template.flags;
        let extra = self.grid.cursor.template.extra.clone();

        let mut cursor_cell = self.grid.cursor_cell();

        // Clear all related cells when overwriting a fullwidth cell.
        if cursor_cell
            .flags
            .intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER)
        {
            let wide = cursor_cell.flags.contains(Flags::WIDE_CHAR);
            let point = self.grid.cursor.point;
            if wide && point.column < self.last_column() {
                self.grid[point.line][point.column + 1]
                    .flags
                    .remove(Flags::WIDE_CHAR_SPACER);
            } else if point.column > 0 {
                self.grid[point.line][point.column - 1].clear_wide();
            }

            // Remove leading spacers.
            if point.column <= 1 && point.line != self.topmost_line() {
                let column = self.last_column();
                self.grid[point.line - 1i32][column]
                    .flags
                    .remove(Flags::LEADING_WIDE_CHAR_SPACER);
            }

            cursor_cell = self.grid.cursor_cell();
        }

        cursor_cell.c = c;
        cursor_cell.fg = fg;
        cursor_cell.bg = bg;
        cursor_cell.flags = flags;
        cursor_cell.extra = extra;
    }

    /// Scroll the region up relative to the scroll region's origin.
    fn scroll_up_relative(&mut self, origin: Line, mut lines: usize) {
        lines = cmp::min(
            lines,
            (self.scroll_region.end - self.scroll_region.start).0 as usize,
        );

        let region = origin..self.scroll_region.end;

        self.grid.scroll_up(&region, lines);
    }

    /// Scroll the region down relative to the scroll region's origin.
    ///
    /// Consumed by reverse index (`ESC M`) and line insert/delete, which are
    /// not part of the M1 sequence set yet.
    #[expect(
        dead_code,
        reason = "wired up with reverse_index in the next milestone"
    )]
    fn scroll_down_relative(&mut self, origin: Line, mut lines: usize) {
        lines = cmp::min(
            lines,
            (self.scroll_region.end - self.scroll_region.start).0 as usize,
        );
        lines = cmp::min(lines, (self.scroll_region.end - origin).0 as usize);

        let region = origin..self.scroll_region.end;

        self.grid.scroll_down(&region, lines);
    }
}

impl<T: EventListener> Dimensions for Term<T> {
    #[inline]
    fn total_lines(&self) -> usize {
        self.grid.total_lines()
    }

    #[inline]
    fn screen_lines(&self) -> usize {
        self.grid.screen_lines()
    }

    #[inline]
    fn columns(&self) -> usize {
        self.grid.columns()
    }
}

impl<T: EventListener> vte::ansi::Handler for Term<T> {
    /// A character to be displayed.
    #[inline(never)]
    fn input(&mut self, c: char) {
        // Number of cells the char will occupy.
        let width = match c.width() {
            Some(width) => width,
            None => return,
        };

        // Handle zero-width characters.
        if width == 0 {
            // Get previous column.
            let mut column = self.grid.cursor.point.column;
            if !self.grid.cursor.input_needs_wrap {
                column.0 = column.saturating_sub(1);
            }

            // Put zerowidth characters over first fullwidth character cell.
            let line = self.grid.cursor.point.line;
            if self.grid[line][column]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
            {
                column.0 = column.saturating_sub(1);
            }

            self.grid[line][column].push_zerowidth(c);
            return;
        }

        // Move cursor to next line.
        if self.grid.cursor.input_needs_wrap {
            self.wrapline();
        }

        // If in insert mode, first shift cells to the right.
        let columns = self.columns();
        if self.mode.contains(TermMode::INSERT) && self.grid.cursor.point.column + width < columns {
            let line = self.grid.cursor.point.line;
            let col = self.grid.cursor.point.column;
            let row = &mut self.grid[line][..];

            for col in (col.0..(columns - width)).rev() {
                row.swap(col + width, col);
            }
        }

        if width == 1 {
            self.write_at_cursor(c);
        } else {
            if self.grid.cursor.point.column + 1 >= columns {
                if self.mode.contains(TermMode::LINE_WRAP) {
                    // Insert placeholder before wide char if glyph does not fit in this row.
                    self.grid
                        .cursor
                        .template
                        .flags
                        .insert(Flags::LEADING_WIDE_CHAR_SPACER);
                    self.write_at_cursor(' ');
                    self.grid
                        .cursor
                        .template
                        .flags
                        .remove(Flags::LEADING_WIDE_CHAR_SPACER);
                    self.wrapline();
                } else {
                    // Prevent out of bounds crash when linewrapping is disabled.
                    self.grid.cursor.input_needs_wrap = true;
                    return;
                }
            }

            // Write full width glyph to current cursor cell.
            self.grid.cursor.template.flags.insert(Flags::WIDE_CHAR);
            self.write_at_cursor(c);
            self.grid.cursor.template.flags.remove(Flags::WIDE_CHAR);

            // Write spacer to cell following the wide glyph.
            self.grid.cursor.point.column += 1;
            self.grid
                .cursor
                .template
                .flags
                .insert(Flags::WIDE_CHAR_SPACER);
            self.write_at_cursor(' ');
            self.grid
                .cursor
                .template
                .flags
                .remove(Flags::WIDE_CHAR_SPACER);
        }

        if self.grid.cursor.point.column + 1 < columns {
            self.grid.cursor.point.column += 1;
        } else {
            self.grid.cursor.input_needs_wrap = true;
        }
    }

    /// Set cursor to position.
    #[inline]
    fn goto(&mut self, line: i32, col: usize) {
        let line = Line(line);
        let col = Column(col);

        let (y_offset, max_y) = if self.mode.contains(TermMode::ORIGIN) {
            (self.scroll_region.start, self.scroll_region.end - 1)
        } else {
            (Line(0), self.bottommost_line())
        };

        self.grid.cursor.point.line = cmp::max(cmp::min(line + y_offset, max_y), Line(0));
        self.grid.cursor.point.column = cmp::min(col, self.last_column());
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Set cursor to specific row.
    #[inline]
    fn goto_line(&mut self, line: i32) {
        self.goto(line, self.grid.cursor.point.column.0)
    }

    /// Set cursor to specific column.
    #[inline]
    fn goto_col(&mut self, col: usize) {
        self.goto(self.grid.cursor.point.line.0, col)
    }

    /// Move cursor up `rows`.
    #[inline]
    fn move_up(&mut self, rows: usize) {
        let line = self.grid.cursor.point.line - rows;
        let column = self.grid.cursor.point.column;
        self.goto(line.0, column.0)
    }

    /// Move cursor down `rows`.
    #[inline]
    fn move_down(&mut self, rows: usize) {
        let line = self.grid.cursor.point.line + rows;
        let column = self.grid.cursor.point.column;
        self.goto(line.0, column.0)
    }

    /// Move cursor forward `cols`.
    #[inline]
    fn move_forward(&mut self, cols: usize) {
        let last_column = cmp::min(self.grid.cursor.point.column + cols, self.last_column());

        self.grid.cursor.point.column = last_column;
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Move cursor backward `cols`.
    #[inline]
    fn move_backward(&mut self, cols: usize) {
        let column = self.grid.cursor.point.column.saturating_sub(cols);

        self.grid.cursor.point.column = Column(column);
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Insert tab at cursor position.
    #[inline]
    fn put_tab(&mut self, mut count: u16) {
        // A tab after the last column is the same as a linebreak.
        if self.grid.cursor.input_needs_wrap {
            self.wrapline();
            return;
        }

        while self.grid.cursor.point.column < self.columns() && count != 0 {
            count -= 1;

            let c = self.grid.cursor.charsets[self.active_charset].map('\t');
            let cell = self.grid.cursor_cell();
            if cell.c == ' ' {
                cell.c = c;
            }

            loop {
                if (self.grid.cursor.point.column + 1) == self.columns() {
                    break;
                }

                self.grid.cursor.point.column += 1;

                if self.tabs[self.grid.cursor.point.column] {
                    break;
                }
            }
        }
    }

    /// Backspace.
    #[inline]
    fn backspace(&mut self) {
        if self.grid.cursor.point.column > Column(0) {
            self.grid.cursor.point.column -= 1;
            self.grid.cursor.input_needs_wrap = false;
        }
    }

    /// Carriage return.
    #[inline]
    fn carriage_return(&mut self) {
        self.grid.cursor.point.column = Column(0);
        self.grid.cursor.input_needs_wrap = false;
    }

    /// Linefeed.
    #[inline]
    fn linefeed(&mut self) {
        let next = self.grid.cursor.point.line + 1;
        if next == self.scroll_region.end {
            self.scroll_up_relative(self.scroll_region.start, 1);
        } else if next < self.screen_lines() {
            self.grid.cursor.point.line += 1;
        }
    }

    /// Ring the bell.
    ///
    /// Bell events are forwarded through the event system, which is not
    /// wired up yet; the sequence is accepted but ignored for now.
    #[inline]
    fn bell(&mut self) {}

    /// Identify the terminal (should write back to the pty stream).
    ///
    /// Primary device attributes are answered as a VT102 with sixel
    /// graphics support, matching alacritty's report.
    #[inline]
    fn identify_terminal(&mut self, intermediate: Option<char>) {
        if intermediate.is_none() {
            let text = String::from("\x1b[?6c");
            self.event_proxy.send_event(Event::PtyWrite(text));
        }
    }

    /// Report device status.
    #[inline]
    fn device_status(&mut self, arg: usize) {
        match arg {
            // Device status report.
            5 => {
                let text = String::from("\x1b[0n");
                self.event_proxy.send_event(Event::PtyWrite(text));
            }
            // Cursor position report.
            6 => {
                let pos = self.grid.cursor.point;
                let text = format!("\x1b[{};{}R", pos.line + 1, pos.column + 1);
                self.event_proxy.send_event(Event::PtyWrite(text));
            }
            _ => (),
        };
    }

    /// Clear current line.
    #[inline]
    fn clear_line(&mut self, mode: vte::ansi::LineClearMode) {
        let cursor = &self.grid.cursor;
        let bg = cursor.template.bg;
        let point = cursor.point;

        let (left, right) = match mode {
            vte::ansi::LineClearMode::Right if cursor.input_needs_wrap => return,
            vte::ansi::LineClearMode::Right => (point.column, Column(self.columns())),
            vte::ansi::LineClearMode::Left => (Column(0), point.column + 1),
            vte::ansi::LineClearMode::All => (Column(0), Column(self.columns())),
        };

        let row = &mut self.grid[point.line];
        for cell in &mut row[left..right] {
            *cell = bg.into();
        }
    }

    /// Clear screen.
    #[inline]
    fn clear_screen(&mut self, mode: vte::ansi::ClearMode) {
        let bg = self.grid.cursor.template.bg;
        let screen_lines = self.screen_lines();

        match mode {
            vte::ansi::ClearMode::Above => {
                let cursor = self.grid.cursor.point;

                // If clearing more than one line.
                if cursor.line > 1 {
                    // Fully clear all lines before the current line.
                    self.grid.reset_region(..cursor.line);
                }

                // Clear up to the current column in the current line.
                let end = cmp::min(cursor.column + 1, Column(self.columns()));
                for cell in &mut self.grid[cursor.line][..end] {
                    *cell = bg.into();
                }
            }
            vte::ansi::ClearMode::Below => {
                let cursor = self.grid.cursor.point;
                for cell in &mut self.grid[cursor.line][cursor.column..] {
                    *cell = bg.into();
                }

                if (cursor.line.0 as usize) < screen_lines - 1 {
                    self.grid.reset_region((cursor.line + 1)..);
                }
            }
            vte::ansi::ClearMode::All => {
                self.grid.clear_viewport();
            }
            vte::ansi::ClearMode::Saved if self.history_size() > 0 => {
                self.grid.clear_history();
            }
            // We have no history to clear.
            vte::ansi::ClearMode::Saved => (),
        }
    }

    /// Set a terminal attribute.
    #[inline]
    fn terminal_attribute(&mut self, attr: Attr) {
        let cursor = &mut self.grid.cursor;
        match attr {
            Attr::Foreground(color) => cursor.template.fg = color,
            Attr::Background(color) => cursor.template.bg = color,
            Attr::UnderlineColor(color) => cursor.template.set_underline_color(color),
            Attr::Reset => {
                cursor.template.fg = Color::Named(NamedColor::Foreground);
                cursor.template.bg = Color::Named(NamedColor::Background);
                cursor.template.flags = Flags::empty();
                cursor.template.set_underline_color(None);
            }
            Attr::Reverse => cursor.template.flags.insert(Flags::INVERSE),
            Attr::CancelReverse => cursor.template.flags.remove(Flags::INVERSE),
            Attr::Bold => cursor.template.flags.insert(Flags::BOLD),
            Attr::CancelBold => cursor.template.flags.remove(Flags::BOLD),
            Attr::Dim => cursor.template.flags.insert(Flags::DIM),
            Attr::CancelBoldDim => cursor.template.flags.remove(Flags::BOLD | Flags::DIM),
            Attr::Italic => cursor.template.flags.insert(Flags::ITALIC),
            Attr::CancelItalic => cursor.template.flags.remove(Flags::ITALIC),
            Attr::Underline => {
                cursor.template.flags.remove(Flags::ALL_UNDERLINES);
                cursor.template.flags.insert(Flags::UNDERLINE);
            }
            Attr::DoubleUnderline => {
                cursor.template.flags.remove(Flags::ALL_UNDERLINES);
                cursor.template.flags.insert(Flags::DOUBLE_UNDERLINE);
            }
            Attr::Undercurl => {
                cursor.template.flags.remove(Flags::ALL_UNDERLINES);
                cursor.template.flags.insert(Flags::UNDERCURL);
            }
            Attr::DottedUnderline => {
                cursor.template.flags.remove(Flags::ALL_UNDERLINES);
                cursor.template.flags.insert(Flags::DOTTED_UNDERLINE);
            }
            Attr::DashedUnderline => {
                cursor.template.flags.remove(Flags::ALL_UNDERLINES);
                cursor.template.flags.insert(Flags::DASHED_UNDERLINE);
            }
            Attr::CancelUnderline => cursor.template.flags.remove(Flags::ALL_UNDERLINES),
            Attr::Hidden => cursor.template.flags.insert(Flags::HIDDEN),
            Attr::CancelHidden => cursor.template.flags.remove(Flags::HIDDEN),
            Attr::Strike => cursor.template.flags.insert(Flags::STRIKEOUT),
            Attr::CancelStrike => cursor.template.flags.remove(Flags::STRIKEOUT),
            _ => (),
        }
    }
}

/// Tabstop storage for the terminal.
struct TabStops {
    tabs: Vec<bool>,
}

impl TabStops {
    fn new(columns: usize) -> TabStops {
        TabStops {
            tabs: (0..columns).map(|i| i % INITIAL_TABSTOPS == 0).collect(),
        }
    }

    /// Resize the tabstop list to the given number of columns.
    ///
    /// Newly covered columns follow the default interval pattern, matching
    /// upstream behavior.
    fn resize(&mut self, columns: usize) {
        let mut index = self.tabs.len();
        self.tabs.resize_with(columns, || {
            let is_tabstop = index.is_multiple_of(INITIAL_TABSTOPS);
            index += 1;
            is_tabstop
        });
    }
}

impl Index<Column> for TabStops {
    type Output = bool;

    #[inline]
    fn index(&self, index: Column) -> &bool {
        &self.tabs[index.0]
    }
}

impl IndexMut<Column> for TabStops {
    #[inline]
    fn index_mut(&mut self, index: Column) -> &mut bool {
        &mut self.tabs[index.0]
    }
}
