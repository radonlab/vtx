//! The cell model: the atomic unit of terminal screen content.
//!
//! A [`Cell`] pairs one grapheme cluster (almost always a single
//! `char`) with the [`CellAttrs`] that were in effect when it was
//! written. Style attributes pack into a `u32` bitfield beside two
//! small color enums, so the common case involves no heap allocation.
//! The layout follows the design popularized by WezTerm.
//!
//! Conventions:
//!
//! * A cell holding a double-width grapheme is followed by *spacer*
//!   cells — blanks sharing its attributes — so the grid stays one
//!   column per cell and cursor arithmetic stays trivial.
//! * Control characters never enter a cell; they are rewritten to
//!   spaces at construction time.

use std::fmt;
use std::sync::Arc;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Foreground or background color of a cell.
///
/// `Default` defers to the active color scheme so themes can change
/// without rewriting cell contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// The color scheme default.
    Default,
    /// An index into the 16/256-color palette.
    Indexed(u8),
    /// A 24-bit RGB color.
    Rgb {
        /// Red component.
        r: u8,
        /// Green component.
        g: u8,
        /// Blue component.
        b: u8,
    },
}

/// Text intensity (SGR 1/2/22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Intensity {
    /// Regular intensity.
    #[default]
    Normal = 0,
    /// Bold (SGR 1).
    Bold = 1,
    /// Dim / half-bright (SGR 2).
    Half = 2,
}

impl Intensity {
    /// Decodes a bitfield value; unknown patterns fall back to the default.
    fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Bold,
            2 => Self::Half,
            _ => Self::Normal,
        }
    }
}

/// Underline style (SGR 4/21/24 and the `4:x` substyles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Underline {
    /// No underline.
    #[default]
    None = 0,
    /// A single line (SGR 4).
    Single = 1,
    /// A double line (SGR 21).
    Double = 2,
    /// A curly line (SGR 4:3).
    Curly = 3,
    /// A dotted line (SGR 4:4).
    Dotted = 4,
    /// A dashed line (SGR 4:5).
    Dashed = 5,
}

impl Underline {
    /// Decodes a bitfield value; unknown patterns fall back to the default.
    fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Single,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::None,
        }
    }
}

/// Blink mode (SGR 5/6/25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Blink {
    /// No blinking.
    #[default]
    None = 0,
    /// Slow blink, roughly twice per second (SGR 5).
    Slow = 1,
    /// Rapid blink, roughly ten times per second (SGR 6).
    Rapid = 2,
}

impl Blink {
    /// Decodes a bitfield value; unknown patterns fall back to the default.
    fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Slow,
            2 => Self::Rapid,
            _ => Self::None,
        }
    }
}

/// Styling attributes for a cell.
///
/// All style flags live in a single `u32` bitfield so that comparing
/// and copying attributes stays cheap:
///
/// | bits  | attribute     |
/// |-------|---------------|
/// | 0-1   | intensity     |
/// | 2-4   | underline     |
/// | 5-6   | blink         |
/// | 7     | italic        |
/// | 8     | reverse       |
/// | 9     | strikethrough |
/// | 10    | invisible     |
/// | 11    | wrapped       |
/// | 12    | overline      |
///
/// Setters return `&mut Self` so they can be chained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellAttrs {
    /// Packed style bits, see the table above.
    bits: u32,
    /// Foreground color.
    fg: Color,
    /// Background color.
    bg: Color,
}

/// Generates a getter/setter pair for a boolean attribute stored in a
/// single bit of the attribute bitfield.
macro_rules! bool_attr {
    ($get:ident, $set:ident, $bit:expr) => {
        /// Returns whether this attribute is set.
        #[inline]
        pub fn $get(&self) -> bool {
            self.bits & (1 << $bit) != 0
        }

        /// Sets or clears this attribute.
        #[inline]
        pub fn $set(&mut self, value: bool) -> &mut Self {
            self.bits = (self.bits & !(1 << $bit)) | ((value as u32) << $bit);
            self
        }
    };
}

/// Generates a getter/setter pair for an enum attribute stored in a bit
/// range of the attribute bitfield.
macro_rules! enum_attr {
    ($get:ident, $set:ident, $ty:ty, $mask:expr, $shift:expr) => {
        /// Returns the current value of this attribute.
        #[inline]
        pub fn $get(&self) -> $ty {
            <$ty>::from_bits(((self.bits >> $shift) & $mask) as u8)
        }

        /// Sets this attribute.
        #[inline]
        pub fn $set(&mut self, value: $ty) -> &mut Self {
            self.bits = (self.bits & !($mask << $shift)) | ((value as u32) << $shift);
            self
        }
    };
}

impl CellAttrs {
    enum_attr!(intensity, set_intensity, Intensity, 0b11, 0);
    enum_attr!(underline, set_underline, Underline, 0b111, 2);
    enum_attr!(blink, set_blink, Blink, 0b11, 5);
    bool_attr!(italic, set_italic, 7);
    bool_attr!(reverse, set_reverse, 8);
    bool_attr!(strikethrough, set_strikethrough, 9);
    bool_attr!(invisible, set_invisible, 10);
    bool_attr!(wrapped, set_wrapped, 11);
    bool_attr!(overline, set_overline, 12);

    /// Attributes with no styling and default colors.
    pub const fn blank() -> Self {
        Self {
            bits: 0,
            fg: Color::Default,
            bg: Color::Default,
        }
    }

    /// The foreground color.
    pub const fn foreground(&self) -> Color {
        self.fg
    }

    /// Sets the foreground color.
    pub fn set_foreground(&mut self, fg: Color) -> &mut Self {
        self.fg = fg;
        self
    }

    /// The background color.
    pub const fn background(&self) -> Color {
        self.bg
    }

    /// Sets the background color.
    pub fn set_background(&mut self, bg: Color) -> &mut Self {
        self.bg = bg;
        self
    }

    /// Resets all styling and colors.
    pub fn clear(&mut self) {
        *self = Self::blank();
    }
}

impl Default for CellAttrs {
    fn default() -> Self {
        Self::blank()
    }
}

/// The textual content of a cell: one grapheme cluster.
///
/// The overwhelmingly common case is a single character, stored inline
/// as a `char`. Clusters that need multiple code points (combining
/// marks, ZWJ emoji sequences) spill over to a reference-counted
/// string so that cloning cells while scrolling or rewrapping stays
/// cheap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CellText {
    /// A single character.
    Char(char),
    /// A multi-code-point grapheme cluster.
    Grapheme(Arc<str>),
}

impl CellText {
    /// Builds the text for one grapheme cluster.
    ///
    /// Control characters have no glyph of their own and only cause
    /// confusion in later layout stages, so they are stored as spaces.
    /// Single-character clusters are stored inline without allocating.
    pub fn from_grapheme(grapheme: &str) -> Self {
        let defanged = match grapheme.as_bytes() {
            [] | [b'\r', b'\n'] => " ",
            [b] if *b < 0x20 || *b == 0x7f => " ",
            _ => grapheme,
        };
        let mut chars = defanged.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Self::Char(c),
            _ => Self::Grapheme(Arc::from(defanged)),
        }
    }

    /// The number of terminal columns this text occupies (0-2).
    ///
    /// Following WezTerm, the summed character widths are capped at 2:
    /// a grapheme cluster never occupies more than two columns no
    /// matter how many code points (e.g. ZWJ emoji sequences) it
    /// contains.
    pub fn width(&self) -> usize {
        match self {
            Self::Char(c) => c.width().unwrap_or_default(),
            Self::Grapheme(s) => s.width().min(2),
        }
    }

    /// Whether the text consists solely of Unicode whitespace.
    pub fn is_whitespace(&self) -> bool {
        match self {
            Self::Char(c) => c.is_whitespace(),
            Self::Grapheme(s) => s.chars().all(char::is_whitespace),
        }
    }

    /// Appends the text to `buf`.
    pub fn push_to(&self, buf: &mut String) {
        match self {
            Self::Char(c) => buf.push(*c),
            Self::Grapheme(s) => buf.push_str(s),
        }
    }
}

impl fmt::Display for CellText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(c) => write!(f, "{c}"),
            Self::Grapheme(s) => f.write_str(s),
        }
    }
}

/// The content of a single cell of the terminal screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    text: CellText,
    attrs: CellAttrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank()
    }
}

impl Cell {
    /// A cell holding a single character.
    ///
    /// Control characters are rewritten to spaces.
    pub fn new(text: char, attrs: CellAttrs) -> Self {
        let mut buf = [0u8; 4];
        Self::new_grapheme(text.encode_utf8(&mut buf), attrs)
    }

    /// A cell holding a grapheme cluster.
    ///
    /// Control characters are rewritten to spaces.
    pub fn new_grapheme(text: &str, attrs: CellAttrs) -> Self {
        Self {
            text: CellText::from_grapheme(text),
            attrs,
        }
    }

    /// A blank cell: a space with default attributes.
    pub const fn blank() -> Self {
        Self {
            text: CellText::Char(' '),
            attrs: CellAttrs::blank(),
        }
    }

    /// A blank cell carrying `attrs`.
    ///
    /// Used for the spacer cells that follow double-width graphemes
    /// and for erase operations that must preserve a background color.
    pub const fn blank_with_attrs(attrs: CellAttrs) -> Self {
        Self {
            text: CellText::Char(' '),
            attrs,
        }
    }

    /// The cell's text.
    pub const fn text(&self) -> &CellText {
        &self.text
    }

    /// The number of columns this cell occupies.
    pub fn width(&self) -> usize {
        self.text.width()
    }

    /// The cell's attributes.
    pub const fn attrs(&self) -> &CellAttrs {
        &self.attrs
    }

    /// Mutable access to the cell's attributes.
    pub fn attrs_mut(&mut self) -> &mut CellAttrs {
        &mut self.attrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn memory_usage() {
        assert_eq!(std::mem::size_of::<Color>(), 4);
        assert_eq!(std::mem::size_of::<CellAttrs>(), 12);
        assert_eq!(std::mem::size_of::<CellText>(), 16);
        assert_eq!(std::mem::size_of::<Cell>(), 32);
    }

    #[test]
    fn blank_cell() {
        let cell = Cell::blank();
        assert_eq!(cell.text(), &CellText::Char(' '));
        assert_eq!(cell.width(), 1);
        assert_eq!(cell.attrs(), &CellAttrs::blank());
        assert!(cell.text().is_whitespace());
    }

    #[test]
    fn control_chars_are_defanged() {
        for c in ['\0', '\n', '\r', '\t', '\x1b', '\x7f'] {
            assert_eq!(
                Cell::new(c, CellAttrs::blank()).text(),
                &CellText::Char(' ')
            );
        }
        assert_eq!(
            Cell::new_grapheme("", CellAttrs::blank()).text(),
            &CellText::Char(' ')
        );
        assert_eq!(
            Cell::new_grapheme("\r\n", CellAttrs::blank()).text(),
            &CellText::Char(' ')
        );
    }

    #[test]
    fn multi_code_point_clusters_spill_to_grapheme() {
        // e + combining acute accent: one cluster, two code points.
        let combining = Cell::new_grapheme("e\u{301}", CellAttrs::blank());
        assert_eq!(combining.text(), &CellText::Grapheme("e\u{301}".into()));
        assert_eq!(combining.width(), 1);

        // A ZWJ family emoji sequence stays one cluster and one cell.
        let family = "\u{1f469}\u{200d}\u{1f91d}\u{200d}\u{1f468}";
        let emoji = Cell::new_grapheme(family, CellAttrs::blank());
        assert_eq!(emoji.text(), &CellText::Grapheme(family.into()));
        assert_eq!(emoji.width(), 2);
    }

    #[test]
    fn widths() {
        assert_eq!(CellText::Char('a').width(), 1);
        assert_eq!(CellText::Char('永').width(), 2);
        // Ideographic space occupies two columns.
        assert_eq!(CellText::Char('\u{3000}').width(), 2);
        // Wide chars cap at 2 even inside long clusters.
        let family = "\u{1f469}\u{200d}\u{1f91d}\u{200d}\u{1f468}";
        assert_eq!(CellText::Grapheme(family.into()).width(), 2);
    }

    #[test]
    fn attribute_roundtrip() {
        let mut attrs = CellAttrs::blank();
        attrs
            .set_intensity(Intensity::Bold)
            .set_underline(Underline::Curly)
            .set_blink(Blink::Rapid)
            .set_italic(true)
            .set_reverse(true)
            .set_strikethrough(true)
            .set_invisible(true)
            .set_wrapped(true)
            .set_overline(true)
            .set_foreground(Color::Indexed(10))
            .set_background(Color::Rgb { r: 1, g: 2, b: 3 });

        assert_eq!(attrs.intensity(), Intensity::Bold);
        assert_eq!(attrs.underline(), Underline::Curly);
        assert_eq!(attrs.blink(), Blink::Rapid);
        assert!(attrs.italic());
        assert!(attrs.reverse());
        assert!(attrs.strikethrough());
        assert!(attrs.invisible());
        assert!(attrs.wrapped());
        assert!(attrs.overline());
        assert_eq!(attrs.foreground(), Color::Indexed(10));
        assert_eq!(attrs.background(), Color::Rgb { r: 1, g: 2, b: 3 });
        assert_ne!(attrs, CellAttrs::blank());

        attrs.set_italic(false).set_intensity(Intensity::Normal);
        assert!(!attrs.italic());
        assert_eq!(attrs.intensity(), Intensity::Normal);

        attrs.clear();
        assert_eq!(attrs, CellAttrs::blank());
    }

    #[test]
    fn attribute_bits_are_isolated() {
        let mut attrs = CellAttrs::blank();
        attrs.set_underline(Underline::Double);
        assert_eq!(attrs.intensity(), Intensity::Normal);
        assert_eq!(attrs.blink(), Blink::None);
        attrs.set_intensity(Intensity::Half);
        assert_eq!(attrs.underline(), Underline::Double);
    }

    #[test]
    fn display_and_push_to() {
        let mut s = String::new();
        CellText::Char('x').push_to(&mut s);
        CellText::Grapheme("e\u{301}".into()).push_to(&mut s);
        assert_eq!(s, "xe\u{301}");
        assert_eq!(CellText::Char('y').to_string(), "y");
        assert_eq!(CellText::Grapheme("永".into()).to_string(), "永");
    }

    #[test]
    fn whitespace_detection() {
        assert!(CellText::Char(' ').is_whitespace());
        assert!(CellText::Char('\u{a0}').is_whitespace()); // NBSP
        assert!(!CellText::Char('x').is_whitespace());
        assert!(CellText::Grapheme("\u{3000}".into()).is_whitespace());
    }
}
