//! The rio-vt selection model, one arm of [`super::SelectionModel`].
//!
//! The state itself lives where rio-vt keeps its own, in
//! `Crosswords::selection`, and that placement is the whole reason this arm
//! is short. rio-vt numbers a selection's rows from the top of the screen,
//! so every line that scrolls out from under a held selection moves it; the
//! emulation core already rotates `self.selection` on each grid scroll, and a
//! selection kept anywhere else would need that arithmetic copied out. Text
//! comes back the same way, from `selection_to_string`, which is the one
//! place that knows a `Lines` selection ends in a newline and a `Block` one
//! joins its rows with them.
//!
//! What this type holds is the little that rio-vt does not: the column count
//! a resize is measured against, and whether a gesture has put anything on
//! the glass at all.
//!
//! [`super::SelectionModel`] speaks absolute lines in and out; the two
//! conversions at the edges are the only coordinate work here.

use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::Crosswords;
use rio_vt::event::EventListener;
use rio_vt::selection::{Selection, SelectionType};

use super::{Gesture, MarkedRange};

#[derive(Clone, Debug)]
pub struct Rio {
    columns: usize,
    /// A gesture has marked something. Cleared on a resize, where the range
    /// in the grid is no longer the user's, and cleared with the rest of the
    /// model on a channel switch: a mark is a region of one grid and does
    /// not survive the glass turning to another.
    active: bool,
}

impl Rio {
    pub fn new(columns: usize) -> Self {
        Self {
            columns,
            active: false,
        }
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
        self.active = false;
    }

    pub fn clear(&mut self) {
        self.active = false;
    }

    pub fn has_selection(&self) -> bool {
        self.active
    }

    /// Left button down. A press with no drag behind it selects nothing:
    /// rio-vt reads a zero-width simple range as empty, so the anchor sits
    /// there until the pointer moves.
    pub fn press<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
        block: bool,
    ) {
        let ty = if block {
            SelectionType::Block
        } else {
            SelectionType::Simple
        };
        self.start(g, cell, ty);
    }

    pub fn drag_to<U: EventListener>(&mut self, g: Gesture<'_, U>, cell: (usize, usize)) {
        let pos = self.pos(g.term, cell);
        let side = g.side;
        if let Some(selection) = g.term.selection.as_mut() {
            selection.update(pos, side);
        }
    }

    pub fn double_click<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
    ) -> Option<String> {
        self.start_and_read(g, cell, SelectionType::Semantic)
    }

    pub fn triple_click<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
    ) -> Option<String> {
        self.start_and_read(g, cell, SelectionType::Lines)
    }

    pub fn release<U: EventListener>(&mut self, g: Gesture<'_, U>) -> Option<String> {
        self.selected_text(g)
    }

    pub fn selected_text<U: EventListener>(&self, g: Gesture<'_, U>) -> Option<String> {
        if !self.active {
            return None;
        }
        g.term.selection_to_string().filter(|t| !t.is_empty())
    }

    /// The marked cells, in absolute lines.
    pub fn range<U: EventListener>(&self, term: &Crosswords<U>) -> Option<MarkedRange> {
        if !self.active {
            return None;
        }
        let range = term.selection.clone()?.to_range(term)?;
        let history = term.history_size() as i32;
        let line = |row: Line| (row.0 + history).max(0) as usize;
        Some(MarkedRange {
            start: (range.start.col.0, line(range.start.row)),
            end: (range.end.col.0, line(range.end.row)),
            block: range.is_block,
        })
    }

    fn start<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
        ty: SelectionType,
    ) {
        let pos = self.pos(g.term, cell);
        let side = g.side;
        g.term.selection = Some(Selection::new(ty, pos, side));
        self.active = true;
    }

    fn start_and_read<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
        ty: SelectionType,
    ) -> Option<String> {
        let Gesture { term, win, side } = g;
        self.start(
            Gesture {
                term: &mut *term,
                win,
                side,
            },
            cell,
            ty,
        );
        self.selected_text(Gesture { term, win, side })
    }

    /// An absolute `(column, line)` as rio-vt addresses it: rows counted from
    /// the top of the screen, history running negative.
    fn pos<U: EventListener>(&self, term: &Crosswords<U>, cell: (usize, usize)) -> Pos {
        let column = cell.0.min(self.columns.saturating_sub(1));
        let row = cell.1 as i32 - term.history_size() as i32;
        Pos::new(Line(row), Column(column))
    }
}
