//! The scrollback viewport and the damage path, driven by a real rio-vt
//! terminal (no PTY: bytes are fed straight to the processor, which is what
//! `Processor::advance` is for and keeps the test deterministic).
//!
//! What is being pinned down here:
//!
//! * scrolling back shows history, and the renderer's cells follow it;
//! * a viewport move forces a full rebuild, because rio-vt's per-line damage
//!   is in viewport coordinates and cannot describe lines that arrived from
//!   history;
//! * ordinary output does *not* force a full rebuild, so the damage path is
//!   actually narrowing the work rather than redrawing the screen every frame;
//! * a terminal swapped underneath the renderer rebuilds the screen, which
//!   no damage either side of the swap reports;
//! * a position between two lines (a wheel glide, a touchpad's pixels) is
//!   held as rio-vt's whole lines plus a shift, and drawn as the grid moved
//!   up by the shift with the next line filling the gap at the bottom.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use gpu::harness::GpuLock;
use std::time::Instant;
use term::atlas::Rasterization;
use term::color::Scheme;
use term::fonts::{font_by_name, FontSource};
use term::fonts::sizing::{self, SizingRequest};
use gpu::Gpu;
use term::render::GridRenderer;
use term::viewport::{ScrollPosition, WHEEL_GLIDE};
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

const COLS: usize = 40;
const ROWS: usize = 6;
const SCROLLBACK: usize = 200;

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

/// The device, and the machine-wide GPU lock it is held under.
///
/// `gpu::Gpu` is the shipping type and takes no lock of its own -- the
/// application is not competing with anyone for a device. These tests are, with
/// every other GPU test in the tree and in any other process running the suite,
/// which is what `gpu::harness::GpuLock` exists for. The tuple's order
/// is its drop order: the device goes first, the lock after it.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = match GpuLock::acquire() {
        Ok(lock) => lock,
        Err(e) => panic!("cannot take the GPU lock: {e}"),
    };
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

/// The renderer and the shaping context that fed its atlas. A test drives a
/// frame through `sync`, which resolves any character the atlas lacks against
/// this context, so the two travel together the way they do in the app.
fn renderer(gpu: &Gpu, scheme: &Scheme) -> (GridRenderer, FontContext) {
    let terminess = font_by_name("TERMINESS_SCALED", FontSource::Bundled)
        .expect("TERMINESS_SCALED in the catalogue");
    let resolved = sizing::resolve(terminess, &SizingRequest::default());
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

#[test]
fn scrollback_viewport_follows_history() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();

    let mut term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    let mut processor = Processor::default();

    // Twenty lines through a six-row screen: fourteen end up in history.
    for i in 0..20 {
        processor.advance(&mut term, format!("LINE-{i:03}\r\n").as_bytes());
    }
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert!(stats.rows_updated > 0, "output produced no damage at all");
    assert_eq!(
        viewport.offset(),
        0,
        "output should leave the view at the bottom"
    );
    assert!(viewport.is_following());
    assert_eq!(renderer.grid().row_text(0).trim_end(), "LINE-015");
    assert_eq!(renderer.grid().row_text(4).trim_end(), "LINE-019");

    // Scroll back five lines: the same screen, five lines earlier.
    viewport.scroll(&mut term, 5);
    assert_eq!(viewport.offset(), 5);
    assert!(!viewport.is_following());
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert!(
        stats.full,
        "a viewport move has to rebuild every row: rio-vt's damage is in \
         viewport coordinates and says nothing about lines that came from history"
    );
    assert_eq!(renderer.grid().row_text(0).trim_end(), "LINE-010");
    assert_eq!(renderer.grid().row_text(4).trim_end(), "LINE-014");

    // Back to the bottom.
    viewport.to_bottom(&mut term);
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert_eq!(viewport.offset(), 0);
    assert!(viewport.is_following());
    assert_eq!(renderer.grid().row_text(4).trim_end(), "LINE-019");

    // A page is a screenful, clamped by the history that exists.
    viewport.page_up(&mut term);
    assert_eq!(viewport.offset(), ROWS);
    viewport.page_down(&mut term);
    assert_eq!(viewport.offset(), 0);
    viewport.to_top(&mut term);
    assert_eq!(viewport.offset(), term.history_size());
}

#[test]
fn ordinary_output_does_not_force_a_full_rebuild() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();

    let mut term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    let mut processor = Processor::default();

    // Settle: the first sync after construction is allowed to be full.
    processor.advance(&mut term, b"first\r\n");
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );

    // One line of output on a screen with room to spare. Two rows can
    // legitimately need rewriting (the text, and the row the cursor left).
    processor.advance(&mut term, b"second\r\n");
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert!(
        !stats.full,
        "one line of output should not repaint the screen"
    );
    assert!(
        stats.rows_updated <= 3,
        "one line of output rewrote {} of {ROWS} rows",
        stats.rows_updated
    );
    assert_eq!(renderer.grid().row_text(1).trim_end(), "second");

    // Nothing at all: nothing to do.
    let stats = renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );
    assert!(!stats.full);
    assert_eq!(stats.rows_updated, 0, "an idle frame rewrote rows");
}

/// A channel switch puts a different terminal under the same renderer, and
/// the one arriving reports no damage if it has been idle since it left. The
/// cells then held are the other channel's, so the glass keeps the old
/// picture and moves only the cursor into it.
#[test]
fn a_terminal_swapped_underneath_rebuilds_the_screen() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();

    let channel = |text: &[u8]| {
        let mut term = Crosswords::new(
            Size,
            CursorShape::Block,
            VoidListener {},
            WindowId::from(0u64),
            0,
            SCROLLBACK,
        );
        Processor::default().advance(&mut term, text);
        term
    };
    let mut first = channel(b"FIRST\r\n");
    let mut second = channel(b"SECOND\r\n");

    let mut show = |renderer: &mut GridRenderer, font: &mut _, term: &mut _| {
        renderer.sync(&gpu.device, &gpu.queue, font, term, &mut viewport, None, None)
    };
    show(&mut renderer, &mut font, &mut first);
    // Both are now idle: whatever either has to report, it has reported.
    show(&mut renderer, &mut font, &mut second);
    let stats = show(&mut renderer, &mut font, &mut first);
    assert!(!stats.full, "the swap alone is not damage rio-vt reports");

    renderer.invalidate();
    let stats = show(&mut renderer, &mut font, &mut second);
    assert!(stats.full, "an invalidated renderer skipped the rebuild");
    assert_eq!(renderer.grid().row_text(0).trim_end(), "SECOND");
}

#[test]
fn untouched_cells_read_as_blanks_not_nuls() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();

    let mut term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    let mut processor = Processor::default();
    processor.advance(&mut term, b"AB");
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut viewport,
        None,
        None,
    );

    // The trap: an untouched `Square` reads `'\0'`, not `' '`. If the
    // normalisation is ever dropped, this row becomes two characters followed
    // by thirty-eight NULs and the atlas quietly maps none of them.
    let row = renderer.grid().row_text(0);
    assert_eq!(row.len(), COLS);
    assert!(
        !row.contains('\0'),
        "row 0 still carries NUL cells: {row:?}"
    );
    assert_eq!(row.trim_end(), "AB");
}

/// A bare terminal with twenty numbered lines through the six-row screen:
/// fourteen in history, the same fixture the renderer tests use, for the
/// position model on its own, no device needed.
fn twenty_lines() -> Crosswords<VoidListener> {
    let mut term = Crosswords::new(
        Size,
        CursorShape::Block,
        VoidListener {},
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    let mut processor = Processor::default();
    for i in 0..20 {
        processor.advance(&mut term, format!("LINE-{i:03}\r\n").as_bytes());
    }
    term
}

/// A wheel notch sets the view gliding: nothing moves at the notch itself,
/// the position is part of the way there part of the way through, rio-vt
/// holds the ceiling of it with the remainder as the shift, and the glide
/// lands on the whole line. A second notch during the glide moves the
/// destination, not the picture.
#[test]
fn a_wheel_notch_glides_three_lines_and_lands_on_the_line() {
    let mut term = twenty_lines();
    let mut view = ScrollPosition::default();
    let t0 = Instant::now();

    view.scroll_wheel(&mut term, 1, t0);
    assert!(view.is_gliding());
    assert_eq!(view.position(), 0.0, "the notch itself moves nothing");

    // Half way through the glide, eased out: past half way there.
    let half = t0 + WHEEL_GLIDE / 2;
    assert!(view.advance(&mut term, half), "still gliding at half time");
    let pos = view.position();
    assert!(pos > 1.5 && pos < 3.0, "eased past half way: {pos}");
    assert_eq!(
        view.offset(),
        pos.ceil() as usize,
        "rio-vt holds the ceiling"
    );
    assert_eq!(term.grid.display_offset(), view.offset());
    let shift = view.shift();
    assert!(
        (shift - (pos.ceil() - pos)).abs() < 1e-6,
        "shift is the remainder"
    );
    assert!(!view.is_following());

    // A second notch mid-glide: the destination moves to six.
    view.scroll_wheel(&mut term, 1, half);
    assert!(!view.advance(&mut term, half + WHEEL_GLIDE), "arrived");
    assert_eq!(view.position(), 6.0);
    assert_eq!(view.offset(), 6);
    assert_eq!(view.shift(), 0.0, "a landed glide sits on a line");
    assert_eq!(term.grid.display_offset(), 6);

    // A glide back below the bottom stops at the bottom and follows again.
    view.scroll_wheel(&mut term, -5, half + WHEEL_GLIDE);
    view.advance(&mut term, half + 2 * WHEEL_GLIDE);
    assert_eq!(view.position(), 0.0);
    assert!(view.is_following());
    assert_eq!(term.grid.display_offset(), 0);
}

/// A touchpad's pixels move the view as they come, no glide: half a cell up
/// is half a row into history, which rio-vt holds as one whole line drawn
/// shifted up by the other half. Back down the same distance is the bottom.
#[test]
fn touchpad_pixels_move_the_view_by_fractions_of_a_row() {
    let mut term = twenty_lines();
    let mut view = ScrollPosition::default();
    let cell_h = 18.0;

    view.scroll_pixels(&mut term, 9.0, cell_h);
    assert!(!view.is_gliding());
    assert_eq!(view.position(), 0.5);
    assert_eq!(view.offset(), 1, "half a row back is one line held");
    assert_eq!(term.grid.display_offset(), 1);
    assert_eq!(view.shift(), 0.5);
    assert!(!view.is_following(), "any distance back pins the view");

    view.scroll_pixels(&mut term, 27.0, cell_h);
    assert_eq!(view.position(), 2.0);
    assert_eq!(view.offset(), 2);
    assert_eq!(view.shift(), 0.0);

    view.scroll_pixels(&mut term, -36.0, cell_h);
    assert_eq!(view.position(), 0.0);
    assert_eq!(view.offset(), 0);
    assert!(view.is_following());

    // Past the history that exists stops at the history that exists.
    view.scroll_pixels(&mut term, 10_000.0, cell_h);
    assert_eq!(view.position(), term.history_size() as f32);
    assert_eq!(view.offset(), term.history_size());
}

/// Output under a scrolled view grows the history beneath it, and rio-vt
/// carries the view up with the lines it shows. The position follows, the
/// fraction of a row it sat at kept; a glide under way is carried too.
#[test]
fn history_growing_under_a_scrolled_view_carries_the_position() {
    let mut term = twenty_lines();
    let mut view = ScrollPosition::default();
    let mut processor = Processor::default();

    view.scroll_pixels(&mut term, 27.0, 18.0);
    assert_eq!(view.position(), 1.5);
    assert_eq!(view.offset(), 2);

    processor.advance(&mut term, b"MORE-1\r\nMORE-2\r\n");
    assert_eq!(term.grid.display_offset(), 4, "rio-vt carried the view");
    let change = view.sync(&term);
    assert!(change.moved);
    assert_eq!(view.position(), 3.5);
    assert_eq!(view.offset(), 4);
    assert_eq!(view.shift(), 0.5);

    let t0 = Instant::now();
    view.scroll_wheel(&mut term, 1, t0);
    processor.advance(&mut term, b"MORE-3\r\n");
    view.sync(&term);
    view.advance(&mut term, t0 + WHEEL_GLIDE);
    assert_eq!(
        view.position(),
        8.0,
        "the glide's destination moved with the history under it"
    );

    view.to_bottom(&mut term);
    assert_eq!(view.position(), 0.0);
    assert_eq!(term.grid.display_offset(), 0);
    assert!(view.is_following());
}

/// The picture of a position between two lines: the grid drawn shifted up
/// by the fraction's pixels, with the spare row showing that much of the
/// line after the last one, the padding around the grid untouched.
///
/// Against the same view drawn unshifted: every pixel of the shifted
/// picture inside the grid's rectangle is the unshifted picture's pixel
/// that many rows lower, the band at the bottom is the top of the next
/// line, and the padding is the clear colour in both.
#[test]
fn a_shifted_view_draws_the_grid_higher_with_the_next_line_filling_the_gap() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut view = ScrollPosition::default();
    let mut term = twenty_lines();

    // One and a half rows back: two lines held, drawn half a row up.
    let cell_h = renderer.atlas().cell.height;
    view.scroll_pixels(&mut term, 1.5 * cell_h as f32, cell_h as f32);
    assert_eq!(view.offset(), 2);
    assert_eq!(view.shift(), 0.5);
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut view,
        None,
        None,
    );
    assert_eq!(renderer.grid().row_text(0).trim_end(), "LINE-013");
    assert_eq!(renderer.grid().row_text(5).trim_end(), "LINE-018");
    assert_eq!(
        renderer.grid().row_text(ROWS).trim_end(),
        "LINE-019",
        "the spare row is the line after the last one on screen"
    );

    // A target with a cell of padding on every side.
    let (grid_w, grid_h) = renderer.pixel_size();
    let pad = cell_h;
    let (w, h) = (grid_w + 2 * pad, grid_h + 2 * pad);
    let clear = wgpu::Color::BLACK;

    renderer.set_origin(pad as i32, pad as i32, 0);
    let flat = renderer.render_to_image_sized(&gpu, clear, w, h);
    let shift_px = (0.5 * cell_h as f32).round() as u32;
    renderer.set_origin(pad as i32, pad as i32, shift_px as i32);
    let shifted = renderer.render_to_image_sized(&gpu, clear, w, h);

    let lit = |px: [u8; 4]| px[0] > 0 || px[1] > 0 || px[2] > 0;
    let mut band_lit = 0;
    for y in 0..h {
        for x in 0..w {
            let in_grid = x >= pad && x < pad + grid_w && y >= pad && y < pad + grid_h;
            let f = flat.pixel(x, y);
            let s = shifted.pixel(x, y);
            if !in_grid {
                assert!(!lit(f) && !lit(s), "padding lit at ({x},{y})");
                continue;
            }
            let gy = y - pad;
            if gy + shift_px < grid_h {
                assert_eq!(
                    s,
                    flat.pixel(x, y + shift_px),
                    "({x},{y}): the shifted picture is the flat one {shift_px} rows lower"
                );
            } else if lit(s) {
                band_lit += 1;
            }
        }
    }
    assert!(
        band_lit > 0,
        "the band under the last row shows the top of the next line"
    );

    // At the bottom, drawn flat, the spare row is not on the glass.
    view.to_bottom(&mut term);
    renderer.sync(
        &gpu.device,
        &gpu.queue,
        &mut font,
        &mut term,
        &mut view,
        None,
        None,
    );
    renderer.set_origin(pad as i32, pad as i32, 0);
    let bottom = renderer.render_to_image_sized(&gpu, clear, w, h);
    for y in pad + grid_h..h {
        for x in 0..w {
            assert!(!lit(bottom.pixel(x, y)), "below the grid lit at ({x},{y})");
        }
    }
}
