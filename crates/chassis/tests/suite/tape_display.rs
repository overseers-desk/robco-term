//! Done-test: the tape display kit's own composition -- the proven
//! `ledTextImage` raster mounted through `tape_label.wgsl` with
//! `displays::tape`'s appearance mapping (the double-size raster trick, the
//! centred glyph rectangle, the fixed shader constants it pins), for a
//! sampled channel set.
//!
//! Unlike `led_matrix`, `tape_label`'s `punched()` dilates the mask by
//! sampling `Source` at eight neighbor offsets
//! (`shaders/wgsl/tape_label.wgsl`), so a single raster pixel does
//! not determine its own output pixel the way it does for the LED grid.
//! `tests/suite/tape_label.rs` sidesteps that with a synthetic
//! solid block; this test instead builds a small CPU oracle of
//! `maskAt`/`punched` over the *real* raster (nearest-sampled, since the
//! preset sets `filter_linear0 = false` -- deterministic, no bilinear
//! blending to reproduce) and scans the real label for a point genuinely
//! saturated by that oracle (`mC == mToward == mAway == mAwayFar == 1`,
//! the "past the antialiasing/dilation band" condition, just computed
//! instead of eyeballed), then asserts the GPU readback there against the
//! same `letterColor * 0.74` reduction the shader's own oracle used. A body
//! point outside the centred glyph rectangle is the plastic-color check.

use chassis::displays::{raster, tape};
use chassis::params::record_bytes;
use gpu::harness::{px_index, render_wgsl_quad_io, Locked};

fn source() -> String {
    format!(
        "{}{}{}",
        chassis::shaders::COMMON_WGSL,
        chassis::shaders::TAPE_LABEL_WGSL,
        "@group(0) @binding(0) var<uniform> p: TapeParams;\n\
         fn shade(uv: vec2<f32>) -> vec4<f32> { return tape_label(uv, p); }\n",
    )
}

const DISPLAY_HEIGHT: f64 = 44.0; // tape::NATURAL_HEIGHT, a fixture naming no height.

/// Nearest-sampled `maskAt(px, glyphRectPx)`, matching the preset's
/// `filter_linear0 = false`.
fn mask_at(r: &term::fonts::led::LedRaster, px: (f32, f32), rect: (f32, f32, f32, f32)) -> f32 {
    let guv = ((px.0 - rect.0) / rect.2, (px.1 - rect.1) / rect.3);
    if !(0.0..=1.0).contains(&guv.0) || !(0.0..=1.0).contains(&guv.1) {
        return 0.0;
    }
    let tx = ((guv.0 * r.width as f32) as i64).clamp(0, r.width as i64 - 1) as u32;
    let ty = ((guv.1 * r.height as f32) as i64).clamp(0, r.height as i64 - 1) as u32;
    r.alpha[(ty * r.width + tx) as usize] as f32 / 255.0
}

/// `punched(px, glyphRectPx)`: `maskAt` dilated by `dilatePx` along the
/// cardinal and diagonal directions.
fn punched(r: &term::fonts::led::LedRaster, px: (f32, f32), rect: (f32, f32, f32, f32)) -> f32 {
    let d = tape::DILATE_PX;
    let mut m = mask_at(r, px, rect);
    for (dx, dy) in [(d, 0.0), (-d, 0.0), (0.0, d), (0.0, -d)] {
        m = m.max(mask_at(r, (px.0 + dx, px.1 + dy), rect));
    }
    let rd = d * std::f32::consts::FRAC_1_SQRT_2;
    for (dx, dy) in [(rd, rd), (rd, -rd), (-rd, rd), (-rd, -rd)] {
        m = m.max(mask_at(r, (px.0 + dx, px.1 + dy), rect));
    }
    m
}

/// `light_dir` normalised, `L` in the shader.
fn normalize((x, y): (f32, f32)) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    (x / len, y / len)
}

/// A point deep enough inside the mask that `punched` at `px` and at every
/// bevel-offset sample used to compute it is fully saturated -- the deep
/// letter interior the shader test picks by construction, found here by
/// scanning the real glyph instead.
fn find_saturated_interior(
    r: &term::fonts::led::LedRaster,
    rect: (f32, f32, f32, f32),
    out_w: u32,
    out_h: u32,
    light: (f32, f32),
) -> Option<(u32, u32)> {
    for row in 0..out_h {
        for c in 0..out_w {
            let px = (c as f32 + 0.5, row as f32 + 0.5);
            let m_c = punched(r, px, rect);
            let m_toward = punched(
                r,
                (
                    px.0 + light.0 * tape::BEVEL_PX,
                    px.1 + light.1 * tape::BEVEL_PX,
                ),
                rect,
            );
            let m_away = punched(
                r,
                (
                    px.0 - light.0 * tape::BEVEL_PX,
                    px.1 - light.1 * tape::BEVEL_PX,
                ),
                rect,
            );
            let m_away_far = punched(
                r,
                (
                    px.0 - light.0 * tape::BEVEL_PX * 1.8,
                    px.1 - light.1 * tape::BEVEL_PX * 1.8,
                ),
                rect,
            );
            if m_c > 0.999 && m_toward > 0.999 && m_away > 0.999 && m_away_far > 0.999 {
                return Some((c, row));
            }
        }
    }
    None
}

#[test]
fn tape_display_composes_the_proven_raster_with_tape_label() {
    let gpu = Locked::new().expect("headless wgpu device");

    let font = term::fonts::font_by_name(tape::FONT_NAME, term::fonts::FontSource::Bundled)
        .unwrap();
    let light = normalize(tape::DISPLAY_LIGHT_DIR);
    let tape_color = tape::tape_color();
    let letter_color = tape::letter_color();

    // A sampled channel set. Full-block glyphs are not required: the
    // interior scan below finds whatever safely-saturated point the real
    // rasterised text actually has, or the test fails loudly (skipping a
    // font/text pair rather than silently asserting nothing).
    let cases: &[&str] = &["LOG", "AUX", "0088", "session/tail"];

    let letter_scale = tape::letter_scale(DISPLAY_HEIGHT);
    let raster_size = tape::raster_size(DISPLAY_HEIGHT, letter_scale);

    let mut checked_any_letter = false;

    for &text in cases {
        let shown = tape::visible_text(
            text,
            true,
            chassis::displays::led::DEFAULT_LED_CHARACTERS as usize,
            0,
        )
        .to_uppercase();
        let r = term::fonts::led::led_text_image(font.data(), raster_size, &shown)
            .expect("non-empty text rasterises");
        let input = raster::to_rgba8(&r);

        // The raster is at double size; the on-screen glyph box is half its
        // resolution.
        let glyph_w = r.width as f64 / 2.0;
        let glyph_h = r.height as f64 / 2.0;
        let end_padding = tape::END_PAD as f64;
        let out_w = tape::tape_label_implicit_width(glyph_w, end_padding) as u32;
        let out_h = DISPLAY_HEIGHT as u32;
        let rect = tape::glyph_rect_px(out_w as f64, out_h as f64, glyph_w, glyph_h);

        let params =
            chassis::furniture::tape_params((out_w as f64, out_h as f64), rect);

        let record = record_bytes(&params.record([0.0, 0.0, 1.0, 1.0]));
        let out = render_wgsl_quad_io(
            &gpu, &source(), &record, r.width, r.height, out_w, out_h, &input,
        );

        // Body: a point in the end-pad margin, safely outside the centred
        // glyph rectangle (glyphRectPx.x is at least END_PAD past the
        // canvas edge by construction).
        let body_c = 2u32;
        let body_row = out_h / 2;
        let body_px = out[px_index(out_w, body_c, body_row)];
        let tol = 0.12;
        assert!(
            (body_px[0] - tape_color.r).abs() < tol
                && (body_px[1] - tape_color.g).abs() < tol
                && (body_px[2] - tape_color.b).abs() < tol,
            "text={text:?} plastic body: gpu={:?} expected~={tape_color:?}",
            &body_px[0..3]
        );
        assert!(
            body_px[3] > 0.95,
            "text={text:?} body alpha should saturate near 1.0"
        );

        // Letter interior, if this text/font pair has one thick enough to
        // clear the dilate/bevel band; not every short sample string will.
        if let Some((c, row)) = find_saturated_interior(&r, rect, out_w, out_h, light) {
            checked_any_letter = true;
            let letter_px = out[px_index(out_w, c, row)];
            // The shader's own reduction at full saturation: face = letterColor *
            // (0.74 + 0.10*faceGrain), faceGrain in [-0.5, 0.5].
            let expected = [
                letter_color.r * 0.74,
                letter_color.g * 0.74,
                letter_color.b * 0.74,
            ];
            let letter_tol = 0.10;
            assert!(
                (letter_px[0] - expected[0]).abs() < letter_tol
                    && (letter_px[1] - expected[1]).abs() < letter_tol
                    && (letter_px[2] - expected[2]).abs() < letter_tol,
                "text={text:?} letter interior at ({c},{row}): gpu={:?} expected~={expected:?}",
                &letter_px[0..3]
            );
            assert!(
                letter_px[3] > 0.95,
                "text={text:?} letter alpha should saturate near 1.0"
            );
        }
    }

    assert!(
        checked_any_letter,
        "no sample text produced a saturated letter interior at all -- widen the sample set"
    );
}
