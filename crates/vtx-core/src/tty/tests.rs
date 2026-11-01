//! TTY backend smoke tests.
//!
//! These run against a real pseudoterminal. On Windows, `portable-pty`'s
//! ConPTY backend passes `INHERIT_CURSOR`, which makes conhost emit a
//! cursor position query (`ESC [6n`) and block the child spawn until it
//! receives a reply. The pump below is a minimal terminal driver which
//! parses the output through `Term` and writes PTY replies back; the
//! session's event loop is the production version of the same idea, which
//! is why both start their reader before spawning the child.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use vte::ansi::Processor;

use super::{LocalTty, Options, Shell, Tty, TtyReader, TtySize, TtyWriter};
use crate::event::{Event, EventListener};
use crate::term::Term;

/// Event listener forwarding terminal events into a channel.
struct ChannelListener(mpsc::Sender<Event>);

impl EventListener for ChannelListener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// Command echoing a fixed marker line and exiting immediately.
fn echo_options() -> Options {
    #[cfg(windows)]
    let shell = Shell::new(
        "cmd".to_owned(),
        vec!["/C".to_owned(), "echo vtx_tty_smoke".to_owned()],
    );

    #[cfg(not(windows))]
    let shell = Shell::new("echo".to_owned(), vec!["vtx_tty_smoke".to_owned()]);

    Options {
        shell: Some(shell),
        ..Options::default()
    }
}

/// Command staying alive for a moment, used when the child must still be
/// running at the moment under test.
fn sleeping_options() -> Options {
    #[cfg(windows)]
    let shell = Shell::new(
        "ping".to_owned(),
        vec!["-n".to_owned(), "2".to_owned(), "127.0.0.1".to_owned()],
    );

    #[cfg(not(windows))]
    let shell = Shell::new("sleep".to_owned(), vec!["1".to_owned()]);

    Options {
        shell: Some(shell),
        ..Options::default()
    }
}

/// Start the pump thread for the TTY streams.
///
/// The pump is a minimal terminal driver: it parses output through a
/// [`Term`], writes PTY replies (like the ConPTY startup cursor query) back
/// to the child, and reports the accumulated output once `marker` appeared.
/// It then stays parked on the reader; ConPTY never signals EOF on its own,
/// and the thread dies with the test process.
///
/// Returns the receiver the pump reports through; when the TTY closes, it
/// delivers whatever output arrived before the close.
fn start_pump(
    mut reader: TtyReader,
    mut writer: TtyWriter,
    marker: &'static [u8],
) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (event_tx, event_rx) = mpsc::channel();
        let listener = ChannelListener(event_tx);
        let mut term = Term::new(80, 24, 0, listener);
        let mut processor: Processor = Processor::new();

        let mut output = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(String::from_utf8_lossy(&output).into_owned());
                    return;
                }
                Ok(n) => {
                    output.extend_from_slice(&chunk[..n]);
                    processor.advance(&mut term, &chunk[..n]);

                    while let Ok(event) = event_rx.try_recv() {
                        if let Event::PtyWrite(text) = event {
                            let _ = writer.write_all(text.as_bytes());
                            let _ = writer.flush();
                        }
                    }

                    if output.windows(marker.len()).any(|w| w == marker) {
                        let _ = tx.send(String::from_utf8_lossy(&output).into_owned());
                        // Keep the writer alive so the child's stdin stays
                        // open; ConPTY never delivers EOF anyway.
                        loop {
                            thread::park();
                        }
                    }
                }
            }
        }
    });
    rx
}

/// Open a TTY, start the pump, and spawn the child.
///
/// The pump runs before the child connects: conhost blocks the spawn on its
/// startup cursor query until a reply arrives, and the pump is the reply
/// source. `marker` tells the pump when to report its output; tests that do
/// not inspect it pass a marker that never matches and get the output when
/// the TTY closes instead.
fn spawn_pumped(options: &Options, marker: &'static [u8]) -> (LocalTty, mpsc::Receiver<String>) {
    let size = TtySize {
        rows: 24,
        cols: 80,
        ..TtySize::default()
    };
    let mut tty = LocalTty::new(size).expect("opening the TTY failed");
    let reader = tty.reader().expect("taking the reader failed");
    let writer = tty.writer().expect("taking the writer failed");

    let output = start_pump(reader, writer, marker);

    tty.spawn_child(options).expect("spawning the child failed");
    (tty, output)
}

/// Spawn a child and pump its output until `marker` appears.
///
/// Returns the TTY together with the raw output accumulated until then.
fn pump_until_marker(options: &Options, marker: &'static [u8]) -> (LocalTty, String) {
    let (tty, output) = spawn_pumped(options, marker);
    let output = output
        .recv_timeout(Duration::from_secs(20))
        .expect("did not observe the marker within 20 seconds");
    (tty, output)
}

#[test]
fn spawn_read_marker_and_wait() {
    let (mut tty, output) = pump_until_marker(&echo_options(), b"vtx_tty_smoke");

    assert!(
        output.contains("vtx_tty_smoke"),
        "unexpected output: {output:?}"
    );

    let mut child = tty.take_child().expect("taking the child failed");
    let status = child.wait().expect("waiting for the child failed");
    assert!(status.success(), "the child exited with {status}");
}

#[test]
fn resize_while_child_runs() {
    // The marker never appears in the child's output; the pump's report is
    // not inspected, and the child is expected to outlive the resize.
    let (mut tty, _output) = spawn_pumped(&sleeping_options(), b"vtx_resize_marker");

    let size = TtySize {
        rows: 30,
        cols: 120,
        ..TtySize::default()
    };
    tty.resize(size).expect("resizing the TTY failed");

    let mut child = tty.take_child().expect("taking the child failed");
    let status = child.wait().expect("waiting for the child failed");
    assert!(status.success(), "the child exited with {status}");
}

#[test]
fn writer_can_be_taken_once() {
    let (mut tty, _output) = pump_until_marker(&echo_options(), b"vtx_tty_smoke");

    // The pump already owns the writer.
    assert!(
        tty.writer().is_err(),
        "a second writer must not be handed out"
    );

    let mut child = tty.take_child().expect("taking the child failed");
    let status = child.wait().expect("waiting for the child failed");
    assert!(status.success(), "the child exited with {status}");
}
