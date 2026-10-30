//! Terminal events and the listener interface.

use std::sync::mpsc::Sender;

/// Terminal event.
///
/// These events instruct the UI over changes that can't be handled by the
/// terminal emulation layer itself.
///
/// The M1 set covers the events required to drive the PTY data loop; bell,
/// title, clipboard, and color report events land together with their
/// terminal features in later milestones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Write some text to the PTY.
    ///
    /// Produced for terminal replies, like device attribute or status
    /// reports, which must be sent back to the child process.
    PtyWrite(String),

    /// New terminal content available.
    Wakeup,

    /// Shutdown request.
    Exit,
}

/// Event loop for notifying the UI about terminal events.
///
/// Implementors forward events to whoever drives the session, like a UI
/// thread waiting on an event channel.
pub trait EventListener {
    /// Handle an event.
    fn send_event(&self, _event: Event) {}
}

/// Null sink for events.
pub struct VoidListener;

impl EventListener for VoidListener {}

/// Forwards terminal events into a channel.
///
/// Terminal replies, like device status reports, are parsed on the session's
/// reader thread and drained back into the PTY from there.
#[derive(Debug, Clone)]
pub struct EventProxy {
    sender: Sender<Event>,
}

impl EventProxy {
    /// Create a proxy forwarding events to the given sender.
    pub fn new(sender: Sender<Event>) -> Self {
        Self { sender }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        // A closed channel means nobody is draining replies anymore; drop
        // them instead of failing the parser.
        let _ = self.sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn void_listener_accepts_all_events() {
        let listener = VoidListener;

        listener.send_event(Event::Wakeup);
        listener.send_event(Event::PtyWrite(String::new()));
        listener.send_event(Event::Exit);
    }

    #[test]
    fn pty_write_carries_payload() {
        let Event::PtyWrite(reply) = Event::PtyWrite("\x1b[0n".to_owned()) else {
            panic!("expected a PtyWrite event");
        };

        assert_eq!(reply, "\x1b[0n");
    }
}
