//! The antialiasing-on path: the switch is on for every entry that is not a
//! low-resolution face, measured on both sides of the switch through the
//! renderer that ships.
//!
//! `pixel_properties` states the low-resolution half -- every atlas byte and
//! every readback channel is 0 or 255 -- and it is deliberately not touched by
//! this file. That half is a pixel-exactness claim about the low-resolution
//! rendering path, and a claim like that is worth nothing if the test carrying
//! it moves whenever something new is added next to it. So the scalable half is
//! asserted here, in its own file, about the same two things in the same two
//! places: the bytes in the atlas, and the channels in the readback.
//!
//! The measurement is taken on the picture the grid renderer presents -- the
//! texture the CRT chain takes as its input -- and not past the chain. Not an
//! omission: the chain adds static noise, a glowing line and a bloom, so every
//! pixel past it holds intermediate values whatever the atlas did, and a count
//! of them there would be a count of the noise. `crt-render`'s
//! `glyph_survival` is what says the strokes then reach the glass, and it is
//! the same picture going in.
//!
//! Colours are white on transparent for the reason `pixel_properties` gives:
//! amber's green channel is 176, so on the shipped scheme every lit pixel would
//! count as an intermediate value and the measurement would say nothing about
//! antialiasing at all.
//!
//! ## Antialiasing on is not "outlines always"
//!
//! Turning the switch on does not change *what is rasterised*, only whether the
//! coverage that comes back is thresholded. `fonts::raster` stays one rule for
//! both modes -- embedded strike first, outline as the fallback. Antialiasing
//! only changes how outline drawing renders (very large or transformed text);
//! with antialiasing on, an embedded bitmap at the requested ppem is still
//! loaded when one exists, and that 1-bit picture is converted into an 8-bit
//! mask holding 0 and 255.
//!
//! `TERMINESS` is the catalogue entry where this is visible. It is a scalable
//! entry, so it takes `Rasterization::Coverage`, but the file carries strikes at
//! 12, 14, 16, 18, 20, 22, 24, 28 and 32 ppem and the shipped request resolves
//! it to 24 -- a strike. The coverage that comes back is already binary, the
//! no-op threshold changes nothing, and the picture stands as the strike drew
//! it. That is asserted below rather than worked around, and the same face
//! one pixel off a strike is the counterexample that keeps the assertion from
//! being a description of a bug.

use gpu::harness::GpuLock;
use term::atlas::Rasterization;
use term::cells::CellGrid;
use term::color::Scheme;
use term::fonts::{font_by_name, FontSource};
use term::fonts::sizing::SizingRequest;
use gpu::{Gpu, Image};
use term::render::GridRenderer;
use term::FontEntry;

/// Text with curves and diagonals in it. A face is antialiased on the edges
/// that are not axis-aligned, so a line of `|` and `-` would understate the
/// case for the scalable face and prove nothing extra about the pixel one.
const LINES: &[&str] = &[
    "ROBCO INDUSTRIES (TM) TERMLINK",
    "SeCuRe CoMmS 0369 @&%$",
    "WXYZ 0123456789 |[]{}#@",
    "iiil ||| ---- .... ####",
];

const COLS: usize = 32;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// Same shape as `pixel_properties`': a real device, the machine-wide lock so
/// two checkouts do not hold one at once, and a loud stop rather than a vacuous
/// pass where there is no adapter.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = GpuLock::acquire().expect("cannot take the GPU lock");
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

/// What the picture is made of: the atlas's own bytes and the readback's
/// channels, counted the same way on both sides of the switch.
struct Ink {
    mode: Rasterization,
    atlas_intermediate: u64,
    atlas_total: u64,
    image_intermediate: usize,
    lit: usize,
}

/// Build and draw one face **through the path the application takes**:
/// `term::build_font` picks the rasterisation, and nothing here may override
/// it. That is the point of the measurement -- a test that passed the mode in
/// itself would be measuring the test's opinion of the catalogue.
fn ink(gpu: &Gpu, entry: &FontEntry) -> Ink {
    ink_at(gpu, entry, SizingRequest::default())
}

fn ink_at(gpu: &Gpu, entry: &FontEntry, request: SizingRequest) -> Ink {
    let scheme = Scheme::monochrome(WHITE, TRANSPARENT);
    let (resolved, _font, atlas) = term::build_font(&gpu.device, &gpu.queue, entry, &request);
    let mode = atlas.rasterization;
    let (atlas_intermediate, atlas_total) =
        (atlas.intermediate_value_count(), atlas.total_value_count());

    let mut renderer = GridRenderer::new(
        &gpu.device,
        &gpu.queue,
        atlas,
        COLS,
        LINES.len(),
        scheme.clone(),
    );
    renderer.set_scale(resolved.integer_scale);
    renderer.set_grid(
        &gpu.queue,
        &CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme),
        None,
    );
    let image: Image = renderer.render_to_image(gpu, wgpu::Color::BLACK);

    let ink = Ink {
        mode,
        atlas_intermediate,
        atlas_total,
        image_intermediate: image.intermediate_channel_values(),
        lit: image.lit_pixels(),
    };
    eprintln!(
        "{}: {mode:?}, raster {}px, scale {}x, atlas {}/{} intermediate bytes, \
         readback {} intermediate channels over {} lit pixels",
        entry.name,
        resolved.raster_pixel_size,
        resolved.integer_scale,
        ink.atlas_intermediate,
        ink.atlas_total,
        ink.image_intermediate,
        ink.lit,
    );
    ink
}

/// The done-test, both halves of it in one run so the pair is the evidence: a
/// scalable face arrives with coverage in the presented picture, and the
/// low-resolution face it is measured against does not.
///
/// One test rather than two, because either half alone is weak. "Terminess has
/// no intermediate values" could mean the readback is blind; "Hack has some"
/// could mean the readback is noisy. The two together, from one device and one
/// picture, can only mean the switch is doing what the antialiasing flag says
/// it should.
#[test]
fn the_scalable_half_is_antialiased_and_the_pixel_half_is_not() {
    let Some((gpu, _lock)) = gpu() else { return };

    // Hack rather than `TERMINESS`, and the module doc says why: Terminess is
    // a bitmap-derived face carrying a strike at the size the shipped request
    // resolves to, so it is the exception rather than the case. Hack is an
    // outline face, which is what seven of the eight scalable entries are.
    let scalable = font_by_name("HACK", FontSource::Bundled).expect("HACK");
    let pixel = font_by_name("TERMINESS_SCALED", FontSource::Bundled).expect("TERMINESS_SCALED");

    let aa = ink(&gpu, scalable);
    let binary = ink(&gpu, pixel);

    assert_eq!(
        aa.mode,
        Rasterization::Coverage,
        "a scalable face took the thresholding path"
    );
    assert!(
        matches!(binary.mode, Rasterization::Binary { .. }),
        "a low-resolution face took the antialiasing path"
    );

    assert!(
        aa.lit > 100 && binary.lit > 100,
        "one of the two drew almost nothing ({} and {} lit pixels)",
        aa.lit,
        binary.lit
    );

    // Present for the scalable face, in the atlas and in the picture drawn
    // from it. The shader carries the byte through as alpha; if it thresholded
    // anywhere, or if the pipeline dropped the fractional alpha, the atlas
    // count would stand and the readback count would be zero.
    assert!(
        aa.atlas_intermediate > 0,
        "HACK rasterised binary: {} of {} bytes were neither 0 nor 255",
        aa.atlas_intermediate,
        aa.atlas_total
    );
    assert!(
        aa.image_intermediate > 0,
        "HACK has {} intermediate atlas bytes but the readback has none, \
         so the coverage is being lost between the atlas and the target",
        aa.atlas_intermediate
    );

    // Absent for the pixel face, in both places. This is `pixel_properties`'
    // property 1 restated, on purpose: it is the control that makes the count
    // above mean antialiasing rather than mean the pipeline.
    assert_eq!(
        binary.atlas_intermediate, 0,
        "TERMINESS_SCALED: {} of {} atlas bytes were neither 0 nor 255",
        binary.atlas_intermediate, binary.atlas_total
    );
    assert_eq!(
        binary.image_intermediate, 0,
        "TERMINESS_SCALED: {} intermediate channel values in the readback",
        binary.image_intermediate
    );
}

/// Every scalable entry in the catalogue takes the antialiasing-on
/// rasterisation, and every one whose face has no strike at the size asked for
/// actually presents coverage. The switch is a per-entry flag and a catalogue
/// is exactly the place a flag gets typed wrong.
#[test]
fn every_scalable_catalogue_face_takes_the_coverage_path() {
    let Some((gpu, _lock)) = gpu() else { return };

    let scalable: Vec<_> = term::fonts::bundled_fonts()
        .iter()
        .filter(|f| !f.low_resolution)
        .collect();
    assert_eq!(scalable.len(), 8, "A6 counted eight scalable bundled faces");

    let mut antialiased = Vec::new();
    for entry in scalable {
        let ink = ink(&gpu, entry);
        assert_eq!(
            ink.mode,
            Rasterization::Coverage,
            "{} is a scalable entry drawn with antialiasing off",
            entry.name
        );
        assert!(ink.lit > 100, "{} drew almost nothing", entry.name);
        if ink.image_intermediate > 0 {
            antialiased.push(entry.name);
        }
    }
    // Seven of the eight are outline faces with nothing to load but outlines.
    // `TERMINESS` is the eighth, and its own test below is what accounts for
    // it; a *second* face going binary would mean a strike nobody knew about
    // or the mode failing to reach the atlas, and either wants looking at.
    assert_eq!(
        antialiased.len(),
        7,
        "expected the seven outline faces to present coverage; got {antialiased:?}"
    );
    assert!(!antialiased.contains(&"TERMINESS"));
}

/// The exception, stated rather than avoided: `TERMINESS` is scalable, takes
/// the coverage path, and still comes out binary at the shipped size, because
/// the face has an embedded strike at 24 ppem and the strike is a one-bit
/// picture. Embedded strikes take precedence over outline rendering even with
/// antialiasing on, so this binary result is the expected behavior, not a
/// gap.
///
/// The second half is the counterexample that stops this being a description of
/// a bug: one pixel off a strike, the same face through the same path
/// antialiases. So the binary picture above is the strike being found, not the
/// coverage mode failing to arrive.
#[test]
fn terminess_takes_its_strike_even_with_antialiasing_on() {
    let Some((gpu, _lock)) = gpu() else { return };
    let entry = font_by_name("TERMINESS", FontSource::Bundled).expect("TERMINESS");

    let on_strike = ink(&gpu, entry);
    assert_eq!(on_strike.mode, Rasterization::Coverage);
    assert_eq!(
        on_strike.atlas_intermediate, 0,
        "TERMINESS at a strike ppem produced {} intermediate bytes; either the \
         strike is no longer being found or the face changed",
        on_strike.atlas_intermediate
    );
    assert_eq!(on_strike.image_intermediate, 0);

    // 25 is off the strike grid: Terminess's strikes are at 12, 14, 16, 18,
    // 20, 22, 24, 28 and 32.
    let off_strike = ink_at(
        &gpu,
        entry,
        SizingRequest {
            font_size: 25,
            ..SizingRequest::default()
        },
    );
    assert_eq!(off_strike.mode, Rasterization::Coverage);
    assert!(
        off_strike.atlas_intermediate > 0 && off_strike.image_intermediate > 0,
        "TERMINESS one pixel off its strike is still binary ({} atlas, {} \
         readback), so the coverage mode is not reaching the atlas at all",
        off_strike.atlas_intermediate,
        off_strike.image_intermediate
    );
}
