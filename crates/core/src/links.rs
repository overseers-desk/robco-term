//! The link under a cell.
//!
//! Two kinds of link share the glass. A program names one outright with
//! OSC 8, and rio-vt keeps that URI on every cell the run covers; anything
//! else is a URL or an e-mail address the [`crate::hotspots`] filters
//! recognise in the text. [`link_at`] answers for both, the declared link
//! first, and hands back the cells the link occupies together with what
//! opening it means.
//!
//! The filters run over the viewport on every call. The pointer asks once
//! per cell crossing while the opening chord is held, and once per press; a
//! regex pass over a screen of text costs less than the frame drawn after
//! it, so nothing here remembers an earlier answer or has to notice that the
//! screen moved under it.

use rio_vt::crosswords::pos::{Column, Line};
use rio_vt::crosswords::Crosswords;
use rio_vt::event::EventListener;

use crate::grid::GridView;
use crate::hotspots::UrlFilterChain;
use crate::rio_grid::RioGrid;
use crate::selection::MarkedRange;

/// The link under `cell`, given as `(column, absolute line)` the way the
/// pointer locates a cell, with `top_line` the absolute line the top row of
/// the screen shows. The range is inclusive at both ends like a selection
/// and covers exactly the link's cells; the string is what opening the link
/// means: an OSC 8 URI as the program gave it, `http://` supplied for a bare
/// `www.` address, `mailto:` for an e-mail address.
pub fn link_at<L: EventListener>(
    chain: &mut UrlFilterChain,
    term: &Crosswords<L>,
    top_line: usize,
    cell: (usize, usize),
) -> Option<(MarkedRange, String)> {
    let grid = RioGrid::new(term);
    let (column, line) = cell;
    if column >= grid.columns() || line < top_line || line - top_line >= grid.screen_lines() {
        return None;
    }
    declared(term, &grid, cell).or_else(|| matched(chain, &grid, top_line, cell))
}

/// rio-vt numbers lines from the top of the live screen, history negative.
fn rio_line<L: EventListener>(term: &Crosswords<L>, line: usize) -> Line {
    Line(line as i32 - term.history_size() as i32)
}

/// The hyperlink id rio-vt keeps on a cell, by `(column, absolute line)`.
fn hyperlink_id<L: EventListener>(term: &Crosswords<L>, cell: (usize, usize)) -> Option<u16> {
    term.cell_hyperlink_id(rio_line(term, cell.1), Column(cell.0))
}

/// An OSC 8 hyperlink: the run of cells around `cell` carrying the same id,
/// continued across a row boundary only where the row wrapped, since a
/// program that wrote a link and then a newline ended the run itself.
fn declared<L: EventListener>(
    term: &Crosswords<L>,
    grid: &RioGrid<'_, L>,
    cell: (usize, usize),
) -> Option<(MarkedRange, String)> {
    let id = hyperlink_id(term, cell)?;
    let uri = term
        .cell_hyperlink(rio_line(term, cell.1), Column(cell.0))?
        .uri()
        .to_string();
    let last = grid.columns() - 1;
    let same = |c: (usize, usize)| hyperlink_id(term, c) == Some(id);

    let mut start = cell;
    loop {
        let before = if start.0 > 0 {
            (start.0 - 1, start.1)
        } else if start.1 > 0 && grid.is_wrapped(start.1 - 1) {
            (last, start.1 - 1)
        } else {
            break;
        };
        if !same(before) {
            break;
        }
        start = before;
    }
    let mut end = cell;
    loop {
        let after = if end.0 < last {
            (end.0 + 1, end.1)
        } else if grid.is_wrapped(end.1) && end.1 + 1 < grid.total_lines() {
            (0, end.1 + 1)
        } else {
            break;
        };
        if !same(after) {
            break;
        }
        end = after;
    }
    Some((
        MarkedRange {
            start,
            end,
            block: false,
        },
        uri,
    ))
}

/// A URL or e-mail address the filters match in the visible text.
fn matched<L: EventListener>(
    chain: &mut UrlFilterChain,
    grid: &RioGrid<'_, L>,
    top_line: usize,
    (column, line): (usize, usize),
) -> Option<(MarkedRange, String)> {
    chain.set_image(grid, top_line, grid.screen_lines());
    let row = line - top_line;
    let spot = chain.hot_spot_at(row, column)?;
    // `end_column` is one past the last character, and `hot_spot_at` admits
    // that cell so a click on a boundary still hits; a link being pointed at
    // ends where its text does.
    if row == spot.end_line && column >= spot.end_column {
        return None;
    }
    let url = spot.activation_url()?;
    let start = (spot.start_column, top_line + spot.start_line);
    // A match that ends flush with a wrapped row's right edge is located at
    // column zero of the row below; the link's last cell is the one before.
    let end = if spot.end_column == 0 {
        (
            grid.columns() - 1,
            (top_line + spot.end_line).checked_sub(1)?,
        )
    } else {
        (spot.end_column - 1, top_line + spot.end_line)
    };
    Some((
        MarkedRange {
            start,
            end,
            block: false,
        },
        url,
    ))
}
