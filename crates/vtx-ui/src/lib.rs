//! vtx-ui: reusable UI widgets for VTX.
//!
//! Widgets implement the [`egui::Widget`] trait so they can be dropped
//! into any egui application.

mod terminal_tab;
mod terminal_view;

pub use terminal_tab::TerminalTab;
pub use terminal_view::TerminalView;
