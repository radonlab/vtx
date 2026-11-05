//! Terminal font and cell metrics.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Context, FontDefinitions, FontFamily, FontId, Galley, Vec2};

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
    /// Single-character galleys, laid out once and reused.
    ///
    /// Terminal glyphs are pinned to their cells; see [`Self::glyph_galley`].
    glyph_galleys: RefCell<HashMap<char, Arc<Galley>>>,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self::new(FontId::monospace(DEFAULT_FONT_SIZE))
    }
}

impl TerminalFont {
    /// Create a terminal font from a font id.
    pub fn new(font_id: FontId) -> Self {
        Self {
            font_id,
            glyph_galleys: RefCell::new(HashMap::new()),
        }
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

    /// A single-character galley for `c`, laid out once and cached.
    ///
    /// Terminal glyphs are drawn one per cell, at the cell's grid position.
    /// Shaping multi-character strings instead would let characters with a
    /// non-uniform advance (CJK fallback, emoji) push the following glyphs
    /// off the grid, and splitting such a run — at a selection or a color
    /// change — would visibly move the text.
    ///
    /// The galley's text color is a placeholder, replaced by the color
    /// passed when painting.
    pub fn glyph_galley(&self, ctx: &Context, c: char) -> Arc<Galley> {
        if let Some(galley) = self.glyph_galleys.borrow().get(&c) {
            return Arc::clone(galley);
        }

        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(c.to_string(), self.font_id.clone(), Color32::PLACEHOLDER)
        });
        self.glyph_galleys
            .borrow_mut()
            .insert(c, Arc::clone(&galley));
        galley
    }
}
