//! Terminal session facade.
//!
//! [`TerminalSession`] is the entry point the UI consumes: it spawns the
//! child inside a TTY, starts the event loop's reader and child watcher
//! threads, and exposes input, resize, and terminal state access.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use parking_lot::Mutex;

use crate::event::{Event, EventProxy};
use crate::event_loop::{EventLoop, ExitCodeSlot, SharedWriter};
use crate::sync::FairMutex;
use crate::term::Term;
use crate::tty::{LocalTty, Options, Tty, TtyKiller, TtySize};

/// A running terminal: child process, TTY, and terminal state.
pub struct TerminalSession {
    /// Terminal state, shared with the reader thread.
    terminal: Arc<FairMutex<Term<EventProxy>>>,

    /// TTY backend, kept for resizing.
    tty: Box<dyn Tty>,

    /// PTY writer shared with the reader thread.
    writer: SharedWriter,

    /// Handle for terminating the child when the session goes away.
    killer: TtyKiller,

    /// Channel receiving wakeup and exit events.
    events: Receiver<Event>,

    /// Exit code slot, filled by the child watcher.
    exit_code: ExitCodeSlot,

    /// Child watcher thread handle; `None` once joined.
    child_watcher: Option<JoinHandle<()>>,
}

impl TerminalSession {
    /// Spawn a session running `options` inside a TTY of `size`.
    ///
    /// `history_size` limits the number of scrollback lines kept beyond the
    /// viewport.
    pub fn new(options: &Options, size: TtySize, history_size: usize) -> io::Result<Self> {
        let mut tty: Box<dyn Tty> = Box::new(LocalTty::new(options, size)?);

        let reader = tty.reader()?;
        let writer = tty.writer()?;
        let child = tty.take_child()?;
        let killer = child.clone_killer();

        // Events for the UI, and replies from the terminal back into the PTY.
        let (event_sender, events) = mpsc::channel();
        let (reply_sender, reply_receiver) = mpsc::channel();

        let proxy = EventProxy::new(reply_sender);
        let term = Term::new(size.cols as usize, size.rows as usize, history_size, proxy);
        let terminal = Arc::new(FairMutex::new(term));

        let writer: SharedWriter = Arc::new(Mutex::new(writer));
        let exit_code = ExitCodeSlot::default();

        let child_watcher = EventLoop::new(
            reader,
            Arc::clone(&writer),
            child,
            Arc::clone(&terminal),
            reply_receiver,
            event_sender,
        )
        .spawn(Arc::clone(&exit_code))?;

        Ok(Self {
            terminal,
            tty,
            writer,
            killer,
            events,
            exit_code,
            child_watcher: Some(child_watcher),
        })
    }

    /// The terminal state, shared with the session's reader thread.
    pub fn terminal(&self) -> &Arc<FairMutex<Term<EventProxy>>> {
        &self.terminal
    }

    /// The channel of terminal events: wakeup on new content, exit once the
    /// child terminated.
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// Write input, like keypresses, to the child process.
    pub fn write(&self, input: &[u8]) -> io::Result<()> {
        if input.is_empty() {
            return Ok(());
        }

        let mut writer = self.writer.lock();
        writer.write_all(input)?;
        writer.flush()
    }

    /// Resize the TTY and the terminal to `size`.
    pub fn resize(&mut self, size: TtySize) -> io::Result<()> {
        self.tty.resize(size)?;
        self.terminal
            .lock()
            .resize(size.cols as usize, size.rows as usize);
        Ok(())
    }

    /// The child's exit code, or `None` while it is still running.
    pub fn exit_code(&self) -> Option<u32> {
        *self.exit_code.lock()
    }

    /// Whether the child process exited.
    pub fn is_exited(&self) -> bool {
        self.exit_code().is_some()
    }

    /// Wait for the child watcher thread to finish.
    ///
    /// After the exit event arrived, this only joins the thread; it is
    /// idempotent.
    pub fn join(&mut self) {
        if let Some(handle) = self.child_watcher.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Make sure the child does not outlive the session. Killing an
        // already-exited child is a no-op.
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::TerminalSession;
    use crate::event::{Event, EventProxy};
    use crate::grid::Dimensions;
    use crate::index::{Column, Line};
    use crate::sync::FairMutex;
    use crate::term::Term;
    use crate::tty::{Options, Shell, TtySize};

    use std::sync::mpsc::RecvTimeoutError;

    /// Options for a command printing `marker` and exiting.
    fn echo_options(marker: &str) -> Options {
        #[cfg(windows)]
        let shell = Shell::new(
            "cmd".to_owned(),
            vec!["/C".to_owned(), format!("echo {marker}")],
        );

        #[cfg(not(windows))]
        let shell = Shell::new("echo".to_owned(), vec![marker.to_owned()]);

        Options {
            shell: Some(shell),
            ..Options::default()
        }
    }

    /// Options for a command outliving the test.
    fn sleeping_options() -> Options {
        #[cfg(windows)]
        let shell = Shell::new(
            "ping".to_owned(),
            vec!["-n".to_owned(), "10".to_owned(), "127.0.0.1".to_owned()],
        );

        #[cfg(not(windows))]
        let shell = Shell::new("sleep".to_owned(), vec!["10".to_owned()]);

        Options {
            shell: Some(shell),
            ..Options::default()
        }
    }

    /// Default test size.
    fn test_size() -> TtySize {
        TtySize {
            rows: 24,
            cols: 80,
            ..TtySize::default()
        }
    }

    /// The viewport's text, one line per row.
    fn visible_text(terminal: &FairMutex<Term<EventProxy>>) -> String {
        let term = terminal.lock();
        let mut text = String::new();
        for line in 0..term.screen_lines() {
            let row = &term.grid()[Line(line as i32)];
            for column in 0..term.columns() {
                text.push(row[Column(column)].c);
            }
            text.push('\n');
        }
        text
    }

    /// Wait for the exit event, ignoring wakeups on the way.
    fn wait_for_exit(session: &TerminalSession) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "the child did not exit within 20 seconds"
            );

            match session.events().recv_timeout(remaining) {
                Ok(Event::Exit) => return,
                Ok(_) => (),
                Err(RecvTimeoutError::Timeout) => {
                    panic!("the child did not exit within 20 seconds")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the event channel closed before the child exited")
                }
            }
        }
    }

    #[test]
    fn session_spawns_echoes_and_exits() {
        let mut session = TerminalSession::new(&echo_options("vtx_session_smoke"), test_size(), 0)
            .expect("creating the session failed");

        // The reader thread answers the ConPTY startup queries on its own,
        // so no explicit handshake is needed here.
        wait_for_exit(&session);

        session.join();
        assert_eq!(session.exit_code(), Some(0));
        assert!(session.is_exited());

        let text = visible_text(session.terminal());
        assert!(
            text.contains("vtx_session_smoke"),
            "unexpected output: {text:?}"
        );
    }

    #[test]
    fn session_resize_updates_term() {
        let mut session = TerminalSession::new(&sleeping_options(), test_size(), 0)
            .expect("creating the session failed");

        let size = TtySize {
            rows: 10,
            cols: 40,
            ..TtySize::default()
        };
        session.resize(size).expect("resizing the session failed");

        let term = session.terminal().lock();
        assert_eq!((term.columns(), term.screen_lines()), (40, 10));
    }

    #[cfg(not(windows))]
    #[test]
    fn session_write_reaches_child() {
        // `cat` echoes its input back through the TTY.
        let options = Options {
            shell: Some(Shell::new("cat".to_owned(), Vec::new())),
            ..Options::default()
        };
        let session =
            TerminalSession::new(&options, test_size(), 0).expect("creating the session failed");

        session
            .write(b"vtx_session_write\n")
            .expect("writing to the session failed");

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if visible_text(session.terminal()).contains("vtx_session_write") {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "the input was not echoed within 20 seconds"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
