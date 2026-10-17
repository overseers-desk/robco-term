//! The grid seam, and the two grid-to-text paths every consumer above it uses.
//!
//! Selection and the URL filters both work on text, and both reach it
//! through the same pair of functions: one that copies a line (or part of
//! one, clamped to the cells that were actually written) and one that
//! decodes cells into text (dropping the second half of every double-width
//! character). Sharing them means the two consumers share one definition of
//! "what does this line say".
//!
//! Everything above this module addresses lines by **absolute index**: 0 is
//! the oldest line still in scrollback and `total_lines() - 1` is the bottom
//! of the screen — a single linear index over history and screen together,
//! matching how Konsole numbers lines. It is also rio-vt's, shifted: rio-vt's
//! `Line` is signed with 0 at the top of the screen, so
//! `absolute = line + history_lines()`.

use unicode_width::UnicodeWidthChar;

/// A readable terminal grid: history plus screen, addressed absolutely.
///
/// Implementors normalise cells on the way out. rio-vt's `Square` is a packed
/// u64 and an untouched cell reads `'\0'`, not `' '`; a caller must never see
/// that, or every row is `columns` characters wide and every haystack the
/// URL filters scan is full of NULs. `cell()` returns `' '` for an untouched cell and
/// `line_len()` reports where the written part of the line stops, so a
/// caller can tell "untouched" from "space" without scanning every column
/// itself.
pub trait GridView {
    fn columns(&self) -> usize;
    /// Lines of scrollback above the screen.
    fn history_lines(&self) -> usize;
    /// Lines on the visible screen.
    fn screen_lines(&self) -> usize;
    fn total_lines(&self) -> usize {
        self.history_lines() + self.screen_lines()
    }
    /// The character at an absolute position. Untouched cells read `' '`.
    fn cell(&self, line: usize, column: usize) -> char;
    /// How many cells of this line were ever written.
    fn line_len(&self, line: usize) -> usize;
    /// Did this line wrap into the next one, rather than end?
    fn is_wrapped(&self, line: usize) -> bool;
}

/// Character width by wcwidth-like classification: -1 for a control
/// character, 0 for a combining mark, 2 for a double-width character, 1
/// otherwise. `unicode-width` returns `None` where wcwidth would return -1,
/// which is the only mapping needed here.
pub fn char_width(c: char) -> i32 {
    match UnicodeWidthChar::width(c) {
        Some(w) => w as i32,
        None => -1,
    }
}

/// The raw sum of character widths, control characters included at -1,
/// clamped at zero because a column index cannot be negative — a leading
/// control character would otherwise push the running sum below zero.
pub fn string_width(s: &str) -> usize {
    let w: i32 = s.chars().map(char_width).sum();
    w.max(0) as usize
}

/// Decodes a run of cells already selected into text.
///
/// The `i += max(1, wcwidth)` step is what drops the spacer cell that follows
/// a double-width character; it is not an optimisation and removing it
/// doubles every CJK character in a copied selection.
///
/// Trailing whitespace is always included here (the URL filter explicitly
/// asks for it too); trailing blanks are instead excluded upstream, by
/// `line_len`.
fn decode_cells(cells: &[char], out: &mut String) {
    let mut i = 0usize;
    while i < cells.len() {
        out.push(cells[i]);
        i += char_width(cells[i]).max(1) as usize;
    }
}

/// How many cells `copy_line` should take: a fixed count, or the rest of the
/// line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    Cells(usize),
    ToEndOfLine,
}

/// Appends the decoded text to `out` and returns the number of *cells*
/// consumed, which is what `write_range` compares against the requested
/// count to decide whether a selection ran past the end of its last line.
///
/// The two clamping branches are not interchangeable. A line in history
/// clamps `start` back to the last written cell; a line on the screen
/// leaves `start` alone and clamps the count to zero. The difference shows
/// up when a selection starts to the right of the text on a line: from
/// history you get the last character, from the screen you get nothing.
pub fn copy_line(
    grid: &impl GridView,
    line: usize,
    start: usize,
    count: Count,
    append_newline: bool,
    preserve_line_breaks: bool,
    out: &mut String,
) -> usize {
    let line_length = grid.line_len(line);
    let mut start = start;
    let mut count = count;

    if line < grid.history_lines() {
        // History: trailing whitespace was never stored, so no trimming is
        // needed here, but `start` must be pulled back inside the line.
        start = start.min(line_length.saturating_sub(1));
        count = Count::Cells(match count {
            Count::ToEndOfLine => line_length.saturating_sub(start),
            Count::Cells(n) => (start + n).min(line_length).saturating_sub(start),
        });
    } else {
        let requested = match count {
            Count::ToEndOfLine => grid.columns().saturating_sub(start),
            Count::Cells(n) => n,
        };
        let (s, c) = if start >= line_length {
            (line_length, 0)
        } else {
            (start, requested.min(line_length - start))
        };
        start = s;
        count = Count::Cells(c.min(line_length.saturating_sub(start)));
    }

    let mut n = match count {
        Count::Cells(n) => n,
        Count::ToEndOfLine => unreachable!("normalised above"),
    };

    let mut cells: Vec<char> = (start..start + n).map(|c| grid.cell(line, c)).collect();

    // A wrapped line did not end, so it takes no line break; without
    // `preserve_line_breaks` nothing does.
    let omit_line_break = grid.is_wrapped(line) || !preserve_line_breaks;
    if !omit_line_break && append_newline {
        cells.push('\n');
        n += 1;
    }

    decode_cells(&cells, out);
    n
}

/// `Screen::writeToStream`: the linear-index range that a selection is stored
/// as, decoded line by line.
///
/// `start_index` and `end_index` are `loc(x, y) = y * columns + x`.
pub fn write_range(
    grid: &impl GridView,
    start_index: usize,
    end_index: usize,
    block_mode: bool,
    preserve_line_breaks: bool,
) -> String {
    let columns = grid.columns();
    let top = start_index / columns;
    let left = start_index % columns;
    let bottom = end_index / columns;
    let right = end_index % columns;

    let mut out = String::new();
    for y in top..=bottom.min(grid.total_lines().saturating_sub(1)) {
        let start = if y == top || block_mode { left } else { 0 };
        let count = if y == bottom || block_mode {
            Count::Cells((right + 1).saturating_sub(start))
        } else {
            Count::ToEndOfLine
        };

        let append_newline = y != bottom;
        let copied = copy_line(
            grid,
            y,
            start,
            count,
            append_newline,
            preserve_line_breaks,
            &mut out,
        );

        // If the selection reached past the text on its last line, the user
        // was selecting the newline after it too, so produce one. This is
        // what makes "drag past the end of the line" copy a line break.
        if y == bottom {
            if let Count::Cells(requested) = count {
                if copied < requested {
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// The buffer the filter chain scans, and the byte offset each line starts at.
///
/// `TerminalImageFilterChain::setImage` does not go through `copyLineToStream`
/// at all: it decodes the *display image*, a full `lines x columns` rectangle
/// that `Screen::getImage` pads with spaces, with trailing whitespace
/// explicitly kept ("otherwise, if a string is wrapped at the end of a space,
/// that space will not be taken into account"). A newline is appended after
/// every line that did not wrap, so a URL at the end of one line cannot fuse
/// with text at the start of the next.
///
/// Lines are numbered relative to `first_line`, matching `setImage`, which is
/// handed the visible window and reports hotspots in window coordinates.
pub fn filter_buffer(
    grid: &impl GridView,
    first_line: usize,
    lines: usize,
) -> (String, Vec<usize>) {
    let columns = grid.columns();
    let mut buffer = String::new();
    let mut positions = Vec::with_capacity(lines);

    for i in 0..lines {
        positions.push(buffer.len());
        let line = first_line + i;
        if line < grid.total_lines() {
            let cells: Vec<char> = (0..columns).map(|c| grid.cell(line, c)).collect();
            decode_cells(&cells, &mut buffer);
            if !grid.is_wrapped(line) {
                buffer.push('\n');
            }
        } else {
            buffer.push('\n');
        }
    }
    (buffer, positions)
}

/// A grid written out longhand, for tests and for anything that needs a grid
/// without a terminal behind it.
///
/// Cells past the end of a row's string are untouched, which is the state the
/// `line_len` distinction exists to describe: `"ab"` in an 80-column grid has
/// two written cells and 78 that were never lit, and copying columns 5..10 out
/// of it yields nothing rather than five spaces.
#[derive(Clone, Debug)]
pub struct ScriptedGrid {
    columns: usize,
    history_lines: usize,
    rows: Vec<Vec<char>>,
    wrapped: Vec<bool>,
}

impl ScriptedGrid {
    /// All lines on screen, none in history.
    pub fn new(columns: usize, rows: &[&str]) -> Self {
        Self::with_history(columns, 0, rows)
    }

    /// The first `history_lines` rows are scrollback, the rest are the screen.
    pub fn with_history(columns: usize, history_lines: usize, rows: &[&str]) -> Self {
        let rows: Vec<Vec<char>> = rows
            .iter()
            .map(|r| {
                let mut cells: Vec<char> = Vec::new();
                for c in r.chars() {
                    cells.push(c);
                    // A double-width character occupies its spacer cell too;
                    // the decoder skips the spacer by width, so what sits in
                    // it never reaches any text path.
                    for _ in 1..char_width(c).max(1) {
                        cells.push(' ');
                    }
                }
                cells.truncate(columns);
                cells
            })
            .collect();
        let wrapped = vec![false; rows.len()];
        Self {
            columns,
            history_lines,
            rows,
            wrapped,
        }
    }

    /// Mark lines as wrapping into the next one (`LINE_WRAPPED`).
    pub fn wrap(mut self, lines: &[usize]) -> Self {
        for &l in lines {
            self.wrapped[l] = true;
        }
        self
    }
}

impl GridView for ScriptedGrid {
    fn columns(&self) -> usize {
        self.columns
    }
    fn history_lines(&self) -> usize {
        self.history_lines
    }
    fn screen_lines(&self) -> usize {
        self.rows.len() - self.history_lines
    }
    fn cell(&self, line: usize, column: usize) -> char {
        self.rows
            .get(line)
            .and_then(|r| r.get(column))
            .copied()
            .unwrap_or(' ')
    }
    fn line_len(&self, line: usize) -> usize {
        self.rows.get(line).map(|r| r.len()).unwrap_or(0)
    }
    fn is_wrapped(&self, line: usize) -> bool {
        self.wrapped.get(line).copied().unwrap_or(false)
    }
}
