//! Mouse reporting protocol encoding.
//!
//! Pure functions translating mouse events into the escape sequence the
//! application requested via DECSET: the X10-style default encoding
//! (`CSI M cb cx cy`), the UTF-8 extension (DECSET 1005), and the SGR
//! extension (DECSET 1006). The frontend calls these while
//! [`TermMode::MOUSE_MODE`] is active and Shift is not held.

use crate::mode::TermMode;

/// Buttons with their xterm protocol codes.
///
/// The wheel buttons are what the scrolling frontend reports instead of
/// scrolling locally while mouse reporting is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// Left button (code 0).
    Left,
    /// Middle button (code 1).
    Middle,
    /// Right button (code 2).
    Right,
    /// Wheel scrolled up (code 64).
    WheelUp,
    /// Wheel scrolled down (code 65).
    WheelDown,
}

impl MouseButton {
    /// The xterm protocol button code.
    fn code(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
        }
    }
}

/// A mouse event to encode for the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    /// The button the event belongs to.
    pub button: MouseButton,

    /// Whether this is a motion event with the button held.
    ///
    /// Motion events add 32 to the button code.
    pub motion: bool,

    /// Whether the button was pressed (as opposed to released).
    pub pressed: bool,

    /// Zero-based grid cell column.
    pub column: usize,

    /// Zero-based grid cell line.
    pub line: usize,

    /// Whether Shift was held.
    ///
    /// Note that the frontend never reports while Shift alone is held; the
    /// flag still matters in combination with other modifiers.
    pub shift: bool,

    /// Whether Alt was held.
    pub alt: bool,

    /// Whether Ctrl was held.
    pub ctrl: bool,
}

impl MouseEvent {
    /// The modifier flags of this event.
    fn modifier_code(&self) -> u8 {
        let mut mods = 0;
        if self.shift {
            mods += 4;
        }
        if self.alt {
            mods += 8;
        }
        if self.ctrl {
            mods += 16;
        }
        mods
    }

    /// The button code including motion and modifier flags.
    fn button_code(&self) -> u8 {
        let mut button = self.button.code();
        if self.motion {
            button += 32;
        }
        button + self.modifier_code()
    }
}

/// Encode `event` for the active mouse reporting protocol.
///
/// Returns `None` when the coordinate cannot be represented in the
/// requested encoding; the frontend then silently drops the event.
pub fn encode_mouse_event(mode: &TermMode, event: MouseEvent) -> Option<Vec<u8>> {
    if mode.contains(TermMode::SGR_MOUSE) {
        sgr_mouse_report(&event)
    } else {
        // The legacy encodings signal a release with button code 3,
        // independent of the actual button.
        let button = if event.pressed {
            event.button_code()
        } else {
            3 + event.modifier_code()
        };
        normal_mouse_report(mode, button, event.column, event.line)
    }
}

/// SGR mouse report (`CSI < cb ; cx ; cy M/m`).
fn sgr_mouse_report(event: &MouseEvent) -> Option<Vec<u8>> {
    let state = if event.pressed { 'M' } else { 'm' };
    Some(
        format!(
            "\x1b[<{};{};{}{}",
            event.button_code(),
            event.column + 1,
            event.line + 1,
            state
        )
        .into_bytes(),
    )
}

/// X10-style mouse report (`CSI M cb cx cy`), optionally UTF-8 extended.
fn normal_mouse_report(mode: &TermMode, button: u8, column: usize, line: usize) -> Option<Vec<u8>> {
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max_point = if utf8 { 2015 } else { 223 };
    if line >= max_point || column >= max_point {
        return None;
    }

    // Translate to 1-based coordinates within the printable range.
    let column = 32 + 1 + column;
    let line = 32 + 1 + line;

    // The UTF-8 extension reports wide coordinates as two-byte UTF-8
    // sequences instead of raw bytes.
    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];
    if utf8 && column >= 95 {
        msg.push(0xC0 + (column / 64) as u8);
        msg.push(0x80 + (column & 63) as u8);
    } else {
        msg.push(column as u8);
    }
    if utf8 && line >= 95 {
        msg.push(0xC0 + (line / 64) as u8);
        msg.push(0x80 + (line & 63) as u8);
    } else {
        msg.push(line as u8);
    }

    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr_mode() -> TermMode {
        TermMode::SGR_MOUSE
    }

    fn utf8_mode() -> TermMode {
        TermMode::UTF8_MOUSE | TermMode::MOUSE_REPORT_CLICK
    }

    fn plain_mode() -> TermMode {
        TermMode::MOUSE_REPORT_CLICK
    }

    #[test]
    fn sgr_press_release() {
        let event = MouseEvent {
            button: MouseButton::Left,
            motion: false,
            pressed: true,
            column: 0,
            line: 0,
            shift: false,
            alt: false,
            ctrl: false,
        };
        assert_eq!(
            encode_mouse_event(&sgr_mode(), event),
            Some(b"\x1b[<0;1;1M".to_vec())
        );

        let released = MouseEvent {
            pressed: false,
            ..event
        };
        assert_eq!(
            encode_mouse_event(&sgr_mode(), released),
            Some(b"\x1b[<0;1;1m".to_vec())
        );
    }

    #[test]
    fn sgr_wheel_and_modifiers() {
        let event = MouseEvent {
            button: MouseButton::WheelUp,
            motion: false,
            pressed: true,
            column: 79,
            line: 23,
            shift: false,
            alt: true,
            ctrl: true,
        };
        // 64 (wheel) + 8 (alt) + 16 (ctrl) = 88.
        assert_eq!(
            encode_mouse_event(&sgr_mode(), event),
            Some(b"\x1b[<88;80;24M".to_vec())
        );
    }

    #[test]
    fn normal_press_release_and_motion() {
        let event = MouseEvent {
            button: MouseButton::Middle,
            motion: false,
            pressed: true,
            column: 5,
            line: 9,
            shift: false,
            alt: false,
            ctrl: false,
        };
        // 32+1 (button), 32+1+5 (column), 32+1+9 (line).
        assert_eq!(
            encode_mouse_event(&plain_mode(), event),
            Some(b"\x1b[M!&*".to_vec())
        );

        // Releases use button code 3 regardless of the button.
        let released = MouseEvent {
            pressed: false,
            ..event
        };
        assert_eq!(
            encode_mouse_event(&plain_mode(), released),
            Some(b"\x1b[M#&*".to_vec())
        );

        // Motion adds 32 to the button code.
        let motion = MouseEvent {
            motion: true,
            pressed: true,
            ..event
        };
        assert_eq!(
            encode_mouse_event(&plain_mode(), motion),
            Some(b"\x1b[MA&*".to_vec())
        );
    }

    #[test]
    fn utf8_extended_coordinates() {
        let event = MouseEvent {
            button: MouseButton::Left,
            motion: false,
            pressed: true,
            column: 95,
            line: 127,
            shift: false,
            alt: false,
            ctrl: false,
        };
        // Column 95 -> 128 -> [0xC2, 0x80]; line 127 -> 160 -> [0xC2, 0xA0].
        assert_eq!(
            encode_mouse_event(&utf8_mode(), event),
            Some(b"\x1b[M \xc2\x80\xc2\xa0".to_vec())
        );

        // Coordinates beyond the encodable range are dropped.
        let out_of_range = MouseEvent {
            column: 2016,
            ..event
        };
        assert_eq!(encode_mouse_event(&utf8_mode(), out_of_range), None);
    }
}
