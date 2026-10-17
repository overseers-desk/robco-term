//! The input method's composition, drawn at the cursor, in pixels.
//!
//! The pre-edit composition is drawn inside the terminal display as a block
//! over the cells from the cursor rightwards, with the half-typed word struck
//! into it inverted. Everything about that claim is a pixel claim, so it is measured
//! rather than asserted about state:
//!
//! - the composition **lights the cells at the cursor** and no others;
//! - the rest of the frame is **bit-identical** to the frame without it, which
//!   is what says the composition is an overlay on the grid rather than an edit
//!   of it;
//! - it **follows the cursor**;
//! - and it **leaves nothing behind** when the composition ends, which is the
//!   commit and the abandon both.
//!
//! ASCII on purpose. A real composition is CJK, and the bundled phosphor faces
//! have no CJK glyphs -- typing Japanese draws plates with nothing in them,
//! exactly as typing it into the grid does -- so the glyph half of the claim is
//! measured on characters this appliance can actually strike. The romaji stage
//! of a Japanese composition is this, literally: `nihon` before the conversion.

use gpu::harness::GpuLock;
use term::atlas::Rasterization;
use term::cells::{CellGrid, CursorShape, CursorState};
use term::color::Scheme;
use term::fonts::sizing::{self, SizingRequest};
use term::fonts::{font_by_name, FontEntry, FontSource};
use gpu::{Gpu, Image};
use term::render::GridRenderer;
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

const COLS: usize = 32;
const LINES: &[&str] = &["ROBCO INDUSTRIES (TM) TERMLINK", "> ", ""];

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// Where the shell's cursor stands: the prompt on row 1.
const CURSOR_ROW: usize = 1;
const CURSOR_COL: usize = 2;

fn terminess() -> &'static FontEntry {
    font_by_name("TERMINESS_SCALED", FontSource::Bundled)
        .expect("TERMINESS_SCALED in the catalogue")
}

/// Same rule as `pixel_properties.rs`: real bytes off a real device, under the
/// machine-wide GPU lock, and a machine with no adapter says so rather than
/// passing vacuously.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = GpuLock::acquire().expect("the GPU lock");
    match Gpu::new() {
        Ok(gpu) => Some((gpu, lock)),
        Err(e) => {
            if std::env::var("ROBCO_SKIP_GPU_TESTS").is_ok() {
                eprintln!("skipping: no wgpu adapter ({e}), ROBCO_SKIP_GPU_TESTS set");
                None
            } else {
                panic!("no wgpu adapter: {e}");
            }
        }
    }
}

fn cursor_at(col: usize, row: usize) -> CursorState {
    CursorState {
        col,
        row,
        shape: CursorShape::Block,
        // What `render::vt::cursor_state` fills in from the scheme: the cursor's
        // own colour, and the background for the character under it.
        color: WHITE,
        text_color: BLACK,
    }
}

fn fixture(gpu: &Gpu) -> GridRenderer {
    let scheme = Scheme::monochrome(WHITE, TRANSPARENT);
    let spec = terminess();
    let resolved = sizing::resolve(spec, &SizingRequest::default());
    let mut font = FontContext::new(spec);
    let atlas = font.build_atlas(
        &gpu.device,
        &gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::Binary {
            threshold: DEFAULT_THRESHOLD,
        },
    );
    let mut renderer = GridRenderer::new(
        &gpu.device,
        &gpu.queue,
        atlas,
        COLS,
        LINES.len(),
        scheme.clone(),
    );
    let grid = CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme);
    renderer.set_grid(&gpu.queue, &grid, Some(cursor_at(CURSOR_COL, CURSOR_ROW)));
    renderer
}

fn render(gpu: &Gpu, renderer: &GridRenderer) -> Image {
    renderer.render_to_image(gpu, wgpu::Color::BLACK)
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

fn lit_in(image: &Image, rect: (u32, u32, u32, u32)) -> usize {
    let (x0, y0, w, h) = rect;
    let mut lit = 0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            if image.pixel(x, y)[0] > 128 {
                lit += 1;
            }
        }
    }
    lit
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
                "the frame changed at ({x},{y}), outside the composition"
            );
        }
    }
}

#[test]
fn the_composition_is_drawn_at_the_cursor_and_leaves_when_it_does() {
    let Some((gpu, _lock)) = gpu() else { return };
    let mut renderer = fixture(&gpu);

    let before = render(&gpu, &renderer);
    let composing = "nihon";

    renderer.set_preedit(&gpu.queue, composing);
    assert_eq!(renderer.preedit(), composing);
    let during = render(&gpu, &renderer);

    // The cells the composition stands on, from the cursor rightwards.
    let cells: Vec<_> = (0..composing.chars().count())
        .map(|i| cell_rect(&renderer, CURSOR_COL + i, CURSOR_ROW))
        .collect();

    // Each one is a lit plate with a dark character struck into it: most of the
    // cell is on, and some of it is not (the glyph). A plate with no glyph would
    // be wholly lit, and a glyph with no plate barely lit at all.
    for (i, rect) in cells.iter().enumerate() {
        let area = (rect.2 * rect.3) as usize;
        let lit = lit_in(&during, *rect);
        assert!(
            lit > area / 2,
            "cell {i} of the composition is {lit}/{area} lit: no plate under it"
        );
        assert!(
            lit < area,
            "cell {i} of the composition is wholly lit: the character was not struck into it"
        );
    }

    // The first cell had the block cursor on it before, so it was already lit;
    // the ones after it were empty prompt cells and are lit only now.
    for (i, rect) in cells.iter().enumerate().skip(1) {
        assert!(
            lit_in(&during, *rect) > lit_in(&before, *rect),
            "cell {i} of the composition looks exactly as it did before it was typed"
        );
    }

    // Nothing else on the tube moved: the composition is drawn over the grid,
    // not written into it.
    identical_outside(&before, &during, &cells);

    // A commit (or an abandon) ends the composition, and the frame comes back
    // exactly as it was -- bit for bit, including the cursor it was standing on.
    renderer.set_preedit(&gpu.queue, "");
    let after = render(&gpu, &renderer);
    identical_outside(&before, &after, &[]);
}

/// The composition stands at the cursor, so when the cursor moves it moves --
/// the composition rectangle tracks the cursor position directly.
#[test]
fn the_composition_follows_the_cursor() {
    let Some((gpu, _lock)) = gpu() else { return };
    let mut renderer = fixture(&gpu);
    renderer.set_preedit(&gpu.queue, "ab");

    let at_prompt = render(&gpu, &renderer);
    renderer.set_cursor(&gpu.queue, Some(cursor_at(CURSOR_COL + 6, CURSOR_ROW)));
    let moved = render(&gpu, &renderer);

    let old_second = cell_rect(&renderer, CURSOR_COL + 1, CURSOR_ROW);
    let new_second = cell_rect(&renderer, CURSOR_COL + 7, CURSOR_ROW);
    assert!(
        lit_in(&moved, old_second) < lit_in(&at_prompt, old_second),
        "the composition left nothing behind at the cursor's old cell"
    );
    assert!(
        lit_in(&moved, new_second) > lit_in(&at_prompt, new_second),
        "the composition did not arrive at the cursor's new cell"
    );
}

/// A composition longer than the room to its right is clipped at the last
/// column rather than wrapping into the next row or writing past the buffer.
#[test]
fn a_composition_past_the_last_column_is_clipped() {
    let Some((gpu, _lock)) = gpu() else { return };
    let mut renderer = fixture(&gpu);
    let last = COLS - 2;
    renderer.set_cursor(&gpu.queue, Some(cursor_at(last, CURSOR_ROW)));
    let before = render(&gpu, &renderer);

    // Twice the screen's width of composition, starting two cells from the end.
    renderer.set_preedit(&gpu.queue, &"x".repeat(COLS * 2));
    let during = render(&gpu, &renderer);

    let clipped: Vec<_> = (last..COLS)
        .map(|col| cell_rect(&renderer, col, CURSOR_ROW))
        .collect();
    identical_outside(&before, &during, &clipped);
    assert!(
        lit_in(&during, clipped[1]) > lit_in(&before, clipped[1]),
        "the composition did not reach the last column"
    );
}
