//! Mouse position to grid coordinate translation.

use egui::{Pos2, Rect, Vec2};
use vtx_core::index::{Column, Line, Point, Side};

/// Translate a pointer position over the terminal into a grid point.
///
/// Positions outside the rect clamp to the nearest visible cell, so drags
/// past the screen edges keep updating the selection. The side notes which
/// half of the cell the pointer sits in, so anchor sides resolve to the
/// cell boundary the user sees.
pub fn point_at(
    rect: Rect,
    pos: Pos2,
    cell_size: Vec2,
    columns: usize,
    screen_lines: usize,
    display_offset: usize,
) -> (Point, Side) {
    debug_assert!(cell_size.x > 0.0 && cell_size.y > 0.0);
    debug_assert!(columns > 0 && screen_lines > 0);

    // Positions past the grid clamp to its last cell, so drags beyond the
    // screen edges keep updating the selection. Stepping one float below
    // the grid edge keeps such positions in the right half of the last
    // column.
    let max_x = (columns as f32 * cell_size.x).next_down();
    let max_y = (screen_lines as f32 * cell_size.y).next_down();
    let x = (pos.x - rect.left()).clamp(0.0, max_x);
    let y = (pos.y - rect.top()).clamp(0.0, max_y);

    let column_float = x / cell_size.x;
    let column = (column_float as usize).min(columns - 1);
    let row = ((y / cell_size.y) as usize).min(screen_lines - 1);

    // Right half of the cell anchors to its right boundary.
    let side = if column_float - column_float.floor() >= 0.5 {
        Side::Right
    } else {
        Side::Left
    };

    let line = Line(row as i32 - display_offset as i32);

    (Point::new(line, Column(column)), side)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Vec2 = Vec2::new(10.0, 20.0);
    const RECT: Rect = Rect {
        min: Pos2::new(100.0, 50.0),
        max: Pos2::new(200.0, 150.0),
    };

    #[test]
    fn maps_to_cell_and_side() {
        // Left half of the top-left cell.
        let (point, side) = point_at(RECT, Pos2::new(103.0, 60.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(0), Column(0)));
        assert_eq!(side, Side::Left);

        // Right half of the same cell.
        let (point, side) = point_at(RECT, Pos2::new(106.0, 60.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(0), Column(0)));
        assert_eq!(side, Side::Right);

        // Left half of the fifth cell on the second row.
        let (point, side) = point_at(RECT, Pos2::new(143.0, 81.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(1), Column(4)));
        assert_eq!(side, Side::Left);

        // Right half of the same cell.
        let (point, side) = point_at(RECT, Pos2::new(148.0, 81.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(1), Column(4)));
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn clamps_outside_positions() {
        // Above and left of the rect.
        let (point, side) = point_at(RECT, Pos2::new(0.0, 0.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(0), Column(0)));
        assert_eq!(side, Side::Left);

        // Below and right of the grid.
        let (point, side) = point_at(RECT, Pos2::new(400.0, 300.0), CELL, 10, 5, 0);
        assert_eq!(point, Point::new(Line(4), Column(9)));
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn accounts_for_display_offset() {
        // Row 2 of the viewport scrolled three lines into history.
        let (point, _) = point_at(RECT, Pos2::new(105.0, 60.0 + 2.0 * CELL.y), CELL, 10, 5, 3);
        assert_eq!(point, Point::new(Line(-1), Column(0)));
    }
}
