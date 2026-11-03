//! Grid search helpers backing the selection types.
//!
//! Semantic, line, and bracket searches walk the grid from a point to find
//! the boundaries of a selection. The regex search machinery for the search
//! bar milestone is not part of this yet.

use std::cmp::max;

use crate::cell::Flags;
use crate::event::EventListener;
use crate::grid::{BidirectionalIterator, Dimensions};
use crate::index::{Boundary, Column, Direction, Point};
use crate::term::Term;

/// Characters terminating a semantic selection, following the upstream
/// default configuration.
pub const SEMANTIC_ESCAPE_CHARS: &str = ",│`|:\"' ()*[]{}<>";

/// Pairs matched by the bracket selection search.
const BRACKET_PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

impl<T: EventListener> Term<T> {
    /// Find the next matching bracket.
    pub fn bracket_search(&self, point: Point) -> Option<Point> {
        let start_char = self.grid()[point].c;

        // Find the matching bracket we're looking for.
        let (forward, end_char) = BRACKET_PAIRS.iter().find_map(|(open, close)| {
            if open == &start_char {
                Some((true, *close))
            } else if close == &start_char {
                Some((false, *open))
            } else {
                None
            }
        })?;

        let mut iter = self.grid().iter_from(point);

        // For every character match that equals the starting bracket, we
        // ignore one bracket of the opposite type.
        let mut skip_pairs = 0;

        loop {
            // Check the next cell.
            let cell = if forward { iter.next() } else { iter.prev() };

            // Break if there are no more cells.
            let cell = match cell {
                Some(cell) => cell,
                None => break,
            };

            // Check if the bracket matches.
            if cell.c == end_char && skip_pairs == 0 {
                return Some(cell.point);
            } else if cell.c == start_char {
                skip_pairs += 1;
            } else if cell.c == end_char {
                skip_pairs -= 1;
            }
        }

        None
    }

    /// Find the left end of the semantic block around `point`.
    #[must_use]
    pub fn semantic_search_left(&self, point: Point) -> Point {
        match self.inline_search_left(point, self.semantic_escape_chars()) {
            // If we found a match, reverse for at least one cell, skipping
            // over wide cell spacers.
            Ok(point) => {
                let wide_spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
                self.grid()
                    .iter_from(point)
                    .find(|cell| !cell.flags.intersects(wide_spacer))
                    .map_or(point, |cell| cell.point)
            }
            Err(point) => point,
        }
    }

    /// Find the right end of the semantic block around `point`.
    #[must_use]
    pub fn semantic_search_right(&self, point: Point) -> Point {
        match self.inline_search_right(point, self.semantic_escape_chars()) {
            Ok(point) => self
                .grid()
                .iter_from(point)
                .prev()
                .map_or(point, |cell| cell.point),
            Err(point) => point,
        }
    }

    /// The characters terminating semantic selections.
    ///
    /// Configurable once the configuration milestone lands; until then the
    /// upstream default set applies.
    pub fn semantic_escape_chars(&self) -> &'static str {
        SEMANTIC_ESCAPE_CHARS
    }

    /// Searching to the left, find the next character contained in `needles`.
    pub fn inline_search_left(&self, mut point: Point, needles: &str) -> Result<Point, Point> {
        // Limit the starting point to the last line in the history.
        point.line = max(point.line, self.topmost_line());

        let mut iter = self.grid().iter_from(point);
        let last_column = self.columns() - 1;

        let wide_spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        while let Some(cell) = iter.prev() {
            if cell.point.column == last_column && !cell.flags.contains(Flags::WRAPLINE) {
                break;
            }

            point = cell.point;

            if !cell.flags.intersects(wide_spacer) && needles.contains(cell.c) {
                return Ok(point);
            }
        }

        Err(point)
    }

    /// Searching to the right, find the next character contained in `needles`.
    pub fn inline_search_right(&self, mut point: Point, needles: &str) -> Result<Point, Point> {
        // Limit the starting point to the last line in the history.
        point.line = max(point.line, self.topmost_line());

        let wide_spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        let last_column = self.columns() - 1;

        // Immediately stop if start point is on a line break.
        if point.column == last_column && !self.grid()[point].flags.contains(Flags::WRAPLINE) {
            return Err(point);
        }

        for cell in self.grid().iter_from(point) {
            point = cell.point;

            if !cell.flags.intersects(wide_spacer) && needles.contains(cell.c) {
                return Ok(point);
            }

            if point.column == last_column && !cell.flags.contains(Flags::WRAPLINE) {
                break;
            }
        }

        Err(point)
    }

    /// Find the beginning of the current line across linewraps.
    pub fn line_search_left(&self, mut point: Point) -> Point {
        while point.line > self.topmost_line()
            && self.grid()[point.line - 1i32][self.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            point.line -= 1;
        }

        point.column = Column(0);

        point
    }

    /// Find the end of the current line across linewraps.
    pub fn line_search_right(&self, mut point: Point) -> Point {
        while point.line + 1 < self.screen_lines()
            && self.grid()[point.line][self.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            point.line += 1;
        }

        point.column = self.last_column();

        point
    }

    /// Jump to the end of a wide cell.
    ///
    /// Consumed by the regex search arriving with the search milestone.
    pub fn expand_wide(&self, mut point: Point, direction: Direction) -> Point {
        let flags = self.grid()[point.line][point.column].flags;

        match direction {
            Direction::Right if flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) => {
                point.column = Column(1);
                point.line += 1;
            }
            Direction::Right if flags.contains(Flags::WIDE_CHAR) => {
                point.column = Column(point.column.0.saturating_add(1)).min(self.last_column());
            }
            Direction::Left if flags.intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER) => {
                if flags.contains(Flags::WIDE_CHAR_SPACER) {
                    point.column -= 1;
                }

                let prev = point.sub(self, Boundary::Grid, 1);
                if self.grid()[prev]
                    .flags
                    .contains(Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    point = prev;
                }
            }
            _ => (),
        }

        point
    }
}
