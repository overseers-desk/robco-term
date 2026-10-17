//! The same features, over a real rio-vt grid instead of a scripted one.
//!
//! The scripted tests prove the logic; this one proves the seam: that
//! `RioGrid` presents a `Crosswords` in the coordinates the logic expects,
//! and in particular that the packed-`Square` NUL never leaks through it.
//! No PTY: bytes go straight into the parser, which is enough to fill a grid
//! and to push lines into scrollback.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use term::grid::GridView;
use term::hotspots::UrlFilterChain;
use term::rio_grid::RioGrid;
use term::selection::konsole::Konsole;
use term::selection::Window;

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
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

fn term_with(bytes: &[u8], cols: usize, rows: usize) -> Crosswords<VoidListener> {
    let mut term = Crosswords::<VoidListener>::new(
        Size { cols, rows },
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        1000,
    );
    let mut processor = Processor::default();
    processor.advance(&mut term, bytes);
    term
}

#[test]
fn untouched_cells_never_reach_a_text_path_as_nul() {
    let term = term_with(b"ab\r\n", 20, 4);
    let g = RioGrid::new(&term);

    assert_eq!(g.line_len(0), 2, "two cells were written");
    assert_eq!(g.cell(0, 10), ' ', "and the rest read as spaces, not NULs");
    assert!(
        !term::selection::line_text(&g, 0).contains('\0'),
        "no NUL survives the decoder"
    );
    assert_eq!(term::selection::line_text(&g, 0), "ab");
}

#[test]
fn a_selection_over_a_live_grid_copies_what_is_on_it() {
    let term = term_with(b"hello world\r\nsecond line\r\n", 40, 6);
    let g = RioGrid::new(&term);
    let mut c = Konsole::new(g.columns());
    let w = Window {
        top_line: 0,
        lines: g.total_lines(),
        columns: g.columns(),
    };

    c.press(0, 0);
    c.drag_to(&g, w, 5, 0);
    assert_eq!(c.release(&g).as_deref(), Some("hello"));

    assert_eq!(c.double_click(&g, w, 8, 0).as_deref(), Some("world"));
}

#[test]
fn hotspots_run_over_a_live_grid() {
    let term = term_with(b"visit https://example.com now\r\n", 40, 4);
    let g = RioGrid::new(&term);
    let mut chain = UrlFilterChain::with_url_filter();
    chain.set_image(&g, 0, g.total_lines());

    let spots = chain.hot_spots();
    assert_eq!(spots.len(), 1);
    assert_eq!(spots[0].text(), "https://example.com");
    assert_eq!((spots[0].start_line, spots[0].start_column), (0, 6));
    assert!(chain.hot_spot_at(0, 10).is_some());
}
