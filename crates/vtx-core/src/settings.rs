//! Application-wide configuration, edited in the Settings tab.

/// User-configurable options.
///
/// Pure data model: no UI, no dependency on egui. The app layer renders and
/// edits this in its Settings tab, and `vtx-core` consumes the values when
/// building terminal sessions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppSettings {
    /// Terminal font size in points.
    pub font_size: u32,
    /// Scrollback buffer size in lines.
    pub scrollback_lines: u32,
    /// Whether the terminal cursor blinks.
    pub cursor_blink: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            font_size: 14,
            scrollback_lines: 10_000,
            cursor_blink: true,
        }
    }
}
