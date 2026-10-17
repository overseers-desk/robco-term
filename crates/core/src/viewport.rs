//! Scrollback viewport.
//!
//! rio-vt owns the authoritative line count (`Grid::display_offset`, in
//! whole lines above the bottom), and `Line` indexing folds it in, so this
//! type is not a second copy of that state. It is the policy layer: how a
//! wheel notch, a touchpad's pixels or a Shift-PageUp turn into a position,
//! whether the view follows new output, and the one thing rio-vt cannot
//! hold, a position between two lines.
//!
//! The position is [`ScrollPosition::pos`], rows above the bottom as a
//! float (VTE keeps the same number as `scroll_delta`). rio-vt's offset is
//! held at `ceil(pos)`, and the remainder `ceil(pos) - pos` is the
//! [`shift`](ScrollPosition::shift): the fraction of a row the picture is
//! drawn shifted *up*, with one spare row filled below the last. Ceil rather
//! than floor, because `display_offset == 0` is rio-vt's own definition of
//! "live": with the offset at 0 nothing holds the view against output
//! arriving, and under floor a position of 0.3 rows would leave it there,
//! so the first line of output would yank the picture a whole row in the
//! middle of a gesture. Under ceil any position above 0 pins the view at
//! once; the picture is continuous at 0 (a position of 0.001 is offset 1
//! drawn shifted up by 0.999 of a row, the same pixels as offset 0); and
//! every row index the renderer and the damage path use keeps its meaning.
//!
//! The rule that matters to the renderer is the last one: a viewport that
//! moved by a line invalidates every row on screen, because rio-vt's own
//! damage is expressed in viewport coordinates and says nothing about the
//! rows that scrolled in from history. A change of shift alone moves the
//! picture, not the rows, and invalidates nothing.

use rio_vt::crosswords::grid::Scroll;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::EventListener;
use std::time::{Duration, Instant};

/// Lines per wheel notch.
pub const WHEEL_LINES: i32 = 3;

/// How long a wheel notch takes to arrive. Long enough for the eye to follow
/// the lines on their way, short enough that a run of notches never feels
/// behind the hand.
pub const WHEEL_GLIDE: Duration = Duration::from_millis(120);

/// A glide in progress: from one position to another, on the clock the
/// caller passes in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: f32,
    to: f32,
    started: Instant,
}

/// Where the view sits in the scrollback, and whether it follows output.
///
/// Named for the question it answers rather than for the module, because
/// [`crate::size::Viewport`] is the *other* viewport (a window's pixels
/// and cells), and the two arriving under one name is how the alias this
/// replaced came about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollPosition {
    /// Rows above the bottom, fractional. rio-vt holds `ceil(pos)`.
    pos: f32,
    /// True while the view is pinned to the bottom, so new output scrolls it.
    /// Set by reaching the bottom, cleared by scrolling up, exactly as every
    /// terminal behaves.
    follow: bool,
    glide: Option<Glide>,
}

impl Default for ScrollPosition {
    fn default() -> Self {
        Self {
            pos: 0.0,
            follow: true,
            glide: None,
        }
    }
}

/// The whole lines rio-vt is asked to hold for a position: the ceiling,
/// per the module doc.
fn lines_for(pos: f32) -> usize {
    pos.ceil().max(0.0) as usize
}

impl ScrollPosition {
    /// Whole lines above the bottom of the scrollback that the top of the
    /// screen sits at: rio-vt's own `display_offset`. Zero means "showing
    /// live output".
    pub fn offset(&self) -> usize {
        lines_for(self.pos)
    }

    /// Rows above the bottom, fractional.
    pub fn position(&self) -> f32 {
        self.pos
    }

    /// The fraction of a row (0 inclusive to 1 exclusive) the picture is
    /// drawn shifted up from the whole-line layout rio-vt holds. Zero when
    /// the position is a whole number of lines, the bottom included.
    pub fn shift(&self) -> f32 {
        let shift = self.pos.ceil() - self.pos;
        if shift >= 1.0 {
            0.0
        } else {
            shift
        }
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// A wheel glide is under way: the picture moves on every frame until
    /// [`advance`](Self::advance) says it has arrived.
    pub fn is_gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// Drop a glide where it is. A glide belongs to the terminal it was
    /// started on; the view moving to another terminal leaves it behind.
    pub fn cancel_glide(&mut self) {
        self.glide = None;
    }

    /// Positive `delta` scrolls back into history, matching rio-vt's
    /// `Scroll::Delta` sign. Whole lines, at once, no glide: the keyboard's
    /// repeat would otherwise restart a glide thirty times a second and
    /// crawl.
    pub fn scroll<L: EventListener>(&mut self, term: &mut Crosswords<L>, delta: i32) {
        self.glide = None;
        self.settle(term, self.pos.round() + delta as f32);
    }

    /// A wheel notch: the view sets off for `notches * WHEEL_LINES` lines
    /// further than where it was already going, and arrives over
    /// [`WHEEL_GLIDE`]. Notches during a glide move the destination, not the
    /// picture, so a run of them reads as one motion.
    pub fn scroll_wheel<L: EventListener>(
        &mut self,
        term: &mut Crosswords<L>,
        notches: i32,
        now: Instant,
    ) {
        let going_to = self.glide.map_or(self.pos.round(), |g| g.to);
        let to = clamp(going_to + (notches * WHEEL_LINES) as f32, term);
        if to == self.pos {
            self.glide = None;
            return;
        }
        self.glide = Some(Glide {
            from: self.pos,
            to,
            started: now,
        });
    }

    /// A touchpad's pixels, applied as they arrive: `pixels` up the screen
    /// (into history) over a cell of `cell_height` pixels. No glide; the
    /// fingers are the motion.
    pub fn scroll_pixels<L: EventListener>(
        &mut self,
        term: &mut Crosswords<L>,
        pixels: f32,
        cell_height: f32,
    ) {
        self.glide = None;
        let rows = pixels / cell_height.max(1.0);
        self.settle(term, self.pos + rows);
    }

    /// Move the view the least it can to put an absolute line on the
    /// screen, and leave it where it is when the line is already there.
    ///
    /// The least it can, because the line a search found is not the only
    /// thing the eye is reading: a hit two rows above the top of the view
    /// should bring those two rows down, not jump the hit to the middle of
    /// a screen the user has to re-read. Absolute lines are
    /// [`crate::grid`]'s, 0 at the oldest line still in scrollback.
    pub fn reveal<L: EventListener>(&mut self, term: &mut Crosswords<L>, line: usize) {
        let history = term.history_size() as i64;
        let screen = term.screen_lines() as i64;
        let line = line as i64;
        // With the view held `d` lines above the bottom, the screen shows
        // absolute lines `history - d` down to `history + screen - 1 - d`,
        // so the offsets that show this one run from "on the bottom row" to "on
        // the top row", and the view is already right whenever it is in
        // that band.
        let on_the_bottom_row = history - line;
        let on_the_top_row = history + screen - 1 - line;
        let here = self.offset() as i64;
        let there = here
            .clamp(on_the_bottom_row, on_the_top_row.max(on_the_bottom_row))
            .clamp(0, history);
        if there != here {
            self.scroll(term, (there - here) as i32);
        }
    }

    pub fn page_up<L: EventListener>(&mut self, term: &mut Crosswords<L>) {
        self.glide = None;
        self.apply(term, Scroll::PageUp);
    }

    pub fn page_down<L: EventListener>(&mut self, term: &mut Crosswords<L>) {
        self.glide = None;
        self.apply(term, Scroll::PageDown);
    }

    pub fn to_top<L: EventListener>(&mut self, term: &mut Crosswords<L>) {
        self.glide = None;
        self.apply(term, Scroll::Top);
    }

    /// Straight to the live screen, in one step: what a keystroke does, and
    /// it is a jump on purpose, so the user sees that something changed.
    pub fn to_bottom<L: EventListener>(&mut self, term: &mut Crosswords<L>) {
        self.glide = None;
        self.apply(term, Scroll::Bottom);
    }

    /// Move the glide along to `now`. Returns true while it still runs.
    pub fn advance<L: EventListener>(&mut self, term: &mut Crosswords<L>, now: Instant) -> bool {
        let Some(glide) = self.glide else {
            return false;
        };
        let elapsed = now.saturating_duration_since(glide.started);
        let t = (elapsed.as_secs_f32() / WHEEL_GLIDE.as_secs_f32()).min(1.0);
        // Ease out: fast off the mark, settling into the destination, which
        // is where the eye wants to read.
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        let pos = glide.from + (glide.to - glide.from) * eased;
        if t >= 1.0 {
            self.glide = None;
            self.settle(term, glide.to);
            return false;
        }
        self.settle(term, pos);
        true
    }

    /// A whole-line move through rio-vt's own vocabulary (page, top,
    /// bottom): the terminal computes the destination and this reads it back.
    fn apply<L: EventListener>(&mut self, term: &mut Crosswords<L>, scroll: Scroll) {
        term.scroll_display(scroll);
        self.pos = term.grid.display_offset() as f32;
        self.follow = self.pos == 0.0;
    }

    /// Put the position at `pos`, clamped to the history that exists, and
    /// bring rio-vt's whole-line offset to `ceil(pos)`.
    fn settle<L: EventListener>(&mut self, term: &mut Crosswords<L>, pos: f32) {
        self.pos = clamp(pos, term);
        let want = lines_for(self.pos) as i32;
        let have = term.grid.display_offset() as i32;
        if want != have {
            term.scroll_display(Scroll::Delta(want - have));
        }
        // The terminal clamps for itself as well; its answer is the one that
        // stands, so a position past what it would hold is pulled back.
        let held = term.grid.display_offset();
        if held != lines_for(self.pos) {
            self.pos = held as f32;
        }
        self.follow = self.pos == 0.0;
    }

    /// Re-read the offset the terminal actually holds. Output written while
    /// the view is pinned to the bottom leaves the offset at zero; output
    /// written while it is scrolled up moves it, because the history under
    /// the view grew; a resize can shrink it. Either way the terminal is the
    /// authority and this is how the renderer hears about it: the shift is
    /// kept and the position re-derived from the lines the terminal holds.
    pub fn sync<L: EventListener>(&mut self, term: &Crosswords<L>) -> ViewportChange {
        let before = self.offset();
        let shift = self.shift();
        let held = term.grid.display_offset();
        let moved = held != before;
        if moved {
            self.pos = if held == 0 { 0.0 } else { held as f32 - shift };
            // History that grew under a scrolled view carried the lines it
            // shows up with it; a glide through them goes along.
            if let Some(glide) = self.glide.as_mut() {
                let carried = held as f32 - before as f32;
                glide.from += carried;
                glide.to = clamp(glide.to + carried, term);
            }
        }
        if held == 0 {
            self.follow = true;
        }
        ViewportChange { moved }
    }
}

/// A position no further back than the history rio-vt holds.
fn clamp<L: EventListener>(pos: f32, term: &Crosswords<L>) -> f32 {
    pos.clamp(0.0, term.history_size() as f32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportChange {
    /// The view is looking at different lines than last frame. Every row on
    /// screen is stale, whatever rio-vt's per-line damage says.
    pub moved: bool,
}
