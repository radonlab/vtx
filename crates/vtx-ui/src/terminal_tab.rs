//! Multi-tab container hosting one [`TerminalView`] per session.

use egui::{Align2, Color32, FontId, Response, Sense, Ui, Widget};

use super::TerminalView;

/// One open terminal session.
struct Tab {
    /// Label shown in the tab bar.
    title: String,
}

/// A tab bar plus a content area showing the selected [`TerminalView`].
///
/// Tabs are appended with the `+` button at the end of the tab bar and
/// closed with the `x` button on each tab.
pub struct TerminalTab {
    /// Open tabs in display order.
    tabs: Vec<Tab>,
    /// Index of the tab whose [`TerminalView`] is shown.
    selected: usize,
    /// Monotonic counter used to generate unique tab titles.
    next_number: usize,
}

impl TerminalTab {
    /// Creates the container with one initial tab.
    pub fn new() -> Self {
        let mut this = Self {
            tabs: Vec::new(),
            selected: 0,
            next_number: 1,
        };
        this.open_tab();
        this
    }

    /// Appends a new tab and selects it.
    pub fn open_tab(&mut self) {
        self.tabs.push(Tab {
            title: format!("Shell {}", self.next_number),
        });
        self.selected = self.tabs.len() - 1;
        self.next_number += 1;
    }

    /// Closes the tab at `index`, keeping the selection in range.
    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            self.selected = self.selected.min(self.tabs.len().saturating_sub(1));
        }
    }

    /// Draws the tab bar and the content area of the selected tab.
    pub fn show(&mut self, ui: &mut Ui) -> Response {
        self.tab_bar(ui);
        if self.tabs.is_empty() {
            empty_area(ui)
        } else {
            TerminalView::new().ui(ui)
        }
    }

    /// Draws the row of tab buttons.
    fn tab_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut to_close = None;
            for index in 0..self.tabs.len() {
                let title = self.tabs[index].title.clone();
                let selected = index == self.selected;
                if ui.selectable_label(selected, title).clicked() {
                    self.selected = index;
                }
                if ui.small_button("×").clicked() {
                    to_close = Some(index);
                }
            }
            if ui.button("+").on_hover_text("Open a new tab").clicked() {
                self.open_tab();
            }
            if let Some(index) = to_close {
                self.close_tab(index);
            }
        });
    }
}

impl Default for TerminalTab {
    fn default() -> Self {
        Self::new()
    }
}

/// Paints the empty-state background with a hint on how to open a tab.
fn empty_area(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "No open tabs — click + to add one",
        FontId::default(),
        Color32::from_gray(120),
    );
    response
}
