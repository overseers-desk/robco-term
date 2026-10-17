//! The Konsole selection model, over scripted grids: the select-copy round
//! trip and the gestures that produce a selection.
//!
//! One arm of `term::selection`; the sibling `selection_rio_tests.rs` puts
//! the same questions to the other. What is proved here is Konsole's own
//! arithmetic: a click points at a cell, a rightward drag stops before the
//! cell under the pointer and a leftward one includes it, and the word
//! characters are Konsole's.

use term::grid::{GridView, ScriptedGrid};
use term::pointer::{preserve_line_breaks, Modifiers};
use term::selection::konsole::{char_class, Konsole, Selection, TripleClickMode};
use term::selection::Window;

const COLS: usize = 40;

fn win(grid: &ScriptedGrid) -> Window {
    Window {
        top_line: 0,
        lines: grid.total_lines(),
        columns: grid.columns(),
    }
}

#[test]
fn drag_right_then_copy_returns_what_was_dragged_over() {
    let g = ScriptedGrid::new(COLS, &["hello world", "second line"]);
    let mut c = Konsole::new(COLS);

    // Press on 'h', drag to the space after "hello". A rightward drag stops
    // before the cell under the pointer, so this is exactly "hello".
    c.press(0, 0);
    c.drag_to(&g, win(&g), 5, 0);
    assert_eq!(c.release(&g).as_deref(), Some("hello"));
}

#[test]
fn drag_left_then_copy_returns_the_same_span() {
    let g = ScriptedGrid::new(COLS, &["hello world"]);
    let mut c = Konsole::new(COLS);

    // Press just past "hello" and drag back to its start. Leftward, the cell
    // under the pointer is included and the anchor's is not, so the same five
    // characters come back.
    c.press(5, 0);
    c.drag_to(&g, win(&g), 0, 0);
    assert_eq!(c.release(&g).as_deref(), Some("hello"));
}

#[test]
fn a_drag_that_crosses_back_over_its_anchor_follows_the_pointer() {
    let g = ScriptedGrid::new(COLS, &["abcdefghij"]);
    let mut c = Konsole::new(COLS);

    c.press(5, 0);
    c.drag_to(&g, win(&g), 8, 0);
    assert_eq!(
        c.selection.selected_text(&g, true).as_deref(),
        Some("fgh"),
        "rightward first"
    );
    c.drag_to(&g, win(&g), 2, 0);
    assert_eq!(
        c.selection.selected_text(&g, true).as_deref(),
        Some("cde"),
        "and the selection re-anchors when the pointer crosses back"
    );
}

#[test]
fn a_multi_line_selection_round_trips_with_its_line_breaks() {
    let g = ScriptedGrid::new(COLS, &["first", "second", "third"]);
    let mut c = Konsole::new(COLS);

    c.press(0, 0);
    c.drag_to(&g, win(&g), 5, 2);
    assert_eq!(c.release(&g).as_deref(), Some("first\nsecond\nthird"));
}

/// The modifiers a drag is held with to copy a wrapped run unbroken:
/// Command on macOS, where Control is the secondary click, and Control
/// everywhere else.
fn join_lines() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            control: true,
            ..Modifiers::NONE
        }
    }
}

#[test]
fn the_join_lines_chord_makes_a_wrapped_command_paste_as_one() {
    let g = ScriptedGrid::new(COLS, &["make -j8 ", "--target release"]).wrap(&[0]);
    let mut c = Konsole::new(COLS);
    c.preserve_line_breaks = preserve_line_breaks(join_lines());

    c.press(0, 0);
    c.drag_to(&g, win(&g), 16, 1);
    assert_eq!(c.release(&g).as_deref(), Some("make -j8 --target release"));
}

#[test]
fn a_wrapped_line_takes_no_line_break_even_with_breaks_preserved() {
    // LINE_WRAPPED means the line did not end, so no newline is produced
    // whatever the copy mode: the text was always one line.
    let g = ScriptedGrid::new(COLS, &["make -j8 ", "--target release"]).wrap(&[0]);
    let mut c = Konsole::new(COLS);
    c.press(0, 0);
    c.drag_to(&g, win(&g), 16, 1);
    assert_eq!(c.release(&g).as_deref(), Some("make -j8 --target release"));
}

#[test]
fn double_click_selects_the_word_under_the_pointer() {
    let g = ScriptedGrid::new(COLS, &["cargo test --package term"]);
    let mut c = Konsole::new(COLS);
    let w = win(&g);

    assert_eq!(c.double_click(&g, w, 8, 0).as_deref(), Some("test"));
    assert_eq!(c.double_click(&g, w, 2, 0).as_deref(), Some("cargo"));
    // `-` and `.` are word characters, so a flag or a path is one word.
    assert_eq!(c.double_click(&g, w, 15, 0).as_deref(), Some("--package"));
}

#[test]
fn double_click_keeps_a_path_and_a_url_whole() {
    let g = ScriptedGrid::new(COLS, &["see /usr/local/bin/foo for it"]);
    let mut c = Konsole::new(COLS);
    assert_eq!(
        c.double_click(&g, win(&g), 8, 0).as_deref(),
        Some("/usr/local/bin/foo")
    );
}

#[test]
fn double_click_drops_a_trailing_at_sign() {
    // The reason double-clicking a username in `user@host` does not hand
    // you the separator.
    let g = ScriptedGrid::new(COLS, &["user@ host"]);
    let mut c = Konsole::new(COLS);
    assert_eq!(c.double_click(&g, win(&g), 1, 0).as_deref(), Some("user"));
}

#[test]
fn double_click_then_drag_extends_by_whole_words() {
    let g = ScriptedGrid::new(COLS, &["alpha beta gamma"]);
    let mut c = Konsole::new(COLS);
    let w = win(&g);

    assert_eq!(c.double_click(&g, w, 7, 0).as_deref(), Some("beta"));
    c.drag_to(&g, w, 13, 0);
    assert_eq!(
        c.selection.selected_text(&g, true).as_deref(),
        Some("beta gamma"),
        "the extension snaps to the far end of the word under the pointer"
    );
}

#[test]
fn double_click_crosses_a_wrap_but_not_a_line_end() {
    // A line that wrapped is full width by definition: it wrapped because it
    // ran out of columns, so the word continues at column 0 of the next.
    let g = ScriptedGrid::new(8, &["longword", "s here", "other"]).wrap(&[0]);
    let mut c = Konsole::new(8);
    let w = Window {
        top_line: 0,
        lines: 3,
        columns: 8,
    };
    assert_eq!(c.double_click(&g, w, 2, 0).as_deref(), Some("longwords"));

    // Line 1 did not wrap, so the word stops at its end.
    let g2 = ScriptedGrid::new(8, &["abc", "def"]);
    let mut c2 = Konsole::new(8);
    assert_eq!(c2.double_click(&g2, w, 1, 0).as_deref(), Some("abc"));
}

#[test]
fn triple_click_takes_the_whole_logical_line() {
    let g = ScriptedGrid::new(20, &["one two three ", "four five", "next"]).wrap(&[0]);
    let mut c = Konsole::new(20);
    let w = Window {
        top_line: 0,
        lines: 3,
        columns: 20,
    };
    assert_eq!(
        c.triple_click(&g, w, 4, 1).as_deref(),
        Some("one two three four five\n"),
        "clicking the second half selects both halves of the wrapped line"
    );
}

#[test]
fn triple_click_forwards_from_cursor_starts_at_the_word() {
    let g = ScriptedGrid::new(20, &["alpha beta gamma"]);
    let mut c = Konsole::new(20);
    c.triple_click_mode = TripleClickMode::SelectForwardsFromCursor;
    let w = win(&g);
    assert_eq!(c.triple_click(&g, w, 7, 0).as_deref(), Some("beta gamma\n"));
}

#[test]
fn selection_spans_the_scrollback_boundary_without_a_seam() {
    // Lines 0 and 1 are history, 2 and 3 are the screen. The linear index runs
    // straight through, so a selection across the boundary is one range.
    let g = ScriptedGrid::with_history(COLS, 2, &["old one", "old two", "new one", "new two"]);
    let mut c = Konsole::new(COLS);
    let w = win(&g);

    c.press(4, 1);
    c.drag_to(&g, w, 3, 2);
    assert_eq!(c.release(&g).as_deref(), Some("two\nnew"));
}

#[test]
fn block_selection_takes_a_rectangle() {
    let g = ScriptedGrid::new(COLS, &["aaaXXXbbb", "cccYYYddd", "eeeZZZfff"]);
    let mut s = Selection::new(COLS);
    s.set_start(3, 0, true);
    s.set_end(5, 2);
    assert_eq!(
        s.selected_text(&g, true).as_deref(),
        Some("XXX\nYYY\nZZZ"),
        "columns 3..5 of three lines, not the text between the two corners"
    );
    assert!(s.is_selected(4, 1));
    assert!(!s.is_selected(7, 1), "outside the column band");
}

#[test]
fn is_selected_agrees_with_the_copied_text() {
    let g = ScriptedGrid::new(COLS, &["hello world", "second line"]);
    let mut c = Konsole::new(COLS);
    c.press(6, 0);
    c.drag_to(&g, win(&g), 6, 1);

    // Every cell the renderer would highlight, read back off the grid,
    // must be the text the clipboard gets.
    let mut painted = String::new();
    for line in 0..g.total_lines() {
        let mut any = false;
        for col in 0..g.line_len(line) {
            if c.selection.is_selected(col, line) {
                painted.push(g.cell(line, col));
                any = true;
            }
        }
        if any && line + 1 < g.total_lines() && c.selection.is_selected(0, line + 1) {
            painted.push('\n');
        }
    }
    assert_eq!(Some(painted), c.release(&g));
}

#[test]
fn char_class_groups_words_and_separates_punctuation() {
    let wc = term::selection::konsole::DEFAULT_WORD_CHARACTERS;
    assert_eq!(char_class('a', wc), 'a');
    assert_eq!(char_class('7', wc), 'a');
    assert_eq!(
        char_class('_', wc),
        'a',
        "an underscore is a word character"
    );
    assert_eq!(char_class('/', wc), 'a', "so is a path separator");
    assert_eq!(char_class(' ', wc), ' ');
    assert_eq!(char_class('\t', wc), ' ', "all whitespace is one class");
    assert_eq!(char_class('!', wc), '!', "punctuation is its own class");
    assert_ne!(char_class('!', wc), char_class('?', wc));
}

/// An eviction between the drag and the release: the ring dropped lines off
/// the top, every absolute index re-names content that many lines later,
/// and [`Konsole::rebase`] is what keeps the gesture on the text it
/// covered. The two grids stand for the same buffer before and after two
/// evictions; the release runs against the after, as it does live.
#[test]
fn rebase_keeps_the_gesture_on_its_text_across_an_eviction() {
    let before = ScriptedGrid::new(COLS, &["old-1", "old-2", "hello world", "tail"]);
    let after = ScriptedGrid::new(COLS, &["hello world", "tail", "new-1", "new-2"]);
    let mut c = Konsole::new(COLS);

    c.press(0, 2);
    c.drag_to(&before, win(&before), 5, 2);

    // Two lines fall off the ring while the button is still down.
    c.rebase(2);

    // The read side follows without another gesture: the mark is painted
    // where the text now sits, not two rows below it.
    assert_eq!(c.start_at(2), Some((0, 0)));
    assert_eq!(c.end_at(2), Some((4, 0)));
    assert_eq!(c.release(&after).as_deref(), Some("hello"));
}

/// The same drift read without a rebase call: `start_at`/`end_at` take the
/// eviction count themselves, so a renderer frame between gesture events
/// paints the mark in the numbering the grid holds at that frame.
#[test]
fn the_read_side_shifts_by_the_evictions_it_is_told_of() {
    let g = ScriptedGrid::new(COLS, &["old-1", "old-2", "hello world"]);
    let mut c = Konsole::new(COLS);

    c.press(0, 2);
    c.drag_to(&g, win(&g), 5, 2);

    assert_eq!(c.start_at(0), Some((0, 2)));
    assert_eq!(c.end_at(0), Some((4, 2)));
    assert_eq!(c.start_at(1), Some((0, 1)));
    // Past the top of the ring, the mark pins to the oldest line that
    // still exists rather than going negative.
    assert_eq!(c.start_at(5), Some((0, 0)));
}
