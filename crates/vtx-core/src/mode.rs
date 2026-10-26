//! Terminal mode flags.
//!
//! [`TermMode`] tracks the ANSI/DEC private modes which control terminal
//! behavior (cursor visibility, mouse reporting, line wrap, ...). The bits
//! are toggled by `SM`/`RM` (`CSI h` / `CSI l` and `CSI ? h` / `CSI ? l`)
//! sequences handled by the term state machine.

use bitflags::bitflags;
use vte::ansi::KeyboardModes;

bitflags! {
    /// Terminal mode flags.
    ///
    /// The bit layout matches `alacritty_terminal` so behavior stays
    /// comparable while porting.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct TermMode: u32 {
        /// Dummy mode, never set.
        const NONE                    = 0;
        /// Cursor visible (DECTCEM, mode 25).
        const SHOW_CURSOR             = 1;
        /// Application cursor keys (DECCKM, mode 1).
        const APP_CURSOR              = 1 << 1;
        /// Application keypad (DECNKM, mode 66).
        const APP_KEYPAD              = 1 << 2;
        /// Report mouse clicks (mode 1000).
        const MOUSE_REPORT_CLICK      = 1 << 3;
        /// Bracketed paste (mode 2004).
        const BRACKETED_PASTE         = 1 << 4;
        /// SGR mouse protocol (mode 1006).
        const SGR_MOUSE               = 1 << 5;
        /// Report mouse motion (mode 1002).
        const MOUSE_MOTION            = 1 << 6;
        /// Autowrap lines (DECAWM, mode 7).
        const LINE_WRAP               = 1 << 7;
        /// Line feed implies carriage return (LNM, mode 20).
        const LINE_FEED_NEW_LINE      = 1 << 8;
        /// Origin mode, cursor addressing is scroll region relative (DECOM, mode 6).
        const ORIGIN                  = 1 << 9;
        /// Insert mode, printed characters shift the line (IRM, mode 4).
        const INSERT                  = 1 << 10;
        /// Report focus in/out events (mode 1004).
        const FOCUS_IN_OUT            = 1 << 11;
        /// Alternate screen buffer is active (mode 1049).
        const ALT_SCREEN              = 1 << 12;
        /// Report mouse drag motion (mode 1003).
        const MOUSE_DRAG              = 1 << 13;
        /// UTF-8 mouse protocol (mode 1005).
        const UTF8_MOUSE              = 1 << 14;
        /// Scroll history on the alternate screen (mode 1007).
        const ALTERNATE_SCROLL        = 1 << 15;
        /// Vi mode, keyboard-driven cursor movement.
        const VI                      = 1 << 16;
        /// Request urgency hints on bell.
        const URGENCY_HINTS           = 1 << 17;
        /// Kitty keyboard protocol: disambiguate escape codes.
        const DISAMBIGUATE_ESC_CODES  = 1 << 18;
        /// Kitty keyboard protocol: report event types.
        const REPORT_EVENT_TYPES      = 1 << 19;
        /// Kitty keyboard protocol: report alternate keys.
        const REPORT_ALTERNATE_KEYS   = 1 << 20;
        /// Kitty keyboard protocol: report all keys as escape codes.
        const REPORT_ALL_KEYS_AS_ESC  = 1 << 21;
        /// Kitty keyboard protocol: report associated text.
        const REPORT_ASSOCIATED_TEXT  = 1 << 22;
        /// Union of all mouse reporting modes.
        const MOUSE_MODE              = Self::MOUSE_REPORT_CLICK.bits() | Self::MOUSE_MOTION.bits() | Self::MOUSE_DRAG.bits();
        /// Union of all kitty keyboard protocol modes.
        const KITTY_KEYBOARD_PROTOCOL = Self::DISAMBIGUATE_ESC_CODES.bits()
                                      | Self::REPORT_EVENT_TYPES.bits()
                                      | Self::REPORT_ALTERNATE_KEYS.bits()
                                      | Self::REPORT_ALL_KEYS_AS_ESC.bits()
                                      | Self::REPORT_ASSOCIATED_TEXT.bits();
        /// All modes.
        const ANY                     = u32::MAX;
    }
}

impl From<KeyboardModes> for TermMode {
    fn from(value: KeyboardModes) -> Self {
        let mut mode = Self::empty();

        let disambiguate_esc_codes = value.contains(KeyboardModes::DISAMBIGUATE_ESC_CODES);
        mode.set(TermMode::DISAMBIGUATE_ESC_CODES, disambiguate_esc_codes);

        let report_event_types = value.contains(KeyboardModes::REPORT_EVENT_TYPES);
        mode.set(TermMode::REPORT_EVENT_TYPES, report_event_types);

        let report_alternate_keys = value.contains(KeyboardModes::REPORT_ALTERNATE_KEYS);
        mode.set(TermMode::REPORT_ALTERNATE_KEYS, report_alternate_keys);

        let report_all_keys_as_esc = value.contains(KeyboardModes::REPORT_ALL_KEYS_AS_ESC);
        mode.set(TermMode::REPORT_ALL_KEYS_AS_ESC, report_all_keys_as_esc);

        let report_associated_text = value.contains(KeyboardModes::REPORT_ASSOCIATED_TEXT);
        mode.set(TermMode::REPORT_ASSOCIATED_TEXT, report_associated_text);

        mode
    }
}

impl Default for TermMode {
    fn default() -> TermMode {
        TermMode::SHOW_CURSOR
            | TermMode::LINE_WRAP
            | TermMode::ALTERNATE_SCROLL
            | TermMode::URGENCY_HINTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_modes() {
        // Modes a terminal starts with per DEC/xterm defaults.
        let mode = TermMode::default();

        assert!(mode.contains(TermMode::SHOW_CURSOR));
        assert!(mode.contains(TermMode::LINE_WRAP));
        assert!(mode.contains(TermMode::ALTERNATE_SCROLL));
        assert!(mode.contains(TermMode::URGENCY_HINTS));
        assert!(!mode.contains(TermMode::APP_CURSOR));
        assert!(!mode.contains(TermMode::ALT_SCREEN));
        assert!(!mode.contains(TermMode::ORIGIN));
        assert!(!mode.contains(TermMode::MOUSE_MODE));
        assert_eq!(
            mode,
            TermMode::SHOW_CURSOR
                | TermMode::LINE_WRAP
                | TermMode::ALTERNATE_SCROLL
                | TermMode::URGENCY_HINTS
        );
    }

    #[test]
    fn mouse_mode_union() {
        assert!(TermMode::MOUSE_MODE.contains(TermMode::MOUSE_REPORT_CLICK));
        assert!(TermMode::MOUSE_MODE.contains(TermMode::MOUSE_MOTION));
        assert!(TermMode::MOUSE_MODE.contains(TermMode::MOUSE_DRAG));
        assert!(!TermMode::MOUSE_MODE.contains(TermMode::UTF8_MOUSE));
    }

    #[test]
    fn kitty_keyboard_union() {
        assert!(TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::REPORT_EVENT_TYPES));
        assert!(TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::REPORT_ALTERNATE_KEYS));
        assert!(TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::REPORT_ALL_KEYS_AS_ESC));
        assert!(TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::REPORT_ASSOCIATED_TEXT));
        assert!(!TermMode::KITTY_KEYBOARD_PROTOCOL.contains(TermMode::VI));
    }

    #[test]
    fn from_keyboard_modes() {
        let keyboard_modes =
            KeyboardModes::DISAMBIGUATE_ESC_CODES | KeyboardModes::REPORT_ALL_KEYS_AS_ESC;

        let mode = TermMode::from(keyboard_modes);

        assert!(mode.contains(TermMode::DISAMBIGUATE_ESC_CODES));
        assert!(mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC));
        assert!(!mode.contains(TermMode::REPORT_EVENT_TYPES));
        assert!(!mode.contains(TermMode::REPORT_ALTERNATE_KEYS));
        assert!(!mode.contains(TermMode::REPORT_ASSOCIATED_TEXT));

        // The empty set converts to the empty set.
        assert_eq!(TermMode::from(KeyboardModes::empty()), TermMode::empty());
    }

    #[test]
    fn any_is_all_bits() {
        assert_eq!(TermMode::ANY, TermMode::from_bits_retain(u32::MAX));
        assert!(TermMode::ANY.contains(TermMode::MOUSE_MODE));
        assert!(TermMode::ANY.contains(TermMode::KITTY_KEYBOARD_PROTOCOL));
    }
}
