//! Batched terminal painting.
//!
//! The screen is drawn in layers: the default background fills the whole
//! rect, consecutive cells with the same background merge into one
//! rectangle, every single-width glyph is placed at its own cell, wide
//! glyphs are centered over their two-cell span, and the cursor is painted
//! last, as a block, beam, or underline per DECSCUSR. Selected cells invert
//! their colors; link underlines and underline styles land together with
//! their terminal features.
//!
//! Glyphs are drawn one per cell instead of as shaped strings: fonts give
//! fallback glyphs (CJK, emoji) their own advances, which would push the
//! rest of a string off the grid, and re-anchoring the remainder at a run
//! split — a selection or a color change — would visibly move the text.

/// Cursor line thickness as a fraction of the cell, like upstream's
/// default.
const CURSOR_THICKNESS: f32 = 0.15;

/// Thickness of the hovered hyperlink's underline, in logical pixels.
const LINK_UNDERLINE_THICKNESS: f32 = 2.0;

use std::mem;

use egui::{Color32, CornerRadius, Painter, Rect, Vec2, pos2, vec2};
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

/// A single-width glyph pinned to its cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGlyph {
    /// Column of the cell holding the glyph.
    pub column: usize,
    /// The character to draw.
    pub c: char,
    /// Glyph color.
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
/// Cells are pushed in ascending column order; background runs of equal
/// color merge automatically.
#[derive(Clone, Debug, Default)]
pub struct LineCommands {
    /// Merged background runs.
    pub backgrounds: Vec<BgRun>,
    /// Single-width glyphs, one per cell.
    pub glyphs: Vec<CellGlyph>,
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

        self.glyphs.push(CellGlyph {
            column,
            c: cell.c,
            color: fg,
        });
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

    for (row, commands) in lines.iter().enumerate() {
        let line_top = rect.top() + row as f32 * cell_size.y;

        for run in &commands.backgrounds {
            let run_rect = Rect::from_min_size(
                pos2(rect.left() + run.column as f32 * cell_size.x, line_top),
                vec2(cell_size.x * run.span as f32, cell_size.y),
            );
            painter.rect_filled(run_rect, CornerRadius::ZERO, run.color);
        }

        // Wide glyphs are centered over their two cells.
        for glyph in &commands.wides {
            let galley = font.glyph_galley(painter.ctx(), glyph.c);
            let size = galley.size();
            let center = pos2(
                rect.left() + (glyph.column as f32 + 1.0) * cell_size.x,
                line_top + cell_size.y * 0.5,
            );
            painter.galley(
                pos2(center.x - size.x * 0.5, center.y - size.y * 0.5),
                galley,
                glyph.color,
            );
        }

        // Single-width glyphs sit at their own cell positions.
        for glyph in &commands.glyphs {
            let galley = font.glyph_galley(painter.ctx(), glyph.c);
            painter.galley(
                pos2(rect.left() + glyph.column as f32 * cell_size.x, line_top),
                galley,
                glyph.color,
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

    render_cursor(painter, rect, term, cell_size, font, theme, cursor_visible);
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
    font: &TerminalFont,
    theme: &TerminalTheme,
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

            // Redraw the glyph below the cursor in a contrasting color, at
            // the same cell position the regular glyph pass uses.
            let redraw = !cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                && cell.c != ' '
                && !cell.flags.contains(Flags::HIDDEN);
            if redraw {
                let galley = font.glyph_galley(painter.ctx(), cell.c);
                let pos = if wide {
                    let size = galley.size();
                    pos2(
                        cursor_rect.center().x - size.x * 0.5,
                        cursor_rect.center().y - size.y * 0.5,
                    )
                } else {
                    cursor_rect.left_top()
                };
                painter.galley(pos, galley, theme.background());
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
    fn glyphs_carry_their_cell_and_color() {
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
            commands.glyphs,
            vec![
                CellGlyph {
                    column: 0,
                    c: 'h',
                    color: theme.color(fg),
                },
                CellGlyph {
                    column: 1,
                    c: 'i',
                    color: theme.color(fg),
                },
                CellGlyph {
                    column: 3,
                    c: '!',
                    color: theme.color(fg),
                },
                CellGlyph {
                    column: 4,
                    c: '?',
                    color: theme.color(red),
                },
            ]
        );
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
            commands.glyphs,
            vec![CellGlyph {
                column: 2,
                c: 'a',
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
            commands.glyphs,
            vec![CellGlyph {
                column: 0,
                c: 'x',
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
            commands.glyphs[0].color,
            theme.color(fg).linear_multiply(DIM_FACTOR)
        );
        assert_ne!(commands.glyphs[0].color, theme.color(fg));
    }

    #[test]
    fn hidden_cells_draw_no_glyph() {
        let theme = TerminalTheme::default();
        let (fg, bg) = defaults();
        let mut hidden = cell('x', fg, bg);
        hidden.flags.insert(Flags::HIDDEN);

        let mut commands = LineCommands::default();
        commands.push(0, &hidden, &theme, false);

        assert!(commands.glyphs.is_empty());
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
        assert_eq!(unselected.glyphs[0].color, theme.color(fg));
        assert_eq!(selected.glyphs[0].color, theme.background());
    }

    /// The absolute positions and characters of all painted glyphs.
    fn glyph_positions(
        term: &Term<VoidListener>,
        selection: Option<SelectionRange>,
    ) -> Vec<(f32, char)> {
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput::default());
        let font = TerminalFont::default();
        let theme = TerminalTheme::default();
        render_terminal(
            &ctx.debug_painter(),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 60.0)),
            term,
            &font,
            &theme,
            selection,
            false,
            None,
        );
        let output = ctx.end_pass();
        let mut positions = Vec::new();
        for clipped in &output.shapes {
            if let Shape::Text(text) = &clipped.shape {
                for row in &text.galley.rows {
                    for glyph in &row.glyphs {
                        positions.push((text.pos.x + glyph.pos.x, glyph.chr));
                    }
                }
            }
        }
        output.drop_without_applying_deltas();
        positions
    }

    /// Glyphs never move, whatever the colors or selection around them are.
    ///
    /// The line contains ★ (U+2605): a narrow cell that renders from the
    /// CJK fallback font with a wider advance. Shaping it into a string
    /// with the Bs would push them off the grid, and a selection splitting
    /// the string after it would re-anchor them and visibly move the text.
    #[test]
    fn glyphs_stay_on_the_cell_grid() {
        let mut term = Term::new(30, 3, 0, VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"AAAA\xE2\x98\x85BBBB");
        term.start_selection(
            vtx_core::selection::SelectionType::Simple,
            Point::new(Line(0), Column(5)),
            vtx_core::index::Side::Left,
        );
        term.update_selection(Point::new(Line(0), Column(8)), vtx_core::index::Side::Right);

        let plain = glyph_positions(&term, None);
        let selected = glyph_positions(&term, term.selection_range());

        assert_eq!(plain, selected, "selection must not move glyphs");

        // Every glyph sits exactly on its cell: 9 glyphs in columns 0..=8.
        let ctx = egui::Context::default();
        ctx.begin_pass(RawInput::default());
        let cell_width = TerminalFont::default().cell_size(&ctx).x;
        ctx.end_pass().drop_without_applying_deltas();
        assert_eq!(plain.len(), 9);
        for (i, (x, _)) in plain.iter().enumerate() {
            assert!(
                (x - i as f32 * cell_width).abs() < 0.01,
                "glyph {i} at {x} is off the cell grid ({})",
                i as f32 * cell_width
            );
        }
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
