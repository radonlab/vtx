//! Color theme for the terminal widgets.

use egui::Color32;

/// Color scheme used to translate cell colors to pixels.
///
/// The palette roughly follows the xterm defaults and stands in until the
/// Dracula theme lands.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Default text color.
    pub foreground: Color32,
    /// Default screen background.
    pub background: Color32,
    /// Cursor block overlay color (semi-transparent so the glyph below
    /// stays visible).
    pub cursor: Color32,
    /// The 16 base ANSI colors (indices 0-15).
    pub ansi: [Color32; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: Color32::from_rgb(204, 204, 204),
            background: Color32::from_rgb(12, 12, 12),
            cursor: Color32::from_rgba_unmultiplied(220, 220, 220, 96),
            ansi: [
                Color32::from_rgb(0x00, 0x00, 0x00), // 0 black
                Color32::from_rgb(0xcd, 0x00, 0x00), // 1 red
                Color32::from_rgb(0x00, 0xcd, 0x00), // 2 green
                Color32::from_rgb(0xcd, 0xcd, 0x00), // 3 yellow
                Color32::from_rgb(0x00, 0x00, 0xee), // 4 blue
                Color32::from_rgb(0xcd, 0x00, 0xcd), // 5 magenta
                Color32::from_rgb(0x00, 0xcd, 0xcd), // 6 cyan
                Color32::from_rgb(0xe5, 0xe5, 0xe5), // 7 white
                Color32::from_rgb(0x7f, 0x7f, 0x7f), // 8 bright black
                Color32::from_rgb(0xff, 0x00, 0x00), // 9 bright red
                Color32::from_rgb(0x00, 0xff, 0x00), // 10 bright green
                Color32::from_rgb(0xff, 0xff, 0x00), // 11 bright yellow
                Color32::from_rgb(0x5c, 0x5c, 0xff), // 12 bright blue
                Color32::from_rgb(0xff, 0x00, 0xff), // 13 bright magenta
                Color32::from_rgb(0x00, 0xff, 0xff), // 14 bright cyan
                Color32::from_rgb(0xff, 0xff, 0xff), // 15 bright white
            ],
        }
    }
}

impl Theme {
    /// Resolves a palette index (0-255) to a concrete color.
    pub(crate) fn indexed(&self, index: u8) -> Color32 {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                // 6x6x6 color cube.
                let n = u32::from(index) - 16;
                let levels = [0, 95, 135, 175, 215, 255];
                let (r, g, b) = (
                    levels[(n / 36) as usize],
                    levels[((n / 6) % 6) as usize],
                    levels[(n % 6) as usize],
                );
                Color32::from_rgb(r, g, b)
            }
            _ => {
                // Grayscale ramp.
                let v = 8 + (u32::from(index) - 232) * 10;
                Color32::from_rgb(v as u8, v as u8, v as u8)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_colors_resolve() {
        let theme = Theme::default();
        assert_eq!(theme.indexed(1), Color32::from_rgb(0xcd, 0x00, 0x00));
        // Corners of the 6x6x6 cube.
        assert_eq!(theme.indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(theme.indexed(231), Color32::from_rgb(255, 255, 255));
        // Grayscale ramp.
        assert_eq!(theme.indexed(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(theme.indexed(255), Color32::from_rgb(238, 238, 238));
    }
}
