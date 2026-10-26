//! The built-in color theme.
//!
//! Mirrors the config sections alacritty feeds into its palette fill
//! (`colors.primary`, `colors.normal`, `colors.bright`, and `colors.dim` in
//! `alacritty/src/config/color.rs`), so that the palette fill in
//! `crate::color` can stay a pure copy of
//! `alacritty::display::color::List::fill_named`. The `Default`
//! implementations hold alacritty's built-in default theme.

use crate::color::{Colors, NamedColor, Rgb};

/// A resolved color theme: the built-in defaults with runtime overrides
/// applied.
#[derive(Default)]
pub(crate) struct Theme {
    pub(crate) primary: PrimaryColors,
    pub(crate) normal: NormalColors,
    pub(crate) bright: BrightColors,
    pub(crate) dim: DimColors,
}

/// Foreground and background colors (`colors.primary`).
pub(crate) struct PrimaryColors {
    pub(crate) foreground: Rgb,
    pub(crate) background: Rgb,
    pub(crate) bright_foreground: Option<Rgb>,
    pub(crate) dim_foreground: Option<Rgb>,
}

impl Default for PrimaryColors {
    fn default() -> Self {
        Self {
            background: Rgb {
                r: 0x18,
                g: 0x18,
                b: 0x18,
            },
            foreground: Rgb {
                r: 0xd8,
                g: 0xd8,
                b: 0xd8,
            },
            bright_foreground: None,
            dim_foreground: None,
        }
    }
}

/// Normal intensity ANSI colors (`colors.normal`).
pub(crate) struct NormalColors {
    pub(crate) black: Rgb,
    pub(crate) red: Rgb,
    pub(crate) green: Rgb,
    pub(crate) yellow: Rgb,
    pub(crate) blue: Rgb,
    pub(crate) magenta: Rgb,
    pub(crate) cyan: Rgb,
    pub(crate) white: Rgb,
}

impl Default for NormalColors {
    fn default() -> Self {
        Self {
            black: Rgb {
                r: 0x18,
                g: 0x18,
                b: 0x18,
            },
            red: Rgb {
                r: 0xac,
                g: 0x42,
                b: 0x42,
            },
            green: Rgb {
                r: 0x90,
                g: 0xa9,
                b: 0x59,
            },
            yellow: Rgb {
                r: 0xf4,
                g: 0xbf,
                b: 0x75,
            },
            blue: Rgb {
                r: 0x6a,
                g: 0x9f,
                b: 0xb5,
            },
            magenta: Rgb {
                r: 0xaa,
                g: 0x75,
                b: 0x9f,
            },
            cyan: Rgb {
                r: 0x75,
                g: 0xb5,
                b: 0xaa,
            },
            white: Rgb {
                r: 0xd8,
                g: 0xd8,
                b: 0xd8,
            },
        }
    }
}

/// Bright intensity ANSI colors (`colors.bright`).
pub(crate) struct BrightColors {
    pub(crate) black: Rgb,
    pub(crate) red: Rgb,
    pub(crate) green: Rgb,
    pub(crate) yellow: Rgb,
    pub(crate) blue: Rgb,
    pub(crate) magenta: Rgb,
    pub(crate) cyan: Rgb,
    pub(crate) white: Rgb,
}

impl Default for BrightColors {
    fn default() -> Self {
        Self {
            black: Rgb {
                r: 0x6b,
                g: 0x6b,
                b: 0x6b,
            },
            red: Rgb {
                r: 0xc5,
                g: 0x55,
                b: 0x55,
            },
            green: Rgb {
                r: 0xaa,
                g: 0xc4,
                b: 0x74,
            },
            yellow: Rgb {
                r: 0xfe,
                g: 0xca,
                b: 0x88,
            },
            blue: Rgb {
                r: 0x82,
                g: 0xb8,
                b: 0xc8,
            },
            magenta: Rgb {
                r: 0xc2,
                g: 0x8c,
                b: 0xb8,
            },
            cyan: Rgb {
                r: 0x93,
                g: 0xd3,
                b: 0xc3,
            },
            white: Rgb {
                r: 0xf8,
                g: 0xf8,
                b: 0xf8,
            },
        }
    }
}

/// Dim color overrides (`colors.dim`).
///
/// Unlike alacritty, which applies the section as a whole, overrides are
/// tracked per color; colors without an override are derived from their
/// normal color.
#[derive(Default)]
pub(crate) struct DimColors {
    pub(crate) black: Option<Rgb>,
    pub(crate) red: Option<Rgb>,
    pub(crate) green: Option<Rgb>,
    pub(crate) yellow: Option<Rgb>,
    pub(crate) blue: Option<Rgb>,
    pub(crate) magenta: Option<Rgb>,
    pub(crate) cyan: Option<Rgb>,
    pub(crate) white: Option<Rgb>,
}

impl Theme {
    /// Resolve the built-in default theme with the overrides in `colors`
    /// applied.
    pub(crate) fn resolve(colors: &Colors) -> Self {
        let mut theme = Self::default();

        // Primary.
        theme.primary.foreground =
            colors[NamedColor::Foreground].unwrap_or(theme.primary.foreground);
        theme.primary.background =
            colors[NamedColor::Background].unwrap_or(theme.primary.background);
        theme.primary.bright_foreground = colors[NamedColor::BrightForeground];
        theme.primary.dim_foreground = colors[NamedColor::DimForeground];

        // Normal.
        theme.normal.black = colors[NamedColor::Black].unwrap_or(theme.normal.black);
        theme.normal.red = colors[NamedColor::Red].unwrap_or(theme.normal.red);
        theme.normal.green = colors[NamedColor::Green].unwrap_or(theme.normal.green);
        theme.normal.yellow = colors[NamedColor::Yellow].unwrap_or(theme.normal.yellow);
        theme.normal.blue = colors[NamedColor::Blue].unwrap_or(theme.normal.blue);
        theme.normal.magenta = colors[NamedColor::Magenta].unwrap_or(theme.normal.magenta);
        theme.normal.cyan = colors[NamedColor::Cyan].unwrap_or(theme.normal.cyan);
        theme.normal.white = colors[NamedColor::White].unwrap_or(theme.normal.white);

        // Bright.
        theme.bright.black = colors[NamedColor::BrightBlack].unwrap_or(theme.bright.black);
        theme.bright.red = colors[NamedColor::BrightRed].unwrap_or(theme.bright.red);
        theme.bright.green = colors[NamedColor::BrightGreen].unwrap_or(theme.bright.green);
        theme.bright.yellow = colors[NamedColor::BrightYellow].unwrap_or(theme.bright.yellow);
        theme.bright.blue = colors[NamedColor::BrightBlue].unwrap_or(theme.bright.blue);
        theme.bright.magenta = colors[NamedColor::BrightMagenta].unwrap_or(theme.bright.magenta);
        theme.bright.cyan = colors[NamedColor::BrightCyan].unwrap_or(theme.bright.cyan);
        theme.bright.white = colors[NamedColor::BrightWhite].unwrap_or(theme.bright.white);

        // Dim.
        theme.dim.black = colors[NamedColor::DimBlack];
        theme.dim.red = colors[NamedColor::DimRed];
        theme.dim.green = colors[NamedColor::DimGreen];
        theme.dim.yellow = colors[NamedColor::DimYellow];
        theme.dim.blue = colors[NamedColor::DimBlue];
        theme.dim.magenta = colors[NamedColor::DimMagenta];
        theme.dim.cyan = colors[NamedColor::DimCyan];
        theme.dim.white = colors[NamedColor::DimWhite];

        theme
    }
}
