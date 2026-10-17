//! The Konsole selection model: the anchor/extent state, and the pointer
//! gestures that drive it.
//!
//! Two layers, kept deliberately apart:
//!
//! - [`Selection`] is three integers plus block mode. It knows nothing about
//!   the mouse.
//! - [`Konsole`] is the gesture half: the drag anchor, the word/line modes a
//!   double or triple click puts it in, and the swap-detection that lets a
//!   drag cross back over its own start.
//!
//! Coordinates here are absolute (see [`crate::grid`]) rather than
//! window-relative with a separately added scroll offset; doing it
//! absolutely throughout cancels the scroll term out of the drag arithmetic,
//! at the cost of having to be told the window's extent explicitly: that is
//! [`super::Window`].
//!
//! Absolute is not stable, though: once the scrollback ring is at capacity,
//! every new line evicts the oldest one and re-names every absolute index to
//! content one line later. A gesture that spans an eviction -- output
//! arriving between the drag and the release -- would extract cells below
//! the text it covered. So the model also carries the eviction count its
//! coordinates were taken against ([`Konsole::rebase`], fed from rio-vt's
//! `Grid::lines_evicted`), and every entry point brings the stored
//! coordinates into the numbering the grid holds now.
//!
//! This whole file is one arm of [`super::SelectionModel`] and is written to
//! be deletable; [`super`]'s module documentation lists what goes with it.

use super::Window;
use crate::grid::{write_range, GridView};

/// The selection state: an anchor and the normalised range around it.
///
/// Positions are the same linear index the grid uses, `line * columns +
/// column`, so a selection spanning history and screen is one contiguous
/// range with no special case at the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    columns: usize,
    begin: i64,
    top_left: i64,
    bottom_right: i64,
    block_mode: bool,
}

impl Selection {
    pub fn new(columns: usize) -> Self {
        Self {
            columns,
            begin: -1,
            top_left: -1,
            bottom_right: -1,
            block_mode: false,
        }
    }

    fn loc(&self, x: usize, y: usize) -> i64 {
        (y * self.columns + x) as i64
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Re-geometry. The selection clears outright on resize, because the
    /// linear index it is stored as means something different at a new
    /// width.
    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
        self.clear();
    }

    pub fn clear(&mut self) {
        self.begin = -1;
        self.top_left = -1;
        self.bottom_right = -1;
    }

    /// One linear index, moved `lines` lines up. A cell whose line has
    /// already fallen off the ring pins to line 0 in its own column: the
    /// text it named is gone, and the oldest line that still exists is the
    /// closest thing left to point at.
    fn shifted(&self, index: i64, lines: u64) -> i64 {
        if index < 0 || lines == 0 || self.columns == 0 {
            return index;
        }
        let cols = self.columns as i64;
        let line = (index / cols).saturating_sub(lines.min(i64::MAX as u64) as i64);
        line.max(0) * cols + index % cols
    }

    /// What an eviction of `lines` lines off the top of the ring did to the
    /// text every held position names.
    fn shift_up(&mut self, lines: u64) {
        self.begin = self.shifted(self.begin, lines);
        self.top_left = self.shifted(self.top_left, lines);
        self.bottom_right = self.shifted(self.bottom_right, lines);
    }

    pub fn is_valid(&self) -> bool {
        self.top_left >= 0 && self.bottom_right >= 0
    }

    pub fn block_mode(&self) -> bool {
        self.block_mode
    }

    /// A click one past the last column is pulled back onto it, and it is
    /// load-bearing: without it a drag that starts in the right margin
    /// anchors on the first cell of the next line.
    pub fn set_start(&mut self, x: usize, y: usize, block_mode: bool) {
        let mut begin = self.loc(x, y);
        if x == self.columns {
            begin -= 1;
        }
        self.begin = begin;
        self.top_left = begin;
        self.bottom_right = begin;
        self.block_mode = block_mode;
    }

    /// `Screen::setSelectionEnd`. Extending above the anchor flips the range
    /// rather than emptying it, so a drag upwards selects.
    pub fn set_end(&mut self, x: usize, y: usize) {
        if self.begin == -1 {
            return;
        }
        let mut end_pos = self.loc(x, y);

        if end_pos < self.begin {
            self.top_left = end_pos;
            self.bottom_right = self.begin;
        } else {
            if x == self.columns {
                end_pos -= 1;
            }
            self.top_left = self.begin;
            self.bottom_right = end_pos;
        }

        if self.block_mode {
            let cols = self.columns as i64;
            let top_row = self.top_left / cols;
            let top_column = self.top_left % cols;
            let bottom_row = self.bottom_right / cols;
            let bottom_column = self.bottom_right % cols;
            self.top_left = top_row * cols + top_column.min(bottom_column);
            self.bottom_right = bottom_row * cols + top_column.max(bottom_column);
        }
    }

    /// `Screen::isSelected`, the per-cell query a renderer asks once per cell.
    pub fn is_selected(&self, x: usize, y: usize) -> bool {
        if !self.is_valid() {
            return false;
        }
        let cols = self.columns as i64;
        let column_in_selection = if self.block_mode {
            let x = x as i64;
            x >= self.top_left % cols && x <= self.bottom_right % cols
        } else {
            true
        };
        let pos = self.loc(x, y);
        pos >= self.top_left && pos <= self.bottom_right && column_in_selection
    }

    /// `(column, line)` of the first selected cell, absolutely.
    pub fn start(&self) -> Option<(usize, usize)> {
        (self.top_left >= 0).then(|| {
            (
                (self.top_left as usize) % self.columns,
                (self.top_left as usize) / self.columns,
            )
        })
    }

    /// `(column, line)` of the last selected cell, absolutely.
    pub fn end(&self) -> Option<(usize, usize)> {
        (self.bottom_right >= 0).then(|| {
            (
                (self.bottom_right as usize) % self.columns,
                (self.bottom_right as usize) / self.columns,
            )
        })
    }

    /// The round trip back out of the grid.
    ///
    /// `preserve_line_breaks` mirrors whether Ctrl was held down: when it
    /// was, the selection comes back as one run with no newlines, for
    /// pasting a wrapped command back into a shell.
    pub fn selected_text(
        &self,
        grid: &impl GridView,
        preserve_line_breaks: bool,
    ) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        Some(write_range(
            grid,
            self.top_left as usize,
            self.bottom_right as usize,
            self.block_mode,
            preserve_line_breaks,
        ))
    }
}

/// The extra characters, beyond letters and digits, that count as part of a
/// word -- matching Konsole's default. Its effect is that double-clicking a
/// path or a URL selects the whole thing.
pub const DEFAULT_WORD_CHARACTERS: &str = ":@-./_~";

/// The equivalence a word-wise selection grows along. Letters, digits and
/// the word characters all collapse to `'a'`; all whitespace collapses to
/// `' '`; anything else is its own class, so a run of `!!!` is one word and
/// `!?!` is three.
pub fn char_class(c: char, word_characters: &str) -> char {
    if c.is_whitespace() {
        return ' ';
    }
    if c.is_alphanumeric()
        || word_characters
            .chars()
            .any(|w| w.eq_ignore_ascii_case(&c) || w == c)
    {
        return 'a';
    }
    c
}

/// What a triple click selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripleClickMode {
    /// From the start of the word under the pointer to the end of the line.
    SelectForwardsFromCursor,
    /// The whole (logically wrapped) line. Konsole's default.
    SelectWholeLine,
}

/// Which gesture the current drag is extending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Character,
    Word,
    Line,
}

/// The pointer half of selection: the anchor, head and mode state, and the
/// gesture handlers that drive a [`Selection`] from them.
///
/// The shape to keep in mind is that a drag has an *anchor* (`i_pnt_sel`, set
/// on press and never moved) and a *head* (`pnt_sel`, the last position the
/// pointer reported). Which of the two becomes the selection's start depends
/// on which side of the anchor the head is on, and that is recomputed on every
/// move so the selection follows a pointer that crosses back over its anchor.
#[derive(Clone, Debug)]
pub struct Konsole {
    pub selection: Selection,
    pub word_characters: String,
    pub triple_click_mode: TripleClickMode,
    /// Set from the modifiers on press.
    pub preserve_line_breaks: bool,
    /// Ctrl+Alt: a rectangular selection.
    pub column_selection_mode: bool,
    anchor: (usize, usize),
    head: (usize, usize),
    mode: Mode,
    /// `_actSel`: 0 = none, 1 = pressed but empty, 2 = inside a selection.
    act_sel: u8,
    triple_sel_begin: (usize, usize),
    /// The grid's `lines_evicted()` when the coordinates above were last
    /// taken or rebased: the numbering they are absolute in.
    evicted_base: u64,
}

impl Konsole {
    pub fn new(columns: usize) -> Self {
        Self {
            selection: Selection::new(columns),
            word_characters: DEFAULT_WORD_CHARACTERS.to_string(),
            triple_click_mode: TripleClickMode::SelectWholeLine,
            preserve_line_breaks: true,
            column_selection_mode: false,
            anchor: (0, 0),
            head: (0, 0),
            mode: Mode::Character,
            act_sel: 0,
            triple_sel_begin: (0, 0),
            evicted_base: 0,
        }
    }

    /// Is there a selection the user is inside or has just made?
    pub fn has_selection(&self) -> bool {
        self.act_sel > 1 && self.selection.is_valid()
    }

    /// Bring every stored coordinate into the numbering the grid holds now,
    /// given `evicted` -- the grid's own count of lines ever evicted off the
    /// top of the scrollback ring (`Grid::lines_evicted`). Called at the top
    /// of every gesture, so a drag or a release that follows an eviction
    /// still names the text the pointer was over rather than the cells that
    /// many lines below it.
    pub fn rebase(&mut self, evicted: u64) {
        let lines = evicted.saturating_sub(self.evicted_base);
        self.evicted_base = evicted;
        if lines == 0 {
            return;
        }
        let l = lines.min(usize::MAX as u64) as usize;
        self.anchor.1 = self.anchor.1.saturating_sub(l);
        self.head.1 = self.head.1.saturating_sub(l);
        self.triple_sel_begin.1 = self.triple_sel_begin.1.saturating_sub(l);
        self.selection.shift_up(lines);
    }

    /// [`Selection::start`] in the numbering `evicted` describes, without
    /// touching the stored state: the read side of [`Self::rebase`], for the
    /// renderer asking where the mark sits this frame.
    pub fn start_at(&self, evicted: u64) -> Option<(usize, usize)> {
        let lines = evicted.saturating_sub(self.evicted_base);
        let index = self.selection.shifted(self.selection.top_left, lines);
        (index >= 0 && self.selection.columns > 0).then(|| {
            (
                index as usize % self.selection.columns,
                index as usize / self.selection.columns,
            )
        })
    }

    /// [`Selection::end`] in the numbering `evicted` describes.
    pub fn end_at(&self, evicted: u64) -> Option<(usize, usize)> {
        let lines = evicted.saturating_sub(self.evicted_base);
        let index = self.selection.shifted(self.selection.bottom_right, lines);
        (index >= 0 && self.selection.columns > 0).then(|| {
            (
                index as usize % self.selection.columns,
                index as usize / self.selection.columns,
            )
        })
    }

    /// [`Selection::selected_text`] in the numbering `evicted` describes,
    /// without touching the stored state.
    pub fn selected_text_at(
        &self,
        grid: &impl GridView,
        evicted: u64,
        preserve_line_breaks: bool,
    ) -> Option<String> {
        if !self.selection.is_valid() {
            return None;
        }
        let lines = evicted.saturating_sub(self.evicted_base);
        let top_left = self.selection.shifted(self.selection.top_left, lines);
        let bottom_right = self.selection.shifted(self.selection.bottom_right, lines);
        Some(write_range(
            grid,
            top_left as usize,
            bottom_right as usize,
            self.selection.block_mode(),
            preserve_line_breaks,
        ))
    }

    /// Left button down at an absolute cell. Clears any selection and sets
    /// the anchor; the selection stays empty until the pointer moves, which is
    /// why a bare click does not select the cell under it.
    pub fn press(&mut self, x: usize, y: usize) {
        self.mode = Mode::Character;
        self.selection.clear();
        self.anchor = (x, y);
        self.head = (x, y);
        self.act_sel = 1;
    }

    /// Left button up. Returns the selected text if the drag actually
    /// selected something, which is the point at which a caller would copy
    /// it to the primary selection.
    pub fn release(&mut self, grid: &impl GridView) -> Option<String> {
        let text = if self.act_sel > 1 {
            self.selection
                .selected_text(grid, self.preserve_line_breaks)
        } else {
            None
        };
        self.act_sel = 0;
        text
    }

    /// The pointer moved with the button down.
    ///
    /// The three modes share one skeleton. Decide whether the head is left of
    /// the anchor (`left_not_right`); notice if that answer just changed
    /// (`swapping`), because then the selection's start has to be re-set
    /// rather than just its end; grow both ends outward to word or line
    /// boundaries if a double or triple click put us in that mode; then feed
    /// the far end in as the start and the near end as the end.
    ///
    /// The `offset` of -1 when dragging leftwards is what makes a leftward
    /// drag include the character under the pointer rather than stopping
    /// short of it.
    pub fn drag_to(&mut self, grid: &impl GridView, win: Window, x: usize, y: usize) {
        // The pointer is clamped into the text rectangle before anything
        // else; scrolling the view when it goes past the edge is the
        // caller's job, this only clamps.
        let mut here = (
            x.min(win.columns.saturating_sub(1)),
            y.clamp(win.top_line, win.bottom_line()),
        );
        let anchor = self.anchor;
        let old_head = self.head;
        let mut ohere;
        let swapping;
        let mut offset: i64 = 0;

        match self.mode {
            Mode::Word => {
                let left_not_right = before(here, anchor);
                let old_left_not_right = before(old_head, anchor);
                swapping = left_not_right != old_left_not_right;

                let left_seed = if left_not_right { here } else { anchor };
                let left = self.word_start(grid, win, left_seed);
                let right_seed = if left_not_right { anchor } else { here };
                let right = self.word_end(grid, win, right_seed);

                if left_not_right {
                    here = left;
                    ohere = right;
                } else {
                    here = right;
                    ohere = left;
                }
                ohere.0 += 1;
            }
            Mode::Line => {
                let above_not_below = here.1 < anchor.1;
                let mut above = if above_not_below { here } else { anchor };
                let mut below = if above_not_below { anchor } else { here };

                while above.1 > win.top_line && grid.is_wrapped(above.1 - 1) {
                    above.1 -= 1;
                }
                while below.1 < win.bottom_line() && grid.is_wrapped(below.1) {
                    below.1 += 1;
                }
                above.0 = 0;
                below.0 = win.columns.saturating_sub(1);

                if above_not_below {
                    here = above;
                    ohere = below;
                } else {
                    here = below;
                    ohere = above;
                }

                let new_sel_begin = ohere;
                swapping = self.triple_sel_begin != new_sel_begin;
                self.triple_sel_begin = new_sel_begin;
                ohere.0 += 1;
            }
            Mode::Character => {
                let left_not_right = before(here, anchor);
                let old_left_not_right = before(old_head, anchor);
                swapping = left_not_right != old_left_not_right;

                let left = if left_not_right { here } else { anchor };
                let right = if left_not_right { anchor } else { here };

                if left_not_right {
                    here = left;
                    ohere = right;
                    offset = 0;
                } else {
                    here = right;
                    ohere = left;
                    offset = -1;
                }
            }
        }

        if here == old_head {
            return;
        }
        if here == ohere {
            return;
        }

        if self.act_sel < 2 || swapping {
            if self.column_selection_mode && self.mode == Mode::Character {
                self.selection.set_start(ohere.0, ohere.1, true);
            } else {
                let sx = (ohere.0 as i64 - 1 - offset).max(0) as usize;
                self.selection.set_start(sx, ohere.1, false);
            }
        }

        self.act_sel = 2;
        self.head = here;

        if self.column_selection_mode && self.mode == Mode::Character {
            self.selection.set_end(here.0, here.1);
        } else {
            let ex = (here.0 as i64 + offset).max(0) as usize;
            self.selection.set_end(ex, here.1);
        }
    }

    /// A double click selects the word under the
    /// pointer and stay in word mode, so a drag afterwards extends by whole
    /// words.
    pub fn double_click(
        &mut self,
        grid: &impl GridView,
        win: Window,
        x: usize,
        y: usize,
    ) -> Option<String> {
        self.selection.clear();
        self.anchor = (x, y);
        self.head = (x, y);
        self.mode = Mode::Word;

        let bgn = self.word_start(grid, win, (x, y));
        self.selection.set_start(bgn.0, bgn.1, false);

        let mut end = self.word_end(grid, win, (x, y));

        // Word selection mode doesn't select a trailing `@`: double-clicking
        // `user@host` with the click landing in `user` should not hand you
        // a trailing `@`.
        if grid.cell(end.1, end.0) == '@' && (end.0 as i64 - bgn.0 as i64) > 0 {
            end.0 -= 1;
        }

        self.act_sel = 2;
        self.selection.set_end(end.0, end.1);
        self.selection
            .selected_text(grid, self.preserve_line_breaks)
    }

    /// A triple click selects the whole logical
    /// line, following wrapping in both directions.
    pub fn triple_click(
        &mut self,
        grid: &impl GridView,
        win: Window,
        x: usize,
        y: usize,
    ) -> Option<String> {
        self.selection.clear();
        self.mode = Mode::Line;
        self.act_sel = 2;

        let mut sel = (x, y);
        while sel.1 > win.top_line && grid.is_wrapped(sel.1 - 1) {
            sel.1 -= 1;
        }

        match self.triple_click_mode {
            TripleClickMode::SelectForwardsFromCursor => {
                let start = self.word_start(grid, win, (sel.0, sel.1));
                self.selection.set_start(start.0, start.1, false);
                self.triple_sel_begin = start;
            }
            TripleClickMode::SelectWholeLine => {
                self.selection.set_start(0, sel.1, false);
                self.triple_sel_begin = (0, sel.1);
            }
        }

        while sel.1 < win.bottom_line() && grid.is_wrapped(sel.1) {
            sel.1 += 1;
        }
        self.selection.set_end(win.columns.saturating_sub(1), sel.1);

        self.anchor = (x, sel.1);
        self.head = (win.columns.saturating_sub(1), sel.1);
        self.selection
            .selected_text(grid, self.preserve_line_breaks)
    }

    /// Walk left while the character class does not change, crossing into the
    /// previous line only where that line wrapped into this one.
    fn word_start(
        &self,
        grid: &impl GridView,
        win: Window,
        from: (usize, usize),
    ) -> (usize, usize) {
        let (mut x, mut y) = from;
        let class = char_class(grid.cell(y, x), &self.word_characters);
        loop {
            let can_step_back = x > 0 || (y > win.top_line && grid.is_wrapped(y.saturating_sub(1)));
            if !can_step_back {
                break;
            }
            let (px, py) = if x > 0 {
                (x - 1, y)
            } else {
                (win.columns.saturating_sub(1), y - 1)
            };
            if char_class(grid.cell(py, px), &self.word_characters) != class {
                break;
            }
            x = px;
            y = py;
        }
        (x, y)
    }

    /// The mirror of `word_start`, walking right.
    fn word_end(&self, grid: &impl GridView, win: Window, from: (usize, usize)) -> (usize, usize) {
        let (mut x, mut y) = from;
        let class = char_class(grid.cell(y, x), &self.word_characters);
        let last_col = win.columns.saturating_sub(1);
        loop {
            let can_step_on = x < last_col || (y < win.bottom_line() && grid.is_wrapped(y));
            if !can_step_on {
                break;
            }
            let (nx, ny) = if x < last_col { (x + 1, y) } else { (0, y + 1) };
            if char_class(grid.cell(ny, nx), &self.word_characters) != class {
                break;
            }
            x = nx;
            y = ny;
        }
        (x, y)
    }
}

/// Reading order: is `a` before `b` on the grid?
fn before(a: (usize, usize), b: (usize, usize)) -> bool {
    a.1 < b.1 || (a.1 == b.1 && a.0 < b.0)
}
