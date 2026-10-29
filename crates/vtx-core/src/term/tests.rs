//! Tests for the terminal state machine.

use std::sync::mpsc::{Receiver, channel};

use vte::ansi::{Color, NamedColor, Processor, Rgb};

use super::Term;
use crate::cell::Flags;
use crate::event::{Event, EventListener, VoidListener};
use crate::index::{Column, Line, Point};

/// Event listener recording events into a channel.
#[derive(Clone)]
struct RecordingListener(std::sync::mpsc::Sender<Event>);

impl RecordingListener {
    /// Create a listener and its event receiver.
    fn new() -> (Self, Receiver<Event>) {
        let (tx, rx) = channel();
        (Self(tx), rx)
    }
}

impl EventListener for RecordingListener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// Drain all events currently pending on the receiver.
fn drain(receiver: &Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

/// Create a 10x5 terminal without scrollback.
fn term() -> Term<VoidListener> {
    Term::new(10, 5, 0, VoidListener)
}

/// Feed a string into the terminal, as if it came from the PTY.
fn feed<T: EventListener>(term: &mut Term<T>, content: &str) {
    let mut processor: Processor = Processor::new();
    processor.advance(term, content.as_bytes());
}

/// Assert the cursor's position and wrap state.
#[track_caller]
fn assert_cursor<T: EventListener>(term: &Term<T>, line: i32, column: usize, needs_wrap: bool) {
    assert_eq!(
        term.cursor_point(),
        Point::new(Line(line), Column(column)),
        "unexpected cursor position",
    );
    assert_eq!(
        term.grid().cursor.input_needs_wrap,
        needs_wrap,
        "unexpected input_needs_wrap",
    );
}

#[test]
fn print_and_advance_cursor() {
    let mut term = term();

    feed(&mut term, "ab");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
    assert_cursor(&term, 0, 2, false);
}

#[test]
fn print_zero_width() {
    let mut term = term();

    // Combining acute accent over `e`.
    feed(&mut term, "e\u{301}");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'e');
    assert_eq!(
        term.grid()[Line(0)][Column(0)]
            .extra
            .as_ref()
            .and_then(|extra| extra.zerowidth().first()),
        Some(&'\u{301}'),
    );
    // Zero-width characters do not advance the cursor.
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn print_wide_char() {
    let mut term = term();

    feed(&mut term, "字");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, '字');
    assert!(
        term.grid()[Line(0)][Column(0)]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
    assert!(
        term.grid()[Line(0)][Column(1)]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_cursor(&term, 0, 2, false);
}

#[test]
fn print_wide_char_at_line_end() {
    let mut term = term();

    // Fill 9 columns, then print a wide char which does not fit anymore.
    feed(&mut term, "123456789字");

    // The last column is replaced by a leading spacer, the wide char wraps.
    assert_eq!(term.grid()[Line(0)][Column(9)].c, ' ');
    assert!(
        term.grid()[Line(0)][Column(9)]
            .flags
            .contains(Flags::LEADING_WIDE_CHAR_SPACER)
    );
    assert_eq!(term.grid()[Line(1)][Column(0)].c, '字');
    assert_cursor(&term, 1, 2, false);
}

#[test]
fn line_wrap() {
    let mut term = term();

    feed(&mut term, "1234567890");

    // The cursor stays at the last column while waiting for the next glyph;
    // the WRAPLINE flag is only set once the wrap actually happens.
    assert_cursor(&term, 0, 9, true);
    assert!(
        !term.grid()[Line(0)][Column(9)]
            .flags
            .contains(Flags::WRAPLINE)
    );

    feed(&mut term, "a");

    assert!(
        term.grid()[Line(0)][Column(9)]
            .flags
            .contains(Flags::WRAPLINE)
    );
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'a');
    assert_cursor(&term, 1, 1, false);
}

#[test]
fn no_wrap_when_disabled() {
    let mut term = term();

    term.mode_mut().remove(crate::mode::TermMode::LINE_WRAP);

    feed(&mut term, "1234567890a");

    // With linewrapping disabled the cursor sticks to the last column and
    // further glyphs overwrite it.
    assert_eq!(term.grid()[Line(0)][Column(9)].c, 'a');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 9, true);
}

#[test]
fn carriage_return() {
    let mut term = term();

    feed(&mut term, "abc\rX");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'X');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn linefeed_keeps_column() {
    let mut term = term();

    feed(&mut term, "ab\nc");

    assert_eq!(term.grid()[Line(1)][Column(2)].c, 'c');
    assert_cursor(&term, 1, 3, false);
}

#[test]
fn linefeed_scrolls_at_bottom() {
    let mut term = term();

    // Fill all five lines. Newlines arrive as CRLF since the PTY's ONLCR
    // discipline converts application-side `\n` to `\r\n`.
    feed(
        &mut term,
        "1111111111\r\n2222222222\r\n3333333333\r\n4444444444\r\n5555555555\r\nb",
    );

    // The first line scrolled out (no history configured), everything moved
    // up by one, and `b` was printed on the fresh bottom line.
    assert_eq!(term.grid()[Line(0)][Column(0)].c, '2');
    assert_eq!(term.grid()[Line(3)][Column(0)].c, '5');
    assert_eq!(term.grid()[Line(4)][Column(0)].c, 'b');
}

#[test]
fn backspace() {
    let mut term = term();

    feed(&mut term, "ab\x08c");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'c');
    assert_cursor(&term, 0, 2, false);
}

#[test]
fn backspace_at_start() {
    let mut term = term();

    feed(&mut term, "\x08a");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn tab() {
    let mut term = term();

    feed(&mut term, "\tX");

    assert_eq!(term.grid()[Line(0)][Column(8)].c, 'X');
    assert_cursor(&term, 0, 9, false);
}

#[test]
fn tab_after_last_stop() {
    let mut term = term();

    feed(&mut term, "a\t\t");

    // After the last tabstop (column 8) the cursor just moves by one and
    // stops at the last column, without requesting a wrap.
    assert_cursor(&term, 0, 9, false);
}

#[test]
fn cup() {
    let mut term = term();

    feed(&mut term, "\x1b[3;5HX");

    assert_eq!(term.grid()[Line(2)][Column(4)].c, 'X');
    assert_cursor(&term, 2, 5, false);
}

#[test]
fn cup_defaults() {
    let mut term = term();

    feed(&mut term, "abc\x1b[HX");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'X');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn cup_clamps_to_bounds() {
    let mut term = term();

    feed(&mut term, "\x1b[99;99HX");

    assert_eq!(term.grid()[Line(4)][Column(9)].c, 'X');
    // Writing to the last column leaves the cursor waiting for a wrap.
    assert_cursor(&term, 4, 9, true);
}

#[test]
fn cursor_movement() {
    let mut term = term();

    feed(&mut term, "\x1b[3;5H");
    assert_cursor(&term, 2, 4, false);

    feed(&mut term, "\x1b[A"); // CUU: up one
    assert_cursor(&term, 1, 4, false);

    feed(&mut term, "\x1b[B"); // CUD: down one
    assert_cursor(&term, 2, 4, false);

    feed(&mut term, "\x1b[C"); // CUF: right one
    assert_cursor(&term, 2, 5, false);

    feed(&mut term, "\x1b[D"); // CUB: left one
    assert_cursor(&term, 2, 4, false);

    // Movements are clamped at the viewport borders.
    feed(&mut term, "\x1b[100A");
    assert_cursor(&term, 0, 4, false);

    feed(&mut term, "\x1b[100D");
    assert_cursor(&term, 0, 0, false);
}

#[test]
fn ed_below() {
    let mut term = term();

    feed(&mut term, "1111111111\r\n2222222222\r\n3333333333");
    feed(&mut term, "\x1b[2;5H"); // Cursor to line 2, column 5.
    feed(&mut term, "\x1b[J"); // ED: below.

    // The cursor cell and everything after it is cleared.
    assert_eq!(term.grid()[Line(1)][Column(4)].c, ' ');
    assert_eq!(term.grid()[Line(1)][Column(9)].c, ' ');
    assert_eq!(term.grid()[Line(2)][Column(0)].c, ' ');
    // Content above the cursor is untouched.
    assert_eq!(term.grid()[Line(0)][Column(0)].c, '1');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, '2');
}

#[test]
fn ed_above() {
    let mut term = term();

    feed(&mut term, "1111111111\r\n2222222222\r\n3333333333");
    feed(&mut term, "\x1b[2;5H"); // Cursor to line 2, column 5.
    feed(&mut term, "\x1b[1J"); // ED: above.

    // Content below the cursor is untouched.
    assert_eq!(term.grid()[Line(2)][Column(0)].c, '3');
    // The cursor cell and everything before it is cleared.
    assert_eq!(term.grid()[Line(1)][Column(4)].c, ' ');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, ' ');
    // Lines above are only reset when the cursor is below Line(1); the
    // `clearing more than one line` guard keeps Line(0) intact here.
    assert_eq!(term.grid()[Line(0)][Column(9)].c, '1');
}

#[test]
fn ed_all() {
    let mut term = term();

    feed(&mut term, "1111111111\r\n2222222222\r\n3333333333");
    feed(&mut term, "\x1b[2;5H");
    feed(&mut term, "\x1b[2J"); // ED: all.

    for line in 0..3 {
        for column in 0..10 {
            assert_eq!(term.grid()[Line(line)][Column(column)].c, ' ');
        }
    }

    // The cursor is not moved by ED.
    assert_cursor(&term, 1, 4, false);
}

#[test]
fn el_right() {
    let mut term = term();

    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[3G"); // Cursor to column 3.
    feed(&mut term, "\x1b[K"); // EL: right.

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(9)].c, ' ');
}

#[test]
fn el_left() {
    let mut term = term();

    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[3G"); // Cursor to column 3.
    feed(&mut term, "\x1b[1K"); // EL: left, including the cursor cell.

    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'd');
}

#[test]
fn el_all() {
    let mut term = term();

    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[3G"); // Cursor to column 3.
    feed(&mut term, "\x1b[2K"); // EL: all.

    for column in 0..10 {
        assert_eq!(term.grid()[Line(0)][Column(column)].c, ' ');
    }
}

#[test]
fn sgr_basic_attributes() {
    let mut term = term();

    feed(&mut term, "\x1b[1;4ma"); // Bold + underline.

    let cell = &term.grid()[Line(0)][Column(0)];
    assert!(cell.flags.contains(Flags::BOLD));
    assert!(cell.flags.contains(Flags::UNDERLINE));

    // Cancel bold only.
    feed(&mut term, "\x1b[22mb");
    let cell = &term.grid()[Line(0)][Column(1)];
    assert!(!cell.flags.contains(Flags::BOLD));
    assert!(cell.flags.contains(Flags::UNDERLINE));
}

#[test]
fn sgr_16_colors() {
    let mut term = term();

    feed(&mut term, "\x1b[31;44ma"); // Red foreground, blue background.

    let cell = &term.grid()[Line(0)][Column(0)];
    assert_eq!(cell.fg, Color::Named(NamedColor::Red));
    assert_eq!(cell.bg, Color::Named(NamedColor::Blue));

    // Bright variants.
    feed(&mut term, "\x1b[91;104mb");
    let cell = &term.grid()[Line(0)][Column(1)];
    assert_eq!(cell.fg, Color::Named(NamedColor::BrightRed));
    assert_eq!(cell.bg, Color::Named(NamedColor::BrightBlue));
}

#[test]
fn sgr_indexed_and_truecolor() {
    let mut term = term();

    feed(&mut term, "\x1b[38;5;196ma"); // Indexed foreground.
    let cell = &term.grid()[Line(0)][Column(0)];
    assert_eq!(cell.fg, Color::Indexed(196));

    feed(&mut term, "\x1b[48;2;10;20;30mb"); // Truecolor background.
    let cell = &term.grid()[Line(0)][Column(1)];
    assert_eq!(
        cell.bg,
        Color::Spec(Rgb {
            r: 10,
            g: 20,
            b: 30
        })
    );
}

#[test]
fn sgr_reset() {
    let mut term = term();

    feed(&mut term, "\x1b[1;3;4;31ma"); // Everything on.
    feed(&mut term, "\x1b[0mb"); // Reset.

    let cell = &term.grid()[Line(0)][Column(1)];
    assert_eq!(cell.flags, Flags::empty());
    assert_eq!(cell.fg, Color::Named(NamedColor::Foreground));
    assert_eq!(cell.bg, Color::Named(NamedColor::Background));
}

#[test]
fn sgr_underline_styles() {
    let mut term = term();

    feed(&mut term, "\x1b[4ma");
    assert!(
        term.grid()[Line(0)][Column(0)]
            .flags
            .contains(Flags::UNDERLINE)
    );

    feed(&mut term, "\x1b[4:2mb"); // Double underline.
    assert!(
        term.grid()[Line(0)][Column(1)]
            .flags
            .contains(Flags::DOUBLE_UNDERLINE)
    );

    feed(&mut term, "\x1b[4:3mc"); // Undercurl.
    assert!(
        term.grid()[Line(0)][Column(2)]
            .flags
            .contains(Flags::UNDERCURL)
    );

    feed(&mut term, "\x1b[24md"); // Cancel underline.
    assert!(
        !term.grid()[Line(0)][Column(3)]
            .flags
            .intersects(Flags::ALL_UNDERLINES)
    );
}

#[test]
fn device_status_reports() {
    let (listener, receiver) = RecordingListener::new();
    let mut term = Term::new(10, 5, 0, listener);

    // Status report.
    feed(&mut term, "\x1b[5n");
    assert_eq!(
        drain(&receiver),
        vec![Event::PtyWrite("\x1b[0n".to_owned())]
    );

    // Cursor position report is 1-based and reflects the current cursor.
    feed(&mut term, "ab\x1b[6n");
    assert_eq!(
        drain(&receiver),
        vec![Event::PtyWrite("\x1b[1;3R".to_owned())]
    );

    // Unknown device status queries produce no reply.
    feed(&mut term, "\x1b[7n");
    assert_eq!(drain(&receiver), Vec::new());
}

#[test]
fn identify_terminal_reports_primary_attributes() {
    let (listener, receiver) = RecordingListener::new();
    let mut term = Term::new(10, 5, 0, listener);

    feed(&mut term, "\x1b[c");
    assert_eq!(
        drain(&receiver),
        vec![Event::PtyWrite("\x1b[?6c".to_owned())]
    );

    // Secondary device attributes are not reported yet.
    feed(&mut term, "\x1b[>c");
    assert_eq!(drain(&receiver), Vec::new());
}
