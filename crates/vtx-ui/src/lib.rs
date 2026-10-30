//! vtx-ui: reusable UI components for VTX.
//!
//! The components borrow their state per frame instead of owning it, so
//! they can be embedded into any egui application.

mod font;
mod render;
mod terminal_tab;
mod terminal_view;
mod theme;

pub use font::TerminalFont;
pub use render::render_terminal;
pub use terminal_tab::TerminalTab;
pub use terminal_view::TerminalView;
pub use theme::TerminalTheme;
