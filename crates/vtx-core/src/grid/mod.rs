//! Terminal grid storage.
//!
//! The full grid implementation (rows, scrollback, and resize) is tracked by
//! the roadmap; for now this module only hosts the [`GridCell`] contract which
//! binds the generic grid to the concrete [`Cell`](crate::cell::Cell) type,
//! and the [`Dimensions`] contract used by grid coordinate math.

use crate::cell::Flags;
use crate::index::{Column, Line};

/// Contract between the generic grid storage and its cell type.
pub trait GridCell {
    /// Check whether the cell is blank and can be skipped during rendering
    /// and serialization.
    fn is_empty(&self) -> bool;

    /// Access the cell's attribute flags.
    fn flags(&self) -> &Flags;

    /// Mutably access the cell's attribute flags.
    fn flags_mut(&mut self) -> &mut Flags;

    /// Reset the cell to the default state, preserving properties from
    /// `template` (background color erase).
    fn reset(&mut self, template: &Self);
}

/// Dimensions of a grid or grid-like structure.
pub trait Dimensions {
    /// Total number of lines in the buffer, this includes scrollback and
    /// visible lines.
    fn total_lines(&self) -> usize;

    /// Height of the viewport in lines.
    fn screen_lines(&self) -> usize;

    /// Width of the terminal in columns.
    fn columns(&self) -> usize;

    /// Index for the last column.
    #[inline]
    fn last_column(&self) -> Column {
        Column(self.columns() - 1)
    }

    /// Line farthest up in the grid history.
    #[inline]
    fn topmost_line(&self) -> Line {
        Line(-(self.history_size() as i32))
    }

    /// Line farthest down in the grid history.
    #[inline]
    fn bottommost_line(&self) -> Line {
        Line(self.screen_lines() as i32 - 1)
    }

    /// Number of invisible lines part of the scrollback history.
    #[inline]
    fn history_size(&self) -> usize {
        self.total_lines().saturating_sub(self.screen_lines())
    }
}

impl Dimensions for (usize, usize) {
    fn total_lines(&self) -> usize {
        self.0
    }

    fn screen_lines(&self) -> usize {
        self.0
    }

    fn columns(&self) -> usize {
        self.1
    }
}
