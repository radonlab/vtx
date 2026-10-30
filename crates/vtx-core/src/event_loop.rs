//! Event loop for the PTY data path.
//!
//! Unlike upstream, which multiplexes the PTY through a polling loop, the
//! `portable-pty` endpoints use blocking IO. [`EventLoop`] therefore runs
//! two dedicated threads: a reader parsing child output into the terminal,
//! and a child watcher reporting the process exit.

use std::io::{self, ErrorKind, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::{Builder, JoinHandle};

use parking_lot::Mutex;
use vte::ansi::Processor;

use crate::event::{Event, EventProxy};
use crate::sync::FairMutex;
use crate::term::Term;
use crate::tty::{TtyChild, TtyReader, TtyWriter};

/// Size of the reader thread's read buffer.
///
/// Upstream reads one megabyte per poll to batch as much parsing as possible
/// into a single terminal lock. The blocking reader here parks between reads
/// anyway, so the buffer only needs to cover what the kernel hands over in a
/// single read.
const READ_BUFFER_SIZE: usize = 0x8000;

/// PTY writer shared between the session and the reader thread.
///
/// The session writes user input while the reader thread writes terminal
/// replies, like cursor position reports; the mutex keeps the byte streams
/// from interleaving mid-sequence.
pub type SharedWriter = Arc<Mutex<TtyWriter>>;

/// Slot where the child watcher reports the exit code.
pub type ExitCodeSlot = Arc<Mutex<Option<u32>>>;

/// Drives the PTY data path of one session.
pub struct EventLoop {
    /// Blocking reader for the child's output.
    reader: TtyReader,

    /// PTY writer shared with the session.
    writer: SharedWriter,

    /// Child process, reaped by the watcher thread.
    child: TtyChild,

    /// Terminal state, shared with the UI.
    terminal: Arc<FairMutex<Term<EventProxy>>>,

    /// Terminal replies produced while parsing, written back to the PTY.
    reply_receiver: Receiver<Event>,

    /// Session-facing event sender, for wakeup and exit events.
    event_sender: Sender<Event>,
}

impl EventLoop {
    /// Create an event loop from the session's parts.
    pub fn new(
        reader: TtyReader,
        writer: SharedWriter,
        child: TtyChild,
        terminal: Arc<FairMutex<Term<EventProxy>>>,
        reply_receiver: Receiver<Event>,
        event_sender: Sender<Event>,
    ) -> Self {
        Self {
            reader,
            writer,
            child,
            terminal,
            reply_receiver,
            event_sender,
        }
    }

    /// Start the reader and child watcher threads.
    ///
    /// Returns the watcher's join handle, which is signaled once the exit
    /// code is recorded. The reader's handle is dropped on purpose,
    /// detaching the thread: ConPTY never delivers EOF, so on Windows the
    /// reader can only end with the process itself.
    pub fn spawn(self, exit_code: ExitCodeSlot) -> io::Result<JoinHandle<()>> {
        let Self {
            reader,
            writer,
            child,
            terminal,
            reply_receiver,
            event_sender,
        } = self;

        // Child watcher: reaps the process, records the exit code, and
        // signals the session.
        let watcher_sender = event_sender.clone();
        let watcher = Builder::new()
            .name("vtx-child-watcher".to_owned())
            .spawn(move || run_child_watcher(child, exit_code, watcher_sender))?;

        // Reader: parses the child's output into the terminal.
        Builder::new()
            .name("vtx-pty-reader".to_owned())
            .spawn(move || run_reader(reader, writer, terminal, reply_receiver, event_sender))?;

        Ok(watcher)
    }
}

/// Read child output into the terminal until the TTY closes.
///
/// Runs on the reader thread.
fn run_reader(
    mut reader: TtyReader,
    writer: SharedWriter,
    terminal: Arc<FairMutex<Term<EventProxy>>>,
    reply_receiver: Receiver<Event>,
    event_sender: Sender<Event>,
) {
    let mut parser: Processor = Processor::new();
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    loop {
        let read = match reader.read(&mut buffer) {
            // EOF: the child closed its end of the TTY. ConPTY never gets
            // here; the thread stays parked on the reader instead.
            Ok(0) => break,
            Ok(read) => read,
            Err(err) => match err.kind() {
                ErrorKind::Interrupted | ErrorKind::WouldBlock => continue,
                // The TTY went away; the child watcher reports the exit.
                _ => {
                    log::debug!("reading from the TTY failed: {err}");
                    break;
                }
            },
        };

        // Parse the new bytes into the terminal, handing the lock back
        // before doing any IO, so the UI can redraw in between.
        {
            let mut term = terminal.lock();
            parser.advance(&mut *term, &buffer[..read]);
        }

        drain_replies(&reply_receiver, &writer);

        // Queue a redraw unless every byte went into a synchronized update
        // holdback, which defers the redraw until the update ends.
        if parser.sync_bytes_count() < read {
            let _ = event_sender.send(Event::Wakeup);
        }
    }
}

/// Write pending terminal replies back into the PTY.
///
/// Runs on the reader thread right after parsing, so replies to startup
/// queries, like the ConPTY cursor position report, are answered without a
/// roundtrip through the UI.
fn drain_replies(reply_receiver: &Receiver<Event>, writer: &SharedWriter) {
    while let Ok(event) = reply_receiver.try_recv() {
        if let Event::PtyWrite(reply) = event {
            let mut writer = writer.lock();
            // A closed TTY swallows late replies; the child watcher handles
            // the exit.
            let _ = writer.write_all(reply.as_bytes());
            let _ = writer.flush();
        }
    }
}

/// Reap the child process and signal the session.
///
/// Runs on the child watcher thread.
fn run_child_watcher(mut child: TtyChild, exit_code: ExitCodeSlot, event_sender: Sender<Event>) {
    match child.wait() {
        Ok(status) => *exit_code.lock() = Some(status.exit_code()),
        // Without a status there is nothing to record; the session still
        // learns about the exit through the event.
        Err(err) => log::warn!("waiting for the child failed: {err}"),
    }

    let _ = event_sender.send(Event::Exit);
}
