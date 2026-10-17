//! The drawn half of a critter: `robco-critters` proves the figure walks,
//! this proves the glass shows it and gives the screen back afterwards.
//!
//! Measured in pixels off a real device, like every other picture claim in
//! this crate, and for the same reason `selection_paint.rs` gives: a state
//! assertion would pass throughout a bug nobody could see.
//!
//! What is pinned down here:
//!
//! * a critter is drawn as **its own characters** in the cells it stands on,
//!   in the phosphor, and everything else on the screen is untouched;
//! * taking it down puts the screen back **bit for bit**, because the cells
//!   the renderer keeps were never written to -- which is the same property
//!   that makes a selection copied across a critter yield what the session
//!   sent;
//! * it repaints **the rows it arrived on and left, and no others**, and a
//!   critter that has not moved since the last frame repaints nothing;
//! * the block cursor standing under a critter still shows the **session's**
//!   character, so a critter passes behind the cursor rather than through it.

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use gpu::harness::GpuLock;
use gpu::{Gpu, Image};
use term::atlas::Rasterization;
use term::color::Scheme;
use term::fonts::sizing::{self, SizingRequest};
use term::fonts::{font_by_name, FontSource};
use term::render::GridRenderer;
use term::viewport::ScrollPosition;
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

const COLS: usize = 40;
const ROWS: usize = 6;
const SCROLLBACK: usize = 200;

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

fn cell_rect(renderer: &GridRenderer, col: usize, row: usize) -> (u32, u32, u32, u32) {
    let cell = renderer.atlas().cell;
    (
        col as u32 * cell.width,
        row as u32 * cell.height,
        cell.width,
        cell.height,
    )
}

fn identical_outside(a: &Image, b: &Image, rects: &[(u32, u32, u32, u32)]) {
    for y in 0..a.height {
        for x in 0..a.width {
            let inside = rects
                .iter()
                .any(|(rx, ry, rw, rh)| x >= *rx && x < rx + rw && y >= *ry && y < ry + rh);
            if !inside {
                assert_eq!(a.pixel(x, y), b.pixel(x, y), "pixel {x},{y} moved");
            }
        }
    }
}

/// A little critter: two characters on row 2, well away from the cursor.
fn figure(col: usize) -> Vec<(usize, usize, char)> {
    vec![(2, col, '>'), (2, col + 1, '<')]
}

#[test]
fn a_critter_is_drawn_in_the_cells_and_gives_them_back() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    processor.advance(&mut term, b"HELLO WORLD\r\nSECOND LINE\r\nTHIRD LINE\r\n");
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
    let grid_before = renderer.grid().clone();

    let critter = figure(3);
    let rows = renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &critter);
    assert_eq!(rows, 1, "a critter on one row rebuilt {rows} rows");
    let visited = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    let rects: Vec<_> = (3..=4).map(|col| cell_rect(&renderer, col, 2)).collect();
    assert_ne!(
        plain.pixels, visited.pixels,
        "the critter drew nothing at all"
    );
    identical_outside(&plain, &visited, &rects);

    // The cells the renderer keeps still hold what the session wrote. This is
    // the property the whole feature rests on: the terminal never learns a
    // critter was there, so text scrolls behind it and a copy yields the
    // session's own characters.
    assert_eq!(
        renderer.grid().cells,
        grid_before.cells,
        "the critter was written into the screen the renderer keeps"
    );

    // And down again, bit for bit.
    let rows = renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &[]);
    assert_eq!(rows, 1);
    let after = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    assert_eq!(
        plain.pixels, after.pixels,
        "the screen did not come back the way the critter found it"
    );
}

#[test]
fn a_critter_that_has_not_moved_repaints_nothing() {
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

    let critter = figure(3);
    assert_eq!(
        renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &critter),
        1
    );
    // The frame arrives every fifty milliseconds whether or not the critter
    // has taken a step, so the still frames have to be free.
    for _ in 0..5 {
        assert_eq!(
            renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &critter),
            0,
            "a critter standing still cost a row rewrite"
        );
    }

    // One step to the right touches its row once, not twice.
    assert_eq!(
        renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &figure(4)),
        1
    );

    // A step onto another row touches both.
    let two_rows = vec![(2, 4, '>'), (3, 4, '<')];
    assert_eq!(
        renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &two_rows),
        2
    );
}

#[test]
fn the_cursor_under_a_critter_still_shows_the_session() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = Scheme::monochrome(PHOSPHOR, BEHIND);
    let (mut renderer, mut font) = renderer(&gpu, &scheme);
    let mut viewport = ScrollPosition::default();
    let (mut term, mut processor) = terminal();

    // The cursor sits on row 0, column 5, inside the word.
    processor.advance(&mut term, b"HELLO");
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

    // Walk a critter straight over the cursor's own cell.
    let over_cursor = vec![(0, 5, '@')];
    renderer.set_critter(&gpu.device, &gpu.queue, &mut font, &over_cursor);
    let visited = renderer.render_to_image(&gpu, wgpu::Color::BLACK);

    // The cursor's cell is drawn from the grid, which the critter never
    // touched, so the block is exactly as it was: the critter went behind it.
    let cursor_cell = cell_rect(&renderer, 5, 0);
    let (x0, y0, w, h) = cursor_cell;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            assert_eq!(
                plain.pixel(x, y),
                visited.pixel(x, y),
                "the critter got inside the cursor at {x},{y}"
            );
        }
    }
}
