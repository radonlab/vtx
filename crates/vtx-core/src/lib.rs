//! `vtx-core`: the UI-independent core of the terminal emulator.
//!
//! Owns process/PTY lifecycle, terminal state, and application configuration.
//! The only crate that depends on `alacritty_terminal`. No egui here.

mod settings;

pub use settings::AppSettings;
