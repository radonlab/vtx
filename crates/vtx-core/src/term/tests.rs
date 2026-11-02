//! Tests for the terminal state machine.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, channel};

use vte::ansi::{Color, NamedColor, Processor, Rgb};

use super::TITLE_STACK_MAX_DEPTH;
use super::Term;
use crate::cell::Flags;
use crate::event::{Event, EventListener, VoidListener};
use crate::grid::{Dimensions, Scroll};
use crate::index::{Column, Line, Point};
use crate::mode::TermMode;

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
fn wide_char_narrow_overwrite() {
    let mut term = term();

    // Overwriting the leading half of a wide char drops the spacer.
    feed(&mut term, "字");
    feed(&mut term, "\x1b[1;1Ha");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert!(
        !term.grid()[Line(0)][Column(0)]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
    assert!(
        !term.grid()[Line(0)][Column(1)]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );

    // Overwriting the spacer half clears the wide char before it.
    let mut spacer_term = Term::new(10, 5, 0, VoidListener);
    feed(&mut spacer_term, "字");
    feed(&mut spacer_term, "\x1b[1;2Hb");
    assert_eq!(spacer_term.grid()[Line(0)][Column(0)].c, ' ');
    assert!(
        !spacer_term.grid()[Line(0)][Column(0)]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert_eq!(spacer_term.grid()[Line(0)][Column(1)].c, 'b');
    assert!(
        !spacer_term.grid()[Line(0)][Column(1)]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
}

#[test]
fn wide_char_overwrite_clears_leading_spacer() {
    let mut term = term();

    // Wrap a wide char to the next line, leaving a leading spacer at the
    // end of the first line.
    feed(&mut term, "123456789字");

    // Overwriting the wrapped wide char releases the spacer above.
    feed(&mut term, "\x1b[2;1Hx");

    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'x');
    assert!(
        !term.grid()[Line(0)][Column(9)]
            .flags
            .contains(Flags::LEADING_WIDE_CHAR_SPACER)
    );
}

#[test]
fn wide_char_ignored_when_wrap_disabled() {
    let mut term = term();

    feed(&mut term, "\x1b[?7l");
    feed(&mut term, "123456789字");

    // The wide char does not fit in the remaining column and is dropped
    // instead of wrapping; the last column keeps its content.
    assert_eq!(term.grid()[Line(0)][Column(8)].c, '9');
    assert_eq!(term.grid()[Line(0)][Column(9)].c, ' ');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 9, true);
}

#[test]
fn zero_width_after_wide_char() {
    let mut term = term();

    // A combining char following a wide char stacks on the wide cell, not
    // on the spacer.
    feed(&mut term, "字\u{301}");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, '字');
    assert_eq!(
        term.grid()[Line(0)][Column(0)]
            .extra
            .as_ref()
            .and_then(|extra| extra.zerowidth().first()),
        Some(&'\u{301}'),
    );
    assert_cursor(&term, 0, 2, false);
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

    // DECRST DECAWM: disable autowrap through the escape sequence.
    feed(&mut term, "\x1b[?7l");
    assert!(!term.mode().contains(TermMode::LINE_WRAP));

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
fn sgr_dotted_and_dashed_underline() {
    let mut term = term();

    feed(&mut term, "\x1b[4:4ma");
    assert!(
        term.grid()[Line(0)][Column(0)]
            .flags
            .contains(Flags::DOTTED_UNDERLINE)
    );

    feed(&mut term, "\x1b[4:5mb");
    assert!(
        term.grid()[Line(0)][Column(1)]
            .flags
            .contains(Flags::DASHED_UNDERLINE)
    );

    // Switching styles clears the previous one.
    feed(&mut term, "\x1b[4mc");
    let flags = term.grid()[Line(0)][Column(2)].flags;
    assert!(flags.contains(Flags::UNDERLINE));
    assert!(!flags.intersects(Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE));
}

#[test]
fn sgr_underline_color() {
    let mut term = term();

    // Indexed underline color.
    feed(&mut term, "\x1b[4;58;5;129ma");
    let cell = &term.grid()[Line(0)][Column(0)];
    assert!(cell.flags.contains(Flags::UNDERLINE));
    assert_eq!(cell.underline_color(), Some(Color::Indexed(129)));

    // Truecolor underline color.
    feed(&mut term, "\x1b[58;2;10;20;30mb");
    let cell = &term.grid()[Line(0)][Column(1)];
    assert_eq!(
        cell.underline_color(),
        Some(Color::Spec(Rgb {
            r: 10,
            g: 20,
            b: 30
        }))
    );

    // SGR 59 resets the underline color only; the style survives.
    feed(&mut term, "\x1b[59mc");
    let cell = &term.grid()[Line(0)][Column(2)];
    assert_eq!(cell.underline_color(), None);
    assert!(cell.flags.contains(Flags::UNDERLINE));
}

#[test]
fn sgr_colon_truecolor() {
    let mut term = term();

    // Colon-separated truecolor (with the optional colorspace idiom).
    feed(&mut term, "\x1b[38:2::10:20:30ma");
    assert_eq!(
        term.grid()[Line(0)][Column(0)].fg,
        Color::Spec(Rgb {
            r: 10,
            g: 20,
            b: 30
        })
    );

    // Colon-separated underline color.
    feed(&mut term, "\x1b[58:2::1:2:3mb");
    assert_eq!(
        term.grid()[Line(0)][Column(1)].underline_color(),
        Some(Color::Spec(Rgb { r: 1, g: 2, b: 3 }))
    );
}

#[test]
fn sgr_default_colors() {
    let mut term = term();

    feed(&mut term, "\x1b[31;44;4ma");

    // SGR 39/49 restore the default foreground/background, leaving other
    // attributes untouched.
    feed(&mut term, "\x1b[39;49mb");
    let cell = &term.grid()[Line(0)][Column(1)];
    assert_eq!(cell.fg, Color::Named(NamedColor::Foreground));
    assert_eq!(cell.bg, Color::Named(NamedColor::Background));
    assert!(cell.flags.contains(Flags::UNDERLINE));
}

#[test]
fn sgr_blink_is_ignored() {
    let mut term = term();

    // Blink modes are accepted without any effect.
    feed(&mut term, "\x1b[5;6;25ma");

    assert_eq!(term.grid()[Line(0)][Column(0)].flags, Flags::empty());
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

/// Create a 5x10 terminal with scrollback for display offset tests.
fn scrollable_term() -> Term<VoidListener> {
    Term::new(5, 10, 10_000, VoidListener)
}

/// First character of every visible line, top to bottom.
fn visible_row_chars<T: EventListener>(term: &Term<T>) -> String {
    term.grid()
        .display_iter()
        .filter(|indexed| indexed.point.column == Column(0))
        .map(|indexed| indexed.cell.c)
        .collect()
}

#[test]
fn scroll_display_page_up() {
    let mut term = scrollable_term();

    // Create 11 lines of scrollback: on a 10 line screen, newlines 10
    // through 20 each scroll a line into the history.
    feed(&mut term, &"\r\n".repeat(20));

    // The scrollable amount to the top is 11.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.grid().display_offset(), 10);

    // The scrollable amount to the top is 1.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.grid().display_offset(), 11);

    // The scrollable amount to the top is 0.
    term.scroll_display(Scroll::PageUp);
    assert_eq!(term.grid().display_offset(), 11);
}

#[test]
fn scroll_display_page_down() {
    let mut term = scrollable_term();

    // Create 11 lines of scrollback.
    feed(&mut term, &"\r\n".repeat(20));

    // Move the display offset to the topmost position.
    term.scroll_display(Scroll::Top);
    assert_eq!(term.grid().display_offset(), 11);

    // The scrollable amount to the bottom is 11.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.grid().display_offset(), 1);

    // The scrollable amount to the bottom is 1.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.grid().display_offset(), 0);

    // The scrollable amount to the bottom is 0.
    term.scroll_display(Scroll::PageDown);
    assert_eq!(term.grid().display_offset(), 0);
}

#[test]
fn clearing_scrollback_resets_display_offset() {
    // A 10x20 terminal; 29 newlines leave 10 lines of history.
    let mut term = Term::new(10, 20, 10_000, VoidListener);
    feed(&mut term, &"\r\n".repeat(29));

    term.scroll_display(Scroll::Top);
    assert_eq!(term.grid().display_offset(), 10);

    // ED 3 erases the scrollback.
    feed(&mut term, "\x1b[3J");
    assert_eq!(term.grid().display_offset(), 0);
}

#[test]
fn scroll_display_shows_history_lines() {
    // A 5x3 terminal; eight numbered lines leave five in the history.
    let mut term = Term::new(5, 3, 10_000, VoidListener);
    feed(&mut term, "0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7");

    // At the bottom, the viewport shows the newest three lines.
    assert_eq!(visible_row_chars(&term), "567");

    // At the top, the viewport shows the oldest three lines.
    term.scroll_display(Scroll::Top);
    assert_eq!(visible_row_chars(&term), "012");

    // Scrolling back to the bottom restores the newest lines.
    term.scroll_display(Scroll::Bottom);
    assert_eq!(visible_row_chars(&term), "567");
}

#[test]
fn new_output_keeps_scrolled_view_stationary() {
    let mut term = Term::new(5, 3, 10_000, VoidListener);
    feed(&mut term, "0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7");

    // Scroll up two lines; the viewport shows lines 3 through 5.
    term.scroll_display(Scroll::Delta(2));
    assert_eq!(visible_row_chars(&term), "345");

    // One more linefeed scrolls a line into the history; the offset
    // follows it, so the viewport stays on the same content.
    feed(&mut term, "\r\n");
    assert_eq!(term.grid().display_offset(), 3);
    assert_eq!(visible_row_chars(&term), "345");

    // At the bottom the offset stays glued to zero; the new linefeed
    // scrolls line 6 into the history instead.
    term.scroll_display(Scroll::Bottom);
    feed(&mut term, "\r\n");
    assert_eq!(term.grid().display_offset(), 0);
    assert_eq!(visible_row_chars(&term), "7  ");
}

#[test]
fn alt_screen_enter_and_exit() {
    let mut term = term();

    feed(&mut term, "main");
    feed(&mut term, "\x1b[?1049h");

    // Entering the alternate screen: flag set, contents cleared, and the
    // primary cursor carried over.
    assert!(term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 4, false);

    feed(&mut term, "alt");

    // Setting 1049 again while already active is a no-op.
    feed(&mut term, "\x1b[?1049h");
    assert!(term.mode().contains(TermMode::ALT_SCREEN));

    // Leaving the alternate screen restores the primary screen exactly as
    // it was left, cursor included.
    feed(&mut term, "\x1b[?1049l");
    assert!(!term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'm');
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'n');
    assert_cursor(&term, 0, 4, false);
}

#[test]
fn alt_screen_is_cleared_on_reentry() {
    let mut term = term();

    feed(&mut term, "\x1b[?1049h");
    feed(&mut term, "leftover");
    feed(&mut term, "\x1b[?1049l");

    // Re-entering resets the alternate screen contents again.
    feed(&mut term, "\x1b[?1049h");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 0, false);
}

#[test]
fn alt_screen_writes_do_not_touch_primary_scrollback() {
    let mut term = Term::new(10, 3, 100, VoidListener);

    // Push three lines into the primary screen's history.
    feed(&mut term, "0\r\n1\r\n2\r\n3\r\n4\r\n");
    assert_eq!(term.grid().total_lines(), 6);

    feed(&mut term, "\x1b[?1049h");

    // Scrolling on the alternate screen drops lines off the top; the alt
    // grid has no scrollback to grow into.
    feed(&mut term, "a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\n");

    feed(&mut term, "\x1b[?1049l");

    // The primary screen is untouched: same history, same viewport.
    assert_eq!(term.grid().total_lines(), 6);
    assert_eq!(term.grid()[Line(0)][Column(0)].c, '3');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, '4');
    assert_eq!(term.grid()[Line(-1)][Column(0)].c, '2');
}

#[test]
fn alt_screen_restores_primary_cursor() {
    let mut term = term();

    feed(&mut term, "ab");
    feed(&mut term, "\x1b[?1049h");

    // Move the alternate screen cursor away and write there.
    feed(&mut term, "\x1b[3;3HX");

    feed(&mut term, "\x1b[?1049l");

    // The primary cursor is exactly where it was left, and the alt screen
    // write is not visible on the primary screen.
    assert_cursor(&term, 0, 2, false);
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(2)][Column(2)].c, ' ');
}

#[test]
fn save_and_restore_cursor() {
    let mut term = term();

    // DECSC saves position and SGR attributes (bold).
    feed(&mut term, "\x1b[1mabc\x1b7");

    // Move away and reset attributes.
    feed(&mut term, "\x1b[0m\x1b[2;1H");

    // DECRC restores the saved cursor.
    feed(&mut term, "\x1b8");
    assert_cursor(&term, 0, 3, false);

    // The saved template is restored along with the position.
    feed(&mut term, "x");
    assert!(term.grid()[Line(0)][Column(3)].flags.contains(Flags::BOLD));
}

#[test]
fn full_reset() {
    let mut term = term();

    // Deviate from the defaults in every supported mode.
    feed(&mut term, "\x1b[?7l\x1b[?25l\x1b[?1h");
    feed(&mut term, "abc\r\n");

    // RIS: full reset.
    feed(&mut term, "\x1bc");

    let mode = term.mode();
    assert!(mode.contains(TermMode::LINE_WRAP));
    assert!(mode.contains(TermMode::SHOW_CURSOR));
    assert!(!mode.contains(TermMode::APP_CURSOR));
    assert_eq!(*mode, TermMode::default());

    // The screen is cleared and the cursor is home.
    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 0, false);
}

#[test]
fn full_reset_leaves_alternate_screen() {
    let mut term = term();

    feed(&mut term, "main");
    feed(&mut term, "\x1b[?1049h");
    feed(&mut term, "alt");

    feed(&mut term, "\x1bc");

    // The reset swaps back to the primary screen and clears both grids.
    assert!(!term.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_cursor(&term, 0, 0, false);
}

#[test]
fn private_mode_set_and_unset() {
    let mut term = term();

    // DECCEM (cursor visibility).
    feed(&mut term, "\x1b[?25l");
    assert!(!term.mode().contains(TermMode::SHOW_CURSOR));
    feed(&mut term, "\x1b[?25h");
    assert!(term.mode().contains(TermMode::SHOW_CURSOR));

    // DECCKM (application cursor keys).
    feed(&mut term, "\x1b[?1h");
    assert!(term.mode().contains(TermMode::APP_CURSOR));
    feed(&mut term, "\x1b[?1l");
    assert!(!term.mode().contains(TermMode::APP_CURSOR));

    // Alternate scroll.
    feed(&mut term, "\x1b[?1007l");
    assert!(!term.mode().contains(TermMode::ALTERNATE_SCROLL));
    feed(&mut term, "\x1b[?1007h");
    assert!(term.mode().contains(TermMode::ALTERNATE_SCROLL));

    // Unknown modes are ignored without side effects.
    feed(&mut term, "\x1b[?99h\x1b[?99l");
    assert_eq!(*term.mode(), TermMode::default());
}

#[test]
fn resize_while_in_alt_screen() {
    let mut term = term();

    feed(&mut term, "main");
    feed(&mut term, "\x1b[?1049h");

    term.resize(4, 2);

    // Both grids picked up the new dimensions.
    assert_eq!(term.grid().columns(), 4);
    assert_eq!(term.grid().screen_lines(), 2);

    feed(&mut term, "\x1b[?1049l");
    assert_eq!(term.grid().columns(), 4);
    assert_eq!(term.grid().screen_lines(), 2);
}

#[test]
fn scrolling_region_bounds() {
    let mut term = term();

    // Region covering lines 2-4 (0-based 1..4).
    feed(&mut term, "\x1b[2;4r");

    // DECSTBM homes the cursor.
    assert_cursor(&term, 0, 0, false);

    // Fill the region: LF at the region bottom scrolls only the region.
    feed(&mut term, "\x1b[2;1Ha\r\nb\r\nc\r\nd");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, ' ');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'b');
    assert_eq!(term.grid()[Line(2)][Column(0)].c, 'c');
    assert_eq!(term.grid()[Line(3)][Column(0)].c, 'd');
    // Line 5 is outside the region and was never touched.
    assert_eq!(term.grid()[Line(4)][Column(0)].c, ' ');
    assert_cursor(&term, 3, 1, false);
}

#[test]
fn scrolling_region_restore() {
    let mut term = term();

    feed(&mut term, "\x1b[2;4r");

    // `CSI r` with no parameters restores the full-screen region.
    feed(&mut term, "\x1b[r");
    feed(&mut term, "\x1b[5;1Hab\r\n");

    // LF at the screen bottom scrolled the whole viewport: "ab" is gone.
    assert_eq!(term.grid()[Line(4)][Column(0)].c, ' ');

    // RIS resets the region as well.
    feed(&mut term, "\x1b[2;4r");
    feed(&mut term, "\x1bc");
    feed(&mut term, "\x1b[5;1Hcd\r\n");
    assert_eq!(term.grid()[Line(4)][Column(0)].c, ' ');
}

#[test]
fn invalid_scrolling_region_is_ignored() {
    let mut term = term();

    feed(&mut term, "ab");
    feed(&mut term, "\x1b[4;2r");

    // Top >= bottom is rejected; the cursor is not homed.
    assert_cursor(&term, 0, 2, false);

    // The region still covers the full screen: LF at the bottom scrolls.
    feed(&mut term, "\x1b[5;1Hx\r\n");
    assert_eq!(term.grid()[Line(4)][Column(0)].c, ' ');
}

#[test]
fn origin_mode_addressing() {
    let mut term = term();

    feed(&mut term, "\x1b[2;4r");

    // DECOM homes the cursor to the region top.
    feed(&mut term, "\x1b[?6h");
    assert!(term.mode().contains(TermMode::ORIGIN));
    assert_cursor(&term, 1, 0, false);

    // CUP addresses are region-relative while origin mode is on.
    feed(&mut term, "\x1b[1;1H");
    assert_cursor(&term, 1, 0, false);
    feed(&mut term, "\x1b[99;1H");
    // ... and clamp to the region bottom instead of the screen bottom.
    assert_cursor(&term, 3, 0, false);

    // DECRST switches back to absolute addressing without homing the
    // cursor.
    feed(&mut term, "\x1b[?6l");
    assert!(!term.mode().contains(TermMode::ORIGIN));
    assert_cursor(&term, 3, 0, false);

    feed(&mut term, "\x1b[1;1H");
    assert_cursor(&term, 0, 0, false);
    feed(&mut term, "\x1b[3;5H");
    assert_cursor(&term, 2, 4, false);
}

#[test]
fn reverse_index_scrolls_region_at_top() {
    let mut term = term();

    feed(&mut term, "top\r\nbot");
    feed(&mut term, "\x1b[2;4r");
    feed(&mut term, "\x1b[2;1H");

    // RI at the region top scrolls the region down by one line.
    feed(&mut term, "\x1bM");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 't');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, ' ');
    assert_eq!(term.grid()[Line(2)][Column(0)].c, 'b');
    assert_cursor(&term, 1, 0, false);
}

#[test]
fn reverse_index_moves_cursor() {
    let mut term = term();

    feed(&mut term, "\x1b[3;1H");
    feed(&mut term, "\x1bM");

    assert_cursor(&term, 1, 0, false);
}

#[test]
fn insert_lines() {
    let mut term = term();

    feed(&mut term, "a\r\nb\r\nc\r\nd\r\ne");
    feed(&mut term, "\x1b[3;1H");

    // IL shifts lines from the cursor down; the bottom line drops off.
    feed(&mut term, "\x1b[1L");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'b');
    assert_eq!(term.grid()[Line(2)][Column(0)].c, ' ');
    assert_eq!(term.grid()[Line(3)][Column(0)].c, 'c');
    assert_eq!(term.grid()[Line(4)][Column(0)].c, 'd');
    assert_cursor(&term, 2, 0, false);
}

#[test]
fn insert_lines_outside_region_is_noop() {
    let mut term = term();

    feed(&mut term, "a\r\nb\r\nc\r\nd\r\ne");

    // Region lines 2-4; the cursor at line 0 is outside it.
    feed(&mut term, "\x1b[2;4r");
    feed(&mut term, "\x1b[1L");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'b');
    assert_eq!(term.grid()[Line(4)][Column(0)].c, 'e');
}

#[test]
fn delete_lines() {
    let mut term = term();

    feed(&mut term, "a\r\nb\r\nc\r\nd\r\ne");
    feed(&mut term, "\x1b[3;1H");

    // DL shifts lines below the cursor up; a blank fills the region bottom.
    feed(&mut term, "\x1b[1M");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(1)][Column(0)].c, 'b');
    assert_eq!(term.grid()[Line(2)][Column(0)].c, 'd');
    assert_eq!(term.grid()[Line(3)][Column(0)].c, 'e');
    assert_eq!(term.grid()[Line(4)][Column(0)].c, ' ');
    assert_cursor(&term, 2, 0, false);
}

#[test]
fn insert_blank_characters() {
    let mut term = term();

    // ICH at column 1 with count 2.
    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[1;2H\x1b[2@");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'b');
    assert_eq!(term.grid()[Line(0)][Column(4)].c, 'c');
    assert_eq!(term.grid()[Line(0)][Column(7)].c, 'f');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn delete_characters() {
    let mut term = term();

    // DCH at column 1 with count 2.
    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[1;2H\x1b[2P");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'd');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, 'e');
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'f');
    assert_eq!(term.grid()[Line(0)][Column(4)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(5)].c, ' ');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn erase_characters() {
    let mut term = term();

    // ECH at column 1 with count 2: cells reset, nothing shifts.
    feed(&mut term, "abcdef");
    feed(&mut term, "\x1b[1;2H\x1b[2X");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, ' ');
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'd');
    assert_eq!(term.grid()[Line(0)][Column(4)].c, 'e');
    assert_eq!(term.grid()[Line(0)][Column(5)].c, 'f');
    assert_cursor(&term, 0, 1, false);
}

#[test]
fn erase_characters_clamped_to_line_end() {
    let mut term = term();

    // ECH count beyond the line end only clears up to the last column.
    feed(&mut term, "ab");
    feed(&mut term, "\x1b[1;2H\x1b[99X");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, ' ');
}

#[test]
fn hts_sets_tabstop() {
    let mut term = term();

    // Start from a clean slate without any tabstops.
    feed(&mut term, "\x1b[3g");

    // HTS at column 4.
    feed(&mut term, "\x1b[1;5H\x1bH");

    feed(&mut term, "\x1b[1;1H\t");

    // The TAB lands exactly on the tabstop set by HTS.
    assert_cursor(&term, 0, 4, false);
}

#[test]
fn tbc_clears_current_tabstop() {
    let mut term = term();

    feed(&mut term, "\x1b[3g");
    feed(&mut term, "\x1b[1;4H\x1bH"); // Tabstop at column 3.
    feed(&mut term, "\x1b[1;5H\x1bH"); // Tabstop at column 4.

    // TBC with the cursor on column 4 clears only that stop.
    feed(&mut term, "\x1b[g");

    feed(&mut term, "\x1b[1;1H\t");
    assert_cursor(&term, 0, 3, false);
}

#[test]
fn tbc_clears_all_tabstops() {
    let mut term = term();

    feed(&mut term, "\x1b[3g");

    // Without any tabstops a TAB runs to the last column.
    feed(&mut term, "\t");
    assert_cursor(&term, 0, 9, false);
}

#[test]
fn cht_moves_to_next_tabstop() {
    let mut term = term();

    // Default tabstops every 8 columns; CHT from the home position.
    feed(&mut term, "\x1b[1I");
    assert_cursor(&term, 0, 8, false);

    // A custom tabstop takes precedence for the next hop.
    feed(&mut term, "\x1b[1;5H\x1bH");
    feed(&mut term, "\x1b[1;1H\x1b[1I");
    assert_cursor(&term, 0, 4, false);

    // Without any further tabstop CHT lands on the last column.
    feed(&mut term, "\x1b[3g");
    feed(&mut term, "\x1b[1I");
    assert_cursor(&term, 0, 9, false);
}

#[test]
fn cbt_moves_to_previous_tabstop() {
    let mut term = term();

    feed(&mut term, "\x1b[1;10H");
    feed(&mut term, "\x1b[Z");
    assert_cursor(&term, 0, 8, false);

    feed(&mut term, "\x1b[2Z");
    assert_cursor(&term, 0, 0, false);
}

#[test]
fn cbt_stays_without_earlier_tabstop() {
    let mut term = term();

    feed(&mut term, "\x1b[3g");
    feed(&mut term, "\x1b[1;6H");

    // No tabstop exists before column 5; the cursor stays put.
    feed(&mut term, "\x1b[Z");
    assert_cursor(&term, 0, 5, false);
}

#[test]
fn charset_designate_special_graphics() {
    let mut term = term();

    // SCS: designate DEC special graphics as G0.
    feed(&mut term, "\x1b(0lqk");

    assert_eq!(term.grid()[Line(0)][Column(0)].c, '┌');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, '─');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, '┐');

    // SCS: back to US ASCII.
    feed(&mut term, "\x1b(Bl");
    assert_eq!(term.grid()[Line(0)][Column(3)].c, 'l');
}

#[test]
fn charset_shift_out_and_in() {
    let mut term = term();

    // Designate DEC special graphics as G1, then shift out to invoke it.
    feed(&mut term, "\x1b)0");
    feed(&mut term, "\x0el");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, '┌');

    // Shift in returns to G0, which is still US ASCII.
    feed(&mut term, "\x0fl");
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'l');
}

#[test]
fn charsets_survive_save_restore_cursor() {
    let mut term = term();

    // DECSC snapshots the charsets along with the cursor.
    feed(&mut term, "\x1b(0");
    feed(&mut term, "\x1b7");

    // Switch G0 back to US ASCII.
    feed(&mut term, "\x1b(B");

    // DECRC restores the special graphics designation.
    feed(&mut term, "\x1b8l");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, '┌');
}

#[test]
fn mouse_report_modes_are_exclusive() {
    let mut term = term();

    assert!(!term.mouse_mode());

    // Click reporting.
    feed(&mut term, "\x1b[?1000h");
    assert!(term.mouse_mode());
    assert!(term.mode().contains(TermMode::MOUSE_REPORT_CLICK));

    // Drag reporting replaces click reporting.
    feed(&mut term, "\x1b[?1002h");
    assert!(term.mode().contains(TermMode::MOUSE_DRAG));
    assert!(!term.mode().contains(TermMode::MOUSE_REPORT_CLICK));

    // All-motion reporting replaces drag reporting.
    feed(&mut term, "\x1b[?1003h");
    assert!(term.mode().contains(TermMode::MOUSE_MOTION));
    assert!(!term.mode().contains(TermMode::MOUSE_DRAG));

    // Unsetting one protocol leaves the others off.
    feed(&mut term, "\x1b[?1003l");
    assert!(!term.mouse_mode());

    // The encodings are exclusive as well.
    feed(&mut term, "\x1b[?1005h");
    assert!(term.mode().contains(TermMode::UTF8_MOUSE));
    feed(&mut term, "\x1b[?1006h");
    assert!(term.mode().contains(TermMode::SGR_MOUSE));
    assert!(!term.mode().contains(TermMode::UTF8_MOUSE));
}

#[test]
fn focus_and_bracketed_paste_modes() {
    let mut term = term();

    feed(&mut term, "\x1b[?1004h\x1b[?2004h");
    assert!(term.mode().contains(TermMode::FOCUS_IN_OUT));
    assert!(term.mode().contains(TermMode::BRACKETED_PASTE));

    feed(&mut term, "\x1b[?1004l\x1b[?2004l");
    assert!(!term.mode().contains(TermMode::FOCUS_IN_OUT));
    assert!(!term.mode().contains(TermMode::BRACKETED_PASTE));

    // Both survive an RIS reset to defaults.
    feed(&mut term, "\x1b[?1004h");
    feed(&mut term, "\x1bc");
    assert!(!term.mode().contains(TermMode::FOCUS_IN_OUT));
}

#[test]
fn keypad_application_mode() {
    let mut term = term();

    feed(&mut term, "\x1b=");
    assert!(term.mode().contains(TermMode::APP_KEYPAD));

    feed(&mut term, "\x1b>");
    assert!(!term.mode().contains(TermMode::APP_KEYPAD));
}

#[test]
fn insert_mode_shifts_cells() {
    let mut term = term();

    feed(&mut term, "ac");
    feed(&mut term, "\x1b[1;2H");

    // With IRM active, printing shifts the tail to the right.
    feed(&mut term, "\x1b[4hb");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'a');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
    assert_eq!(term.grid()[Line(0)][Column(2)].c, 'c');

    // Without IRM, printing overwrites in place.
    feed(&mut term, "\x1b[4l");
    feed(&mut term, "\x1b[1;1Hx");
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'x');
    assert_eq!(term.grid()[Line(0)][Column(1)].c, 'b');
}

#[test]
fn line_feed_new_line_mode() {
    let mut term = term();

    feed(&mut term, "ab");
    feed(&mut term, "\x1b[20h\r");

    // With LNM active, CR behaves like CRLF.
    assert_cursor(&term, 1, 0, false);

    feed(&mut term, "\x1b[20l\r");
    assert_cursor(&term, 1, 0, false);
}

/// Event listener collecting all events for inspection.
#[derive(Default)]
struct MockListener {
    events: RefCell<Vec<Event>>,
}

impl EventListener for MockListener {
    fn send_event(&self, event: Event) {
        self.events.borrow_mut().push(event);
    }
}

/// Collecting variant of the standard test terminal.
fn mock_term() -> Term<MockListener> {
    Term::new(10, 5, 0, MockListener::default())
}

#[test]
fn osc_sets_title() {
    let mut term = mock_term();

    // OSC 0 terminated by BEL.
    feed(&mut term, "\x1b]0;my title\x07");
    assert_eq!(term.title(), Some("my title"));
    assert_eq!(
        term.event_proxy.events.borrow().last(),
        Some(&Event::Title("my title".to_owned()))
    );

    // OSC 2 terminated by ST.
    feed(&mut term, "\x1b]2;other\x1b\\");
    assert_eq!(term.title(), Some("other"));

    // An empty payload resets the title.
    feed(&mut term, "\x1b]0;\x07");
    assert_eq!(term.title(), None);
    assert_eq!(
        term.event_proxy.events.borrow().last(),
        Some(&Event::ResetTitle)
    );
}

#[test]
fn bell_sends_event() {
    let mut term = mock_term();

    feed(&mut term, "\x07");

    assert_eq!(term.event_proxy.events.borrow().last(), Some(&Event::Bell));
}

#[test]
fn title_stack_push_pop() {
    let mut term = mock_term();

    feed(&mut term, "\x1b]2;first\x07");
    feed(&mut term, "\x1b[22t");
    feed(&mut term, "\x1b]2;second\x07");

    // Popping restores the pushed title.
    feed(&mut term, "\x1b[23t");
    assert_eq!(term.title(), Some("first"));
    assert_eq!(
        term.event_proxy.events.borrow().last(),
        Some(&Event::Title("first".to_owned()))
    );

    // Popping an empty stack is a no-op.
    feed(&mut term, "\x1b[23t");
    assert_eq!(term.title(), Some("first"));
}

#[test]
fn title_stack_max_depth() {
    let mut term = mock_term();

    // Push the initial untitled state first, then a title.
    feed(&mut term, "\x1b[22t");
    feed(&mut term, "\x1b]2;base\x07");
    for _ in 0..TITLE_STACK_MAX_DEPTH {
        feed(&mut term, "\x1b[22t");
    }

    // The oldest (untitled) entry was dropped once the stack exceeded its
    // limit.
    assert_eq!(term.title_stack.len(), TITLE_STACK_MAX_DEPTH);
    assert_eq!(term.title_stack[0], Some("base".to_owned()));
    assert_eq!(term.title_stack.last(), Some(&Some("base".to_owned())));
}

#[test]
fn ris_resets_title() {
    let mut term = mock_term();

    feed(&mut term, "\x1b]2;title\x07");
    feed(&mut term, "\x1b[22t");

    feed(&mut term, "\x1bc");

    assert_eq!(term.title(), None);
    assert!(term.title_stack.is_empty());
}
