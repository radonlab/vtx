//! The line model: one row of terminal cells with change tracking.

use crate::cell::Cell;

/// Monotonic sequence number stamped on lines as they change.
///
/// Each parsing transaction carries a seqno, and every line it touches
/// is stamped with it. Renderers remember the seqno at which they last
/// laid out a line and re-measure only when the stamps differ, which is
/// cheaper and more precise than hashing line contents.
pub type SequenceNo = u64;

/// One row of the screen or the scrollback buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    cells: Vec<Cell>,
    seqno: SequenceNo,
}

impl Line {
    /// An empty line stamped with `seqno`.
    pub fn new(seqno: SequenceNo) -> Self {
        Self {
            cells: Vec::new(),
            seqno,
        }
    }

    /// A line of `cols` cells, each a copy of `cell`.
    pub fn with_width_and_cell(cols: usize, cell: Cell, seqno: SequenceNo) -> Self {
        Self {
            cells: vec![cell; cols],
            seqno,
        }
    }

    /// The cells of this line.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The sequence number of the last change to this line.
    pub fn seqno(&self) -> SequenceNo {
        self.seqno
    }

    /// Stamps the line as changed at `seqno`.
    pub fn update_last_change_seqno(&mut self, seqno: SequenceNo) {
        if self.seqno != seqno {
            self.seqno = seqno;
        }
    }

    /// Overwrites the cell at `col`, stamping `seqno`.
    ///
    /// Returns whether `col` was in bounds; out-of-bounds writes are
    /// dropped and do not bump the seqno.
    pub fn write_cell(&mut self, col: usize, cell: Cell, seqno: SequenceNo) -> bool {
        if col < self.cells.len() {
            self.cells[col] = cell;
            self.update_last_change_seqno(seqno);
            true
        } else {
            false
        }
    }

    /// Whether the last cell carries the wrap marker, meaning the line
    /// wraps into the next one instead of ending a logical row.
    ///
    /// The marker lives on the final cell's attributes rather than on
    /// the line itself so that it survives line splits and joins while
    /// rewrapping on resize.
    pub fn last_cell_was_wrapped(&self) -> bool {
        self.cells.last().is_some_and(|cell| cell.attrs().wrapped())
    }

    /// Sets or clears the wrap marker on the last cell, stamping `seqno`.
    pub fn set_last_cell_was_wrapped(&mut self, wrapped: bool, seqno: SequenceNo) {
        if let Some(last) = self.cells.last_mut() {
            last.attrs_mut().set_wrapped(wrapped);
        }
        self.update_last_change_seqno(seqno);
    }

    /// Whether the line contains only whitespace.
    ///
    /// Used when deciding whether blank lines scrolled into scrollback
    /// should count against the scrollback limit.
    pub fn is_whitespace(&self) -> bool {
        self.cells.iter().all(|cell| cell.text().is_whitespace())
    }

    /// Resizes the line to `cols` cells, truncating or padding with
    /// blank cells and stamping `seqno`.
    ///
    /// Padding cells are plain blanks; erase-with-background-color
    /// semantics are handled at the screen layer.
    pub fn resize(&mut self, cols: usize, seqno: SequenceNo) {
        self.cells.resize(cols, Cell::blank());
        self.update_last_change_seqno(seqno);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{CellAttrs, CellText, Color};

    #[test]
    fn new_line_is_empty() {
        let line = Line::new(7);
        assert_eq!(line.cells(), &[]);
        assert_eq!(line.seqno(), 7);
        assert!(!line.last_cell_was_wrapped());
        assert!(line.is_whitespace());
    }

    #[test]
    fn seqno_updates() {
        let mut line = Line::new(1);
        line.update_last_change_seqno(2);
        assert_eq!(line.seqno(), 2);
        line.update_last_change_seqno(3);
        assert_eq!(line.seqno(), 3);
    }

    #[test]
    fn write_cell_stamps_seqno() {
        let mut line = Line::with_width_and_cell(4, Cell::blank(), 1);
        assert!(line.write_cell(2, Cell::new('x', CellAttrs::blank()), 5));
        assert_eq!(line.seqno(), 5);
        assert_eq!(line.cells()[2].text(), &CellText::Char('x'));

        assert!(!line.write_cell(9, Cell::blank(), 6));
        assert_eq!(line.seqno(), 5);
    }

    #[test]
    fn wrapped_marker_lives_on_last_cell() {
        let mut line = Line::with_width_and_cell(3, Cell::blank(), 1);
        assert!(!line.last_cell_was_wrapped());

        line.set_last_cell_was_wrapped(true, 2);
        assert!(line.last_cell_was_wrapped());
        assert_eq!(line.seqno(), 2);
        assert!(line.cells()[2].attrs().wrapped());
        assert!(!line.cells()[1].attrs().wrapped());

        line.set_last_cell_was_wrapped(false, 3);
        assert!(!line.last_cell_was_wrapped());
    }

    #[test]
    fn resize_pads_and_truncates() {
        let mut line = Line::with_width_and_cell(2, Cell::new('a', CellAttrs::blank()), 1);

        line.resize(4, 2);
        assert_eq!(line.cells().len(), 4);
        assert_eq!(line.cells()[3], Cell::blank());
        assert_eq!(line.seqno(), 2);

        line.resize(1, 3);
        assert_eq!(line.cells().len(), 1);
        assert_eq!(line.cells()[0].text(), &CellText::Char('a'));
    }

    #[test]
    fn whitespace_detection_ignores_attributes() {
        let blank = Line::with_width_and_cell(3, Cell::blank(), 1);
        assert!(blank.is_whitespace());

        let mut text = Line::with_width_and_cell(3, Cell::blank(), 1);
        text.write_cell(1, Cell::new('x', CellAttrs::blank()), 2);
        assert!(!text.is_whitespace());

        // A space with a non-default background is still whitespace.
        let mut styled = Line::with_width_and_cell(2, Cell::blank(), 1);
        let mut attrs = CellAttrs::blank();
        attrs.set_background(Color::Indexed(1));
        styled.write_cell(0, Cell::blank_with_attrs(attrs), 2);
        assert!(styled.is_whitespace());
    }
}
