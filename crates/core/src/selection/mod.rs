//! What the pointer has marked, in whichever of two models the user chose.
//!
//! A terminal's selection is a house style rather than a standard, and the
//! two houses disagree about the smallest question there is: what a click
//! points at. Konsole points at a *cell* and grows a range of cells; rio (and
//! alacritty before it) points at the *seam between two cells*, which is what
//! makes a drag that starts on the right half of a character leave that
//! character out. Everything else follows from that: which characters count
//! as part of a word, whether a double click on a bracket takes its partner,
//! how a triple click finds the ends of a wrapped line.
//!
//! So both are here, behind [`SelectionModel`], and `general.selection_model`
//! picks one. [`konsole`] is the default and the one this terminal grew up
//! with; [`rio`] hands the question to the emulation core that is already in
//! the process.
//!
//! An enum rather than a trait: the two arms want different things in hand (a
//! [`GridView`] against a whole `Crosswords`, which is not object-safe), the
//! [`crate::render::Marked`] the renderer is handed has to stay `Clone` and
//! `PartialEq`, and deleting a variant makes the compiler name every site
//! that still assumed it.
//!
//! Coordinates across this enum are absolute lines (see [`crate::grid`]): 0 is
//! the oldest line still in scrollback. rio-vt numbers lines from the top of
//! the screen with history running negative, and [`rio`] does that shift at
//! its own edge so nothing above this module sees it.
//!
//! # Deleting the Konsole model
//!
//! It is meant to come out in one sitting, and this is the list: [`konsole`]
//! and its `mod` line; the `Konsole` variant here and the match arms the
//! compiler then names; `SelectionModel::Konsole` in `robco-config`'s
//! `schema.rs` and the `SELECTION_MODELS` entry in its `dump.rs`; the arm in
//! `crates/app/src/window/mod.rs` that maps the config value to a [`Kind`];
//! `crates/core/tests/suite/selection_konsole_tests.rs`, the parity test at
//! the foot of `selection_rio_tests.rs`, and the `selection_model` entries
//! in `crates/config/src/structural.rs`'s rationale, in
//! `crates/app/tests/suite/structure_subset.rs`'s alternates and in
//! `crates/app/tests/suite/pointer_live_settings.rs`; the `selection_model`
//! row in `settings/ui/form.tcl`, its row in `docs/config.md`, and the
//! sentence naming the two models in `docs/controls.md`.

pub mod konsole;
pub mod rio;

use rio_vt::crosswords::pos::Side;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::EventListener;

use crate::grid::{copy_line, Count, GridView};
use crate::pointer::{self, Modifiers};

/// The visible window a gesture happened in. A UI layer would read these off
/// the widget and its scrollbar; a headless caller passes them directly.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    /// Absolute index of the line at the top of the window.
    pub top_line: usize,
    /// How many lines the window shows.
    pub lines: usize,
    /// How many columns hold text.
    pub columns: usize,
}

impl Window {
    /// The last line the window shows, absolutely.
    pub fn bottom_line(&self) -> usize {
        self.top_line + self.lines.saturating_sub(1)
    }
}

/// Which selection model is in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Konsole,
    Rio,
}

/// One pointer gesture's worth of context: the grid it happened over, the
/// window that grid is shown through, and which half of the cell the pointer
/// was on.
///
/// The grid is held mutably because [`rio`] keeps its selection where rio-vt
/// keeps its own, inside the `Crosswords`; [`konsole`] reborrows it shared
/// and reads it through [`crate::RioGrid`].
pub struct Gesture<'a, U: EventListener> {
    pub term: &'a mut Crosswords<U>,
    pub win: Window,
    pub side: Side,
}

/// A marked rectangle or run of cells, as the renderer is told it.
///
/// `end` is inclusive at both ends, and both are `(column, absolute line)`.
/// A non-block range takes whole intermediate rows, the way a run of text
/// wraps; a block range clamps every row it covers to the same column span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkedRange {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub block: bool,
}

impl MarkedRange {
    /// The per-cell question a renderer asks once per cell of the screen.
    pub fn contains(&self, column: usize, line: usize) -> bool {
        if line < self.start.1 || line > self.end.1 {
            return false;
        }
        if self.block {
            return column >= self.start.0 && column <= self.end.0;
        }
        let past_start = line > self.start.1 || column >= self.start.0;
        let before_end = line < self.end.1 || column <= self.end.0;
        past_start && before_end
    }
}

/// The selection model in force, and the gesture handlers it answers.
#[derive(Clone, Debug)]
pub enum SelectionModel {
    Konsole(konsole::Konsole),
    Rio(rio::Rio),
}

impl SelectionModel {
    pub fn new(kind: Kind, columns: usize) -> Self {
        match kind {
            Kind::Konsole => SelectionModel::Konsole(konsole::Konsole::new(columns)),
            Kind::Rio => SelectionModel::Rio(rio::Rio::new(columns)),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            SelectionModel::Konsole(_) => Kind::Konsole,
            SelectionModel::Rio(_) => Kind::Rio,
        }
    }

    /// Left button down. `mods` carries the two chords a press reads: Ctrl
    /// (Command on macOS) for a run copied without its line breaks, and
    /// Ctrl+Alt for a rectangle.
    pub fn press<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
        mods: Modifiers,
    ) {
        match self {
            SelectionModel::Konsole(k) => {
                k.rebase(g.term.lines_evicted());
                k.preserve_line_breaks = pointer::preserve_line_breaks(mods);
                k.column_selection_mode = pointer::column_selection_mode(mods);
                k.press(cell.0, cell.1);
            }
            SelectionModel::Rio(r) => r.press(g, cell, pointer::column_selection_mode(mods)),
        }
    }

    /// The pointer moved with the button down.
    pub fn drag_to<U: EventListener>(&mut self, g: Gesture<'_, U>, cell: (usize, usize)) {
        match self {
            SelectionModel::Konsole(k) => {
                k.rebase(g.term.lines_evicted());
                let grid = crate::RioGrid::new(&*g.term);
                k.drag_to(&grid, g.win, cell.0, cell.1);
            }
            SelectionModel::Rio(r) => r.drag_to(g, cell),
        }
    }

    /// A double click takes the word under the pointer.
    pub fn double_click<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
    ) -> Option<String> {
        match self {
            SelectionModel::Konsole(k) => {
                k.rebase(g.term.lines_evicted());
                let grid = crate::RioGrid::new(&*g.term);
                k.double_click(&grid, g.win, cell.0, cell.1)
            }
            SelectionModel::Rio(r) => r.double_click(g, cell),
        }
    }

    /// A triple click takes the whole logical line, wrapping and all.
    pub fn triple_click<U: EventListener>(
        &mut self,
        g: Gesture<'_, U>,
        cell: (usize, usize),
    ) -> Option<String> {
        match self {
            SelectionModel::Konsole(k) => {
                k.rebase(g.term.lines_evicted());
                let grid = crate::RioGrid::new(&*g.term);
                k.triple_click(&grid, g.win, cell.0, cell.1)
            }
            SelectionModel::Rio(r) => r.triple_click(g, cell),
        }
    }

    /// Left button up: what the gesture selected, if it selected anything.
    pub fn release<U: EventListener>(&mut self, g: Gesture<'_, U>) -> Option<String> {
        match self {
            SelectionModel::Konsole(k) => {
                k.rebase(g.term.lines_evicted());
                let grid = crate::RioGrid::new(&*g.term);
                k.release(&grid)
            }
            SelectionModel::Rio(r) => r.release(g),
        }
    }

    /// The selected text without ending the gesture.
    pub fn selected_text<U: EventListener>(&self, g: Gesture<'_, U>) -> Option<String> {
        match self {
            SelectionModel::Konsole(k) => {
                let evicted = g.term.lines_evicted();
                let grid = crate::RioGrid::new(&*g.term);
                k.selected_text_at(&grid, evicted, k.preserve_line_breaks)
            }
            SelectionModel::Rio(r) => r.selected_text(g),
        }
    }

    pub fn clear(&mut self) {
        match self {
            SelectionModel::Konsole(k) => k.selection.clear(),
            SelectionModel::Rio(r) => r.clear(),
        }
    }

    /// Re-geometry. A selection means something else at a new width, so both
    /// models drop it rather than reinterpreting it.
    pub fn set_columns(&mut self, columns: usize) {
        match self {
            SelectionModel::Konsole(k) => k.selection.set_columns(columns),
            SelectionModel::Rio(r) => r.set_columns(columns),
        }
    }

    pub fn columns(&self) -> usize {
        match self {
            SelectionModel::Konsole(k) => k.selection.columns(),
            SelectionModel::Rio(r) => r.columns(),
        }
    }

    pub fn has_selection(&self) -> bool {
        match self {
            SelectionModel::Konsole(k) => k.has_selection(),
            SelectionModel::Rio(r) => r.has_selection(),
        }
    }

    /// What the renderer paints, in absolute lines, or `None` when nothing
    /// is marked.
    pub fn range<U: EventListener>(&self, term: &Crosswords<U>) -> Option<MarkedRange> {
        match self {
            SelectionModel::Konsole(k) => {
                // In the numbering the grid holds at this frame: an eviction
                // since the last gesture event moves the painted mark with
                // the text it covers rather than leaving it on the rows below.
                let evicted = term.lines_evicted();
                let start = k.start_at(evicted)?;
                let end = k.end_at(evicted)?;
                Some(MarkedRange {
                    start,
                    end,
                    block: k.selection.block_mode(),
                })
            }
            SelectionModel::Rio(r) => r.range(term),
        }
    }
}

/// The [`crate::grid`] copying this module uses for a selection also lets a
/// caller ask for one line of plain text without a selection: handy for a
/// status line or a test.
pub fn line_text(grid: &impl GridView, line: usize) -> String {
    let mut out = String::new();
    copy_line(grid, line, 0, Count::ToEndOfLine, false, false, &mut out);
    out
}
