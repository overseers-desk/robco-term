//! Three pixel properties, asserted against the real renderer.
//!
//! A property proved about scaffolding is not proved about production code;
//! the proof expires the moment the code it measured is replaced. These run
//! the same measurements through `GridRenderer`, on the same two faces, and
//! add the counterexample below, because a property that holds is worth less
//! than a property that holds *and* whose violation is shown to be detectable:
//!
//! 1. **Antialiasing off.** Every byte in the atlas and every colour channel
//!    in the readback is 0 or 255. The control is the same face at a size it
//!    was not designed for, where the unthresholded rasteriser does produce
//!    intermediate values; without it, "no intermediate values" could just
//!    mean the measurement is blind.
//! 2. **Integer scaling.** The 2x and 3x renders equal a nearest-neighbour
//!    duplication of the 1x render, pixel for pixel. Equality rather than
//!    similarity: a one-pixel stem shift is invisible to a tolerance and
//!    obvious on screen.
//! 3. **DPR change mid-run.** The device pixel ratio moves, the atlas is not
//!    rebuilt, the raster size does not move, and the output stays binary and
//!    exactly duplicated.
//!
//! The counterexample, kept as a test of its own: re-rasterising at twice the
//! size instead of scaling the geometry corrupts Terminess (3960 differing
//! pixels) while passing on Pet Me. A shortcut that looks fine
//! on one bundled face and wrecks another is exactly the kind that gets taken.
//!
//! The colours are deliberately white on black. These properties are about
//! coverage, not about phosphor: amber's green channel is 176, so every lit
//! pixel would count as an intermediate value and the measurement would say
//! nothing about antialiasing at all.

use gpu::harness::GpuLock;
use term::atlas::Rasterization;
use term::cells::CellGrid;
use term::color::Scheme;
use term::fonts::sizing::{self, ResolvedFont, SizingRequest};
use term::fonts::{self, font_by_name, FontEntry, FontSource};
use gpu::{Gpu, Image};
use term::render::GridRenderer;

/// The two faces used for measurement, looked up in the bundled catalogue.
/// They used to be `const FontSpec`s; the catalogue is the one home for
/// bundled-face metadata now, so they are looked up by their catalogue name.
fn terminess() -> &'static FontEntry {
    font_by_name("TERMINESS_SCALED", FontSource::Bundled)
        .expect("TERMINESS_SCALED in the catalogue")
}

fn commodore_pet() -> &'static FontEntry {
    font_by_name("COMMODORE_PET_SCALED", FontSource::Bundled)
        .expect("COMMODORE_PET_SCALED in the catalogue")
}
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

const LINES: &[&str] = &[
    "ROBCO INDUSTRIES (TM) TERMLINK",
    "ENTER PASSWORD NOW",
    "WXYZ 0123456789 |[]{}#@",
    "iiil ||| ---- .... ####",
];

const COLS: usize = 32;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// Every property is stated about bytes read back from the GPU, so a machine
/// with no adapter cannot answer them. It says so and stops rather than
/// passing vacuously; `ROBCO_SKIP_GPU_TESTS=1` is the deliberate opt-out.
///
/// The device comes with the machine-wide GPU lock, which is what keeps this
/// binary from holding a device at the same time as another process's chain
/// tests. `gpu::Gpu` is the shipping type and takes no lock of its own --
/// the application is not competing with anyone -- so the lock is taken here,
/// where the competition is. The tuple's order is its drop order: the device
/// goes first, the lock after it.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = match GpuLock::acquire() {
        Ok(lock) => lock,
        Err(e) => panic!("cannot take the GPU lock: {e}"),
    };
    match Gpu::new() {
        Ok(gpu) => {
            eprintln!("adapter: {} ({})", gpu.adapter_name, gpu.backend);
            Some((gpu, lock))
        }
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

fn scheme() -> Scheme {
    // Transparent background, so the clear colour is the background and the
    // only thing the renderer contributes is glyph coverage.
    Scheme::monochrome(WHITE, TRANSPARENT)
}

fn fixture(spec: &FontEntry, gpu: &Gpu, request: SizingRequest) -> (ResolvedFont, GridRenderer) {
    let scheme = scheme();
    let resolved = sizing::resolve(spec, &request);
    let mut font = FontContext::new(spec);
    assert_eq!(
        font.family, spec.family,
        "bundled face reports {:?} but the metadata says {:?}; faces are looked \
         up by family name elsewhere, so a drift here silently falls back",
        font.family, spec.family
    );
    let atlas = font.build_atlas(
        &gpu.device,
        &gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::Binary {
            threshold: DEFAULT_THRESHOLD,
        },
    );
    let renderer = GridRenderer::new(
        &gpu.device,
        &gpu.queue,
        atlas,
        COLS,
        LINES.len(),
        scheme.clone(),
    );
    let mut renderer = renderer;
    let grid = CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme);
    renderer.set_grid(&gpu.queue, &grid, None);
    (resolved, renderer)
}

fn render(gpu: &Gpu, renderer: &GridRenderer) -> Image {
    renderer.render_to_image(gpu, wgpu::Color::BLACK)
}

#[test]
fn property_1_antialiasing_is_off() {
    let Some((gpu, _lock)) = gpu() else { return };
    for spec in [terminess(), commodore_pet()] {
        let (resolved, renderer) = fixture(spec, &gpu, SizingRequest::default());
        let atlas = renderer.atlas();
        assert_eq!(
            atlas.intermediate_value_count(),
            0,
            "{}: {} of {} atlas bytes were neither 0 nor 255",
            spec.name,
            atlas.intermediate_value_count(),
            atlas.total_value_count()
        );

        let image = render(&gpu, &renderer);
        assert!(
            image.lit_pixels() > 100,
            "{}: only {} lit pixels, so nothing was drawn to measure",
            spec.name,
            image.lit_pixels()
        );
        assert_eq!(
            image.intermediate_channel_values(),
            0,
            "{}: {} intermediate channel values in the readback (distinct luma {:?})",
            spec.name,
            image.intermediate_channel_values(),
            image.distinct_luma_values()
        );
        eprintln!(
            "{}: raster {}px, cell {}x{}, {} lit pixels, distinct luma {:?}",
            spec.name,
            resolved.raster_pixel_size,
            atlas.cell.width,
            atlas.cell.height,
            image.lit_pixels(),
            image.distinct_luma_values()
        );
    }
}

#[test]
fn property_1_control_the_measurement_can_see_antialiasing() {
    let Some((gpu, _lock)) = gpu() else { return };
    for spec in [terminess(), commodore_pet()] {
        let resolved = sizing::resolve(spec, &SizingRequest::default());
        // A size the face was never drawn for. Every face antialiases here, so
        // this control holds whatever a face does at its native size.
        //
        // The `+ 1` is load-bearing on Terminess, which carries embedded bitmap
        // strikes at 12, 14, 16, 18, 20, 22, 24, 28 and 32 ppem: 12 * 3 / 2 is
        // 18, a strike, and `fonts::raster` would hand back that picture rather
        // than a rasterised outline. A control has to ask for a size the face
        // really does not have.
        let off_grid = ResolvedFont {
            raster_pixel_size: resolved.raster_pixel_size * 3 / 2 + 1,
            integer_scale: 1,
            ..resolved.clone()
        };
        let mut font = FontContext::new(spec);
        let coverage = font.build_atlas(
            &gpu.device,
            &gpu.queue,
            &off_grid,
            &ascii_charset(),
            Rasterization::Coverage,
        );
        let thresholded = font.build_atlas(
            &gpu.device,
            &gpu.queue,
            &off_grid,
            &ascii_charset(),
            Rasterization::Binary {
                threshold: DEFAULT_THRESHOLD,
            },
        );
        assert!(
            coverage.intermediate_value_count() > 0,
            "{}: even at {}px the unthresholded atlas was binary, so property 1 \
             proves nothing",
            spec.name,
            off_grid.raster_pixel_size
        );
        assert_eq!(
            thresholded.intermediate_value_count(),
            0,
            "{}: {} intermediate bytes survived thresholding",
            spec.name,
            thresholded.intermediate_value_count()
        );
        eprintln!(
            "{}: off-grid {}px, {} of {} coverage bytes intermediate, {} after \
             thresholding",
            spec.name,
            off_grid.raster_pixel_size,
            coverage.intermediate_value_count(),
            coverage.total_value_count(),
            thresholded.intermediate_value_count()
        );
    }
}

/// Property 1's premise, which property 1 itself cannot see: at its design
/// size a low-resolution face has *nothing to threshold*.
///
/// Thresholding a coverage mask is a lossy answer to a question that does not
/// need asking here: with antialiasing off, rendering goes straight to the
/// face's own picture at that ppem, and the picture is already one bit deep.
/// Where the rebuild has to threshold heavily, it is
/// rasterising something else -- and a 50% threshold over a stem split across
/// two pixels at 96/255 apiece deletes the stem outright, which is how a
/// prior version shipped a Terminess whose hyphens were absent, whose `i`
/// read as `:` and whose `w` read as `u`.
///
/// Measured before [`term::fonts::raster`] existed: `TERMINESS_SCALED` came
/// back 2395 intermediate bytes of 3653 with exactly **one** fully-inked pixel
/// in the whole ASCII atlas, and `GOHU_11_SCALED` 1360 of 3321. Those two are
/// the catalogue's faces whose design ppem carries an embedded strike their
/// outline does not match; every other low-resolution face is an outline drawn
/// on the pixel grid and was already binary.
///
/// The bar is a small budget rather than zero because of two faces, both named
/// in the run's own output: Departure Mono is an OTF with no strike at all and
/// leaves 40 bytes of 3240 grey, and Atari 400-800 leaves two. Terminess, the
/// face the shipped profile wears, is held to zero.
#[test]
fn property_1_premise_a_low_resolution_face_needs_no_threshold() {
    let Some((gpu, _lock)) = gpu() else { return };
    // Every low-resolution face a user can select. A face carried only to
    // cover another's gaps has no menu label and no ASCII to draw.
    for spec in fonts::bundled_fonts()
        .iter()
        .filter(|f| f.low_resolution && !f.text.is_empty())
    {
        let resolved = sizing::resolve(spec, &SizingRequest::default());
        assert_eq!(
            resolved.raster_pixel_size, spec.pixel_size,
            "{}: a low-resolution face is rasterised at its design size and \
             nowhere else",
            spec.name
        );
        let mut font = FontContext::new(spec);
        let coverage = font.build_atlas(
            &gpu.device,
            &gpu.queue,
            &resolved,
            &ascii_charset(),
            Rasterization::Coverage,
        );
        let (grey, total) = (
            coverage.intermediate_value_count(),
            coverage.total_value_count(),
        );
        eprintln!(
            "{:<24} {}px: {grey} of {total} coverage bytes intermediate, {} inked",
            spec.name, resolved.raster_pixel_size, coverage.value_histogram[255],
        );
        assert!(
            total > 0 && coverage.value_histogram[255] * 10 > total,
            "{}: {} of {total} bytes inked, so the rasteriser drew almost nothing \
             to measure",
            spec.name,
            coverage.value_histogram[255]
        );
        assert!(
            grey * 50 <= total,
            "{}: {grey} of {total} coverage bytes are neither ink nor paper before \
             any threshold. A low-resolution face at its design size is a picture, \
             not an outline to sample: this is the outline, and the threshold that \
             follows will eat every stem that lands between two pixels",
            spec.name
        );
    }
    let terminess = font_by_name("TERMINESS_SCALED", FontSource::Bundled).expect("catalogue");
    let resolved = sizing::resolve(terminess, &SizingRequest::default());
    let mut font = FontContext::new(terminess);
    let coverage = font.build_atlas(
        &gpu.device,
        &gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::Coverage,
    );
    assert_eq!(
        coverage.intermediate_value_count(),
        0,
        "TERMINESS_SCALED is the shipped profile's face and carries a strike at \
         its design ppem: every byte of it should arrive already binary"
    );
}

#[test]
fn property_2_integer_scaling_is_exact_duplication() {
    let Some((gpu, _lock)) = gpu() else { return };
    for spec in [terminess(), commodore_pet()] {
        let (_, mut renderer) = fixture(spec, &gpu, SizingRequest::default());
        renderer.set_scale(1);
        let one = render(&gpu, &renderer);

        for factor in [2u32, 3] {
            renderer.set_scale(factor);
            let scaled = render(&gpu, &renderer);
            let reference = one.upscale_nearest(factor);
            let diff = scaled.diff(&reference);
            assert_eq!(
                diff.differing,
                0,
                "{} at {factor}x: {}",
                spec.name,
                diff.describe()
            );
            assert_eq!(
                scaled.intermediate_channel_values(),
                0,
                "{} at {factor}x: {} intermediate channel values",
                spec.name,
                scaled.intermediate_channel_values()
            );
            eprintln!(
                "{}: {factor}x render {}x{} equals {factor}x duplication of 1x",
                spec.name, scaled.width, scaled.height
            );
        }
    }
}

#[test]
fn property_3_dpr_changes_do_not_touch_the_atlas() {
    let Some((gpu, _lock)) = gpu() else { return };
    for spec in [terminess(), commodore_pet()] {
        let (base, mut renderer) = fixture(spec, &gpu, SizingRequest::default());
        renderer.set_scale(1);
        let one = render(&gpu, &renderer);
        let atlas_size_before = renderer.atlas().raster_pixel_size;

        for dpr in [1.0f64, 2.0, 3.0, 1.5, 2.5] {
            let resolved = sizing::resolve(
                spec,
                &SizingRequest {
                    device_pixel_ratio: dpr,
                    ..Default::default()
                },
            );
            assert_eq!(
                resolved.raster_pixel_size, base.raster_pixel_size,
                "{}: raster size moved at dpr {dpr}",
                spec.name
            );
            let nearest = (24.0 * dpr / spec.pixel_size as f64).round() as u32;
            assert_eq!(
                resolved.integer_scale,
                nearest.max(1),
                "{}: dpr {dpr} is not one whole magnification of size and density",
                spec.name
            );

            // The atlas is deliberately *not* rebuilt: the renderer keeps the
            // one it was constructed with, and only the scale changes.
            renderer.set_scale(resolved.integer_scale);
            let image = render(&gpu, &renderer);
            assert_eq!(
                renderer.atlas().raster_pixel_size,
                atlas_size_before,
                "{}: the atlas was rebuilt behind a dpr change",
                spec.name
            );
            assert_eq!(
                image.intermediate_channel_values(),
                0,
                "{} at dpr {dpr}: {} intermediate channel values",
                spec.name,
                image.intermediate_channel_values()
            );
            let diff = image.diff(&one.upscale_nearest(resolved.integer_scale));
            assert_eq!(
                diff.differing,
                0,
                "{} at dpr {dpr} (integer_scale {}): {}",
                spec.name,
                resolved.integer_scale,
                diff.describe()
            );
            eprintln!(
                "{}: dpr {dpr} -> integer_scale {}, {}x{}, exact",
                spec.name, resolved.integer_scale, image.width, image.height
            );
        }
    }
}

#[test]
fn counterexample_re_rasterising_is_not_the_same_as_scaling() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = scheme();
    for (spec, must_differ) in [(terminess(), true), (commodore_pet(), false)] {
        let (resolved, mut renderer) = fixture(spec, &gpu, SizingRequest::default());
        renderer.set_scale(2);
        let scaled = render(&gpu, &renderer);
        renderer.set_scale(1);
        let one = render(&gpu, &renderer);
        assert_eq!(scaled.diff(&one.upscale_nearest(2)).differing, 0);

        // The shortcut: ask the rasteriser for a bigger font instead of
        // scaling the geometry. Same face, same text, twice the pixel size.
        let naive = ResolvedFont {
            raster_pixel_size: resolved.raster_pixel_size * 2,
            integer_scale: 1,
            ..resolved.clone()
        };
        let mut font = FontContext::new(spec);
        let atlas = font.build_atlas(
            &gpu.device,
            &gpu.queue,
            &naive,
            &ascii_charset(),
            Rasterization::Binary {
                threshold: DEFAULT_THRESHOLD,
            },
        );
        let mut re_rasterised = GridRenderer::new(
            &gpu.device,
            &gpu.queue,
            atlas,
            COLS,
            LINES.len(),
            scheme.clone(),
        );
        let grid = CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme);
        re_rasterised.set_grid(&gpu.queue, &grid, None);
        let image = render(&gpu, &re_rasterised);

        let diff = image.diff(&scaled);
        eprintln!(
            "{}: re-rasterised at {}px differs from the 2x geometry render in {} pixels",
            spec.name, naive.raster_pixel_size, diff.differing
        );
        if must_differ {
            assert!(
                diff.differing > 0,
                "{}: re-rasterising matched the scaled render, so the \
                 counterexample no longer discriminates and the sizing rule has \
                 lost its evidence",
                spec.name
            );
        }
    }
}

#[test]
fn damage_updates_only_the_rows_that_changed() {
    let Some((gpu, _lock)) = gpu() else { return };
    let scheme = scheme();
    let (_, mut renderer) = fixture(terminess(), &gpu, SizingRequest::default());
    let before = render(&gpu, &renderer);

    // One row rewritten through the per-row path: the rest of the screen must
    // come back byte-identical, which is the property the fixed instance
    // stride exists to give.
    let mut grid = CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme);
    for (col, c) in "CHANGED".chars().enumerate() {
        grid.row_mut(1)[col].c = c;
    }
    renderer.set_row(&gpu.queue, 1, grid.row(1));
    let after = render(&gpu, &renderer);

    let cell_h = renderer.atlas().cell.height;
    let row_top = cell_h;
    let row_bottom = cell_h * 2;
    let mut changed_rows = std::collections::BTreeSet::new();
    for y in 0..after.height {
        for x in 0..after.width {
            if before.pixel(x, y) != after.pixel(x, y) {
                changed_rows.insert(y / cell_h);
            }
        }
    }
    assert!(
        !changed_rows.is_empty(),
        "rewriting a row changed no pixels at all"
    );
    assert_eq!(
        changed_rows,
        std::collections::BTreeSet::from([1]),
        "pixels changed outside the rewritten row (rows {changed_rows:?}, \
         row 1 spans y {row_top}..{row_bottom})"
    );
}
