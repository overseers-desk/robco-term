//! The drawn half of selection: `selection_tests.rs` proves the engine picks
//! the right cells, this proves the glass shows them.
//!
//! Measured in pixels off a real device, like every other picture claim in
//! this crate, because that is the claim: a selection nobody can see is the
//! bug (#12, #20), and a state assertion would have passed throughout it.
//!
//! What is pinned down here:
//!
//! * a marked run comes out as **inverse video** -- the plate in the phosphor
//!   colour, the glyph in the background -- which is the only highlight a tube
//!   with no alpha ever had;
//! * the rest of the frame is **untouched**, and clearing the selection puts
//!   the screen back **bit for bit**, so the highlight is a property of the
//!   marked cells and not a wash over the picture;
//! * a change of selection repaints **the rows it crossed and no others**, the
//!   same narrowing the cursor's own line already gets;
//! * a cell that arrives with its glyph and its plate already the same colour
//!   -- which a program reaches by naming one colour for both, and which the
//!   flat scheme these tests build reaches for any colour at all -- still
//!   reads once marked, because there is no second colour to reach for and the
//!   glyph goes to the dim end of the one there is.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use gpu::harness::GpuLock;
use term::atlas::Rasterization;
use term::color::Scheme;
use term::fonts::{font_by_name, FontSource};
use term::fonts::sizing::{self, ScalePolicy, SizingRequest};
use gpu::{Gpu, Image};
use term::render::{GridRenderer, Marked};
use term::selection::MarkedRange;
use term::viewport::ScrollPosition;
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

const COLS: usize = 40;
const ROWS: usize = 6;
const SCROLLBACK: usize = 200;

/// The shipped appliance's own colours: one phosphor for every glyph, opaque
/// black behind it (`crates/app/src/window/mod.rs`).
const PHOSPHOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BEHIND: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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

/// The device under the machine-wide GPU lock, on the terms `scrollback.rs`
/// and `preedit.rs` already take it: a machine with no adapter says so rather
/// than passing vacuously.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = GpuLock::acquire().expect("the GPU lock");
    match Gpu::new() {
        Ok(gpu) => Some((gpu, lock)),
        Err(e) => {
            if std::env::var("ROBCO_SKIP_GPU_TESTS").is_ok() {
                eprintln!("skipping: no wgpu adapter ({e})");
                None
            } else {
                panic!("no wgpu adapter: {e}");
            }
        }
    }
}

fn renderer(gpu: &Gpu, scheme: &Scheme) -> (GridRenderer, FontContext) {
    let terminess = font_by_name("TERMINESS_SCALED", FontSource::Bundled)
        .expect("TERMINESS_SCALED in the catalogue");
    let resolved = sizing::resolve(terminess, &SizingRequest::default(), ScalePolicy::Floor);
    let mut font = FontContext::new(terminess);
    let atlas = font.build_atlas(
        &gpu.device,
        &gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::Binary {
            threshold: DEFAULT_THRESHOLD,
        },
    );
    let renderer = GridRenderer::new(&gpu.device, &gpu.queue, atlas, COLS, ROWS, scheme.clone());
    (renderer, font)
}

fn terminal() -> (Crosswords<VoidListener>, Processor) {
    let term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    (term, Processor::default())
}

/// A run of cells on one line, as the pointer would have left it: the
/// range plus the absolute line the top of the screen is showing, which
/// with no history scrolled back is line zero.
fn marked(from: usize, to: usize, row: usize) -> Marked {
    Marked {
        range: MarkedRange {
            start: (from, row),
            end: (to, row),
            block: false,
        },
        top_line: 0,
    }
}

/// The cell rectangle of `(col, row)` in the rendered image, at scale 1.
fn cell_rect(renderer: &GridRenderer, col: usize, row: usize) -> (u32, u32, u32, u32) {
    let cell = renderer.atlas().cell;
    (
        col as u32 * cell.width,
        row as u32 * cell.height,
        cell.width,
        cell.height,
    )
}

/// How many pixels of a rectangle are lit, and how many are dark: a cell in
/// inverse video has the two counts of the same cell drawn normally swapped,
/// give or take nothing, because the swap is exactly what inversion is.
fn lit_and_dark(image: &Image, rect: (u32, u32, u32, u32)) -> (usize, usize) {
    let (x0, y0, w, h) = rect;
    let (mut lit, mut dark) = (0, 0);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            if image.pixel(x, y)[0] > 128 {
                lit += 1;
            } else {
                dark += 1;
            }
        }
    }
    (lit, dark)
}

/// Whether a rectangle is all one colour: what a cell whose glyph and plate
/// have collapsed to the same phosphor looks like.
fn is_flat(image: &Image, rect: (u32, u32, u32, u32)) -> bool {
    let (x0, y0, w, h) = rect;
    let first = image.pixel(x0, y0);
    (y0..y0 + h).all(|y| (x0..x0 + w).all(|x| image.pixel(x, y) == first))
}

/// Every pixel outside `rects` is the same in both images.
fn identical_outside(a: &Image, b: &Image, rects: &[(u32, u32, u32, u32)]) {
    for y in 0..a.height {
        for x in 0..a.width {
            let inside = rects
                .iter()
                .any(|(rx, ry, rw, rh)| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh);
            if inside {
                continue;
            }
            assert_eq!(
                a.pixel(x, y),
                b.pixel(x, y),
                "the frame changed at ({x},{y}), outside the marked run"
            );
        }
    }
}

#[test]
fn a_marked_run_is_drawn_in_inverse_video_and_nothing_else_moves() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    // Row 0 carries the word; the cursor lands on row 1 and stays out of it.
    processor.advance(&mut term, b"HELLO WORLD\r\n");
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    let plain = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    // "HELLO": columns 0 to 4 of the top line.
    let run = marked(0, 4, 0);
    let rects: Vec<_> = (0..=4).map(|col| cell_rect(&renderer, col, 0)).collect();
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        Some(&run),
        None,
    );
    let highlighted = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    assert_ne!(
        plain.pixels, highlighted.pixels,
        "a marked run left the screen exactly as it found it: nothing was drawn"
    );
    identical_outside(&plain, &highlighted, &rects);

    // Cell by cell, the lit and the dark counts trade places: the plate is now
    // the phosphor and the glyph is the hole in it.
    for (col, rect) in rects.iter().enumerate() {
        let (was_lit, was_dark) = lit_and_dark(&plain, *rect);
        let (now_lit, now_dark) = lit_and_dark(&highlighted, *rect);
        assert!(
            was_lit > 0 && was_dark > 0,
            "test setup: column {col} of \"HELLO\" should have both glyph and background pixels"
        );
        assert_eq!(
            (now_lit, now_dark),
            (was_dark, was_lit),
            "column {col} is not the inverse of what it was"
        );
    }

    // Clearing puts the screen back, bit for bit.
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    let cleared = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    assert_eq!(
        plain.pixels, cleared.pixels,
        "clearing the selection left the highlight on the glass"
    );
}

#[test]
fn a_selection_repaints_the_rows_it_crossed_and_no_others() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    processor.advance(&mut term, b"ALPHA\r\nBETA\r\nGAMMA\r\n");
    // Settle: the first sync after construction is allowed to be full.
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );

    let run = marked(0, 4, 1);
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        Some(&run),
        None,
    );
    assert!(!stats.full, "marking a run repainted the whole screen");
    assert_eq!(
        stats.marked_rows, 1,
        "one line marked should repaint one row, not {}",
        stats.marked_rows
    );

    // The same selection again is no change at all.
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        Some(&run),
        None,
    );
    assert_eq!(
        stats.rows_updated, 0,
        "an unchanged selection repainted rows"
    );

    // Dragging down a line. Two rows, not one and not the screen: a selection
    // is a run over the whole grid, so crossing onto the next line takes the
    // rest of the first line with it, and both have changed.
    let mut grown = marked(0, 4, 1);
    grown.range.end = (4, 2);
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        Some(&grown),
        None,
    );
    assert_eq!(
        stats.marked_rows, 2,
        "extending onto one more line repainted {} of {ROWS} rows",
        stats.marked_rows
    );

    // Letting go of it: both rows come back.
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert_eq!(
        stats.marked_rows, 2,
        "clearing a two-line selection repainted {} rows",
        stats.marked_rows
    );
}

#[test]
fn a_cell_the_phosphor_already_collapsed_still_reads_when_marked() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    // A red background under a white foreground. The flat scheme this test
    // builds answers every palette entry with the one colour, so both ends of
    // the cell arrive the same colour and the glyph is already invisible: a
    // plain swap would mark it invisibly too.
    processor.advance(&mut term, b"\x1b[41mX\x1b[0m\r\n");
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    let plain = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    let rect = cell_rect(&renderer, 0, 0);
    assert!(
        is_flat(&plain, rect),
        "test setup: the collapsed cell should be one flat plate before it is marked"
    );

    let run = marked(0, 0, 0);
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        Some(&run),
        None,
    );
    let highlighted = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    assert!(
        !is_flat(&highlighted, rect),
        "a marked cell whose glyph and plate had collapsed to one phosphor came out \
         one flat colour again: the mark cannot be seen"
    );
}

#[test]
fn a_linked_run_is_underlined_and_nothing_else_moves() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    processor.advance(&mut term, b"HELLO WORLD\r\n");
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    let plain = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    // "WORLD": columns 6 to 10 of the top line, as the link under the pointer.
    let link = marked(6, 10, 0);
    let rects: Vec<_> = (6..=10).map(|col| cell_rect(&renderer, col, 0)).collect();
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        Some(&link),
    );
    assert_eq!(stats.link_rows, 1, "the one row the link entered is rebuilt for it");
    let underlined = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    assert_ne!(
        plain.pixels, underlined.pixels,
        "a linked run left the screen exactly as it found it: nothing was drawn"
    );
    identical_outside(&plain, &underlined, &rects);

    // A line one pixel below the baseline, lit across every cell of the run,
    // and nothing taken from the glyph above it.
    let cell = renderer.atlas().cell;
    let line_y = (cell.baseline + 1).min(cell.height as i32 - 1) as u32;
    for (i, rect) in rects.iter().enumerate() {
        let col = i + 6;
        let (x0, y0, w, _) = *rect;
        for x in x0..x0 + w {
            assert!(
                underlined.pixel(x, y0 + line_y)[0] > 128,
                "column {col}: the underline is missing at x={x}"
            );
        }
        let (was_lit, _) = lit_and_dark(&plain, *rect);
        let (now_lit, _) = lit_and_dark(&underlined, *rect);
        assert!(now_lit > was_lit, "column {col} gained no pixels");
    }

    // The same link a frame later costs no row.
    let again = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        Some(&link),
    );
    assert_eq!(again.link_rows, 0);
    assert!(!again.full);

    // Taking the link away gives the picture back bit for bit.
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    let restored = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    assert_eq!(
        plain.pixels, restored.pixels,
        "the underline left something behind after the pointer moved off"
    );
}

/// A block cursor is a phosphor plate with a dark glyph, which is what a
/// marked cell is too, so inside a selection it would vanish. There it is
/// drawn the other way round, and a mark arriving under a cursor that has
/// not moved redraws it.
#[test]
fn a_cursor_inside_a_selection_is_drawn_as_its_inverse() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    // The prompt leaves the cursor on the blank at column 2 of row 1.
    processor.advance(&mut term, b"HELLO WORLD\r\n$ ");
    for run in [None, Some(marked(0, 6, 1))] {
        renderer.sync(
            &gpu.device,
            &gpu.queue,
            &mut font,
            &mut term,
            &mut viewport,
            run.as_ref(),
            None,
        );
    }
    let image = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    let (lit, _) = lit_and_dark(&image, cell_rect(&renderer, 3, 1));
    assert!(lit > 0, "test setup: the marked blank beside the cursor is not lit");
    assert_eq!(
        lit_and_dark(&image, cell_rect(&renderer, 2, 1)).0,
        0,
        "the cursor on a marked blank is lit like the marked blank beside it"
    );
}

/// Reverse video gives a cell a plate in the cursor's colour as well, and a
/// cell can turn to it under a cursor that never moves: the cursor is drawn
/// from the rebuilt cell and swaps the same way.
#[test]
fn a_cursor_on_a_reverse_video_cell_is_drawn_as_its_inverse() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    // A reverse-video blank written at column 2 and stepped back onto, so
    // the cursor is where it was for both frames.
    for bytes in [&b"$ "[..], b"\x1b[7m \x1b[D"] {
        processor.advance(&mut term, bytes);
        renderer.sync(
            &gpu.device,
            &gpu.queue,
            &mut font,
            &mut term,
            &mut viewport,
            None,
            None,
        );
    }
    let image = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    assert_eq!(
        lit_and_dark(&image, cell_rect(&renderer, 2, 0)).0,
        0,
        "the cursor on a reverse-video blank is lit like the blank's own plate"
    );
}
