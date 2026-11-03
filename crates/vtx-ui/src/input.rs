//! Translation of egui keyboard events into terminal input bytes.
//!
//! egui-winit delivers one keystroke as up to two events: a [`Key` event
//! for the logical key](egui::Event::Key), and a
//! [text event](egui::Event::Text) with the characters it produced. The text
//! event is suppressed while a command modifier (Ctrl on Windows and Linux,
//! Cmd on macOS) is held, and control characters like `\r` or `\t` never
//! carry text at all. The translation below therefore handles text events
//! for printable input and key events for everything else, so no keystroke
//! is written twice.

use egui::{Event, Key, Modifiers};

use vtx_core::mode::TermMode;

/// Translate this frame's egui input events into bytes for the child.
///
/// `mode` carries the terminal modes that affect the encoding, like
/// application cursor keys.
pub fn encode(events: &[Event], mode: TermMode) -> Vec<u8> {
    let mut input = Vec::new();
    for event in events {
        match event {
            Event::Text(text) => input.extend_from_slice(text.as_bytes()),
            Event::Paste(text) => input.extend_from_slice(&paste(text, mode)),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => encode_key(&mut input, *key, *modifiers, mode),
            _ => (),
        }
    }
    input
}

/// The input bytes for pasted `text`.
///
/// The child expects carriage returns for newlines, and bracketed paste
/// (mode 2004) wraps the text in markers so shells and editors can tell
/// typed and pasted input apart. Escape and ETX are filtered from
/// bracketed text so pasted content cannot forge the bracket end marker.
pub fn paste(text: &str, mode: TermMode) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");

    if mode.contains(TermMode::BRACKETED_PASTE) {
        let text = text.replace(['\x1b', '\x03'], "");

        let mut input = b"\x1b[200~".to_vec();
        input.extend_from_slice(text.as_bytes());
        input.extend_from_slice(b"\x1b[201~");
        input
    } else {
        text.into_bytes()
    }
}

/// The input bytes reporting a focus change to the child (mode 1004).
pub fn focus(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

/// Append the input bytes for one key press.
fn encode_key(input: &mut Vec<u8>, key: Key, modifiers: Modifiers, mode: TermMode) {
    if modifiers.ctrl {
        // Ctrl+Shift+C/V are the terminal's clipboard shortcuts; the view
        // handles them against the system clipboard.
        if modifiers.shift && matches!(key, Key::C | Key::V) {
            return;
        }

        // Control codes: Ctrl+C becomes ETX (0x03), like in any terminal.
        // Other Ctrl combinations, like Ctrl+Arrow, wait for a key binding
        // layer that can map them to escape sequences.
        if (Key::A..=Key::Z).contains(&key) {
            input.push(key as u8 - Key::A as u8 + 1);
        } else if key == Key::Space {
            input.push(0x00);
        }
        return;
    }

    // Shift+PageUp/PageDown/Home/End are the terminal's scroll shortcuts;
    // the view scrolls the scrollback instead of writing to the child.
    if modifiers.shift
        && !modifiers.alt
        && matches!(key, Key::PageUp | Key::PageDown | Key::Home | Key::End)
    {
        return;
    }

    if modifiers.alt {
        // Alt combinations need escaping per key and layout; not supported
        // yet. AltGr is not reported as Alt on Windows, so layouts using it
        // for characters still work through their text events.
        return;
    }

    // Plain printable keys arrive as text events next to this key event;
    // writing them here as well would duplicate them.
    match key {
        Key::Enter => input.push(b'\r'),
        Key::Backspace => input.push(0x7f),
        Key::Tab => input.extend_from_slice(if modifiers.shift { b"\x1b[Z" } else { b"\t" }),
        Key::Escape => input.push(0x1b),
        Key::ArrowUp => input.extend_from_slice(arrow(mode, b'A')),
        Key::ArrowDown => input.extend_from_slice(arrow(mode, b'B')),
        Key::ArrowRight => input.extend_from_slice(arrow(mode, b'C')),
        Key::ArrowLeft => input.extend_from_slice(arrow(mode, b'D')),
        Key::Home => input.extend_from_slice(b"\x1b[H"),
        Key::End => input.extend_from_slice(b"\x1b[F"),
        Key::Insert => input.extend_from_slice(b"\x1b[2~"),
        Key::Delete => input.extend_from_slice(b"\x1b[3~"),
        Key::PageUp => input.extend_from_slice(b"\x1b[5~"),
        Key::PageDown => input.extend_from_slice(b"\x1b[6~"),
        _ => (),
    }
}

/// The input bytes for an arrow key under `mode`.
///
/// Application cursor keys (DECCKM) switch the prefix from CSI to SS3.
fn arrow(mode: TermMode, key: u8) -> &'static [u8] {
    if mode.contains(TermMode::APP_CURSOR) {
        match key {
            b'A' => b"\x1bOA",
            b'B' => b"\x1bOB",
            b'C' => b"\x1bOC",
            _ => b"\x1bOD",
        }
    } else {
        match key {
            b'A' => b"\x1b[A",
            b'B' => b"\x1b[B",
            b'C' => b"\x1b[C",
            _ => b"\x1b[D",
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers};

    use super::{encode, paste};
    use vtx_core::mode::TermMode;

    /// A key press event.
    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn text_events_are_passed_through() {
        let events = [Event::Text("hello ünïcode".to_owned())];
        assert_eq!(
            encode(&events, TermMode::empty()),
            "hello ünïcode".as_bytes()
        );
    }

    #[test]
    fn enter_maps_to_carriage_return() {
        let events = [key(Key::Enter, Modifiers::default())];
        assert_eq!(encode(&events, TermMode::empty()), b"\r");
    }

    #[test]
    fn backspace_maps_to_del() {
        let events = [key(Key::Backspace, Modifiers::default())];
        assert_eq!(encode(&events, TermMode::empty()), b"\x7f");
    }

    #[test]
    fn tab_and_shift_tab() {
        let events = [key(Key::Tab, Modifiers::default())];
        assert_eq!(encode(&events, TermMode::empty()), b"\t");

        let events = [key(Key::Tab, Modifiers::SHIFT)];
        assert_eq!(encode(&events, TermMode::empty()), b"\x1b[Z");
    }

    #[test]
    fn arrows_follow_cursor_mode() {
        let events = [
            key(Key::ArrowUp, Modifiers::default()),
            key(Key::ArrowDown, Modifiers::default()),
            key(Key::ArrowRight, Modifiers::default()),
            key(Key::ArrowLeft, Modifiers::default()),
        ];

        assert_eq!(
            encode(&events, TermMode::empty()),
            b"\x1b[A\x1b[B\x1b[C\x1b[D"
        );
        assert_eq!(
            encode(&events, TermMode::APP_CURSOR),
            b"\x1bOA\x1bOB\x1bOC\x1bOD"
        );
    }

    #[test]
    fn ctrl_letters_become_control_codes() {
        let events = [
            key(Key::C, Modifiers::CTRL),
            key(Key::A, Modifiers::CTRL),
            key(Key::Z, Modifiers::CTRL),
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"\x03\x01\x1a");
    }

    #[test]
    fn ctrl_space_becomes_nul() {
        let events = [key(Key::Space, Modifiers::CTRL)];
        assert_eq!(encode(&events, TermMode::empty()), b"\x00");
    }

    #[test]
    fn clipboard_shortcuts_stay_in_the_frontend() {
        // Ctrl+Shift+C/V must not reach the child as control codes; the
        // view handles them against the system clipboard.
        let events = [
            key(Key::C, Modifiers::CTRL | Modifiers::SHIFT),
            key(Key::V, Modifiers::CTRL | Modifiers::SHIFT),
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"");
    }

    #[test]
    fn scroll_shortcuts_stay_in_the_frontend() {
        // Shift+PageUp/PageDown/Home/End scroll the scrollback; the child
        // never sees them.
        let events = [
            key(Key::PageUp, Modifiers::SHIFT),
            key(Key::PageDown, Modifiers::SHIFT),
            key(Key::Home, Modifiers::SHIFT),
            key(Key::End, Modifiers::SHIFT),
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"");
    }

    #[test]
    fn plain_navigation_keys_reach_the_child() {
        let events = [
            key(Key::PageUp, Modifiers::NONE),
            key(Key::Home, Modifiers::NONE),
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"\x1b[5~\x1b[H");
    }

    #[test]
    fn paste_normalizes_newlines() {
        assert_eq!(paste("a\r\nb\nc", TermMode::empty()), b"a\rb\rc");
    }

    #[test]
    fn paste_wraps_in_brackets_when_mode_set() {
        assert_eq!(
            paste("ls", TermMode::BRACKETED_PASTE),
            b"\x1b[200~ls\x1b[201~"
        );
    }

    #[test]
    fn paste_events_encode_through_brackets() {
        let events = [Event::Paste("a\nb".to_owned())];
        assert_eq!(
            encode(&events, TermMode::BRACKETED_PASTE),
            b"\x1b[200~a\rb\x1b[201~"
        );
    }

    #[test]
    fn plain_keys_wait_for_their_text_event() {
        // Printable keys arrive as both a key and a text event; only the
        // text event is written, so 'a' is not duplicated.
        let events = [
            key(Key::A, Modifiers::default()),
            Event::Text("a".to_owned()),
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"a");
    }

    #[test]
    fn key_repeats_keep_writing() {
        // Holding a key must keep sending it, like in any terminal.
        let events = [
            key(Key::Backspace, Modifiers::default()),
            Event::Key {
                key: Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: Modifiers::default(),
            },
        ];
        assert_eq!(encode(&events, TermMode::empty()), b"\x7f\x7f");
    }

    #[test]
    fn released_keys_are_ignored() {
        let events = [Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::default(),
        }];
        assert_eq!(encode(&events, TermMode::empty()), b"");
    }
}
