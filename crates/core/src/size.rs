//! Geometry: how a window's pixels become a grid of cells.
//!
//! Three quantities decide the grid, and they change independently:
//! the window's size in *physical* pixels, the display's scale factor
//! (DPR), and the cell size the font gives us in *logical* pixels. A DPR
//! change moves only the middle one, but it moves the cell's physical
//! size with it, so the column count changes without the window ever
//! being resized. Keeping all three in one type is what lets a DPR
//! change and a resize run through the same code path.

use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::Side;

/// The cell box a font hands us, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
}

impl CellSize {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// A grid geometry: cell counts plus the physical cell box behind them.
///
/// `rio-vt` asks for `Dimensions`, which is only the three counts. The
/// pixel fields ride along because the PTY's `TIOCSWINSZ` wants them too
/// (image protocols on the child's side read them), and because keeping
/// them together stops the two from drifting apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermSize {
    cols: usize,
    rows: usize,
    /// Cell width in physical pixels.
    pub cell_width: u16,
    /// Cell height in physical pixels.
    pub cell_height: u16,
}

/// Below this a grid stops being a terminal; `sh` and `tput` misbehave
/// on a zero-column screen, and rio-vt's resize divides by these.
const MIN_COLS: usize = 2;
const MIN_ROWS: usize = 1;

/// The grid the window is never dragged under: the 80x24 an ANSI terminal
/// is specified in, and the size a curses program that never asks
/// `TIOCGWINSZ` draws for. [`Viewport::well_minimum`] turns it into the pixels
/// the window's minimum-size hint is set from.
pub const FLOOR_COLS: usize = 80;
pub const FLOOR_ROWS: usize = 24;

impl TermSize {
    /// Build directly from cell counts. Used by tests and by any caller
    /// that already knows the grid it wants.
    pub fn new(cols: usize, rows: usize, cell_width: u16, cell_height: u16) -> Self {
        Self {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Total grid size in physical pixels, which is generally a little
    /// less than the window: the remainder of the division is padding
    /// the renderer distributes, not cells.
    pub fn pixel_size(&self) -> (u16, u16) {
        (
            (self.cols as u16).saturating_mul(self.cell_width),
            (self.rows as u16).saturating_mul(self.cell_height),
        )
    }

    /// The column a point `x` physical pixels from the grid's left edge falls
    /// in, and which half of that column it is on.
    ///
    /// The inverse of the placement the renderer draws with. It lives here
    /// because this type holds what it needs, the column count and the
    /// physical cell box, and because the caller has already taken the point
    /// out of the window and into the grid.
    ///
    /// The side names the seam between two columns, which is what the rio
    /// selection model anchors on: a drag begun on the right half of a
    /// character leaves that character behind. It is read from the unclamped
    /// point, so a press beyond the last column still reports which way it
    /// was heading.
    pub fn column_side_at(&self, x: f64) -> (usize, Side) {
        let cell = f64::from(self.cell_width).max(1.0);
        let last = self.cols.saturating_sub(1) as f64;
        let column = (x / cell).floor().clamp(0.0, last) as usize;
        let side = if (x / cell).fract() >= 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (column, side)
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The window's side of the geometry: physical pixels, the scale factor
/// winit reports, and the font's logical cell.
///
/// This is the type the app mutates on `Resized` and on
/// `ScaleFactorChanged`; `term_size()` is the single place either event
/// turns into a grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Window width in physical pixels.
    pub width: u32,
    /// Window height in physical pixels.
    pub height: u32,
    /// Device pixel ratio.
    pub scale_factor: f64,
    /// Cell box in logical pixels.
    pub cell: CellSize,
    /// The grid's inset from each edge of the well, in **physical** pixels:
    /// `app::settings::distortion_margin`'s logical-pixel value scaled by
    /// `scale_factor` at this boundary, the same way [`Viewport::physical_cell`]
    /// turns the logical cell into a physical one. Zero (the default a bare
    /// [`Viewport::new`] leaves it at) reproduces the pre-inset behavior a
    /// caller that never sets it -- every test below and every headless
    /// surface that never attaches a settings handle -- still gets.
    pub margin: f64,
}

impl Viewport {
    pub fn new(width: u32, height: u32, scale_factor: f64, cell: CellSize) -> Self {
        Self {
            width,
            height,
            scale_factor,
            cell,
            margin: 0.0,
        }
    }

    /// The cell box in physical pixels: the logical cell scaled by DPR.
    ///
    /// Rounded, not truncated: at DPR 1.5 a 9-pixel cell is 13.5 physical
    /// pixels, and truncating biases every cell smaller, which compounds
    /// into an extra column or two across a wide window.
    pub fn physical_cell(&self) -> (u16, u16) {
        let w = (f64::from(self.cell.width) * self.scale_factor).round();
        let h = (f64::from(self.cell.height) * self.scale_factor).round();
        (w.max(1.0) as u16, h.max(1.0) as u16)
    }

    /// The grid this viewport implies: the well, inset by [`Viewport::margin`]
    /// on every edge, divided by the physical cell.
    ///
    /// The inset (`2 * margin`) is subtracted from both edges before
    /// deriving the grid's own size from what is left; the division that
    /// follows is what already turned a window a few pixels wider than the
    /// grid into "the remainder is padding, not cells", and the margin is
    /// now folded into that same remainder rather than sized separately.
    pub fn term_size(&self) -> TermSize {
        let (cw, ch) = self.physical_cell();
        let inset = self.inset();
        let available_width = self.width.saturating_sub(inset);
        let available_height = self.height.saturating_sub(inset);
        TermSize::new(
            available_width as usize / cw as usize,
            available_height as usize / ch as usize,
            cw,
            ch,
        )
    }

    /// The well that holds exactly [`FLOOR_COLS`] x [`FLOOR_ROWS`] cells, in
    /// physical pixels: [`Viewport::term_size`] read backwards, so a well of
    /// this size divides into that grid and one pixel less does not.
    ///
    /// Physical rather than logical because that is the unit the division is
    /// exact in. The cell is rounded to whole physical pixels before anything
    /// is counted (`physical_cell`), and eighty of those rounded cells is not
    /// eighty logical cells scaled: at DPR 1.1 a 9-pixel cell is drawn 10
    /// physical pixels wide, so the eighty need 800 and the logical floor
    /// scaled would have promised 792 and delivered seventy-nine columns.
    pub fn well_minimum(&self) -> (u32, u32) {
        let (cw, ch) = self.physical_cell();
        let inset = self.inset();
        (
            FLOOR_COLS as u32 * u32::from(cw) + inset,
            FLOOR_ROWS as u32 * u32::from(ch) + inset,
        )
    }

    /// The margin taken off both edges of an axis, in physical pixels.
    fn inset(&self) -> u32 {
        (2.0 * self.margin).round().max(0.0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_divides_the_window_by_the_cell() {
        let v = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
        let s = v.term_size();
        assert_eq!((s.cols(), s.rows()), (80, 24));
        assert_eq!(s.pixel_size(), (800, 480));
    }

    #[test]
    fn a_column_comes_back_from_the_pixel_it_is_drawn_at() {
        // The renderer places column `col` at `col * cell_width`. Feeding
        // that x back has to name `col` again, or a press lands on a
        // different character than the one under the pointer.
        let s = TermSize::new(80, 24, 9, 18);
        for col in 0..s.cols() {
            let x = (col * 9) as f64;
            assert_eq!(s.column_side_at(x), (col, Side::Left), "column {col}");
            assert_eq!(s.column_side_at(x + 5.0), (col, Side::Right), "column {col}");
        }
        // A press past the last column belongs to the last column.
        assert_eq!(s.column_side_at(10_000.0).0, 79);
    }

    #[test]
    fn a_dpr_change_halves_the_grid_without_touching_the_window() {
        // Same window, twice the pixel density: cells are physically
        // twice as big, so half as many fit. This is the case that has
        // no `Resized` event behind it.
        let v = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
        let doubled = Viewport {
            scale_factor: 2.0,
            ..v
        };
        assert_eq!((v.term_size().cols(), v.term_size().rows()), (80, 24));
        assert_eq!(
            (doubled.term_size().cols(), doubled.term_size().rows()),
            (40, 12)
        );
    }

    #[test]
    fn fractional_dpr_rounds_the_cell_rather_than_truncating() {
        let v = Viewport::new(800, 480, 1.5, CellSize::new(9.0, 20.0));
        // 9 * 1.5 = 13.5 -> 14, not 13.
        assert_eq!(v.physical_cell(), (14, 30));
        assert_eq!(v.term_size().cols(), 800 / 14);
    }

    #[test]
    fn the_well_minimum_is_the_least_well_holding_eighty_by_twenty_four() {
        // Every combination of a cell that rounds and a margin that is not a
        // whole number of pixels: the floor holds the grid, and a pixel off
        // either axis loses a column or a row. That pair is what makes it a
        // floor rather than an estimate of one.
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for cell in [(9.0, 20.0), (12.0, 28.0), (6.0, 13.0)] {
                for margin in [0.0, 7.3, 28.820842763492422] {
                    let mut v = Viewport::new(0, 0, scale, CellSize::new(cell.0, cell.1));
                    v.margin = margin * scale;
                    let (w, h) = v.well_minimum();
                    let at_floor = Viewport {
                        width: w,
                        height: h,
                        ..v
                    };
                    let grid = at_floor.term_size();
                    assert_eq!(
                        (grid.cols(), grid.rows()),
                        (FLOOR_COLS, FLOOR_ROWS),
                        "at floor {w}x{h}, scale {scale}, cell {cell:?}, margin {margin}"
                    );
                    for narrower in [
                        Viewport {
                            width: w - 1,
                            ..at_floor
                        },
                        Viewport {
                            height: h - 1,
                            ..at_floor
                        },
                    ] {
                        let grid = narrower.term_size();
                        assert!(
                            grid.cols() < FLOOR_COLS || grid.rows() < FLOOR_ROWS,
                            "{}x{} still holds the floor grid",
                            narrower.width,
                            narrower.height
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_window_too_small_for_one_cell_still_yields_a_usable_grid() {
        let v = Viewport::new(4, 4, 1.0, CellSize::new(10.0, 20.0));
        let s = v.term_size();
        assert!(s.cols() >= MIN_COLS && s.rows() >= MIN_ROWS);
    }
}
