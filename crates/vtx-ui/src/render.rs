//! Batched terminal painting.
//!
//! The screen is drawn in layers: the default background fills the whole
//! rect, consecutive cells with the same background merge into one
//! rectangle, single-width text is grouped into per-line runs of equal
//! color, wide glyphs are centered over their two-cell span, and the cursor
//! is painted last, as a block, beam, or underline per DECSCUSR. Selected
//! cells invert their colors; link underlines and underline styles land
//! together with their terminal features.

/// Cursor line thickness as a fraction of the cell, like upstream's
/// default.
const CURSOR_THICKNESS: f32 = 0.15;

/// Thickness of the hovered hyperlink's underline, in logical pixels.
const LINK_UNDERLINE_THICKNESS: f32 = 2.0;

use std::mem;

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Rect, Vec2, pos2, vec2};
use vtx_core::cell::{Cell, Flags, Hyperlink};
use vtx_core::color::DIM_FACTOR;
use vtx_core::event::EventListener;
use vtx_core::grid::Dimensions;
use vtx_core::mode::TermMode;
use vtx_core::selection::SelectionRange;
use vtx_core::term::{CursorShape, Term};

use crate::font::TerminalFont;
use crate::theme::TerminalTheme;

/// A horizontal run of cells sharing one background color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BgRun {
    /// Column of the first cell in the run.
    pub column: usize,
    /// Number of consecutive cells covered by the run.
    pub span: usize,
    /// Background fill color.
    pub color: Color32,
}

/// A stretch of consecutive single-width glyphs sharing one color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRun {
    /// Column of the first glyph.
    pub column: usize,
    /// Text content of the run.
    pub text: String,
    /// Text color.
    pub color: Color32,
}

/// A wide glyph covering two cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WideGlyph {
    /// Column of the wide cell.
    pub column: usize,
    /// The wide character.
    pub c: char,
    /// Text color.
    pub color: Color32,
}

/// Drawing commands collected for one grid line.
///
/// Cells are pushed in ascending column order; runs of equal color merge
/// automatically, so one command usually covers a whole stretch of cells.
#[derive(Clone, Debug, Default)]
pub struct LineCommands {
    /// Merged background runs.
    pub backgrounds: Vec<BgRun>,
    /// Text runs.
    pub texts: Vec<TextRun>,
    /// Wide glyphs.
    pub wides: Vec<WideGlyph>,
}

impl LineCommands {
    /// Add the cell at `column` to the line.
    ///
    /// Selected cells invert their colors, following upstream's default
    /// when no dedicated selection color is configured.
    pub fn push(&mut self, column: usize, cell: &Cell, theme: &TerminalTheme, selected: bool) {
        let (mut fg, mut bg) = cell_colors(cell, theme);

        if selected {
            mem::swap(&mut fg, &mut bg);
        }

        // Merge into the previous run when it ends right before this cell.
        if bg != theme.background() {
            match self.backgrounds.last_mut() {
                Some(run) if run.color == bg && run.column + run.span == column => {
                    run.span += 1;
                }
                _ => self.backgrounds.push(BgRun {
                    column,
                    span: 1,
                    color: bg,
                }),
            }
        }

        // Spacer cells only reserve the space of a wide glyph.
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            return;
        }

        // Wide glyphs are drawn individually, centered over both cells.
        if cell.flags.contains(Flags::WIDE_CHAR) {
            self.wides.push(WideGlyph {
                column,
                c: cell.c,
                color: fg,
            });
            return;
        }

        // Spaces and hidden cells draw no glyph; the background covers them.
        if cell.c == ' ' || cell.flags.contains(Flags::HIDDEN) {
            return;
        }

        // Extend the current run when color and position line up.
        match self.texts.last_mut() {
            Some(run) if run.color == fg && run.column + run.text.chars().count() == column => {
                run.text.push(cell.c);
            }
            _ => self.texts.push(TextRun {
                column,
                text: cell.c.to_string(),
                color: fg,
            }),
        }
    }
}

/// Resolve the effective foreground and background colors of a cell.
///
/// Inverse video swaps both colors and dimmed text darkens the foreground.
fn cell_colors(cell: &Cell, theme: &TerminalTheme) -> (Color32, Color32) {
    let mut fg = theme.color(cell.fg);
    let mut bg = theme.color(cell.bg);

    if cell.flags.contains(Flags::INVERSE) {
        mem::swap(&mut fg, &mut bg);
    }

    if cell.flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
        fg = fg.linear_multiply(DIM_FACTOR);
    }

    (fg, bg)
}

/// Paint the visible area of a terminal into `rect`.
///
/// The whole rect is filled with the default background first, so cells
/// without an explicit background need no rectangle of their own. Cells of
/// the `hovered_link` get an underline, marking the link as clickable.
#[expect(clippy::too_many_arguments)]
pub fn render_terminal<T: EventListener>(
    painter: &Painter,
    rect: Rect,
    term: &Term<T>,
    font: &TerminalFont,
    theme: &TerminalTheme,
    selection: Option<SelectionRange>,
    cursor_visible: bool,
    hovered_link: Option<&Hyperlink>,
) {
    painter.rect_filled(rect, CornerRadius::ZERO, theme.background());

    let cell_size = font.cell_size(painter.ctx());
    if cell_size.x <= 0.0 || cell_size.y <= 0.0 {
        // Without glyph metrics there is nothing left to place.
        return;
    }

    let grid = term.grid();
    let display_offset = grid.display_offset();
    let screen_lines = grid.screen_lines();

    // Collect the drawing commands of every visible line.
    let mut lines = vec![LineCommands::default(); screen_lines];
    let mut link_cells = Vec::new();
    for indexed in grid.display_iter() {
        let row = (indexed.point.line.0 + display_offset as i32) as usize;
        if let Some(commands) = lines.get_mut(row) {
            let selected = selection.is_some_and(|s| s.contains_cell(indexed.point, indexed.cell));
            commands.push(indexed.point.column.0, indexed.cell, theme, selected);
        }

        // The hovered link is underlined across all of its cells.
        if let Some(hovered) = hovered_link
            && indexed
                .cell
                .hyperlink()
                .is_some_and(|link| link.id() == hovered.id())
        {
            link_cells.push((row, indexed.point.column.0, theme.color(indexed.cell.fg)));
        }
    }

    let font_id = font.font_id().clone();
    for (row, commands) in lines.iter().enumerate() {
        let line_top = rect.top() + row as f32 * cell_size.y;

        for run in &commands.backgrounds {
            let run_rect = Rect::from_min_size(
                pos2(rect.left() + run.column as f32 * cell_size.x, line_top),
                vec2(cell_size.x * run.span as f32, cell_size.y),
            );
            painter.rect_filled(run_rect, CornerRadius::ZERO, run.color);
        }

        for glyph in &commands.wides {
            painter.text(
                pos2(
                    rect.left() + (glyph.column as f32 + 1.0) * cell_size.x,
                    line_top + cell_size.y * 0.5,
                ),
                Align2::CENTER_CENTER,
                glyph.c,
                font_id.clone(),
                glyph.color,
            );
        }

        for run in &commands.texts {
            painter.text(
                pos2(rect.left() + run.column as f32 * cell_size.x, line_top),
                Align2::LEFT_TOP,
                run.text.as_str(),
                font_id.clone(),
                run.color,
            );
        }
    }

    // Underline the hovered hyperlink's cells.
    let thickness = LINK_UNDERLINE_THICKNESS;
    for (row, column, color) in link_cells {
        let cell_rect = Rect::from_min_size(
            pos2(
                rect.left() + column as f32 * cell_size.x,
                rect.top() + row as f32 * cell_size.y,
            ),
            cell_size,
        );
        let underline = Rect::from_min_size(
            pos2(cell_rect.left(), cell_rect.bottom() - thickness),
            vec2(cell_rect.width(), thickness),
        );
        painter.rect_filled(underline, CornerRadius::ZERO, color);
    }

    render_cursor(
        painter,
        rect,
        term,
        cell_size,
        theme,
        &font_id,
        cursor_visible,
    );
}

/// Paint the cursor overlay.
///
/// The cursor takes the shape the child requested via DECSCUSR, or a block
/// by default. A block covers its cell and the glyph below is redrawn in
/// the screen background color, which keeps it readable; the beam and
/// underline shapes only overlay part of the cell and leave the glyph
/// untouched. `visible` is false during a blink's dark phase.
fn render_cursor<T: EventListener>(
    painter: &Painter,
    rect: Rect,
    term: &Term<T>,
    cell_size: Vec2,
    theme: &TerminalTheme,
    font_id: &FontId,
    visible: bool,
) {
    if !visible || !term.mode().contains(TermMode::SHOW_CURSOR) {
        return;
    }

    let shape = term
        .cursor_style()
        .map_or(CursorShape::Block, |style| style.shape);

    let grid = term.grid();
    let point = term.cursor_point();
    let row = point.line.0 + grid.display_offset() as i32;
    if !(0..grid.screen_lines() as i32).contains(&row) {
        // The cursor is scrolled out of the visible area.
        return;
    }

    let cell = &grid[point];
    let wide = cell.flags.contains(Flags::WIDE_CHAR);
    let span = if wide { 2.0 } else { 1.0 };

    let cursor_rect = Rect::from_min_size(
        pos2(
            rect.left() + point.column.0 as f32 * cell_size.x,
            rect.top() + row as f32 * cell_size.y,
        ),
        vec2(cell_size.x * span, cell_size.y),
    );

    match shape {
        CursorShape::Block => {
            painter.rect_filled(cursor_rect, CornerRadius::ZERO, theme.cursor_color());

            // Redraw the glyph below the cursor in a contrasting color.
            let redraw = !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                && cell.c != ' '
                && !cell.flags.contains(Flags::HIDDEN);
            if redraw {
                let (pos, anchor) = if wide {
                    (cursor_rect.center(), Align2::CENTER_CENTER)
                } else {
                    (cursor_rect.left_top(), Align2::LEFT_TOP)
                };
                painter.text(pos, anchor, cell.c, font_id.clone(), theme.background());
            }
        }
        CursorShape::Beam => {
            let width = (cell_size.x * CURSOR_THICKNESS).max(1.0);
            let bar = Rect::from_min_size(cursor_rect.left_top(), vec2(width, cell_size.y));
            painter.rect_filled(bar, CornerRadius::ZERO, theme.cursor_color());
        }
        CursorShape::Underline => {
            let height = (cell_size.y * CURSOR_THICKNESS).max(1.0);
            let strip = Rect::from_min_size(
                pos2(cursor_rect.left(), cursor_rect.bottom() - height),
                vec2(cursor_rect.width(), height),
            );
            painter.rect_filled(strip, CornerRadius::ZERO, theme.cursor_color());
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, RawInput, Rect, Shape, Vec2};
    use vte::ansi::Processor;
    use vtx_core::color::{Color, NamedColor};
    use vtx_core::event::VoidListener;
    use vtx_core::index::{Column, Line, Point};
    use vtx_core::term::Term;

    use super::*;
    use crate::font::TerminalFont;
    use crate::theme::TerminalTheme;

    /// A cell with explicit colors and no flags.
    fn cell(c: char, fg: Color, bg: Color) -> Cell {
        Cell {
            c,
            fg,
            bg,
            flags: Flags::empty(),
            extra: None,
        }
    }

    /// The default foreground and background colors.
    fn defaults() -> (Color, Color) {
        (
            Color::Named(NamedColor::Foreground),
            Color::Named(NamedColor::Background),
        )
    }

    #[test]
    fn backgrounds_merge_into_runs() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let blue = Color::Named(NamedColor::Blue);
        let red = Color::Named(NamedColor::Red);
        let mut commands = LineCommands::default();

        commands.push(0, &cell('a', fg, blue), &theme, false);
        commands.push(1, &cell('b', fg, blue), &theme, false);
        commands.push(2, &cell('c', fg, red), &theme, false);
        commands.push(3, &cell('d', fg, bg), &theme, false);
        commands.push(4, &cell('e', fg, blue), &theme, false);

        assert_eq!(
            commands.backgrounds,
            vec![
                BgRun {
                    column: 0,
                    span: 2,
                    color: theme.color(blue),
                },
                BgRun {
                    column: 2,
                    span: 1,
                    color: theme.color(red),
                },
                BgRun {
                    column: 4,
                    span: 1,
                    color: theme.color(blue),
                },
            ]
        );
    }

    #[test]
    fn default_background_needs_no_run() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut commands = LineCommands::default();

        commands.push(0, &cell('a', fg, bg), &theme, false);
        commands.push(1, &cell('b', fg, bg), &theme, false);

        assert!(commands.backgrounds.is_empty());
    }

    #[test]
    fn texts_group_by_color_and_position() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let red = Color::Named(NamedColor::Red);
        let mut commands = LineCommands::default();

        commands.push(0, &cell('h', fg, bg), &theme, false);
        commands.push(1, &cell('i', fg, bg), &theme, false);
        commands.push(2, &cell(' ', fg, bg), &theme, false);
        commands.push(3, &cell('!', fg, bg), &theme, false);
        commands.push(4, &cell('?', red, bg), &theme, false);

        assert_eq!(
            commands.texts,
            vec![
                TextRun {
                    column: 0,
                    text: "hi".to_owned(),
                    color: theme.color(fg),
                },
                TextRun {
                    column: 3,
                    text: "!".to_owned(),
                    color: theme.color(fg),
                },
                TextRun {
                    column: 4,
                    text: "?".to_owned(),
                    color: theme.color(red),
                },
            ]
        );
    }

    #[test]
    fn column_gap_starts_new_run() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut commands = LineCommands::default();

        commands.push(0, &cell('a', fg, bg), &theme, false);
        // Column 1 is skipped, e.g. because a wide glyph was drawn there.
        commands.push(2, &cell('b', fg, bg), &theme, false);

        assert_eq!(commands.texts.len(), 2);
    }

    #[test]
    fn wide_glyphs_are_drawn_individually() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut wide = cell('漢', fg, bg);
        wide.flags.insert(Flags::WIDE_CHAR);
        let mut spacer = cell(' ', fg, bg);
        spacer.flags.insert(Flags::WIDE_CHAR_SPACER);

        let mut commands = LineCommands::default();
        commands.push(0, &wide, &theme, false);
        commands.push(1, &spacer, &theme, false);
        commands.push(2, &cell('a', fg, bg), &theme, false);

        assert_eq!(
            commands.wides,
            vec![WideGlyph {
                column: 0,
                c: '漢',
                color: theme.color(fg),
            }]
        );
        assert_eq!(
            commands.texts,
            vec![TextRun {
                column: 2,
                text: "a".to_owned(),
                color: theme.color(fg),
            }]
        );
    }

    #[test]
    fn inverse_video_swaps_colors() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut inverse = cell('x', fg, bg);
        inverse.flags.insert(Flags::INVERSE);

        let mut commands = LineCommands::default();
        commands.push(0, &inverse, &theme, false);

        assert_eq!(
            commands.backgrounds,
            vec![BgRun {
                column: 0,
                span: 1,
                color: theme.color(fg),
            }]
        );
        assert_eq!(
            commands.texts,
            vec![TextRun {
                column: 0,
                text: "x".to_owned(),
                color: theme.color(bg),
            }]
        );
    }

    #[test]
    fn dimmed_text_darkens_the_foreground() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut dim = cell('x', fg, bg);
        dim.flags.insert(Flags::DIM);

        let mut commands = LineCommands::default();
        commands.push(0, &dim, &theme, false);

        assert_eq!(
            commands.texts[0].color,
            theme.color(fg).linear_multiply(DIM_FACTOR)
        );
        assert_ne!(commands.texts[0].color, theme.color(fg));
    }

    #[test]
    fn hidden_cells_draw_no_glyph() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut hidden = cell('x', fg, bg);
        hidden.flags.insert(Flags::HIDDEN);

        let mut commands = LineCommands::default();
        commands.push(0, &hidden, &theme, false);

        assert!(commands.texts.is_empty());
    }

    #[test]
    fn selection_inverts_cell_colors() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();

        let mut unselected = LineCommands::default();
        unselected.push(0, &cell('a', fg, bg), &theme, false);

        let mut selected = LineCommands::default();
        selected.push(0, &cell('a', fg, bg), &theme, true);

        // The selected cell swaps foreground and background: the cell gains
        // a background run in the foreground color and its text is drawn in
        // the background color.
        assert_eq!(unselected.backgrounds, Vec::new());
        assert_eq!(
            selected.backgrounds,
            vec![BgRun {
                column: 0,
                span: 1,
                color: theme.color(fg)
            }]
        );
        assert_eq!(unselected.texts[0].color, theme.color(fg));
        assert_eq!(selected.texts[0].color, theme.background());
    }

    #[test]
    fn render_terminal_paints_background_text_and_cursor() {
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput::default());

        let mut term = Term::new(10, 3, 0, VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"\x1b[31mred\x1b[0m plain");

        let font = TerminalFont::default();
        let theme = TerminalTheme::default();
        render_terminal(
            &ctx.debug_painter(),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 60.0)),
            &term,
            &font,
            &theme,
            None,
            true,
            None,
        );

        let output = ctx.end_pass();

        let mut backgrounds = 0;
        let mut cursors = 0;
        let mut texts = 0;
        for clipped in &output.shapes {
            match &clipped.shape {
                Shape::Rect(shape) => {
                    if shape.fill == theme.background() {
                        backgrounds += 1;
                    } else if shape.fill == theme.cursor_color() {
                        cursors += 1;
                    }
                }
                Shape::Text(_) => texts += 1,
                _ => {}
            }
        }
        output.drop_without_applying_deltas();

        assert!(backgrounds >= 1, "the default background was not painted");
        assert_eq!(cursors, 1, "the block cursor was not painted exactly once");
        assert!(texts >= 1, "no text was painted");
    }

    #[test]
    fn beam_and_underline_cursors_paint_thin_overlays() {
        for sequence in ["\x1b[5 q", "\x1b[3 q"] {
            let ctx = egui::Context::default();
            ctx.begin_pass(RawInput::default());

            let mut term = Term::new(10, 3, 0, VoidListener);
            let mut processor: Processor = Processor::new();
            processor.advance(&mut term, b"text");
            processor.advance(&mut term, sequence.as_bytes());

            let font = TerminalFont::default();
            let theme = TerminalTheme::default();
            render_terminal(
                &ctx.debug_painter(),
                Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 60.0)),
                &term,
                &font,
                &theme,
                None,
                true,
                None,
            );

            let output = ctx.end_pass();

            // The thin cursor shapes leave the glyph untouched, so only the
            // thin overlay itself is painted in the cursor color.
            let thin: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    Shape::Rect(rect) => Some((rect.fill, rect.rect)),
                    _ => None,
                })
                .filter(|(fill, _)| *fill == theme.cursor_color())
                .collect();
            output.drop_without_applying_deltas();

            assert_eq!(thin.len(), 1, "one cursor overlay was expected");
            let (_, rect) = thin[0];
            let cell = font.cell_size(&ctx);
            let near = |a: f32, b: f32| (a - b).abs() < 0.01;
            if sequence == "\x1b[5 q" {
                // Beam: a vertical bar over the full cell height.
                assert!(near(rect.width(), (cell.x * CURSOR_THICKNESS).max(1.0)));
                assert!(near(rect.height(), cell.y));
            } else {
                // Underline: a strip along the cell bottom.
                assert!(near(rect.width(), cell.x));
                assert!(near(rect.height(), (cell.y * CURSOR_THICKNESS).max(1.0)));
            }
        }
    }

    #[test]
    fn invisible_cursor_paints_nothing() {
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput::default());

        let term = Term::new(10, 3, 0, VoidListener);
        let font = TerminalFont::default();
        let theme = TerminalTheme::default();
        render_terminal(
            &ctx.debug_painter(),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 60.0)),
            &term,
            &font,
            &theme,
            None,
            false,
            None,
        );

        let output = ctx.end_pass();

        let cursors = output
            .shapes
            .iter()
            .filter(|clipped| matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == theme.cursor_color()))
            .count();
        output.drop_without_applying_deltas();
        assert_eq!(cursors, 0, "the blink's dark phase paints no cursor");
    }

    #[test]
    fn hovered_hyperlink_is_underlined() {
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput::default());

        let mut term = Term::new(20, 3, 0, VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"\x1b]8;;https://example.com\x1b\\link");

        let hovered = term.grid()[Point::new(Line(0), Column(0))]
            .hyperlink()
            .expect("written cells carry the link");

        let font = TerminalFont::default();
        let theme = TerminalTheme::default();
        render_terminal(
            &ctx.debug_painter(),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 60.0)),
            &term,
            &font,
            &theme,
            None,
            false,
            Some(&hovered),
        );

        let output = ctx.end_pass();

        // One thin underline per cell of the hovered link, in the cells'
        // foreground color.
        let underlines = output
            .shapes
            .iter()
            .filter(|clipped| {
                matches!(&clipped.shape, Shape::Rect(rect) if rect.fill == theme.foreground()
                    && (rect.rect.height() - LINK_UNDERLINE_THICKNESS).abs() < 0.01)
            })
            .count();
        output.drop_without_applying_deltas();

        assert_eq!(underlines, 4, "every link cell gets an underline");
    }
}
