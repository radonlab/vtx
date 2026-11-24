//! Terminal color theme layer.
//!
//! `alacritty_terminal` stores only semantic color references (`vte::ansi::Color`)
//! in cells plus a runtime override table filled by OSC sequences. The static
//! theme palette and the final RGB resolution are the renderer's job — this
//! module provides them, UI-independent.
//!
//! Slot layout matches `alacritty_terminal::term::color::COUNT` (269):
//!
//! | Indices    | Description       |
//! | ---------- | ----------------- |
//! | 0..=15     | Named ANSI colors |
//! | 16..=231   | Color cube        |
//! | 232..=255  | Grayscale ramp    |
//! | 256        | Foreground        |
//! | 257        | Background        |
//! | 258        | Cursor            |
//! | 259..=266  | Dim colors        |
//! | 267        | Bright foreground |
//! | 268        | Dim foreground    |

use alacritty_terminal::term::color::COUNT;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// A complete theme: every slot filled, no `Option`s.
///
/// Build one from named colors via [`Palette::build``]; the color cube and
/// grayscale ramp are generated mathematically, so themes only specify the
/// ~30 interesting slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette(pub [Rgb; COUNT]);

/// Named slots a theme must provide; everything else is derived.
pub struct ThemeColors {
    pub foreground: Rgb,
    pub background: Rgb,
    /// Cursor color; when `None` the renderer should invert fg/bg instead.
    pub cursor: Option<Rgb>,
    /// The 16 ANSI colors, normal then bright.
    pub normal: [Rgb; 8],
    pub bright: [Rgb; 8],
}

impl Palette {
    /// Assemble a full palette from a theme's named colors, generating the
    /// xterm 256-color cube and grayscale ramp for slots 16..=255.
    pub fn build(theme: &ThemeColors) -> Self {
        let fg = theme.foreground;
        let bg = theme.background;
        let mut slots = [bg; COUNT];

        // Slots 0..=15: named ANSI colors, normal then bright.
        for (i, rgb) in theme.normal.iter().chain(theme.bright.iter()).enumerate() {
            slots[i] = *rgb;
        }
        // Slots 16..=255: xterm color cube (16..=231) then grayscale ramp (232..=255).
        for (i, rgb) in xterm_cube_and_grayscale().enumerate() {
            slots[16 + i] = rgb;
        }
        // Slot 256: foreground.
        slots[NamedColor::Foreground as usize] = fg;
        // Slot 257: background.
        slots[NamedColor::Background as usize] = bg;
        // Slot 258: cursor (theme override or bg fallback).
        slots[NamedColor::Cursor as usize] = theme.cursor.unwrap_or(bg);
        // Slots 259..=266: dim variants (DimBlack..=DimWhite), 50% toward black.
        for i in 0..8 {
            slots[NamedColor::DimBlack as usize + i] = dim_rgb(slots[i]);
        }
        // Slot 267: bright foreground.
        slots[NamedColor::BrightForeground as usize] = fg;
        // Slot 268: dim foreground.
        slots[NamedColor::DimForeground as usize] = dim_rgb(fg);

        Self(slots)
    }

    /// The palette entry for a `NamedColor` slot.
    #[inline]
    pub fn named(&self, color: NamedColor) -> Rgb {
        self.0[color as usize]
    }

    /// The palette entry for an 8-bit index (0..=255).
    #[inline]
    pub fn indexed(&self, index: u8) -> Rgb {
        self.0[index as usize]
    }

    /// Resolve a cell's semantic color to its final RGB value.
    ///
    /// The OSC override table (from `Term::colors()`, sparse) wins over the
    /// theme palette, mirroring Alacritty's own fallback chain.
    pub fn resolve(&self, color: Color, overrides: &PaletteOverrides) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => self.resolve_index(named as usize, overrides),
            Color::Indexed(index) => self.resolve_index(index as usize, overrides),
        }
    }

    #[inline]
    fn resolve_index(&self, index: usize, overrides: &PaletteOverrides) -> Rgb {
        match (overrides.0)[index] {
            Some(rgb) => rgb,
            None => self.0[index],
        }
    }
}

/// Runtime color overrides set by OSC 4/10/11/12 escape sequences.
///
/// Wraps `alacritty_terminal`'s sparse `Colors` table so callers don't need to
/// import it themselves.
pub struct PaletteOverrides<'a>(pub &'a alacritty_terminal::term::color::Colors);

/// Generate xterm color-cube slots 16..=231 and grayscale slots 232..=255.
fn xterm_cube_and_grayscale() -> impl Iterator<Item = Rgb> {
    fn channel(level: u8) -> u8 {
        match level {
            0 => 0,
            _ => level * 40 + 55,
        }
    }

    let cube = (0u8..6).flat_map(move |r| {
        (0u8..6).flat_map(move |g| {
            (0u8..6).map(move |b| Rgb {
                r: channel(r),
                g: channel(g),
                b: channel(b),
            })
        })
    });

    let ramp = (0u8..24).map(|i| {
        let v = i * 10 + 8;
        Rgb { r: v, g: v, b: v }
    });

    cube.chain(ramp)
}

/// Blend an RGB color 50% toward black.
const fn dim_rgb(rgb: Rgb) -> Rgb {
    Rgb {
        r: rgb.r / 2,
        g: rgb.g / 2,
        b: rgb.b / 2,
    }
}

/// Default built-in theme, roughly following the xterm defaults.
impl Default for Palette {
    fn default() -> Self {
        Self::build(&DEFAULT_THEME)
    }
}

/// The default theme's named colors.
pub const DEFAULT_THEME: ThemeColors = ThemeColors {
    foreground: Rgb { r: 0xc7, g: 0xc7, b: 0xc7 },
    background: Rgb { r: 0x00, g: 0x00, b: 0x00 },
    cursor: None,
    normal: [
        Rgb { r: 0x00, g: 0x00, b: 0x00 }, // black
        Rgb { r: 0xcd, g: 0x00, b: 0x00 }, // red
        Rgb { r: 0x00, g: 0xcd, b: 0x00 }, // green
        Rgb { r: 0xcd, g: 0xcd, b: 0x00 }, // yellow
        Rgb { r: 0x00, g: 0x00, b: 0xee }, // blue
        Rgb { r: 0xcd, g: 0x00, b: 0xcd }, // magenta
        Rgb { r: 0x00, g: 0xcd, b: 0xcd }, // cyan
        Rgb { r: 0xe5, g: 0xe5, b: 0xe5 }, // white
    ],
    bright: [
        Rgb { r: 0x7f, g: 0x7f, b: 0x7f }, // bright black
        Rgb { r: 0xff, g: 0x00, b: 0x00 }, // bright red
        Rgb { r: 0x00, g: 0xff, b: 0x00 }, // bright green
        Rgb { r: 0xff, g: 0xff, b: 0x00 }, // bright yellow
        Rgb { r: 0x5c, g: 0x5c, b: 0xff }, // bright blue
        Rgb { r: 0xff, g: 0x00, b: 0xff }, // bright magenta
        Rgb { r: 0x00, g: 0xff, b: 0xff }, // bright cyan
        Rgb { r: 0xff, g: 0xff, b: 0xff }, // bright white
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_build_fills_all_slots() {
        let palette = Palette::default();
        // Cube and ramp spots are filled with distinct generated values.
        assert_eq!(palette.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(palette.indexed(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(palette.indexed(255), Rgb { r: 238, g: 238, b: 238 });
        // Named slots.
        assert_eq!(palette.named(NamedColor::Foreground), DEFAULT_THEME.foreground);
        assert_eq!(palette.named(NamedColor::Background), DEFAULT_THEME.background);
        assert_eq!(palette.named(NamedColor::Red), DEFAULT_THEME.normal[1]);
        assert_eq!(palette.named(NamedColor::BrightWhite), DEFAULT_THEME.bright[7]);
        // Dim variants derived.
        assert_eq!(
            palette.named(NamedColor::DimRed),
            dim_rgb(DEFAULT_THEME.normal[1])
        );
    }

    #[test]
    fn resolve_prefers_spec_over_everything() {
        let palette = Palette::default();
        let mut overrides = alacritty_terminal::term::color::Colors::default();
        let spec = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(
            palette.resolve(Color::Spec(spec), &PaletteOverrides(&overrides)),
            spec
        );
        // Even if an override exists for the same slot, Spec colors are literal.
        overrides[NamedColor::Red as usize] = Some(Rgb { r: 9, g: 9, b: 9 });
        assert_eq!(
            palette.resolve(Color::Spec(spec), &PaletteOverrides(&overrides)),
            spec
        );
    }

    #[test]
    fn resolve_prefers_osc_override_over_theme() {
        let palette = Palette::default();
        let mut overrides = alacritty_terminal::term::color::Colors::default();
        let osc_color = Rgb { r: 9, g: 9, b: 9 };
        overrides[NamedColor::Red as usize] = Some(osc_color);
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Red), &PaletteOverrides(&overrides)),
            osc_color
        );
    }

    #[test]
    fn resolve_falls_back_to_theme_without_overrides() {
        let palette = Palette::default();
        let overrides = alacritty_terminal::term::color::Colors::default();
        assert_eq!(
            palette.resolve(Color::Indexed(200), &PaletteOverrides(&overrides)),
            palette.indexed(200)
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Blue), &PaletteOverrides(&overrides)),
            DEFAULT_THEME.normal[4]
        );
    }
}
