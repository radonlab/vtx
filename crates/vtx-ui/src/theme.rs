//! Theme mapping terminal colors to egui colors.

use egui::Color32;
use vtx_core::color::{Color, Colors, List, NamedColor, Rgb};

/// Colors for rendering a terminal.
///
/// Wraps the core palette — the built-in defaults with a terminal's runtime
/// overrides applied — and converts terminal colors to egui colors.
#[derive(Debug, Clone)]
pub struct TerminalTheme {
    /// Fully resolved color palette.
    palette: List,
    /// Block cursor color; the text foreground by default.
    cursor: Rgb,
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::new(&Colors::default())
    }
}

impl TerminalTheme {
    /// Resolve a theme from a terminal's color overrides.
    pub fn new(colors: &Colors) -> Self {
        let palette = List::from(colors);
        let cursor = colors[NamedColor::Cursor].unwrap_or(palette[NamedColor::Foreground]);
        Self { palette, cursor }
    }

    /// Convert a terminal color to an egui color.
    pub fn color(&self, color: Color) -> Color32 {
        let rgb = match color {
            Color::Named(named) => self.palette[named],
            Color::Spec(rgb) => rgb,
            Color::Indexed(index) => self.palette[index as usize],
        };
        Color32::from_rgb(rgb.r, rgb.g, rgb.b)
    }

    /// The default screen background color.
    pub fn background(&self) -> Color32 {
        self.color(Color::Named(NamedColor::Background))
    }

    /// The block cursor color.
    pub fn cursor_color(&self) -> Color32 {
        Color32::from_rgb(self.cursor.r, self.cursor.g, self.cursor.b)
    }
}
