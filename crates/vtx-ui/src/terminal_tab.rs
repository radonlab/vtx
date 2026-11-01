//! Dockable container hosting one [`TerminalView`] per session.

use std::io;
use std::thread;

use egui::{Align, Color32, Id, Layout, Response, Sense, Ui, UiBuilder, WidgetText};
use egui_dock::{DockArea, DockState, NodePath, Style, TabViewer};
use vtx_core::session::TerminalSession;
use vtx_core::tty::{Options, TtySize};

use super::TerminalView;

/// Scrollback lines kept beyond the viewport.
const HISTORY_SIZE: usize = 10_000;

/// Columns and rows a session starts with.
///
/// Sessions spawn before the first layout pass, when the viewport size is
/// still unknown; the view adopts the real size on its first frame.
const INITIAL_SIZE: (u16, u16) = (80, 24);

/// One dockable terminal session shown as a tab in the dock area.
struct Pane {
    /// Unique number used for the tab id and title.
    id: usize,
    /// The terminal session behind this tab.
    session: TerminalSession,
    /// Terminal screen rendered inside the tab.
    view: TerminalView,
}

impl Pane {
    /// Spawns a session running the default shell.
    fn new(id: usize) -> io::Result<Self> {
        let (cols, rows) = INITIAL_SIZE;
        let size = TtySize {
            rows,
            cols,
            ..TtySize::default()
        };
        let session = TerminalSession::new(&Options::default(), size, HISTORY_SIZE)?;
        Ok(Self {
            id,
            session,
            view: TerminalView::new(),
        })
    }

    /// Draws the terminal screen.
    fn show(&mut self, ui: &mut Ui) -> Response {
        // Hand the event channel to a waker thread once: every wakeup, like
        // new output or the child exiting, schedules a repaint of this
        // context. The channel has a single receiver, so only the first
        // frame can take it.
        if let Some(events) = self.session.take_events() {
            let ctx = ui.ctx().clone();
            let waker = thread::Builder::new()
                .name("vtx-event-waker".to_owned())
                .spawn(move || {
                    while events.recv().is_ok() {
                        ctx.request_repaint();
                    }
                });
            if let Err(err) = waker {
                log::warn!("spawning the event waker failed: {err}");
            }
        }

        let response = self.view.show(ui, &mut self.session);

        // Reap the child watcher once the child is gone; joining is
        // idempotent, and the watcher finishes right after the exit.
        if self.session.is_exited() {
            self.session.join();
        }

        response
    }
}

/// Adapts terminal [`Pane`]s to the egui_dock tab interface.
struct TerminalTabViewer {
    /// Leaf whose `+` button was pressed, to be handled once the dock area
    /// is done borrowing the [`DockState`].
    pending_add: Option<NodePath>,
}

impl TabViewer for TerminalTabViewer {
    type Tab = Pane;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(tab.id)
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        format!("Shell {}", tab.id).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        tab.show(ui);
    }

    fn on_add(&mut self, path: NodePath) {
        self.pending_add = Some(path);
    }

    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        // The terminal scrolls its own buffer, so the dock area
        // must not attach its own scroll bars.
        [false, false]
    }
}

/// A dock area with one [`TerminalView`] per tab.
///
/// Tabs can be reordered, dragged between panes, split, and undocked into
/// separate windows. New tabs are appended with the `+` button at the end of
/// each tab bar and closed with the `x` button on each tab.
pub struct TerminalTab {
    /// Dock layout carrying the open panes.
    dock_state: DockState<Pane>,
    /// Monotonic counter used to generate unique pane ids.
    next_number: usize,
}

impl TerminalTab {
    /// Creates the container with one initial tab.
    pub fn new() -> Self {
        let mut this = Self {
            dock_state: DockState::new(Vec::new()),
            next_number: 1,
        };
        this.open_tab();
        this
    }

    /// Appends a new tab to the focused pane.
    pub fn open_tab(&mut self) {
        if let Some(pane) = self.try_create_pane() {
            self.dock_state.push_to_focused_leaf(pane);
        }
    }

    /// Draws the dock area, or the empty-state hint when all tabs are closed.
    pub fn show(&mut self, ui: &mut Ui) -> Response {
        if self.dock_state.iter_all_tabs().next().is_none() {
            return self.empty_area(ui);
        }

        let mut viewer = TerminalTabViewer { pending_add: None };
        DockArea::new(&mut self.dock_state)
            .style(Style::from_egui(ui.style()))
            .show_add_buttons(true)
            .show_inside(ui, &mut viewer);

        // The viewer cannot touch `dock_state` while the dock area borrows
        // it, so additions requested via the `+` button are applied here.
        if let Some(path) = viewer.pending_add
            && let Some(pane) = self.try_create_pane()
            && let Ok(leaf) = self.dock_state.leaf_mut(path)
        {
            leaf.append_tab(pane);
        }

        ui.response()
    }

    /// Builds a pane with the next free number, if its session spawns.
    fn try_create_pane(&mut self) -> Option<Pane> {
        let pane = match Pane::new(self.next_number) {
            Ok(pane) => pane,
            Err(err) => {
                log::error!("spawning the session failed: {err}");
                return None;
            }
        };
        self.next_number += 1;
        Some(pane)
    }

    /// Paints the empty-state background with a way to open a tab.
    fn empty_area(&mut self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);

        let mut center = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Center)),
        );
        center.add_space((rect.height() / 2.0 - 30.0).max(0.0));
        center.weak("No open tabs");
        if center.button("Open a new tab").clicked() {
            self.open_tab();
        }
        response
    }
}

impl Default for TerminalTab {
    fn default() -> Self {
        Self::new()
    }
}
