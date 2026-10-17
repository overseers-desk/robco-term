//! The renderer's own idea of a screen, and the bridge that fills it from a
//! rio-vt grid.
//!
//! Two reasons the renderer does not read `Crosswords` directly. It has to be
//! drivable from a test with no terminal core in the picture (every pixel
//! property in this crate is stated about a fixed grid of characters), and the
//! `Square` -> colour resolution is a decision with a palette in it, which is
//! not the renderer's business.

use crate::color::{Rgba, Scheme};

/// What a cell renders as, after the palette has spoken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub c: char,
    pub fg: Rgba,
    pub bg: Rgba,
    pub underline: bool,
    pub strikeout: bool,
    /// Underline/strikeout colour; SGR 58 can differ from the text colour.
    pub line_color: Rgba,
}

impl Cell {
    pub fn blank(scheme: &Scheme) -> Self {
        Self {
            c: ' ',
            fg: scheme.foreground,
            bg: scheme.background,
            underline: false,
            strikeout: false,
            line_color: scheme.foreground,
        }
    }
}

/// A screen of cells in viewport order, row major, top row first.
#[derive(Clone, Debug)]
pub struct CellGrid {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
}

impl CellGrid {
    pub fn new(cols: usize, rows: usize, scheme: &Scheme) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(scheme); cols * rows],
        }
    }

    /// Build a grid from lines of text. The fixture constructor: the pixel
    /// properties are asserted about grids made this way.
    pub fn from_lines(lines: &[&str], cols: usize, rows: usize, scheme: &Scheme) -> Self {
        let mut grid = Self::new(cols, rows, scheme);
        for (row, line) in lines.iter().take(rows).enumerate() {
            for (col, c) in line.chars().take(cols).enumerate() {
                grid.cells[row * cols + col].c = c;
            }
        }
        grid
    }

    pub fn resize(&mut self, cols: usize, rows: usize, scheme: &Scheme) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells.resize(cols * rows, Cell::blank(scheme));
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    pub fn row_mut(&mut self, row: usize) -> &mut [Cell] {
        let (cols, start) = (self.cols, row * self.cols);
        &mut self.cells[start..start + cols]
    }

    /// Every distinct character on the screen. What the atlas has to hold.
    pub fn charset(&self) -> String {
        let mut v: Vec<char> = self.cells.iter().map(|c| c.c).collect();
        v.sort_unstable();
        v.dedup();
        v.into_iter().collect()
    }

    /// Row as text, for scripted grid tests and for the ASCII previews the
    /// pixel checks are read alongside.
    pub fn row_text(&self, row: usize) -> String {
        self.row(row).iter().map(|c| c.c).collect::<String>()
    }
}

/// Where the block cursor is, and what it looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorState {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
    pub color: Rgba,
    /// Colour the glyph under the cursor is redrawn in. A block cursor that
    /// does not do this eats the character it is sitting on.
    pub text_color: Rgba,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Hidden,
}

/// rio-vt -> `CellGrid`. Kept in one function so the `'\0'` normalisation and
/// the INVERSE/HIDDEN rules have exactly one home.
pub mod vt {
    use super::*;
    use rio_vt::crosswords::grid::Dimensions;
    use rio_vt::crosswords::pos::{Column, Line};
    use rio_vt::crosswords::square::Square;
    use rio_vt::crosswords::style::{Style, StyleFlags};
    use rio_vt::crosswords::Crosswords;
    use rio_vt::event::EventListener;

    use crate::grid::GridView;
    use crate::rio_grid::RioGrid;

    /// Convert one packed `Square` plus its interned style.
    pub fn cell_from_square(square: Square, style: Style, scheme: &Scheme) -> Cell {
        // Untouched cells read `'\0'`, not `' '`. Normalise here, which is
        // the only place a `Square` becomes a character.
        let c = match square.c() {
            '\0' => ' ',
            c => c,
        };

        // A bg-only cell carries its background inline and has no style id:
        // reading the style table for one would return the default style and
        // silently drop the colour the erase actually painted.
        if square.is_bg_only() {
            use rio_vt::crosswords::square::ContentTag;
            let bg = match square.content_tag() {
                ContentTag::BgPalette => scheme.palette[square.bg_palette_index() as usize],
                ContentTag::BgRgb => {
                    let (r, g, b) = square.bg_rgb();
                    crate::color::rgb(r, g, b)
                }
                _ => scheme.background,
            };
            return Cell {
                c: ' ',
                fg: scheme.foreground,
                bg,
                underline: false,
                strikeout: false,
                line_color: scheme.foreground,
            };
        }

        let bold = style.flags.contains(StyleFlags::BOLD);
        let styled_fg = if bold {
            crate::color::brightened(style.fg)
        } else {
            style.fg
        };
        let mut fg = scheme.resolve(styled_fg);
        let mut bg = scheme.resolve(style.bg);
        if style.flags.contains(StyleFlags::DIM) && !bold {
            fg = crate::color::dim(fg, scheme.dim_factor);
        }
        if style.flags.contains(StyleFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            // An inverted cell has to paint its background even when the
            // scheme's default background is transparent, or inverse video
            // renders as nothing at all.
            if bg[3] == 0.0 {
                bg = scheme.background;
                bg[3] = 1.0;
            }
            if fg[3] == 0.0 {
                fg = scheme.background;
                fg[3] = 1.0;
            }
        }
        // Whichever end of the cell the beam ends up drawing is the one
        // lifted, so an inverted cell's glyph is lifted and its plate is
        // not. A colour that weighs almost nothing on one phosphor -- blue
        // weighs four percent -- would otherwise be text nobody can read.
        fg = crate::color::lift(fg, scheme.foreground, crate::color::GLYPH_LIFT);
        let hidden = style.flags.contains(StyleFlags::HIDDEN);
        let line_color = style
            .underline_color
            .map(|c| {
                crate::color::lift(
                    scheme.resolve(c),
                    scheme.foreground,
                    crate::color::GLYPH_LIFT,
                )
            })
            .unwrap_or(fg);

        Cell {
            c: if hidden { ' ' } else { c },
            fg,
            bg,
            underline: style.flags.intersects(StyleFlags::ALL_UNDERLINES),
            strikeout: style.flags.contains(StyleFlags::STRIKEOUT),
            line_color,
        }
    }

    /// Copy one viewport row out of the terminal.
    ///
    /// `row` is a viewport row: 0 is the top of the visible screen. rio-vt's
    /// `Line` index is *not* viewport-relative: line 0 is the top of the live
    /// screen, and negative indices walk up into history, so the display
    /// offset has to be subtracted here. Leaving it out is silent: the screen
    /// renders correctly until the user scrolls, and then shows live output
    /// while claiming to show history.
    pub fn fill_row<L: EventListener>(
        term: &Crosswords<L>,
        row: usize,
        scheme: &Scheme,
        out: &mut [Cell],
    ) {
        let line = Line(row as i32 - term.display_offset() as i32);
        let columns = term.grid.columns().min(out.len());
        for col in 0..columns {
            let square = term.grid[line][Column(col)];
            let style = term.grid.style_set.get(square.style_id());
            out[col] = cell_from_square(square, style, scheme);
        }
        for cell in out.iter_mut().skip(columns) {
            *cell = Cell::blank(scheme);
        }
    }

    /// The whole *viewport* as text, trailing blanks trimmed.
    ///
    /// The viewport, not the live screen: scrolled back, this says what the
    /// user is looking at, which is the question a renderer asks.
    /// [`crate::rio_grid::live_text`] answers the other one.
    ///
    /// Both go through the same grid seam, so `'\0'` becomes `' '` in
    /// exactly one place ([`GridView::cell`]) rather than once per caller.
    pub fn viewport_text<L: EventListener>(term: &Crosswords<L>) -> Vec<String> {
        let grid = RioGrid::new(term);
        // A viewport row is an absolute line once history and the display
        // offset are both accounted for.
        let base = term.history_size() as isize - term.display_offset() as isize;
        (0..grid.screen_lines())
            .map(|row| {
                let absolute = (base + row as isize).max(0) as usize;
                let mut s = String::with_capacity(grid.columns());
                for column in 0..grid.columns() {
                    s.push(grid.cell(absolute, column));
                }
                s.trim_end().to_string()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_lines_pads_and_clips() {
        let scheme = Scheme::default();
        let grid = CellGrid::from_lines(&["AB", "CDEFG"], 4, 3, &scheme);
        assert_eq!(grid.row_text(0), "AB  ");
        assert_eq!(grid.row_text(1), "CDEF");
        assert_eq!(grid.row_text(2), "    ");
    }

    #[test]
    fn blank_cell_uses_the_scheme_background() {
        let scheme = Scheme::monochrome([1.0, 0.5, 0.0, 1.0], crate::color::TRANSPARENT);
        let cell = Cell::blank(&scheme);
        assert_eq!(cell.bg, crate::color::TRANSPARENT);
        assert_eq!(cell.fg, [1.0, 0.5, 0.0, 1.0]);
    }
}
