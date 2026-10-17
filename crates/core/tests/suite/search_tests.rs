//! Scrollback search, over a real `Crosswords`.
//!
//! The search itself is rio-vt's `Crosswords::search_next`, and this crate
//! owns none of it. What is pinned down here is the contract the find line
//! in `crates/app` is written against, because that contract is the whole
//! of what the feature stands on: which hit "next" means from an origin,
//! that the walk wraps once rather than stopping at the end, that a miss is
//! `None` and not the nearest thing, that `(?i)` really does reach the DFA
//! builder, and that a match found in history comes back at the negative
//! `Line` the grid addresses it by.
//!
//! Coordinates here are rio-vt's own: `Line(0)` is the top of the screen
//! and history runs negative. `crates/app/src/find.rs` is where they become
//! the absolute lines everything above [`term::grid`] speaks.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::{Column, Direction, Line, Pos, Side};
use rio_vt::crosswords::search::RegexSearch;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

const COLS: usize = 40;
const ROWS: usize = 4;

#[derive(Clone, Copy)]
struct Size;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        ROWS
    }
    fn screen_lines(&self) -> usize {
        ROWS
    }
    fn columns(&self) -> usize {
        COLS
    }
}

fn term_with(lines: &[&str]) -> Crosswords<VoidListener> {
    let mut term = Crosswords::<VoidListener>::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        1000,
    );
    let mut processor = Processor::default();
    for line in lines {
        processor.advance(&mut term, line.as_bytes());
        processor.advance(&mut term, b"\r\n");
    }
    term
}

/// The pattern the find line builds: the typed text, literally, and case
/// folded whether or not the user shifted anything.
fn literal(query: &str) -> RegexSearch {
    RegexSearch::new(&format!("(?i){}", regex::escape(query))).expect("the query compiles")
}

fn at(row: i32, col: usize) -> Pos {
    Pos::new(Line(row), Column(col))
}

/// Forward from an origin, the way Enter asks: the near end of the hit is
/// what is compared against the origin, so `Side::Left`.
fn next(
    term: &Crosswords<VoidListener>,
    regex: &mut RegexSearch,
    from: Pos,
) -> Option<(i32, usize)> {
    let hit = term.search_next(regex, from, Direction::Right, Side::Left, None)?;
    Some((hit.start().row.0, hit.start().col.0))
}

/// Backward, the way Shift+Enter asks.
fn previous(
    term: &Crosswords<VoidListener>,
    regex: &mut RegexSearch,
    from: Pos,
) -> Option<(i32, usize)> {
    let hit = term.search_next(regex, from, Direction::Left, Side::Right, None)?;
    Some((hit.start().row.0, hit.start().col.0))
}

#[test]
fn enter_walks_the_hits_forward_and_shift_enter_walks_them_back() {
    let term = term_with(&["alpha beta", "gamma alpha", "alpha delta"]);
    let mut regex = literal("alpha");

    assert_eq!(next(&term, &mut regex, at(0, 0)), Some((0, 0)));
    assert_eq!(next(&term, &mut regex, at(0, 1)), Some((1, 6)));
    assert_eq!(next(&term, &mut regex, at(1, 7)), Some((2, 0)));

    assert_eq!(previous(&term, &mut regex, at(2, 0)), Some((1, 6)));
    assert_eq!(previous(&term, &mut regex, at(1, 0)), Some((0, 0)));

    // The origin is inside the search either way, which is why the find
    // line steps one cell past the hit it is on before asking for the next
    // one: from the last cell of a hit, that same hit is the answer.
    assert_eq!(previous(&term, &mut regex, at(2, 4)), Some((2, 0)));
}

#[test]
fn the_walk_wraps_once_at_either_end() {
    let term = term_with(&["alpha beta", "gamma alpha"]);
    let mut regex = literal("alpha");

    assert_eq!(
        next(&term, &mut regex, at(1, 7)),
        Some((0, 0)),
        "past the last hit is the first one again"
    );
    assert_eq!(
        previous(&term, &mut regex, at(0, 0)),
        Some((1, 6)),
        "and before the first is the last"
    );
}

#[test]
fn a_query_that_is_not_there_is_a_miss_and_not_the_nearest_thing() {
    let term = term_with(&["alpha beta", "gamma delta"]);
    let mut regex = literal("epsilon");

    assert_eq!(next(&term, &mut regex, at(0, 0)), None);
    assert_eq!(previous(&term, &mut regex, at(1, 0)), None);
}

#[test]
fn the_query_is_folded_for_case_and_taken_literally() {
    let term = term_with(&["Segmentation fault", "a.b anb"]);

    let mut shouted = literal("SEGMENTATION");
    assert_eq!(next(&term, &mut shouted, at(0, 0)), Some((0, 0)));
    let mut whispered = literal("fault");
    assert_eq!(next(&term, &mut whispered, at(0, 0)), Some((0, 13)));

    // `.` is a full stop the user typed, not a wildcard, so it matches
    // `a.b` and never `anb`.
    let mut dotted = literal("a.b");
    assert_eq!(next(&term, &mut dotted, at(1, 0)), Some((1, 0)));
    assert_eq!(
        next(&term, &mut dotted, at(1, 1)),
        Some((1, 0)),
        "the only hit on the row, come back to by the wrap"
    );
}

#[test]
fn a_hit_in_the_scrollback_comes_back_at_its_negative_line() {
    // Twelve lines through a four-row screen: the first eight are in
    // history, reachable only through the negative `Line` indices.
    let lines: Vec<String> = (0..12).map(|i| format!("line-{i:02}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let term = term_with(&refs);
    assert!(term.history_size() >= 8, "lines did scroll off");

    let mut regex = literal("line-00");
    let hit = next(&term, &mut regex, at(0, 0)).expect("the oldest line is still findable");
    assert_eq!(hit.1, 0);
    assert_eq!(
        hit.0 + term.history_size() as i32,
        0,
        "and it is the oldest line the grid still holds"
    );
}
