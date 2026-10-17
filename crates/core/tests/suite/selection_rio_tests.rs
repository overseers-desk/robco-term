//! The rio selection model, over a real `Crosswords`.
//!
//! Scripted grids are enough for Konsole's arithmetic, and not enough here:
//! this arm keeps its state where rio-vt keeps its own, and asks the
//! emulation core the questions a word and a line boundary are. So the grid
//! is the real one, filled by the real parser, the way `rio_grid_tests.rs`
//! and `selection_paint.rs` already fill theirs.
//!
//! What is pinned down:
//!
//! * a press points at the **seam between two cells**, so the half of the
//!   cell the pointer is on decides whether that character is in;
//! * a double click takes a **semantic** word, by rio's separators: `@` is
//!   part of a word and `:` is not, so `user@host` comes whole and
//!   `key:value` splits;
//! * a double click on a bracket takes everything up to **its partner**;
//! * a triple click takes the **logical line** and the newline that ends it;
//! * `Ctrl`+`Alt` drags a **rectangle**;
//! * the range the renderer paints covers exactly the text the gesture
//!   copied.
//!
//! The last section puts one plain character drag through both models and
//! asks them for the same picture: whatever else the two disagree about, a
//! left-to-right drag across a word is the gesture everybody makes.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::Side;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use term::pointer::Modifiers;
use term::selection::{Gesture, Kind, MarkedRange, SelectionModel, Window};

const COLS: usize = 40;
const ROWS: usize = 8;

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

fn window(term: &Crosswords<VoidListener>) -> Window {
    Window {
        top_line: term.history_size(),
        lines: ROWS,
        columns: COLS,
    }
}

/// A screen row as an absolute line index, which is what `SelectionModel` speaks.
fn line(term: &Crosswords<VoidListener>, row: usize) -> usize {
    term.history_size() + row
}

fn ctrl_alt() -> Modifiers {
    Modifiers {
        control: true,
        alt: true,
        ..Modifiers::NONE
    }
}

/// A press, a drag and a release, in cells and sides. What comes back is
/// what the gesture put on the primary selection.
fn drag(
    model: &mut SelectionModel,
    term: &mut Crosswords<VoidListener>,
    from: ((usize, usize), Side),
    to: ((usize, usize), Side),
    mods: Modifiers,
) -> Option<String> {
    let win = window(term);
    model.press(
        Gesture {
            term,
            win,
            side: from.1,
        },
        from.0,
        mods,
    );
    model.drag_to(
        Gesture {
            term,
            win,
            side: to.1,
        },
        to.0,
    );
    model.release(Gesture {
        term,
        win,
        side: to.1,
    })
}

fn click(
    model: &mut SelectionModel,
    term: &mut Crosswords<VoidListener>,
    cell: (usize, usize),
    clicks: u8,
) -> Option<String> {
    let win = window(term);
    let gesture = Gesture {
        term,
        win,
        side: Side::Left,
    };
    match clicks {
        2 => model.double_click(gesture, cell),
        _ => model.triple_click(gesture, cell),
    }
}

/// A press on the left half of a cell takes that cell; a press on its right
/// half starts after it. That is the whole difference between the two
/// models' idea of what a click points at.
#[test]
fn the_half_of_the_cell_decides_whether_its_character_is_in() {
    let mut term = term_with(&["hello world"]);
    let row = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    let whole = drag(
        &mut model,
        &mut term,
        ((0, row), Side::Left),
        ((5, row), Side::Left),
        Modifiers::NONE,
    );
    assert_eq!(whole.as_deref(), Some("hello"));

    let mut model = SelectionModel::new(Kind::Rio, COLS);
    let trimmed = drag(
        &mut model,
        &mut term,
        ((0, row), Side::Right),
        ((5, row), Side::Left),
        Modifiers::NONE,
    );
    assert_eq!(
        trimmed.as_deref(),
        Some("ello"),
        "a drag begun on the right half of the h starts after it"
    );
}

/// rio's word separators, which are not Konsole's: `@` is inside a word and
/// `:` ends one.
#[test]
fn a_double_click_takes_a_semantic_word() {
    let mut term = term_with(&["user@host key:value"]);
    let row = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    assert_eq!(
        click(&mut model, &mut term, (2, row), 2).as_deref(),
        Some("user@host"),
        "an at sign is part of the word"
    );
    assert_eq!(
        click(&mut model, &mut term, (11, row), 2).as_deref(),
        Some("key"),
        "a colon ends one"
    );
    assert_eq!(
        click(&mut model, &mut term, (15, row), 2).as_deref(),
        Some("value")
    );
}

/// A double click on a bracket takes it and its partner and everything
/// between, which is what a shell command's argument list is.
#[test]
fn a_double_click_on_a_bracket_takes_its_partner() {
    let mut term = term_with(&["call(a, b) done"]);
    let row = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    assert_eq!(
        click(&mut model, &mut term, (4, row), 2).as_deref(),
        Some("(a, b)")
    );
    assert_eq!(
        click(&mut model, &mut term, (9, row), 2).as_deref(),
        Some("(a, b)"),
        "and from the closing bracket back"
    );
}

/// A triple click takes the logical line, and the newline that ends it comes
/// with it: pasting three lines somewhere should put three lines there.
#[test]
fn a_triple_click_takes_the_line_and_its_newline() {
    let mut term = term_with(&["first line", "second line"]);
    let row = line(&term, 1);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    assert_eq!(
        click(&mut model, &mut term, (3, row), 3).as_deref(),
        Some("second line\n")
    );
}

/// `Ctrl`+`Alt` drags a rectangle rather than a run of lines: the columns
/// the drag crossed, on every row it crossed, and nothing outside them.
#[test]
fn ctrl_alt_drags_a_rectangle() {
    let mut term = term_with(&["abcdefgh", "ijklmnop", "qrstuvwx"]);
    let top = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    let text = drag(
        &mut model,
        &mut term,
        ((2, top), Side::Left),
        ((4, top + 2), Side::Right),
        ctrl_alt(),
    );
    assert_eq!(text.as_deref(), Some("cde\nklm\nstu"));

    let range = model.range(&term).expect("a rectangle is marked");
    assert!(range.block, "the marked range is a rectangle");
    assert!(range.contains(2, top + 1));
    assert!(range.contains(4, top + 1));
    assert!(
        !range.contains(5, top + 1),
        "a rectangle stops at its right edge on every row it covers"
    );
}

/// What the renderer paints and what the gesture copied are the same cells:
/// every column of the selected run is inside the range, and the columns on
/// either side of it are not.
#[test]
fn the_painted_range_covers_the_copied_text() {
    let mut term = term_with(&["hello world"]);
    let row = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    let text = drag(
        &mut model,
        &mut term,
        ((6, row), Side::Left),
        ((11, row), Side::Left),
        Modifiers::NONE,
    );
    assert_eq!(text.as_deref(), Some("world"));

    let range = model.range(&term).expect("something is marked");
    assert_eq!(
        range,
        MarkedRange {
            start: (6, row),
            end: (10, row),
            block: false,
        }
    );
    for column in 6..=10 {
        assert!(range.contains(column, row), "column {column} should be in");
    }
    assert!(!range.contains(5, row));
    assert!(!range.contains(11, row));
}

/// A selection that crosses onto the next line takes the rest of the first
/// one with it, the way a run of text wraps.
#[test]
fn a_run_across_two_lines_takes_the_whole_line_between() {
    let mut term = term_with(&["first line", "second line"]);
    let top = line(&term, 0);
    let mut model = SelectionModel::new(Kind::Rio, COLS);

    drag(
        &mut model,
        &mut term,
        ((6, top), Side::Left),
        ((6, top + 1), Side::Left),
        Modifiers::NONE,
    );
    let range = model.range(&term).expect("something is marked");
    assert!(!range.block);
    assert!(range.contains(39, top), "to the end of the first row");
    assert!(range.contains(0, top + 1), "from the start of the second");
    assert!(!range.contains(5, top));
    assert!(!range.contains(6, top + 1));
}

/// A gesture the models are supposed to agree about: press on the left edge
/// of a character, drag right, let go. Whatever else the two disagree about,
/// this is the drag everybody makes, and both should mark the same cells and
/// hand back the same text.
#[test]
fn both_models_mark_the_same_cells_for_a_plain_character_drag() {
    let mut term = term_with(&["hello world", "second line"]);
    let row = line(&term, 0);

    let mut pictures = Vec::new();
    let mut texts = Vec::new();
    for kind in [Kind::Konsole, Kind::Rio] {
        let mut model = SelectionModel::new(kind, COLS);
        texts.push(drag(
            &mut model,
            &mut term,
            ((0, row), Side::Left),
            ((5, row), Side::Left),
            Modifiers::NONE,
        ));
        let range = model
            .range(&term)
            .unwrap_or_else(|| panic!("{kind:?} marked nothing"));
        let picture: Vec<bool> = (0..ROWS)
            .flat_map(|r| {
                let l = line(&term, r);
                (0..COLS).map(move |c| (c, l))
            })
            .map(|(c, l)| range.contains(c, l))
            .collect();
        pictures.push(picture);
    }

    assert_eq!(texts[0], texts[1], "the two models copied different text");
    assert_eq!(texts[0].as_deref(), Some("hello"));
    assert_eq!(
        pictures[0], pictures[1],
        "the two models marked different cells for the same drag"
    );
    assert!(
        pictures[0].iter().any(|&marked| marked),
        "nothing was marked"
    );
}
