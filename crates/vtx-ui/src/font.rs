//! Terminal font and cell metrics.

use egui::{Context, FontId, Vec2};

/// Default terminal font size in points.
const DEFAULT_FONT_SIZE: f32 = 14.0;

/// The monospace font used for the terminal grid.
///
/// Cell dimensions are measured from the actual glyph metrics, so the grid
/// tracks whatever font and zoom level the embedder configured. Custom font
/// registration, bold/italic variants, and fallback chains land in a later
/// milestone; until then the egui default monospace font is used.
#[derive(Debug, Clone)]
pub struct TerminalFont {
    /// Font used for all terminal text.
    font_id: FontId,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self::new(FontId::monospace(DEFAULT_FONT_SIZE))
    }
}

impl TerminalFont {
    /// Create a terminal font from a font id.
    pub fn new(font_id: FontId) -> Self {
        Self { font_id }
    }

    /// The font id used for terminal text.
    pub fn font_id(&self) -> &FontId {
        &self.font_id
    }

    /// Size of a single grid cell in points.
    ///
    /// The width is the advance of a monospace reference glyph and the height
    /// is the font's row height, matching how egui lays out text.
    pub fn cell_size(&self, ctx: &Context) -> Vec2 {
        let font_id = self.font_id.clone();
        ctx.fonts_mut(|fonts| {
            Vec2::new(fonts.glyph_width(&font_id, 'm'), fonts.row_height(&font_id))
        })
    }
}
