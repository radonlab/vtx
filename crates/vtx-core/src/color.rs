//! Terminal colors: runtime overrides, default palette, and dim handling.
//!
//! This module merges the two color halves of alacritty:
//!
//! * [`Colors`] mirrors `alacritty_terminal::term::color::Colors` — the
//!   override table attached to `Term`, mutated at runtime via OSC 10/11/12
//!   and pre-populated from user configuration.
//! * [`List`] mirrors `alacritty::display::color::List` — a fully resolved
//!   palette built from the built-in default theme plus [`Colors`] overrides.
//!   The default theme mirrors the defaults from alacritty's
//!   `config/color.rs` and lives in the `theme` module, so the fill functions
//!   stay mechanical copies of their alacritty counterparts.
//!
//! The concrete color types ([`Color`], [`NamedColor`], [`Rgb`]) are provided
//! by `vte` and re-exported here so downstream crates do not need to depend on
//! vte directly for them.

use std::ops::{Index, IndexMut};

use crate::theme::Theme;

pub use vte::ansi::{Color, NamedColor, Rgb};

/// Number of terminal colors.
pub const COUNT: usize = 269;

/// Factor for automatic computation of dim colors.
pub const DIM_FACTOR: f32 = 0.66;

// Ensure the palette size stays in sync with vte's `NamedColor` variants.
const _: () = assert!(NamedColor::DimForeground as usize + 1 == COUNT);

/// Array of indexed color overrides.
///
/// | Indices  | Description       |
/// | -------- | ----------------- |
/// | 0..16    | Named ANSI colors |
/// | 16..232  | Color cube        |
/// | 233..256 | Grayscale ramp    |
/// | 256      | Foreground        |
/// | 257      | Background        |
/// | 258      | Cursor            |
/// | 259..267 | Dim colors        |
/// | 267      | Bright foreground |
/// | 268      | Dim foreground    |
///
/// Entries set to `None` fall back to the built-in defaults resolved by
/// [`List`]. The table is `Copy`, so it can be snapshotted cheaply.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Colors([Option<Rgb>; COUNT]);

impl Default for Colors {
    fn default() -> Self {
        Self([None; COUNT])
    }
}

impl Index<usize> for Colors {
    type Output = Option<Rgb>;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for Colors {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<NamedColor> for Colors {
    type Output = Option<Rgb>;

    #[inline]
    fn index(&self, index: NamedColor) -> &Self::Output {
        &self.0[index as usize]
    }
}

impl IndexMut<NamedColor> for Colors {
    #[inline]
    fn index_mut(&mut self, index: NamedColor) -> &mut Self::Output {
        &mut self.0[index as usize]
    }
}

/// Fully resolved terminal palette.
///
/// Built from the built-in default theme (alacritty's defaults) with every
/// [`Colors`] override applied. Slots without a meaningful default, like the
/// cursor color, stay black and are resolved by the renderer instead.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct List([Rgb; COUNT]);

impl Default for List {
    fn default() -> Self {
        Self::from(&Colors::default())
    }
}

impl From<&Colors> for List {
    fn from(colors: &Colors) -> List {
        let mut list = List([Rgb::default(); COUNT]);

        list.fill_named(colors);
        list.fill_cube(colors);
        list.fill_gray_ramp(colors);

        list
    }
}

impl List {
    /// Fill the named colors from the resolved theme.
    fn fill_named(&mut self, colors: &Colors) {
        let theme = Theme::resolve(colors);

        // Normals.
        self[NamedColor::Black] = theme.normal.black;
        self[NamedColor::Red] = theme.normal.red;
        self[NamedColor::Green] = theme.normal.green;
        self[NamedColor::Yellow] = theme.normal.yellow;
        self[NamedColor::Blue] = theme.normal.blue;
        self[NamedColor::Magenta] = theme.normal.magenta;
        self[NamedColor::Cyan] = theme.normal.cyan;
        self[NamedColor::White] = theme.normal.white;

        // Brights.
        self[NamedColor::BrightBlack] = theme.bright.black;
        self[NamedColor::BrightRed] = theme.bright.red;
        self[NamedColor::BrightGreen] = theme.bright.green;
        self[NamedColor::BrightYellow] = theme.bright.yellow;
        self[NamedColor::BrightBlue] = theme.bright.blue;
        self[NamedColor::BrightMagenta] = theme.bright.magenta;
        self[NamedColor::BrightCyan] = theme.bright.cyan;
        self[NamedColor::BrightWhite] = theme.bright.white;
        self[NamedColor::BrightForeground] = theme
            .primary
            .bright_foreground
            .unwrap_or(theme.primary.foreground);

        // Foreground and background.
        self[NamedColor::Foreground] = theme.primary.foreground;
        self[NamedColor::Background] = theme.primary.background;

        // Dims, derived from the normal colors when not overridden.
        self[NamedColor::DimForeground] = theme
            .primary
            .dim_foreground
            .unwrap_or(theme.primary.foreground * DIM_FACTOR);
        self[NamedColor::DimBlack] = theme.dim.black.unwrap_or(theme.normal.black * DIM_FACTOR);
        self[NamedColor::DimRed] = theme.dim.red.unwrap_or(theme.normal.red * DIM_FACTOR);
        self[NamedColor::DimGreen] = theme.dim.green.unwrap_or(theme.normal.green * DIM_FACTOR);
        self[NamedColor::DimYellow] = theme.dim.yellow.unwrap_or(theme.normal.yellow * DIM_FACTOR);
        self[NamedColor::DimBlue] = theme.dim.blue.unwrap_or(theme.normal.blue * DIM_FACTOR);
        self[NamedColor::DimMagenta] = theme
            .dim
            .magenta
            .unwrap_or(theme.normal.magenta * DIM_FACTOR);
        self[NamedColor::DimCyan] = theme.dim.cyan.unwrap_or(theme.normal.cyan * DIM_FACTOR);
        self[NamedColor::DimWhite] = theme.dim.white.unwrap_or(theme.normal.white * DIM_FACTOR);
    }

    /// Fill the 6x6x6 color cube (indices 16..232).
    fn fill_cube(&mut self, colors: &Colors) {
        let mut index: usize = 16;
        // Build colors.
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    // Override colors 16..232 (if present).
                    if let Some(color) = colors[index] {
                        self[index] = color;
                    } else {
                        self[index] = Rgb {
                            r: if r == 0 { 0 } else { r * 40 + 55 },
                            g: if g == 0 { 0 } else { g * 40 + 55 },
                            b: if b == 0 { 0 } else { b * 40 + 55 },
                        };
                    }
                    index += 1;
                }
            }
        }

        debug_assert!(index == 232);
    }

    /// Fill the grayscale ramp (indices 232..256).
    fn fill_gray_ramp(&mut self, colors: &Colors) {
        let mut index: usize = 232;

        for i in 0..24 {
            // Override colors 232..256 (if present).
            if let Some(color) = colors[index] {
                self[index] = color;
            } else {
                let value = i * 10 + 8;
                self[index] = Rgb {
                    r: value,
                    g: value,
                    b: value,
                };
            }
            index += 1;
        }

        debug_assert!(index == 256);
    }
}

impl Index<usize> for List {
    type Output = Rgb;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for List {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<NamedColor> for List {
    type Output = Rgb;

    #[inline]
    fn index(&self, index: NamedColor) -> &Self::Output {
        &self.0[index as usize]
    }
}

impl IndexMut<NamedColor> for List {
    #[inline]
    fn index_mut(&mut self, index: NamedColor) -> &mut Self::Output {
        &mut self.0[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_colors_are_unset() {
        let colors = Colors::default();

        for index in 0..COUNT {
            assert_eq!(colors[index], None, "slot {index} should default to None");
        }
    }

    #[test]
    fn default_list_named_colors() {
        let list = List::default();

        assert_eq!(
            list[NamedColor::Black],
            Rgb {
                r: 0x18,
                g: 0x18,
                b: 0x18
            }
        );
        assert_eq!(
            list[NamedColor::Red],
            Rgb {
                r: 0xac,
                g: 0x42,
                b: 0x42
            }
        );
        assert_eq!(
            list[NamedColor::White],
            Rgb {
                r: 0xd8,
                g: 0xd8,
                b: 0xd8
            }
        );
        assert_eq!(
            list[NamedColor::BrightWhite],
            Rgb {
                r: 0xf8,
                g: 0xf8,
                b: 0xf8
            }
        );
        assert_eq!(
            list[NamedColor::Foreground],
            Rgb {
                r: 0xd8,
                g: 0xd8,
                b: 0xd8
            }
        );
        assert_eq!(
            list[NamedColor::Background],
            Rgb {
                r: 0x18,
                g: 0x18,
                b: 0x18
            }
        );
    }

    #[test]
    fn cube_fill_matches_xterm_palette() {
        let list = List::default();

        // Index 16 + r * 36 + g * 6 + b with steps of 40 starting at 55.
        assert_eq!(list[16], Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(list[17], Rgb { r: 0, g: 0, b: 95 });
        assert_eq!(list[21], Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(list[52], Rgb { r: 95, g: 0, b: 0 });
        assert_eq!(list[196], Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            list[231],
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    #[test]
    fn gray_ramp_fill_matches_xterm_palette() {
        let list = List::default();

        assert_eq!(
            list[232],
            Rgb {
                r: 0x08,
                g: 0x08,
                b: 0x08
            }
        );
        assert_eq!(
            list[244],
            Rgb {
                r: 0x80,
                g: 0x80,
                b: 0x80
            }
        );
        assert_eq!(
            list[255],
            Rgb {
                r: 0xee,
                g: 0xee,
                b: 0xee
            }
        );
    }

    #[test]
    fn overrides_take_precedence() {
        let mut colors = Colors::default();
        colors[NamedColor::Background] = Some(Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        });
        colors[16] = Some(Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        });
        colors[232] = Some(Rgb {
            r: 0x01,
            g: 0x02,
            b: 0x03,
        });

        let list = List::from(&colors);

        assert_eq!(
            list[NamedColor::Background],
            Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
        assert_eq!(
            list[16],
            Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff
            }
        );
        assert_eq!(
            list[232],
            Rgb {
                r: 0x01,
                g: 0x02,
                b: 0x03
            }
        );
    }

    #[test]
    fn dim_colors_derived_from_normals() {
        let list = List::default();

        // The default dim colors are the normal colors scaled by DIM_FACTOR.
        // 0x18 = 24, 24 * 0.66 = 15.84 which truncates to 15 (0x0f).
        assert_eq!(
            list[NamedColor::DimBlack],
            Rgb {
                r: 0x0f,
                g: 0x0f,
                b: 0x0f
            }
        );
        assert_eq!(
            list[NamedColor::DimForeground],
            Rgb {
                r: 0x8e,
                g: 0x8e,
                b: 0x8e
            }
        );
    }

    #[test]
    fn dim_follows_overridden_normal() {
        let mut colors = Colors::default();
        // 100 * 0.66 = 66.0 exactly, so no rounding is involved.
        colors[NamedColor::Black] = Some(Rgb {
            r: 100,
            g: 100,
            b: 100,
        });

        let list = List::from(&colors);

        assert_eq!(
            list[NamedColor::DimBlack],
            Rgb {
                r: 66,
                g: 66,
                b: 66
            }
        );
    }

    #[test]
    fn dim_override_wins_over_derivation() {
        let mut colors = Colors::default();
        colors[NamedColor::DimRed] = Some(Rgb { r: 1, g: 2, b: 3 });

        let list = List::from(&colors);

        assert_eq!(list[NamedColor::DimRed], Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn bright_foreground_defaults_to_foreground() {
        let mut colors = Colors::default();
        colors[NamedColor::Foreground] = Some(Rgb {
            r: 10,
            g: 20,
            b: 30,
        });

        let list = List::from(&colors);

        assert_eq!(
            list[NamedColor::BrightForeground],
            Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }
}
