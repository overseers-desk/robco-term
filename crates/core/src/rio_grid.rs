//! [`GridView`] over rio-vt's `Crosswords`.
//!
//! The whole adapter is the coordinate shift and the NUL normalisation. rio-vt
//! numbers lines with 0 at the top of the screen and negatives walking up into
//! scrollback; everything above [`crate::grid`] numbers them from the oldest
//! line in history. One addition converts between them.
//!
//! `line_len` is where the packed-`Square` trap is paid off: an untouched cell
//! reads `'\0'`, so the written length of a line is one past its last non-NUL
//! cell. Without that, every row in an 80-column grid is 80 characters wide,
//! selections copy trailing blanks, and search haystacks are full of NULs.

use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::crosswords::Crosswords;
use rio_vt::event::EventListener;

use crate::grid::GridView;

/// Borrows a `Crosswords` for as long as a grid-reading operation runs.
pub struct RioGrid<'a, U: EventListener> {
    term: &'a Crosswords<U>,
}

impl<'a, U: EventListener> RioGrid<'a, U> {
    pub fn new(term: &'a Crosswords<U>) -> Self {
        Self { term }
    }

    /// The rio-vt `Line` for an absolute line index.
    fn line(&self, line: usize) -> Line {
        Line(line as i32 - self.history_lines() as i32)
    }
}

impl<U: EventListener> GridView for RioGrid<'_, U> {
    fn columns(&self) -> usize {
        self.term.grid.columns()
    }

    fn history_lines(&self) -> usize {
        self.term.history_size()
    }

    fn screen_lines(&self) -> usize {
        self.term.grid.screen_lines()
    }

    fn cell(&self, line: usize, column: usize) -> char {
        if column >= self.columns() || line >= self.total_lines() {
            return ' ';
        }
        let c = self.term.grid[self.line(line)][Column(column)].c();
        if c == '\0' {
            ' '
        } else {
            c
        }
    }

    fn line_len(&self, line: usize) -> usize {
        if line >= self.total_lines() {
            return 0;
        }
        let row = &self.term.grid[self.line(line)];
        let columns = self.columns();
        for column in (0..columns).rev() {
            if row[Column(column)].c() != '\0' {
                return column + 1;
            }
        }
        0
    }

    fn is_wrapped(&self, line: usize) -> bool {
        if line >= self.total_lines() {
            return false;
        }
        // rio-vt stores wrap state as a per-cell flag on the last cell of the
        // row (alacritty's convention, and Konsole's), so "did this line
        // wrap" is a question about column `columns - 1`.
        let columns = self.columns();
        if columns == 0 {
            return false;
        }
        self.term.grid[self.line(line)][Column(columns - 1)].wrapline()
    }
}

// -- Reading the grid back out as text ------------------------------------
//
// The read-back path, now on top of [`RioGrid`] rather than beside
// it. One trap governs all of it: a `Square` is a packed `u64`, an
// untouched cell is all-zero bits, so `Square::c()` returns `'\0'` and not
// `' '`. That normalisation happens once, in `RioGrid::cell`, and these
// functions inherit it.

/// The one place `'\0'` becomes `' '`, for callers holding a bare char
/// rather than a grid.
#[inline]
pub fn cell_char(c: char) -> char {
    if c == '\0' {
        ' '
    } else {
        c
    }
}

/// One row as a `String`, normalised, with trailing blanks trimmed.
///
/// `line` is rio-vt's signed index: `0..screen_lines` is the live screen
/// and negative indices walk up into scrollback, so this reads both. Note
/// that this is the *live* screen, not the viewport -- see
/// [`crate::cells::vt::viewport_text`] for the scrolled-back view.
pub fn row_text<U: EventListener>(term: &Crosswords<U>, line: i32) -> String {
    let grid = RioGrid::new(term);
    let absolute = (line + term.history_size() as i32).max(0) as usize;
    let mut s = String::with_capacity(grid.columns());
    for column in 0..grid.columns() {
        s.push(grid.cell(absolute, column));
    }
    s.trim_end().to_string()
}

/// One row untrimmed, exactly `columns()` chars wide.
///
/// The renderer wants this: it needs a cell per column, including the
/// trailing blanks `row_text` drops.
pub fn row_cells<U: EventListener>(term: &Crosswords<U>, line: i32) -> Vec<char> {
    let grid = RioGrid::new(term);
    let absolute = (line + term.history_size() as i32).max(0) as usize;
    (0..grid.columns())
        .map(|column| grid.cell(absolute, column))
        .collect()
}

/// The live screen, one entry per row, top to bottom.
///
/// The live screen, which is not the viewport: scrolled back, the user is
/// looking at something else. [`crate::cells::vt::viewport_text`] answers
/// that question.
pub fn live_text<U: EventListener>(term: &Crosswords<U>) -> Vec<String> {
    (0..term.grid.screen_lines() as i32)
        .map(|l| row_text(term, l))
        .collect()
}

/// Scrollback and screen together, oldest line first.
pub fn all_text<U: EventListener>(term: &Crosswords<U>) -> Vec<String> {
    let history = term.history_size() as i32;
    (-history..term.grid.screen_lines() as i32)
        .map(|l| row_text(term, l))
        .collect()
}

/// Whether any visible row contains `needle`. The predicate the
/// transcript tests are written against.
pub fn screen_contains<U: EventListener>(term: &Crosswords<U>, needle: &str) -> bool {
    live_text(term).iter().any(|l| l.contains(needle))
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn nul_normalises_to_space_and_nothing_else_moves() {
        assert_eq!(cell_char('\0'), ' ');
        assert_eq!(cell_char(' '), ' ');
        assert_eq!(cell_char('A'), 'A');
    }
}
