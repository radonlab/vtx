//! A terminal session: the screen state plus, eventually, its PTY.
//!
//! The current implementation is a hand-fed mock: [`Session::write_str`]
//! deposits plain text into the cell grid with naive wrapping, which is
//! enough to drive the paint layer. Once the VTE parser lands, PTY output
//! will be parsed into escape sequences and routed through the same grid
//! operations, replacing `write_str` and `set_pen`.

use crate::cell::{Cell, CellAttrs};
use crate::line::{Line, SequenceNo};

/// Default terminal width in columns.
pub const DEFAULT_COLS: usize = 80;
/// Default terminal height in rows.
pub const DEFAULT_ROWS: usize = 24;

/// Distance between tab stops, in columns.
const TAB_WIDTH: usize = 8;

/// Screen state of one terminal session.
#[derive(Debug, Clone)]
pub struct Session {
    /// Number of columns.
    cols: usize,
    /// Number of rows.
    rows: usize,
    /// The cell grid, one [`Line`] per row.
    lines: Vec<Line>,
    /// Cursor column, 0-based.
    cursor_x: usize,
    /// Cursor row, 0-based.
    cursor_y: usize,
    /// Attributes applied to subsequently written text (the "pen").
    pen: CellAttrs,
    /// Sequence number of the most recent write transaction.
    seqno: SequenceNo,
}

impl Session {
    /// Creates a blank session of the given geometry.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            lines: (0..rows)
                .map(|_| Line::with_width_and_cell(cols, Cell::blank(), 0))
                .collect(),
            cursor_x: 0,
            cursor_y: 0,
            pen: CellAttrs::blank(),
            seqno: 0,
        }
    }

    /// The number of columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The number of rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The screen rows.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The cursor position as `(column, row)`, both 0-based.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }

    /// Sets the attributes applied to text written afterwards.
    ///
    /// This stands in for SGR escape sequences until the parser exists.
    pub fn set_pen(&mut self, pen: CellAttrs) {
        self.pen = pen;
    }

    /// Writes plain text at the cursor, wrapping and scrolling as needed.
    ///
    /// Mock semantics, standing in for the escape-sequence feed:
    ///
    /// * `\r` moves to column 0 of the current row.
    /// * `\n` moves to the next row *and* column 0, like a cooked PTY
    ///   (a real parser distinguishes LF from the CR+LF the tty layer
    ///   usually injects).
    /// * `\t` jumps to the next tab stop.
    /// * everything else becomes a cell painted with the current pen.
    ///
    /// Writing past the last column wraps immediately instead of deferring
    /// the wrap like a real terminal does.
    pub fn write_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.seqno += 1;
        for c in text.chars() {
            match c {
                '\r' => self.cursor_x = 0,
                '\n' => {
                    self.cursor_x = 0;
                    self.linefeed();
                }
                '\t' => self.tab(),
                _ => self.put_char(c),
            }
        }
    }

    /// Writes one character cell and advances the cursor.
    fn put_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_y];
        line.write_cell(self.cursor_x, Cell::new(c, self.pen.clone()), self.seqno);
        self.cursor_x += 1;
        if self.cursor_x >= self.cols {
            self.lines[self.cursor_y].set_last_cell_was_wrapped(true, self.seqno);
            self.cursor_x = 0;
            self.linefeed();
        }
    }

    /// Moves the cursor to the next tab stop.
    fn tab(&mut self) {
        let next = (self.cursor_x / TAB_WIDTH + 1) * TAB_WIDTH;
        self.cursor_x = next.min(self.cols.saturating_sub(1));
    }

    /// Moves the cursor down one row, scrolling the screen if it would
    /// leave the grid. The column is left unchanged.
    fn linefeed(&mut self) {
        if self.cursor_y + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cursor_y += 1;
        }
    }

    /// Scrolls every row up by one, dropping the top row.
    fn scroll_up(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        self.lines.remove(0);
        self.lines.push(Line::with_width_and_cell(
            self.cols,
            Cell::blank(),
            self.seqno,
        ));

        // The shifted rows changed *position* without changing content, so
        // their seqnos no longer identify what a viewer cached for their
        // old row index. Re-stamp everything to force a rebuild. A real
        // screen model will solve this with stable row indices instead.
        for line in &mut self.lines {
            line.update_last_change_seqno(self.seqno);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{CellText, Color};

    #[test]
    fn new_session_is_blank() {
        let session = Session::new(4, 3);
        assert_eq!(session.cols(), 4);
        assert_eq!(session.rows(), 3);
        assert_eq!(session.cursor(), (0, 0));
        assert_eq!(session.lines().len(), 3);
        for line in session.lines() {
            assert_eq!(line.cells().len(), 4);
            assert!(line.is_whitespace());
            assert_eq!(line.seqno(), 0);
        }
    }

    #[test]
    fn write_puts_cells_and_advances_cursor() {
        let mut session = Session::new(10, 3);
        session.write_str("ab");
        assert_eq!(session.cursor(), (2, 0));
        assert_eq!(session.lines()[0].cells()[0].text(), &CellText::Char('a'));
        assert_eq!(session.lines()[0].cells()[1].text(), &CellText::Char('b'));
        assert_eq!(session.lines()[0].cells()[2], Cell::blank());
        assert!(session.lines()[0].seqno() > 0);
    }

    #[test]
    fn newline_moves_to_next_row() {
        let mut session = Session::new(10, 3);
        session.write_str("ab\ncd");
        assert_eq!(session.cursor(), (2, 1));
        assert_eq!(session.lines()[1].cells()[0].text(), &CellText::Char('c'));
    }

    #[test]
    fn carriage_return_returns_to_column_zero() {
        let mut session = Session::new(10, 3);
        session.write_str("abc\rX");
        assert_eq!(session.cursor(), (1, 0));
        assert_eq!(session.lines()[0].cells()[0].text(), &CellText::Char('X'));
        assert_eq!(session.lines()[0].cells()[2].text(), &CellText::Char('c'));
    }

    #[test]
    fn tab_jumps_to_next_stop() {
        let mut session = Session::new(20, 3);
        session.write_str("a\tb");
        // 'a' at column 0, tab to column 8, 'b' at column 8.
        assert_eq!(session.cursor(), (9, 0));
        assert_eq!(session.lines()[0].cells()[8].text(), &CellText::Char('b'));
    }

    #[test]
    fn writing_past_the_bottom_scrolls() {
        let mut session = Session::new(10, 2);
        session.write_str("one\ntwo\nthree");
        // "one" scrolled off; the grid shows "two" then "three".
        assert_eq!(session.cursor(), (5, 1));
        let texts: Vec<char> = session.lines()[0]
            .cells()
            .iter()
            .map(|cell| match cell.text() {
                CellText::Char(c) => *c,
                _ => '?',
            })
            .collect();
        assert_eq!(&texts[..3], &['t', 'w', 'o']);
        assert!(session.lines()[1].cells()[2].text() == &CellText::Char('r'));
    }

    #[test]
    fn scroll_restamps_all_rows() {
        let mut session = Session::new(10, 2);
        session.write_str("one\ntwo\nthree");
        // The last transaction scrolled the grid; every row must carry its
        // seqno, or a per-row view cache would keep stale entries.
        let latest = session.lines()[1].seqno();
        assert!(latest > 0);
        assert!(session.lines().iter().all(|line| line.seqno() >= latest));
    }

    #[test]
    fn wrap_at_last_column() {
        let mut session = Session::new(4, 3);
        session.write_str("abcde");
        // "abcd" fills row 0 and is marked wrapped; "e" starts row 1.
        assert_eq!(session.cursor(), (1, 1));
        assert!(session.lines()[0].last_cell_was_wrapped());
        assert_eq!(session.lines()[1].cells()[0].text(), &CellText::Char('e'));
    }

    #[test]
    fn pen_applies_to_written_cells() {
        let mut session = Session::new(10, 3);
        let mut pen = CellAttrs::blank();
        pen.set_foreground(Color::Indexed(3))
            .set_background(Color::Indexed(4));
        session.set_pen(pen.clone());
        session.write_str("x");
        let attrs = session.lines()[0].cells()[0].attrs().clone();
        assert_eq!(attrs.foreground(), Color::Indexed(3));
        assert_eq!(attrs.background(), Color::Indexed(4));

        session.set_pen(CellAttrs::blank());
        session.write_str("y");
        assert_eq!(session.lines()[0].cells()[1].attrs(), &CellAttrs::blank());
    }

    #[test]
    fn control_characters_become_spaces() {
        let mut session = Session::new(10, 3);
        session.write_str("\x07");
        // BEL is not CR/LF/TAB, so it lands in a cell and is defanged.
        assert_eq!(session.lines()[0].cells()[0].text(), &CellText::Char(' '));
    }
}
