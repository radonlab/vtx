//! vtx-ui: a library of reusable UI widgets.
//!
//! Widgets implement the [`egui::Widget`] trait so they can be dropped into
//! any egui application.

use egui::{Color32, Painter, Rect, Response, Sense, Stroke, Ui, Vec2, Widget};

/// A gridded canvas widget that demonstrates custom drawing via [`Painter`].
///
/// It serves as the entry point for more advanced custom rendering.
pub struct GridCanvas {
    /// Canvas size in logical points.
    pub size: Vec2,
    /// Background color.
    pub background: Color32,
    /// Grid spacing in logical points.
    pub grid_step: f32,
    /// Grid line color.
    pub grid_color: Color32,
}

impl GridCanvas {
    /// Creates a canvas with the default color scheme.
    pub fn new(size: Vec2) -> Self {
        Self {
            size,
            background: Color32::from_rgb(24, 24, 28),
            grid_step: 32.0,
            grid_color: Color32::from_rgb(52, 52, 60),
        }
    }

    fn draw_grid(&self, painter: &Painter, rect: Rect) {
        let mut x = rect.left() + self.grid_step;
        while x < rect.right() {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                Stroke::new(1.0, self.grid_color),
            );
            x += self.grid_step;
        }
        let mut y = rect.top() + self.grid_step;
        while y < rect.bottom() {
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                Stroke::new(1.0, self.grid_color),
            );
            y += self.grid_step;
        }
    }
}

impl Widget for GridCanvas {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);

        painter.rect_filled(rect, 4.0, self.background);
        self.draw_grid(&painter, rect);
        painter.circle_filled(
            response.interact_pointer_pos().unwrap_or(rect.center()),
            6.0,
            Color32::LIGHT_BLUE,
        );

        response
    }
}
