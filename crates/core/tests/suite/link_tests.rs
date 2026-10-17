//! The link under a cell: an OSC 8 hyperlink the program declared, or a URL
//! the filters matched, each with the exact cells it occupies.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use term::hotspots::UrlFilterChain;
use term::links::link_at;
use term::selection::MarkedRange;
use term::viewport_text;

const COLS: usize = 40;
const ROWS: usize = 6;

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

/// A terminal with `bytes` written to it.
fn written(bytes: &[u8]) -> Crosswords<VoidListener> {
    let mut term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        200,
    );
    let mut processor = Processor::default();
    processor.advance(&mut term, bytes);
    term
}

fn run(from: (usize, usize), to: (usize, usize)) -> MarkedRange {
    MarkedRange {
        start: from,
        end: to,
        block: false,
    }
}

/// The link under `(column, row)` of the visible screen.
fn link(
    term: &Crosswords<VoidListener>,
    column: usize,
    row: usize,
) -> Option<(MarkedRange, String)> {
    let top = term.history_size() - term.display_offset();
    let mut chain = UrlFilterChain::with_url_filter();
    link_at(&mut chain, term, top, (column, top + row))
}

const OPEN: &str = "\x1b]8;;https://example.org\x1b\\";
const CLOSE: &str = "\x1b]8;;\x1b\\";

#[test]
fn a_declared_link_covers_its_run_and_ends_where_the_program_ended_it() {
    let term = written(format!("{OPEN}click me{CLOSE} tail\r\n").as_bytes());
    let expected = Some((run((0, 0), (7, 0)), "https://example.org".to_string()));
    assert_eq!(link(&term, 0, 0), expected);
    assert_eq!(link(&term, 5, 0), expected);
    assert_eq!(link(&term, 7, 0), expected);
    assert_eq!(link(&term, 8, 0), None, "the space after the run");
    assert_eq!(link(&term, 10, 0), None, "plain text");
}

#[test]
fn two_runs_sharing_an_id_are_one_link_with_the_span_of_the_run_pointed_at() {
    let open = "\x1b]8;id=x;https://a.example\x1b\\";
    let term = written(format!("{open}AAA{CLOSE} {open}BBB{CLOSE}\r\n").as_bytes());
    let uri = "https://a.example".to_string();
    assert_eq!(link(&term, 1, 0), Some((run((0, 0), (2, 0)), uri.clone())));
    assert_eq!(link(&term, 5, 0), Some((run((4, 0), (6, 0)), uri)));
    assert_eq!(link(&term, 3, 0), None, "the gap between the runs");
}

#[test]
fn a_declared_link_that_wraps_continues_on_the_row_below() {
    let text = "x".repeat(COLS + 5);
    let term = written(format!("{OPEN}{text}{CLOSE}\r\n").as_bytes());
    let expected = Some((run((0, 0), (4, 1)), "https://example.org".to_string()));
    assert_eq!(link(&term, COLS - 1, 0), expected);
    assert_eq!(link(&term, 3, 1), expected);
    assert_eq!(link(&term, 5, 1), None);
}

#[test]
fn a_declared_link_broken_by_the_program_keeps_to_its_row() {
    // A full row and a newline: the run reaches the right edge, and the
    // program, not the width, started the next line.
    let text = "y".repeat(COLS);
    let term = written(format!("{OPEN}{text}\r\n{text}{CLOSE}\r\n").as_bytes());
    assert_eq!(
        link(&term, 10, 0).map(|l| l.0),
        Some(run((0, 0), (COLS - 1, 0)))
    );
    assert_eq!(
        link(&term, 10, 1).map(|l| l.0),
        Some(run((0, 1), (COLS - 1, 1)))
    );
}

#[test]
fn a_matched_url_resolves_under_a_scrolled_top_line_and_stops_at_its_last_character() {
    // Enough blank lines first that the screen has history above it and the
    // top line is not zero.
    let term = written(b"\r\n\r\n\r\n\r\n\r\n\r\n\r\nsee https://example.com now\r\n");
    let top = term.history_size();
    assert!(top > 0, "the screen should have scrolled");
    let row = viewport_text(&term)
        .iter()
        .position(|l| l.contains("example.com"))
        .expect("the URL is on screen");
    let expected = Some((
        run((4, top + row), (22, top + row)),
        "https://example.com".to_string(),
    ));
    assert_eq!(link(&term, 4, row), expected);
    assert_eq!(link(&term, 22, row), expected);
    assert_eq!(link(&term, 23, row), None, "the space after the URL");
    assert_eq!(link(&term, 1, row), None, "plain text before it");
}
