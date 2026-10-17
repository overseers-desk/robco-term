//! The grid seam: the two grid-to-text paths everything above it shares.
//!
//! Small on purpose. What is asserted here is the pair of facts the rest of
//! the crate assumes without restating: an untouched cell reads as a space
//! and is nonetheless not part of the line, and a double-width character
//! occupies two cells but produces one.

use term::grid::{write_range, GridView, ScriptedGrid};

const COLS: usize = 40;

#[test]
fn untouched_cells_are_not_part_of_a_line() {
    let g = ScriptedGrid::new(COLS, &["ab", ""]);
    assert_eq!(g.line_len(0), 2);
    assert_eq!(g.line_len(1), 0);
    // Reading past the written part still yields a space, never a NUL: the
    // renderer asks for every cell whether it was written or not.
    assert_eq!(g.cell(0, 30), ' ');
    // Copying past it yields no *text* (not 16 spaces), but it does yield
    // the line break, because a selection that ran past the end of a line
    // means the user selected the newline after it.
    assert_eq!(write_range(&g, 5, 20, false, true), "\n");
}

#[test]
fn double_width_characters_do_not_double_on_the_way_out() {
    // The spacer cell after a wide character is skipped by width, not by a
    // flag; dropping the step is how a CJK selection comes back doubled.
    let g = ScriptedGrid::new(COLS, &["漢字ok"]);
    assert_eq!(write_range(&g, 0, 5, false, true), "漢字ok");
}
