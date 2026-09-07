//! The terminal screen widget.
//!
//! Renders the cell grid of a [`Session`] into an egui [`Ui`]:
//!
//! 1. one [`LayoutJob`] per screen row, with one section per run of cells
//!    sharing the same attributes;
//! 2. a per-row galley cache keyed by the row's [`SequenceNo`], so only
//!    changed rows are re-shaped;
//! 3. cell-aligned background rectangles, merged per run, painted below
//!    the text;
//! 4. a cursor block overlaid on top.
//!
//! Wide graphemes are laid out with font metrics rather than being forced
//! onto exact cell boundaries; precise grid alignment for double-width
//! glyphs is a known follow-up.

use std::sync::Arc;

use crate::theme::Theme;
use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextWrapping};
use egui::{
    Color32, Context, FontId, Galley, Painter, Pos2, Rect, Response, Sense, Stroke, TextFormat, Ui,
    Vec2, Widget,
};
use vtx_core::cell::{CellAttrs, Color, Intensity, Underline};
use vtx_core::line::{Line, SequenceNo};
use vtx_core::session::Session;

/// Cached galley for one screen row.
#[derive(Default)]
struct RowCache {
    /// Sequence number of the line the galley was built from.
    seqno: SequenceNo,
    /// The shaped row, `None` until first built.
    galley: Option<Arc<Galley>>,
}

/// Renders the terminal screen of a single session.
pub struct TerminalView {
    /// The session whose screen is rendered.
    session: Session,
    /// Monospace font size in points.
    font_size: f32,
    /// Color scheme.
    theme: Theme,
    /// Measured monospace cell size in points.
    cell_size: Option<Vec2>,
    /// Font size the cell size and row cache are valid for.
    cached_font_size: f32,
    /// Per-row galley cache, indexed by screen row.
    row_cache: Vec<RowCache>,
}

impl TerminalView {
    /// Creates a view with a blank session of the default geometry.
    pub fn new() -> Self {
        Self::with_session(Session::new(
            vtx_core::session::DEFAULT_COLS,
            vtx_core::session::DEFAULT_ROWS,
        ))
    }

    /// Creates a view rendering `session`.
    pub fn with_session(session: Session) -> Self {
        Self {
            session,
            font_size: 14.0,
            theme: Theme::default(),
            cell_size: None,
            cached_font_size: 0.0,
            row_cache: Vec::new(),
        }
    }

    /// Mutable access to the session.
    ///
    /// Used for feeding mock output and, later, keyboard input.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Sets the font size, invalidating the cell metrics and row caches.
    pub fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size;
    }

    /// Draws the view, keeping its state across frames.
    ///
    /// Unlike the [`Widget`] implementation, this method takes `&mut self`,
    /// which allows long-lived views (e.g. one per terminal session) to
    /// carry state between frames.
    pub fn show(&mut self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();
        let font_id = FontId::monospace(self.font_size);
        let cell = self.cell_size(&ctx, &font_id);

        // Screen background.
        painter.rect_filled(rect, 0.0, self.theme.background);

        // Row backgrounds, then text.
        self.row_cache
            .resize_with(self.session.rows(), RowCache::default);
        for row in 0..self.session.rows() {
            let row_top = rect.min.y + row as f32 * cell.y;
            if row_top > rect.bottom() {
                break; // fully below the visible area
            }
            {
                let line = &self.session.lines()[row];
                self.paint_row_backgrounds(&painter, line, rect.min, row_top, cell);
            }

            // Cache check and rebuild are kept in separate borrows: the
            // session lines are only read, while the row cache is written.
            let seqno = self.session.lines()[row].seqno();
            let hit = {
                let cache = &self.row_cache[row];
                if cache.seqno == seqno {
                    cache.galley.clone()
                } else {
                    None
                }
            };
            let galley = match hit {
                Some(galley) => galley,
                None => {
                    let job = self.build_job(&self.session.lines()[row], &font_id);
                    let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
                    self.row_cache[row] = RowCache {
                        seqno,
                        galley: Some(Arc::clone(&galley)),
                    };
                    galley
                }
            };
            painter.galley(
                Pos2::new(rect.min.x, row_top),
                galley,
                self.theme.foreground,
            );
        }

        // Cursor block overlay.
        let (cursor_x, cursor_y) = self.session.cursor();
        let cursor_rect = Rect::from_min_size(
            Pos2::new(
                rect.min.x + cursor_x as f32 * cell.x,
                rect.min.y + cursor_y as f32 * cell.y,
            ),
            cell,
        );
        painter.rect_filled(cursor_rect, 0.0, self.theme.cursor);

        response
    }

    /// Returns the monospace cell size, measuring it on first use or after
    /// a font change (which also drops all cached galleys).
    fn cell_size(&mut self, ctx: &Context, font_id: &FontId) -> Vec2 {
        if let Some(size) = self.cell_size
            && self.cached_font_size == self.font_size
        {
            return size;
        }
        let size = ctx.fonts_mut(|fonts| {
            Vec2::new(fonts.glyph_width(font_id, 'M'), fonts.row_height(font_id))
        });
        self.cell_size = Some(size);
        self.cached_font_size = self.font_size;
        self.row_cache.clear();
        size
    }

    /// Builds a layout job for one row: one section per run of cells that
    /// share the same attributes.
    fn build_job(&self, line: &Line, font_id: &FontId) -> LayoutJob {
        // Wrapping is owned by the session model, never by the galley.
        let mut job = LayoutJob {
            wrap: TextWrapping {
                max_width: f32::INFINITY,
                ..Default::default()
            },
            ..Default::default()
        };

        // Trailing cells that cannot produce a glyph (plain whitespace
        // without decorations) are trimmed: backgrounds are painted from
        // the full row anyway, and this keeps the shaped text minimal.
        // Whitespace that carries an underline or strikethrough is kept,
        // since those decorations draw on blank cells too.
        let cells = line.cells();
        let len = cells
            .iter()
            .rposition(|cell| {
                !cell.text().is_whitespace()
                    || cell.attrs().underline() != Underline::None
                    || cell.attrs().strikethrough()
            })
            .map_or(0, |idx| idx + 1);

        let mut text = String::new();
        for run in cells[..len].chunk_by(|a, b| a.attrs() == b.attrs()) {
            let start = text.len();
            for cell in run {
                cell.text().push_to(&mut text);
            }
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: ByteIndex(start)..ByteIndex(text.len()),
                format: self.text_format(run[0].attrs(), font_id),
            });
        }
        job.text = text;
        job
    }

    /// Translates cell attributes into an egui text format.
    fn text_format(&self, attrs: &CellAttrs, font_id: &FontId) -> TextFormat {
        let fg = self.foreground_of(attrs);
        let mut format = TextFormat::simple(font_id.clone(), fg);
        if attrs.intensity() == Intensity::Bold {
            // egui 0.36 dropped `FontId::weight`; bold is set through the
            // variable-font `wght` axis instead.
            format.coords.push(b"wght", 700.0);
        }
        if attrs.intensity() == Intensity::Half {
            // Approximate half-brightness by darkening the text.
            format.color = fg.linear_multiply(0.55);
        }
        format.italics = attrs.italic();
        if attrs.underline() != Underline::None {
            format.underline = Stroke {
                width: 1.0,
                color: fg,
            };
        }
        if attrs.strikethrough() {
            format.strikethrough = Stroke {
                width: 1.0,
                color: fg,
            };
        }
        if attrs.invisible() {
            // Keep the cell occupied and its background painted, but hide
            // the glyphs by painting them in the background color.
            format.color = self.background_of(attrs);
        }
        format
    }

    /// Resolves a cell color to a concrete color, falling back to `default`.
    fn resolve_color(&self, color: Color, default: Color32) -> Color32 {
        match color {
            Color::Default => default,
            Color::Indexed(index) => self.theme.indexed(index),
            Color::Rgb { r, g, b } => Color32::from_rgb(r, g, b),
        }
    }

    /// The effective foreground color, honoring reverse video.
    fn foreground_of(&self, attrs: &CellAttrs) -> Color32 {
        let fg = self.resolve_color(attrs.foreground(), self.theme.foreground);
        let bg = self.resolve_color(attrs.background(), self.theme.background);
        if attrs.reverse() { bg } else { fg }
    }

    /// The effective background color, honoring reverse video.
    fn background_of(&self, attrs: &CellAttrs) -> Color32 {
        let fg = self.resolve_color(attrs.foreground(), self.theme.foreground);
        let bg = self.resolve_color(attrs.background(), self.theme.background);
        if attrs.reverse() { fg } else { bg }
    }

    /// Paints the background rectangles of one row.
    ///
    /// Runs of cells resolving to the same background color are merged
    /// into a single rectangle. Runs matching the screen background are
    /// skipped entirely, since the screen is already filled with it.
    fn paint_row_backgrounds(
        &self,
        painter: &Painter,
        line: &Line,
        origin: Pos2,
        row_top: f32,
        cell: Vec2,
    ) {
        let runs = line
            .cells()
            .chunk_by(|a, b| self.background_of(a.attrs()) == self.background_of(b.attrs()));
        let mut col = 0;
        for run in runs {
            let bg = self.background_of(run[0].attrs());
            let run_len = run.len();
            if bg != self.theme.background {
                let rect = Rect::from_min_max(
                    Pos2::new(origin.x + col as f32 * cell.x, row_top),
                    Pos2::new(origin.x + (col + run_len) as f32 * cell.x, row_top + cell.y),
                );
                painter.rect_filled(rect, 0.0, bg);
            }
            col += run_len;
        }
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TerminalView {
    fn ui(mut self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::ByteRangeExt as _;

    /// Builds a pen from attribute mutations.
    fn pen(f: impl FnOnce(&mut CellAttrs)) -> CellAttrs {
        let mut attrs = CellAttrs::blank();
        f(&mut attrs);
        attrs
    }

    #[test]
    fn job_has_one_section_per_attribute_run() {
        let mut session = Session::new(10, 2);
        session.write_str("ab");
        session.set_pen(pen(|a| {
            a.set_foreground(Color::Indexed(1));
        }));
        session.write_str("cd");
        let view = TerminalView::with_session(session);
        let font_id = FontId::monospace(14.0);

        let job = view.build_job(&view.session.lines()[0], &font_id);
        assert_eq!(job.text, "abcd");
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].byte_range.as_usize(), 0..2);
        assert_eq!(job.sections[1].byte_range.as_usize(), 2..4);
        assert_eq!(job.sections[0].format.color, view.theme.foreground);
        assert_eq!(
            job.sections[1].format.color,
            Color32::from_rgb(0xcd, 0x00, 0x00)
        );
    }

    #[test]
    fn job_never_wraps() {
        let mut session = Session::new(4, 3);
        session.write_str("abcdefgh");
        let view = TerminalView::with_session(session);
        let font_id = FontId::monospace(14.0);

        let job = view.build_job(&view.session.lines()[0], &font_id);
        assert_eq!(job.text, "abcd");
        assert_eq!(job.wrap.max_width, f32::INFINITY);
    }

    #[test]
    fn bold_sets_weight_axis() {
        let mut session = Session::new(10, 2);
        session.set_pen(pen(|a| {
            a.set_intensity(Intensity::Bold);
        }));
        session.write_str("x");
        let view = TerminalView::with_session(session);
        let font_id = FontId::monospace(14.0);

        let job = view.build_job(&view.session.lines()[0], &font_id);
        assert_eq!(
            job.sections[0].format.coords,
            egui::epaint::text::VariationCoords::new([(b"wght", 700.0)])
        );
    }

    #[test]
    fn decorations_map_to_strokes() {
        let mut session = Session::new(10, 2);
        session.set_pen(pen(|a| {
            a.set_underline(Underline::Curly);
            a.set_strikethrough(true);
        }));
        session.write_str("x");
        let view = TerminalView::with_session(session);
        let font_id = FontId::monospace(14.0);

        let format = view.build_job(&view.session.lines()[0], &font_id).sections[0]
            .format
            .clone();
        assert_ne!(format.underline, Stroke::NONE);
        assert_ne!(format.strikethrough, Stroke::NONE);
    }

    #[test]
    fn invisible_text_adopts_background_color() {
        let mut session = Session::new(10, 2);
        session.set_pen(pen(|a| {
            a.set_invisible(true);
        }));
        session.write_str("x");
        let view = TerminalView::with_session(session);
        let font_id = FontId::monospace(14.0);

        let format = view.build_job(&view.session.lines()[0], &font_id).sections[0]
            .format
            .clone();
        assert_eq!(format.color, view.theme.background);
    }

    #[test]
    fn reverse_video_swaps_colors() {
        let view = TerminalView::new();
        let attrs = pen(|a| {
            a.set_foreground(Color::Indexed(1));
            a.set_reverse(true);
        });
        assert_eq!(
            view.foreground_of(&attrs),
            view.theme.background // default bg, swapped into fg
        );
        assert_eq!(
            view.background_of(&attrs),
            Color32::from_rgb(0xcd, 0x00, 0x00) // indexed(1), swapped into bg
        );
    }
}
