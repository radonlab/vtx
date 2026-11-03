//! Terminal state machine.
//!
//! [`Term`] implements the `vte` ANSI handler trait, translating parsed
//! escape sequences into grid mutations. The current milestone covers
//! printable input, basic C0 controls (CR/LF/BS/TAB/SO/SI/BEL/RI),
//! fundamental CSI sequences (CUP/CUU/CUD/CUF/CUB/ED/EL), SGR attributes,
//! the DEC private modes around the alternate screen and reporting
//! protocols (DECSET/DECRST incl. mouse reporting, focus reporting,
//! bracketed paste; DECSC/DECRC, RIS, DECAWM, DECOM), the scrolling region
//! (DECSTBM), the line/character editing sequences (ICH/DCH/DL/IL/ECH),
//! tabstop manipulation (HTS/TBC/CHT/CBT), the DEC character sets (SCS/SO/
//! SI), and the public modes IRM/LNM.

use std::cmp;
use std::mem;
use std::ops::{Index, IndexMut, Range};

use unicode_width::UnicodeWidthChar;
use vte::ansi::{
    Attr, CharsetIndex, Color, Handler, Mode, NamedColor, NamedMode, NamedPrivateMode, PrivateMode,
    StandardCharset, TabulationClearMode,
};

use crate::cell::{Cell, Flags, Hyperlink};
use crate::color::Colors;
use crate::event::{Event, EventListener};
use crate::grid::{Dimensions, Grid, Scroll};
use crate::index::{Column, Line, Point, Side};
use crate::mode::TermMode;
use crate::selection::{Selection, SelectionRange, SelectionType};

pub mod search;
#[cfg(test)]
mod tests;

/// Number of columns between automatically created tabstops.
const INITIAL_TABSTOPS: usize = 8;

/// Max size of the window title stack.
const TITLE_STACK_MAX_DEPTH: usize = 4096;

/// Minimum number of columns.
///
/// A minimum of 2 is necessary to hold fullwidth unicode characters.
pub const MIN_COLUMNS: usize = 2;

/// Minimum number of visible lines.
pub const MIN_SCREEN_LINES: usize = 1;

/// The shape of the terminal cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// A filled block over the whole cell, `▒`.
    #[default]
    Block,
    /// An underscore at the cell bottom, `_`.
    Underline,
    /// A vertical bar at the cell's left edge, `⎸`.
    Beam,
}

/// The style of the terminal cursor, as requested by DECSCUSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorStyle {
    /// Which shape the cursor takes.
    pub shape: CursorShape,
    /// Whether the cursor blinks.
    pub blinking: bool,
}

/// Terminal state: grid, cursor, modes, and the ANSI handler implementation.
pub struct Term<T> {
    /// Event proxy for emitting [`Event`]s, like PTY replies.
    event_proxy: T,

    /// Currently active grid.
    ///
    /// Tracks the screen buffer currently in use. While the alternate
    /// screen buffer is active, this is the alternate grid. Otherwise it is
    /// the primary screen buffer.
    grid: Grid<Cell>,

    /// Currently inactive grid.
    ///
    /// The opposite of the active grid. While the alternate screen buffer
    /// is active, this is the primary grid. Otherwise it is the alternate
    /// screen buffer.
    inactive_grid: Grid<Cell>,

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

    /// Window title set by the application (OSC 0/2).
    title: Option<String>,

    /// Saved window titles (CSI 22/23 t).
    title_stack: Vec<Option<String>>,

    /// Currently active selection, if any.
    selection: Option<Selection>,

    /// Cursor style requested by the child (DECSCUSR).
    ///
    /// `None` means the child never asked; the frontend then uses its
    /// default style.
    cursor_style: Option<CursorStyle>,
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
            // The alternate screen has no scrollback history.
            inactive_grid: Grid::new(screen_lines, columns, 0),
            active_charset: CharsetIndex::default(),
            tabs: TabStops::new(columns),
            mode: TermMode::default(),
            scroll_region: Line(0)..Line(screen_lines as i32),
            colors: Colors::default(),
            title: None,
            title_stack: Vec::new(),
            selection: None,
            cursor_style: None,
        }
    }

    /// Immutable access to the active grid.
    pub fn grid(&self) -> &Grid<Cell> {
        &self.grid
    }

    /// Mutable access to the active grid.
    ///
    /// Bypasses the ANSI state machine; meant for tests and tools that
    /// prepare grid content directly.
    pub fn grid_mut(&mut self) -> &mut Grid<Cell> {
        &mut self.grid
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

    /// The window title set by the application, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Whether the application requested mouse event reporting.
    ///
    /// While active, the frontend encodes mouse events and writes them to
    /// the PTY instead of selecting text (unless Shift is held).
    pub fn mouse_mode(&self) -> bool {
        self.mode.intersects(TermMode::MOUSE_MODE)
    }

    /// The cursor style requested by the child (DECSCUSR).
    ///
    /// `None` means the child never asked; the frontend then uses its
    /// default style.
    pub fn cursor_style(&self) -> Option<CursorStyle> {
        self.cursor_style
    }

    /// Start a new selection of the given type at `point`.
    pub fn start_selection(&mut self, ty: SelectionType, point: Point, side: Side) {
        self.selection = Some(Selection::new(ty, point, side));
    }

    /// Update the end of the active selection, if any.
    pub fn update_selection(&mut self, point: Point, side: Side) {
        if let Some(selection) = self.selection.as_mut() {
            selection.update(point, side);
        }
    }

    /// Switch the kind of the active selection, if any.
    ///
    /// Upstream switches to block selection while Ctrl is held during a
    /// drag.
    pub fn set_selection_ty(&mut self, ty: SelectionType) {
        if let Some(selection) = self.selection.as_mut() {
            selection.ty = ty;
        }
    }

    /// Remove the active selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// The active selection's range in grid coordinates, if it covers any
    /// cells.
    pub fn selection_range(&self) -> Option<SelectionRange> {
        self.selection
            .as_ref()
            .and_then(|selection| selection.to_range(self))
    }

    /// Convert the active selection to a String.
    pub fn selection_to_string(&self) -> Option<String> {
        let selection_range = self.selection.as_ref().and_then(|s| s.to_range(self))?;
        let SelectionRange { start, end, .. } = selection_range;

        let mut res = String::new();

        match self.selection.as_ref() {
            Some(Selection {
                ty: SelectionType::Block,
                ..
            }) => {
                for line in (start.line.0..end.line.0).map(Line::from) {
                    res += self
                        .line_to_string(line, start.column..end.column, start.column.0 != 0)
                        .trim_end();
                    res += "\n";
                }

                res += self
                    .line_to_string(end.line, start.column..end.column, true)
                    .trim_end();
            }
            Some(Selection {
                ty: SelectionType::Lines,
                ..
            }) => {
                res = self.bounds_to_string(start, end) + "\n";
            }
            _ => {
                res = self.bounds_to_string(start, end);
            }
        }

        Some(res)
    }

    /// Convert the range between two points to a String.
    pub fn bounds_to_string(&self, start: Point, end: Point) -> String {
        let mut res = String::new();

        for line in (start.line.0..=end.line.0).map(Line::from) {
            let start_col = if line == start.line {
                start.column
            } else {
                Column(0)
            };
            let end_col = if line == end.line {
                end.column
            } else {
                self.last_column()
            };

            res += &self.line_to_string(line, start_col..end_col, line == end.line);
        }

        res.strip_suffix('\n').map(str::to_owned).unwrap_or(res)
    }

    /// Convert a single line in the grid to a String.
    fn line_to_string(
        &self,
        line: Line,
        mut cols: Range<Column>,
        include_wrapped_wide: bool,
    ) -> String {
        let mut text = String::new();

        let grid_line = &self.grid[line];
        let line_length = cmp::min(grid_line.line_length(), cols.end + 1);

        // Include wide char when trailing spacer is selected.
        if grid_line[cols.start]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
        {
            cols.start -= 1;
        }

        let mut tab_mode = false;
        for column in (cols.start.0..line_length.0).map(Column::from) {
            let cell = &grid_line[column];

            // Skip over cells until the next tab-stop once a tab was found.
            if tab_mode {
                if self.tabs[column] || cell.c != ' ' {
                    tab_mode = false;
                } else {
                    continue;
                }
            }

            if cell.c == '\t' {
                tab_mode = true;
            }

            if !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                // Push the cell's primary character.
                text.push(cell.c);

                // Push zero-width characters.
                for c in cell.zerowidth().into_iter().flatten() {
                    text.push(*c);
                }
            }
        }

        if cols.end >= self.columns() - 1
            && (line_length.0 == 0
                || !self.grid[line][line_length - 1]
                    .flags
                    .contains(Flags::WRAPLINE))
        {
            text.push('\n');
        }

        // If a wide char is not part of the selection, but its leading
        // spacer is, include it.
        if line_length == self.columns()
            && line_length.0 >= 2
            && grid_line[line_length - 1]
                .flags
                .contains(Flags::LEADING_WIDE_CHAR_SPACER)
            && include_wrapped_wide
        {
            text.push(self.grid[line - 1i32][Column(0)].c);
        }

        text
    }

    /// Scroll the viewport within the scrollback history.
    ///
    /// Only the display offset changes; the grid itself is untouched. While
    /// the offset is non-zero, newly arriving lines keep the viewport
    /// stationary on the same content, since `Grid::scroll_up` moves the
    /// offset along with the scrolled lines.
    ///
    /// Upstream also clamps the vi mode cursor, invalidates damage, and
    /// requests a cursor-dirty event here; those arrive with their
    /// milestones.
    pub fn scroll_display(&mut self, scroll: Scroll) {
        self.grid.scroll_display(scroll);
    }

    /// Resize the terminal to the given dimensions.
    ///
    /// Both grids are resized: the primary screen reflows into the new
    /// dimensions, while the alternate screen keeps its fixed-size viewport.
    ///
    /// The selection is dropped when the width changes (its column anchors
    /// no longer line up with the reflowed content) and rotated along with
    /// the content otherwise. Vi mode cursor tracking and damage bookkeeping
    /// land together with their features in later milestones.
    pub fn resize(&mut self, columns: usize, screen_lines: usize) {
        let old_cols = self.grid.columns();
        let old_lines = self.grid.screen_lines();

        let num_cols = columns.max(MIN_COLUMNS);
        let num_lines = screen_lines.max(MIN_SCREEN_LINES);

        if old_cols == num_cols && old_lines == num_lines {
            return;
        }

        // Lines the content moves up by when the viewport shrinks, clamped
        // to the available history.
        let history_size = self.history_size() as i32;
        let mut delta = num_lines as i32 - old_lines as i32;
        let min_delta = cmp::min(0, num_lines as i32 - self.grid.cursor.point.line.0 - 1);
        delta = cmp::min(cmp::max(delta, min_delta), history_size);

        // Only the primary screen reflows; it has scrollback to redistribute.
        // Whichever grid is currently the alternate one is resized without
        // reflow.
        let is_alt = self.mode.contains(TermMode::ALT_SCREEN);
        self.grid.resize(!is_alt, num_lines, num_cols);
        self.inactive_grid.resize(is_alt, num_lines, num_cols);

        // Invalidate the selection and tabs only when necessary.
        if old_cols != num_cols {
            self.selection = None;

            // Recreate the tabstop list when the width changed.
            self.tabs.resize(num_cols);
        } else if let Some(selection) = self.selection.take() {
            let max_lines = cmp::max(num_lines, old_lines) as i32;
            let range = Line(0)..Line(max_lines);
            self.selection = selection.rotate(self, &range, -delta);
        }

        // Reset the scrolling region.
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
    }

    /// Swap primary and alternate screen buffer.
    ///
    /// On the way into the alternate screen, the alt grid starts out with
    /// the primary cursor, the primary screen's saved cursor is dropped, and
    /// the alt grid contents are cleared. On the way back out, the primary
    /// screen is restored exactly as it was left.
    fn swap_alt(&mut self) {
        if !self.mode.contains(TermMode::ALT_SCREEN) {
            // Set the alt screen cursor to the current primary screen
            // cursor.
            self.inactive_grid.cursor = self.grid.cursor.clone();

            // Drop information about the primary screen's saved cursor.
            self.grid.saved_cursor = self.grid.cursor.clone();

            // Reset the alternate screen contents.
            self.inactive_grid.reset_region(..);
        }

        mem::swap(&mut self.grid, &mut self.inactive_grid);
        self.mode ^= TermMode::ALT_SCREEN;
        self.selection = None;
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

        // Scroll the selection along with the content.
        self.selection = self
            .selection
            .take()
            .and_then(|s| s.rotate(self, &region, lines as i32));

        self.grid.scroll_up(&region, lines);
    }

    /// Scroll the region down relative to the scroll region's origin.
    ///
    /// Consumed by reverse index (`ESC M`) and line insert (`CSI L`).
    fn scroll_down_relative(&mut self, origin: Line, mut lines: usize) {
        lines = cmp::min(
            lines,
            (self.scroll_region.end - self.scroll_region.start).0 as usize,
        );
        lines = cmp::min(lines, (self.scroll_region.end - origin).0 as usize);

        let region = origin..self.scroll_region.end;

        // Scroll the selection along with the content.
        self.selection = self
            .selection
            .take()
            .and_then(|s| s.rotate(self, &region, -(lines as i32)));

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

    /// Insert blank characters (ICH, `CSI n @`).
    ///
    /// Existing characters from the cursor onward shift right; the shifted
    /// in cells carry the current background color.
    #[inline]
    fn insert_blank(&mut self, count: usize) {
        let cursor = &self.grid.cursor;
        let bg = cursor.template.bg;

        // Ensure inserting within terminal bounds.
        let count = cmp::min(count, self.columns() - cursor.point.column.0);

        let source = cursor.point.column;
        let destination = cursor.point.column.0 + count;
        let num_cells = self.columns() - destination;

        let line = cursor.point.line;
        let row = &mut self.grid[line][..];

        for offset in (0..num_cells).rev() {
            row.swap(destination + offset, source.0 + offset);
        }

        // Cells were just moved out toward the end of the line; fill in
        // between source and destination with blanks.
        for cell in &mut row[source.0..destination] {
            *cell = bg.into();
        }
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

    /// Move cursor forward `count` tab stops (CHT, `CSI n I`).
    ///
    /// The cursor clamps to the last column when no further tabstop exists.
    #[inline]
    fn move_forward_tabs(&mut self, count: u16) {
        let num_cols = self.columns();
        for _ in 0..count {
            let mut col = self.grid.cursor.point.column;

            if col.0 == num_cols - 1 {
                break;
            }

            for i in col.0 + 1..num_cols {
                col = Column(i);
                if self.tabs[col] {
                    break;
                }
            }

            self.grid.cursor.point.column = col;
        }
    }

    /// Move cursor backward `count` tab stops (CBT, `CSI n Z`).
    ///
    /// The cursor stays at its column when no earlier tabstop exists.
    #[inline]
    fn move_backward_tabs(&mut self, count: u16) {
        for _ in 0..count {
            let mut col = self.grid.cursor.point.column;

            if col == Column(0) {
                break;
            }

            for i in (0..col.0).rev() {
                if self.tabs[Column(i)] {
                    col = Column(i);
                    break;
                }
            }
            self.grid.cursor.point.column = col;
        }
    }

    /// Set a tabstop at the cursor position (HTS, `ESC H`).
    #[inline]
    fn set_horizontal_tabstop(&mut self) {
        self.tabs[self.grid.cursor.point.column] = true;
    }

    /// Clear tab stops (TBC, `CSI n g`).
    ///
    /// `CSI 0 g` clears the stop under the cursor; `CSI 3 g` clears all
    /// stops.
    #[inline]
    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        match mode {
            TabulationClearMode::Current => {
                self.tabs[self.grid.cursor.point.column] = false;
            }
            TabulationClearMode::All => self.tabs.clear_all(),
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
    ///
    /// A carriage return executes the linefeed control as well while LNM is
    /// active.
    #[inline]
    fn carriage_return(&mut self) {
        if self.mode.contains(TermMode::LINE_FEED_NEW_LINE) {
            self.linefeed();
        }

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
    /// The event is forwarded through the event system; the frontend
    /// decides whether to flash the display or play a sound.
    #[inline]
    fn bell(&mut self) {
        self.event_proxy.send_event(Event::Bell);
    }

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

        // Drop the selection on lines that were just cleared.
        let range = point.line..=point.line;
        self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
    }

    /// Insert blank lines (IL, `CSI n L`).
    ///
    /// Lines from the cursor down shift toward the region bottom; a no-op
    /// when the cursor is outside the scrolling region.
    #[inline]
    fn insert_blank_lines(&mut self, lines: usize) {
        let origin = self.grid.cursor.point.line;
        if self.scroll_region.contains(&origin) {
            self.scroll_down_relative(origin, lines);
        }
    }

    /// Delete lines (DL, `CSI n M`).
    ///
    /// Lines below the cursor shift up; a no-op when the cursor is outside
    /// the scrolling region.
    #[inline]
    fn delete_lines(&mut self, lines: usize) {
        let origin = self.grid.cursor.point.line;
        let lines = cmp::min(self.screen_lines() - origin.0 as usize, lines);

        if lines > 0 && self.scroll_region.contains(&origin) {
            self.scroll_up_relative(origin, lines);
        }
    }

    /// Erase characters (ECH, `CSI n X`).
    ///
    /// The cells are reset to blanks with the current background color;
    /// nothing shifts.
    #[inline]
    fn erase_chars(&mut self, count: usize) {
        let cursor = &self.grid.cursor;

        let start = cursor.point.column;
        let end = cmp::min(start + count, Column(self.columns()));

        // Cleared cells have the current background color set.
        let bg = cursor.template.bg;
        let line = cursor.point.line;
        let row = &mut self.grid[line];
        for cell in &mut row[start..end] {
            *cell = bg.into();
        }
    }

    /// Delete characters (DCH, `CSI n P`).
    ///
    /// Like the delete key: everything to the right of the cursor shifts
    /// left, blanks fill in at the line end.
    #[inline]
    fn delete_chars(&mut self, count: usize) {
        let columns = self.columns();
        let cursor = &self.grid.cursor;
        let bg = cursor.template.bg;

        // Ensure deleting within terminal bounds.
        let count = cmp::min(count, columns);

        let start = cursor.point.column.0;
        let end = cmp::min(start + count, columns - 1);
        let num_cells = columns - end;

        let line = cursor.point.line;
        let row = &mut self.grid[line][..];

        for offset in 0..num_cells {
            row.swap(start + offset, end + offset);
        }

        // Clear the last `count` cells in the row.
        let end = columns - count;
        for cell in &mut row[end..] {
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

                // Drop the selection on the cleared lines.
                let range = Line(0)..=cursor.line;
                self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
            }
            vte::ansi::ClearMode::Below => {
                let cursor = self.grid.cursor.point;
                for cell in &mut self.grid[cursor.line][cursor.column..] {
                    *cell = bg.into();
                }

                if (cursor.line.0 as usize) < screen_lines - 1 {
                    self.grid.reset_region((cursor.line + 1)..);
                }

                // Drop the selection on the cleared lines.
                let range = cursor.line..Line(screen_lines as i32);
                self.selection = self.selection.take().filter(|s| !s.intersects_range(range));
            }
            vte::ansi::ClearMode::All => {
                self.grid.clear_viewport();

                self.selection = None;
            }
            vte::ansi::ClearMode::Saved if self.history_size() > 0 => {
                self.grid.clear_history();

                // Drop the selection on the removed history lines.
                self.selection = self
                    .selection
                    .take()
                    .filter(|s| !s.intersects_range(..Line(0)));
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
            // Blink modes are accepted but ignored; text never blinks.
            Attr::BlinkSlow | Attr::BlinkFast | Attr::CancelBlink => (),
        }
    }

    /// Save cursor position (DECSC, `ESC 7` / `CSI s`).
    #[inline]
    fn save_cursor_position(&mut self) {
        self.grid.saved_cursor = self.grid.cursor.clone();
    }

    /// Restore cursor position (DECRC, `ESC 8` / `CSI u`).
    #[inline]
    fn restore_cursor_position(&mut self) {
        self.grid.cursor = self.grid.saved_cursor.clone();
    }

    /// Reset the terminal state (RIS, `ESC c`).
    ///
    /// A full reset: both grids, the scroll region, the tabstops, and all
    /// non-default modes are restored to their initial values. A reset
    /// issued from the alternate screen first swaps back to the primary
    /// screen.
    #[inline]
    fn reset_state(&mut self) {
        if self.mode.contains(TermMode::ALT_SCREEN) {
            mem::swap(&mut self.grid, &mut self.inactive_grid);
        }
        self.active_charset = CharsetIndex::default();
        self.grid.reset();
        self.inactive_grid.reset();
        self.scroll_region = Line(0)..Line(self.screen_lines() as i32);
        self.tabs = TabStops::new(self.columns());
        self.title_stack.clear();
        self.title = None;
        self.selection = None;
        self.cursor_style = None;

        // Preserve vi mode across resets.
        self.mode &= TermMode::VI;
        self.mode.insert(TermMode::default());
    }

    /// Reverse index (RI, `ESC M`).
    ///
    /// Move the cursor up one line; at the top of the scrolling region the
    /// region scrolls down instead.
    #[inline]
    fn reverse_index(&mut self) {
        // If the cursor is at the top of the scrolling region, scroll it.
        if self.grid.cursor.point.line == self.scroll_region.start {
            self.scroll_down_relative(self.scroll_region.start, 1);
        } else {
            self.grid.cursor.point.line = cmp::max(self.grid.cursor.point.line - 1, Line(0));
        }
    }

    /// Set a private mode (DECSET, `CSI ? h`).
    ///
    /// Modes for later milestones (mouse reporting, focus reporting,
    /// bracketed paste, origin, column mode, blinking cursor, synchronized
    /// updates) are accepted but ignored.
    #[inline]
    fn set_private_mode(&mut self, mode: PrivateMode) {
        let mode = match mode {
            PrivateMode::Named(mode) => mode,
            PrivateMode::Unknown(mode) => {
                log::debug!("ignoring unknown mode {mode} in set_private_mode");
                return;
            }
        };

        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor
                if !self.mode.contains(TermMode::ALT_SCREEN) =>
            {
                self.swap_alt();
            }
            NamedPrivateMode::LineWrap => self.mode.insert(TermMode::LINE_WRAP),
            NamedPrivateMode::ShowCursor => self.mode.insert(TermMode::SHOW_CURSOR),
            NamedPrivateMode::AlternateScroll => self.mode.insert(TermMode::ALTERNATE_SCROLL),
            NamedPrivateMode::CursorKeys => self.mode.insert(TermMode::APP_CURSOR),
            NamedPrivateMode::Origin => {
                self.mode.insert(TermMode::ORIGIN);
                self.goto(0, 0);
            }
            // Mouse reporting protocols are mutually exclusive.
            NamedPrivateMode::ReportMouseClicks => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_REPORT_CLICK);
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_DRAG);
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.mode.remove(TermMode::MOUSE_MODE);
                self.mode.insert(TermMode::MOUSE_MOTION);
            }
            NamedPrivateMode::ReportFocusInOut => self.mode.insert(TermMode::FOCUS_IN_OUT),
            NamedPrivateMode::BracketedPaste => self.mode.insert(TermMode::BRACKETED_PASTE),
            // Mouse reporting encodings are mutually exclusive.
            NamedPrivateMode::SgrMouse => {
                self.mode.remove(TermMode::UTF8_MOUSE);
                self.mode.insert(TermMode::SGR_MOUSE);
            }
            NamedPrivateMode::Utf8Mouse => {
                self.mode.remove(TermMode::SGR_MOUSE);
                self.mode.insert(TermMode::UTF8_MOUSE);
            }
            _ => (),
        }
    }

    /// Unset a private mode (DECRST, `CSI ? l`).
    ///
    /// See [`set_private_mode`](Term::set_private_mode) for the set of
    /// supported modes.
    #[inline]
    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let mode = match mode {
            PrivateMode::Named(mode) => mode,
            PrivateMode::Unknown(mode) => {
                log::debug!("ignoring unknown mode {mode} in unset_private_mode");
                return;
            }
        };

        match mode {
            NamedPrivateMode::SwapScreenAndSetRestoreCursor
                if self.mode.contains(TermMode::ALT_SCREEN) =>
            {
                self.swap_alt();
            }
            NamedPrivateMode::LineWrap => self.mode.remove(TermMode::LINE_WRAP),
            NamedPrivateMode::ShowCursor => self.mode.remove(TermMode::SHOW_CURSOR),
            NamedPrivateMode::AlternateScroll => self.mode.remove(TermMode::ALTERNATE_SCROLL),
            NamedPrivateMode::CursorKeys => self.mode.remove(TermMode::APP_CURSOR),
            NamedPrivateMode::Origin => self.mode.remove(TermMode::ORIGIN),
            NamedPrivateMode::ReportMouseClicks => self.mode.remove(TermMode::MOUSE_REPORT_CLICK),
            NamedPrivateMode::ReportCellMouseMotion => self.mode.remove(TermMode::MOUSE_DRAG),
            NamedPrivateMode::ReportAllMouseMotion => self.mode.remove(TermMode::MOUSE_MOTION),
            NamedPrivateMode::ReportFocusInOut => self.mode.remove(TermMode::FOCUS_IN_OUT),
            NamedPrivateMode::BracketedPaste => self.mode.remove(TermMode::BRACKETED_PASTE),
            NamedPrivateMode::SgrMouse => self.mode.remove(TermMode::SGR_MOUSE),
            NamedPrivateMode::Utf8Mouse => self.mode.remove(TermMode::UTF8_MOUSE),
            _ => (),
        }
    }

    /// Set a public mode (SM, `CSI n h`).
    ///
    /// Supports IRM (`4`, insert mode) and LNM (`20`, linefeed/newline).
    #[inline]
    fn set_mode(&mut self, mode: Mode) {
        let mode = match mode {
            Mode::Named(mode) => mode,
            Mode::Unknown(mode) => {
                log::debug!("ignoring unknown mode {mode} in set_mode");
                return;
            }
        };

        match mode {
            NamedMode::Insert => self.mode.insert(TermMode::INSERT),
            NamedMode::LineFeedNewLine => self.mode.insert(TermMode::LINE_FEED_NEW_LINE),
        }
    }

    /// Unset a public mode (RM, `CSI n l`).
    ///
    /// See [`set_mode`](Term::set_mode) for the set of supported modes.
    #[inline]
    fn unset_mode(&mut self, mode: Mode) {
        let mode = match mode {
            Mode::Named(mode) => mode,
            Mode::Unknown(mode) => {
                log::debug!("ignoring unknown mode {mode} in unset_mode");
                return;
            }
        };

        match mode {
            NamedMode::Insert => self.mode.remove(TermMode::INSERT),
            NamedMode::LineFeedNewLine => self.mode.remove(TermMode::LINE_FEED_NEW_LINE),
        }
    }

    /// Set the terminal scrolling region (DECSTBM, `CSI t;b r`).
    ///
    /// Line addresses are 1-based and clamped to the screen. The cursor is
    /// homed, honoring origin mode.
    #[inline]
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        // Fallback to the last line as default.
        let bottom = bottom.unwrap_or_else(|| self.screen_lines());

        if top >= bottom {
            log::debug!("invalid scrolling region: ({top};{bottom})");
            return;
        }

        // Bottom should be included in the range, but range end is not
        // usually included. One option would be to use an inclusive
        // range, but instead we just let the open range end be 1 higher.
        let start = Line(top as i32 - 1);
        let end = Line(bottom as i32);

        let screen_lines = Line(self.screen_lines() as i32);
        self.scroll_region.start = cmp::min(start, screen_lines);
        self.scroll_region.end = cmp::min(end, screen_lines);
        self.goto(0, 0);
    }

    /// Assign a character set to G0-G3 (SCS, `ESC ( ) * + <set>`).
    #[inline]
    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.grid.cursor.charsets[index] = charset;
    }

    /// Invoke one of G0-G3 as the active charset (SO/SI, `C0 0E/0F`).
    #[inline]
    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
    }

    /// Enable the application keypad mode (DECKPAM, `ESC =`).
    #[inline]
    fn set_keypad_application_mode(&mut self) {
        self.mode.insert(TermMode::APP_KEYPAD);
    }

    /// Disable the application keypad mode (DECKPAM, `ESC >`).
    #[inline]
    fn unset_keypad_application_mode(&mut self) {
        self.mode.remove(TermMode::APP_KEYPAD);
    }

    /// Set the window title (OSC 0/2).
    ///
    /// `None` resets the title to the frontend's default. Empty titles are
    /// normalized to `None`, since an empty title carries no meaning for
    /// the frontend.
    #[inline]
    fn set_title(&mut self, title: Option<String>) {
        let title = title.filter(|title| !title.is_empty());
        self.title.clone_from(&title);

        let title_event = match title {
            Some(title) => Event::Title(title),
            None => Event::ResetTitle,
        };
        self.event_proxy.send_event(title_event);
    }

    /// Push the current title onto the title stack (`CSI 22 t`).
    #[inline]
    fn push_title(&mut self) {
        if self.title_stack.len() >= TITLE_STACK_MAX_DEPTH {
            self.title_stack.remove(0);
        }
        self.title_stack.push(self.title.clone());
    }

    /// Pop the last title from the stack (`CSI 23 t`).
    #[inline]
    fn pop_title(&mut self) {
        if let Some(popped) = self.title_stack.pop() {
            self.set_title(popped);
        }
    }

    /// Set the cursor style (`CSI Ps SP q`, DECSCUSR).
    ///
    /// `None` (Ps = 0) returns to the frontend's default style. vte also
    /// exposes hollow and hidden shapes for Kitty's cursor control; those
    /// map to the default block here.
    fn set_cursor_style(&mut self, style: Option<vte::ansi::CursorStyle>) {
        self.cursor_style = style.map(|style| CursorStyle {
            shape: match style.shape {
                vte::ansi::CursorShape::Block => CursorShape::Block,
                vte::ansi::CursorShape::Underline => CursorShape::Underline,
                vte::ansi::CursorShape::Beam => CursorShape::Beam,
                vte::ansi::CursorShape::HollowBlock | vte::ansi::CursorShape::Hidden => {
                    CursorShape::Block
                }
            },
            blinking: style.blinking,
        });
    }

    /// Start or end a hyperlink (OSC 8).
    ///
    /// The link rides on the cursor template, so cells written afterwards
    /// carry it. An empty URI ends the current link.
    fn set_hyperlink(&mut self, hyperlink: Option<vte::ansi::Hyperlink>) {
        self.grid
            .cursor
            .template
            .set_hyperlink(hyperlink.map(Hyperlink::from));
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

    /// Clear all tabstops (TBC `CSI 3 g`).
    fn clear_all(&mut self) {
        for tabstop in self.tabs.iter_mut() {
            *tabstop = false;
        }
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
