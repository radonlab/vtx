//! Dockable container hosting one [`TerminalView`] per session.

use egui::{Align, Color32, Id, Layout, Response, Sense, Ui, UiBuilder, WidgetText};
use egui_dock::{DockArea, DockState, NodePath, Style, TabViewer};
use vtx_core::event::VoidListener;
use vtx_core::term::Term;

use super::TerminalView;

/// One dockable terminal session shown as a tab in the dock area.
struct Pane {
    /// Unique number used for the tab id and title.
    id: usize,
    /// Placeholder terminal rendered by the view until the app milestone
    /// wires real sessions into the dock.
    term: Term<VoidListener>,
    /// Terminal screen rendered inside the tab.
    view: TerminalView,
}

impl Pane {
    /// Draws the terminal screen.
    fn show(&mut self, ui: &mut Ui) -> Response {
        self.view.show(ui, &self.term)
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
        let pane = self.create_pane();
        self.dock_state.push_to_focused_leaf(pane);
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
        if let Some(path) = viewer.pending_add {
            let pane = self.create_pane();
            if let Ok(leaf) = self.dock_state.leaf_mut(path) {
                leaf.append_tab(pane);
            }
        }

        ui.response()
    }

    /// Builds a pane with the next free number.
    fn create_pane(&mut self) -> Pane {
        let pane = Pane {
            id: self.next_number,
            term: Term::new(80, 24, 0, VoidListener),
            view: TerminalView::new(),
        };
        self.next_number += 1;
        pane
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
