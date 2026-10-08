//! The glyphs that reach the glass are the glyphs the grid drew.
//!
//! A shipped picture once had text that was legibly wrong: `w` read as `u`,
//! `i` read as `:`, hyphens and slashes were not there at all. Every stage had
//! a test and every test passed, because each one measured its own stage
//! against its own idea of a glyph. Nothing measured the path: a grid texture
//! in, a presented frame out, and the same strokes in both.
//!
//! That is what this file measures. Deliberately not a similarity metric on a
//! reference image -- the chain at defaults adds noise, a glowing line, jitter
//! and bloom, and a picture-level tolerance loose enough to survive those would
//! be loose enough to lose a stem. It counts two things: *ink*, the lit pixels,
//! and *strokes*, the horizontal runs of them. A stem the chain destroys is ink
//! that did not arrive and a run that is not there.
//!
//! The stress line is chosen for the failure that prompted the file:
//! characters whose whole identity is a one-pixel feature. `-` is a single
//! raster row. `i` is a single raster column with a gap in it. `w` differs from
//! `u` by one raster column. `/` is a staircase one raster pixel wide. A path
//! that keeps `M` and `#` intact can lose all four and still look busy from a
//! distance.
//!
//! What this file does *not* cover, and cannot: a defect in the grid texture
//! itself. Both sides of every ratio here are read from the same picture, so a
//! rasteriser that never drew the hyphen passes with full marks -- which is
//! exactly what the shipped defect did. That half is
//! `term`'s `pixel_properties::property_1_premise_a_low_resolution_face_needs_no_threshold`,
//! and the two are the whole path between them.

use crate::support;

use std::time::{Duration, Instant};

use config::Config;
use crt::{Chain, DegaussState, Pacing, Params};
use term::atlas::Rasterization;
use term::cells::CellGrid;
use term::color::Scheme;
use term::fonts::{font_by_name, FontSource};
use term::fonts::sizing::{self, SizingRequest};
use gpu::Image;
use term::render::GridRenderer;
use term::{ascii_charset, FontContext, DEFAULT_THRESHOLD};

/// One glyph of every kind a resampling defect eats first, and nothing that
/// would pad the count with easy ink.
const STRESS: &str = "-iw|/l-i.w/-il-w";

/// The screen well of the window the parity snaps are taken at: 1448x1086 less
/// the bank column. The size is not decoration. `normalizedScreenScale` divides
/// 1024 by the mean of the two, so a smaller target curves the picture harder
/// than the shipped one is curved and the ink lost to the bend would say
/// nothing about the real path. The control below is exactly that effect,
/// turned into the measurement's own counterexample.
const WELL: (u32, u32) = (1201, 1086);

/// A target small enough that `normalizedScreenScale` is 5.8 rather than 0.90:
/// the same picture through the same chain, bent six times as hard.
const TIGHT: (u32, u32) = (256, 96);

/// How much of the grid's ink and how many of its strokes must survive.
///
/// Not 1.0, and it cannot be: the last pass resamples the picture through the
/// screen curvature, so a glyph edge that lands between two pixels is split
/// between them and one half can fall under any threshold. Measured on this
/// line at the shipped profile and the shipped well, the chain arrives with
/// 1.04 of the grid's lit pixels (the bloom adds a little back at the edges)
/// and 0.97 of its lit runs. The control at [`TIGHT`] arrives with 0.54 and
/// 0.71. The bar sits between the two rather than next to either.
const MIN_INK: f64 = 0.85;
const MIN_RUNS: f64 = 0.85;

/// A pixel is lit if any channel is at least this. The background the chain
/// draws is not black -- static noise, the glowing line and the profile's own
/// colour mix all sit above zero -- so this is well clear of it and well under
/// the amber a lit glyph comes out as.
const LIT: u8 = 128;

fn lit_pixels(img: &Image) -> u64 {
    img.pixels
        .chunks_exact(4)
        .filter(|px| px[0].max(px[1]).max(px[2]) >= LIT)
        .count() as u64
}

/// Horizontal runs of lit pixels: one per stroke crossing a row. Ink alone
/// could be kept by a path that fattened what it left while dropping every
/// second stem; a run count cannot.
fn lit_runs(img: &Image) -> u64 {
    let mut runs = 0;
    for y in 0..img.height {
        let mut inside = false;
        for x in 0..img.width {
            let px = img.pixel(x, y);
            let lit = px[0].max(px[1]).max(px[2]) >= LIT;
            if lit && !inside {
                runs += 1;
            }
            inside = lit;
        }
    }
    runs
}

/// What survived: the fraction of the grid's ink, and of its strokes, that the
/// presented frame still has.
struct Survival {
    ink: f64,
    runs: f64,
}

/// Fill a target of this size with the stress line, run one chain frame over it
/// at the shipped profile, and weigh what came out against what went in.
fn measure(name: &str, (w, h): (u32, u32)) -> Survival {
    let harness = support::Harness::new(name, w, h).expect("gpu");
    let cfg = Config::default();

    // The renderer the application builds, from the same catalogue entry and
    // the same sizing request: a low-resolution face at its design size,
    // magnified as geometry by the integer scale.
    let entry = font_by_name(&cfg.screen.font_name, FontSource::Bundled)
        .expect("the shipped profile's font");
    let request = SizingRequest {
        line_spacing: cfg.screen.line_spacing,
        font_width: cfg.screen.font_width,
        ..SizingRequest::default()
    };
    let resolved = sizing::resolve(entry, &request);
    let scheme = Scheme::monochrome([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    let mut font = FontContext::new(entry);
    let atlas = font.build_atlas(
        &harness.gpu.device,
        &harness.gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::Binary {
            threshold: DEFAULT_THRESHOLD,
        },
    );

    // The whole target, filled, so the measurement covers the curved edges and
    // not only the flat middle.
    let step = resolved.integer_scale.max(1);
    let cols = (w / (atlas.cell.width * step)) as usize;
    let rows = (h / (atlas.cell.height * step)) as usize;
    assert!(cols > 4 && rows > 1, "{name}: {cols}x{rows} is not a grid");
    let line: String = STRESS.chars().cycle().take(cols).collect();
    let lines: Vec<&str> = std::iter::repeat_n(line.as_str(), rows).collect();
    let mut renderer = GridRenderer::new(
        &harness.gpu.device,
        &harness.gpu.queue,
        atlas,
        cols,
        rows,
        scheme.clone(),
    );
    renderer.set_scale(resolved.integer_scale);
    renderer.set_grid(
        &harness.gpu.queue,
        &CellGrid::from_lines(&lines, cols, rows, &scheme),
        None,
    );
    // Centred at a whole-pixel origin, which is what `app::window` does,
    // with the view at the bottom of its scrollback (no shift).
    let (grid_w, grid_h) = renderer.pixel_size();
    renderer.set_origin(
        (w as i32 - grid_w as i32) / 2,
        (h as i32 - grid_h as i32) / 2,
        0,
    );

    // The grid, into the texture the chain takes as its input.
    let mut encoder = harness
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("grid into the chain's input"),
        });
    renderer.draw(
        &harness.gpu.queue,
        &mut encoder,
        &harness.input.view,
        w,
        h,
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
    );
    let index = harness.gpu.queue.submit([encoder.finish()]);
    harness
        .gpu
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .expect("poll after the grid draw");
    let grid = harness
        .input
        .read_rgba(&harness.gpu.device, &harness.gpu.queue);

    // The chain the application builds, at the shipped profile, one frame.
    let mut chain = Chain::from_config(&harness.gpu.device, &harness.gpu.queue, &harness.dir, &cfg)
        .expect("chain");
    let mut pacing = Pacing::new(Instant::now());
    let time = pacing.tick_by(Duration::from_micros(16_667));
    // `app::window::chain_geometry` for this target: the output in logical
    // pixels (DPR 1 here, so the same number), and the virtual resolution as
    // the raster count `floor(size / (integer_scale * font_width))`.
    let scale = step as f32;
    let geom = crt::Geometry {
        output_width: w as f32,
        output_height: h as f32,
        virtual_width: (w as f32 / (scale * cfg.screen.font_width as f32)).floor(),
        virtual_height: (h as f32 / scale).floor(),
        total_font_scaling: 0.75,
        device_pixel_ratio: 1.0,
    };
    chain.set_params(&Params::build(&cfg, &geom, time, DegaussState::IDLE));
    let glass = harness.render(&mut chain, time);

    let (ink_in, ink_out) = (lit_pixels(&grid), lit_pixels(&glass));
    let (runs_in, runs_out) = (lit_runs(&grid), lit_runs(&glass));
    assert!(
        ink_in > 1000 && runs_in > 100,
        "{name}: the grid drew almost nothing ({ink_in} lit, {runs_in} runs)"
    );
    let survival = Survival {
        ink: ink_out as f64 / ink_in as f64,
        runs: runs_out as f64 / runs_in as f64,
    };
    eprintln!(
        "{name} {w}x{h}: ink {ink_in} -> {ink_out} ({:.3}), runs {runs_in} -> {runs_out} ({:.3})",
        survival.ink, survival.runs
    );
    survival
}

#[test]
fn a_stem_that_reaches_the_grid_reaches_the_glass() {
    let s = measure("glyph-survival", WELL);
    assert!(
        s.ink >= MIN_INK,
        "the chain lost {:.1}% of the grid's ink; at this profile a stem is one \
         or two pixels wide, so this is strokes going missing, not edges softening",
        (1.0 - s.ink) * 100.0
    );
    assert!(
        s.runs >= MIN_RUNS,
        "{:.1}% of the grid's strokes are not strokes on the glass",
        (1.0 - s.runs) * 100.0
    );
}

/// The control, in the shape `pixel_properties` uses for the same reason: a
/// property that holds is worth less than a property that holds *and* whose
/// violation is shown to be detectable. Bend the same picture six times as hard
/// and the counts fall through the bar, so a passing run above means the chain
/// kept the strokes rather than that the measurement cannot see them go.
#[test]
fn the_measurement_can_see_strokes_go_missing() {
    let s = measure("glyph-survival-control", TIGHT);
    assert!(
        s.ink < MIN_INK && s.runs < MIN_RUNS,
        "a target curved six times as hard kept {:.3} of the ink and {:.3} of the \
         strokes, both inside the tolerance the real measurement passes with: this \
         control no longer controls anything",
        s.ink,
        s.runs
    );
}
