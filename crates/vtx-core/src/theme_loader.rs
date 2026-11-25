//! Terminal color theme loader.

use std::fs::File;
use std::path::Path;

use alacritty_terminal::vte::ansi::Rgb;

use crate::color::ThemeColors;

/// Errors that can occur while loading a theme file.
#[derive(Debug)]
pub enum ThemeError {
    Io(std::io::Error),
    /// The file is not a valid XML plist with the expected dict structure.
    Malformed(String),
    /// A required key is missing from the top-level dict.
    MissingKey(String),
    /// A color component was present but not a valid number.
    InvalidValue(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Malformed(msg) => write!(f, "malformed theme file: {msg}"),
            Self::MissingKey(key) => write!(f, "missing key: {key}"),
            Self::InvalidValue(msg) => write!(f, "invalid value: {msg}"),
        }
    }
}

impl std::error::Error for ThemeError {}

impl From<std::io::Error> for ThemeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<plist::Error> for ThemeError {
    fn from(e: plist::Error) -> Self {
        Self::Malformed(e.to_string())
    }
}

impl ThemeColors {
    /// Load an iTerm2 `.itermcolors` theme from `path`.
    ///
    /// The file must be an XML plist whose top-level dict maps color names
    /// (e.g. "Ansi 5 Color", "Foreground Color") to dicts of float components
    /// in 0..=1. Extra keys (Bold, Link, Badge, ...) are ignored.
    pub fn load_itermcolors(path: impl AsRef<Path>) -> Result<Self, ThemeError> {
        let values = plist::Value::from_reader(File::open(path.as_ref())?)?;
        let dict = values
            .as_dictionary()
            .ok_or_else(|| ThemeError::Malformed("top-level value is not a dict".into()))?;

        let normal = [
            color_at(dict, "Ansi 0 Color")?,
            color_at(dict, "Ansi 1 Color")?,
            color_at(dict, "Ansi 2 Color")?,
            color_at(dict, "Ansi 3 Color")?,
            color_at(dict, "Ansi 4 Color")?,
            color_at(dict, "Ansi 5 Color")?,
            color_at(dict, "Ansi 6 Color")?,
            color_at(dict, "Ansi 7 Color")?,
        ];
        let bright = [
            color_at(dict, "Ansi 8 Color")?,
            color_at(dict, "Ansi 9 Color")?,
            color_at(dict, "Ansi 10 Color")?,
            color_at(dict, "Ansi 11 Color")?,
            color_at(dict, "Ansi 12 Color")?,
            color_at(dict, "Ansi 13 Color")?,
            color_at(dict, "Ansi 14 Color")?,
            color_at(dict, "Ansi 15 Color")?,
        ];

        Ok(Self {
            foreground: color_at(dict, "Foreground Color")?,
            background: color_at(dict, "Background Color")?,
            // iTerm2 plists always carry a cursor color; treat missing as "invert".
            cursor: dict
                .get("Cursor Color")
                .and_then(|v| color_from_dict(v, "Cursor Color").ok()),
            normal,
            bright,
        })
    }
}

/// Look up a named color in the top-level dict.
fn color_at(dict: &plist::Dictionary, key: &str) -> Result<Rgb, ThemeError> {
    let value = dict
        .get(key)
        .ok_or_else(|| ThemeError::MissingKey(key.to_string()))?;
    color_from_dict(value, key)
}

/// Convert one color dict ({Red, Green, Blue Component: real}) to `Rgb`.
fn color_from_dict(value: &plist::Value, key: &str) -> Result<Rgb, ThemeError> {
    let dict = value
        .as_dictionary()
        .ok_or_else(|| ThemeError::InvalidValue(format!("{key} is not a dict")))?;

    let mut channels = [0u8; 3];
    for (slot, name) in [
        ("Red Component", 0),
        ("Green Component", 1),
        ("Blue Component", 2),
    ] {
        let raw = dict
            .get(slot)
            .ok_or_else(|| ThemeError::InvalidValue(format!("{key} lacks {slot}")))?;
        let component = raw
            .as_real()
            .ok_or_else(|| ThemeError::InvalidValue(format!("{key}.{slot} is not a real")))?;
        if !(0.0..=1.0).contains(&component) {
            return Err(ThemeError::InvalidValue(format!(
                "{key}.{slot} out of range: {component}"
            )));
        }
        channels[name] = (component * 255.0).round() as u8;
    }

    Ok(Rgb {
        r: channels[0],
        g: channels[1],
        b: channels[2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to a fixture in `tests/fixtures/`, resolved from the crate root.
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn missing_keys_are_reported() {
        // Only Ansi 0 + Foreground are present; Ansi 1 is the first gap.
        let err = ThemeColors::load_itermcolors(fixture("minimal.itermcolors")).unwrap_err();
        assert!(
            matches!(err, ThemeError::MissingKey(ref k) if k == "Ansi 1 Color"),
            "{err}"
        );
    }

    #[test]
    fn components_are_converted() {
        // All required keys carry (0.5, 0.0, 1.0); no Cursor Color key.
        let theme = ThemeColors::load_itermcolors(fixture("omnibus.itermcolors")).unwrap();
        let expected = Rgb {
            r: 128,
            g: 0,
            b: 255,
        };
        assert_eq!(theme.foreground, expected);
        assert_eq!(theme.background, expected);
        assert_eq!(theme.normal[0], expected);
        assert_eq!(theme.bright[7], expected);
        assert_eq!(theme.cursor, None);
    }

    #[test]
    fn excessive_component_is_rejected() {
        // Ansi 0 Green Component is 1.5.
        let err = ThemeColors::load_itermcolors(fixture("excessive.itermcolors")).unwrap_err();
        assert!(matches!(err, ThemeError::InvalidValue(_)), "{err}");
    }

    #[test]
    fn malformed_file_is_rejected() {
        let err = ThemeColors::load_itermcolors(fixture("malformed.itermcolors")).unwrap_err();
        assert!(matches!(err, ThemeError::Malformed(_)), "{err}");
    }

    #[test]
    fn missing_file_is_io_error() {
        let err = ThemeColors::load_itermcolors(fixture("missing.itermcolors")).unwrap_err();
        assert!(matches!(err, ThemeError::Io(_)), "{err}");
    }
}
