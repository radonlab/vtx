//! `vtx-core`: the UI-independent core of the terminal emulator.
//!
//! Owns process/PTY lifecycle, terminal state, and application configuration.
//! The only crate that depends on `alacritty_terminal`. No egui here.

mod color;
mod settings;
mod theme_loader;

pub use color::{Palette, PaletteOverrides, ThemeColors, DEFAULT_THEME};
pub use settings::AppSettings;
pub use theme_loader::ThemeError;
