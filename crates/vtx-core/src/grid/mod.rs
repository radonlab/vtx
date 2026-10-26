//! Terminal grid storage.
//!
//! The full grid implementation (rows, scrollback, and resize) is tracked by
//! the roadmap; for now this module only hosts the [`GridCell`] contract which
//! binds the generic grid to the concrete [`Cell`](crate::cell::Cell) type.

use crate::cell::Flags;

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
