//! Done-test: the LED display kit's own composition -- the proven
//! `ledTextImage` raster (`term::fonts::led`) mounted through
//! `led_matrix.wgsl` with `displays::led`'s appearance mapping
//! (`window_colors`, `spill_strength`, `glow`, the fixed `dotRadius`/
//! `threshold`/`spillDead` pins), for a sampled channel set.
//!
//! `tests/suite/led_matrix.rs` already proved the shader itself
//! against a synthetic checkerboard, sampling deep inside a cell, past the
//! antialiasing band and the spill margin, where the output has saturated
//! to one of the mixed colors. This test reuses that same precedent but
//! swaps the checkerboard for the real raster a
//! real channel title produces, and the shader's own `#pragma` defaults for
//! the appearance mapping. The `led_matrix.wgsl` body reads its raster
//! nearest-sampled at exactly one texel per grid cell
//! (`texture(Source, (idx+0.5)/gridSize)`), so -- unlike `tape_label`'s
//! dilating `punched()` -- there is no neighbor bleed to worry about: every
//! raster pixel independently determines its own cell's expected color,
//! and the "sampled channel set" is exercised by iterating several real
//! channel titles rather than hand-picking one safe point.

use chassis::color::str_to_color;
use chassis::displays::{led, raster};
use chassis::params::{record_bytes, LedMetalParams};
use gpu::harness::{px_index, render_wgsl_quad_io, Locked};

fn source() -> String {
    format!(
        "{}{}",
        chassis::shaders::LED_MATRIX_WGSL,
        "@group(0) @binding(0) var<uniform> p: LedParams;\n\
         fn shade(uv: vec2<f32>) -> vec4<f32> { return led_matrix(uv, p); }\n",
    )
}

/// Amber, `robco-config`'s `ScreenSettings` default (`presets.rs:29`,
/// `"#ff8100"`).
const AMBER: &str = "#ff8100";

#[test]
fn led_display_composes_the_proven_raster_with_led_matrix() {
    let gpu = Locked::new().expect("headless wgpu device");

    let font =
        term::fonts::font_by_name(led::DEFAULT_LED_FONT_NAME, term::fonts::FontSource::Bundled)
            .unwrap();
    let font_color = str_to_color(AMBER);

    // A sampled channel set: a couple of real channel titles at both
    // illumination states the display distinguishes (the window on screen,
    // and one merely open).
    let cases: &[(&str, bool, bool)] = &[
        ("LOG", true, true),
        ("LOG", true, false),
        ("01", true, true),
        ("AUX/deep/session", true, true), // longer than the window: reads from its first letter
    ];

    for &(text, powered, bright) in cases {
        let shown = led::visible_text(text, powered, led::DEFAULT_LED_CHARACTERS as usize, 0);
        let r = term::fonts::led::led_text_image(font.data(), font.pixel_size, shown)
            .expect("non-empty text rasterises");
        let input = raster::to_rgba8(&r);

        let colors = led::window_colors(font_color, powered, bright);
        let glow = led::glow(bright);
        let spill_strength = led::spill_strength(powered, bright);

        let params = LedMetalParams {
            grid_size: [r.width as f32, r.height as f32],
            // Zero margin, same reasoning as tests/suite/led_matrix.rs:
            // it collapses `uv` onto the raw grid coordinate so the test's
            // cell-center sampling lines up with the shader's own math
            // without a second spill-band transform to invert.
            spill_margin: [0.0, 0.0],
            spill_dead: [led::SPILL_DEAD.0, led::SPILL_DEAD.1],
            lit_color: [colors.lit.r, colors.lit.g, colors.lit.b, colors.lit.a],
            dim_color: [colors.dim.r, colors.dim.g, colors.dim.b, colors.dim.a],
            panel_color: [
                colors.panel.r,
                colors.panel.g,
                colors.panel.b,
                colors.panel.a,
            ],
            dot_radius: led::DOT_RADIUS,
            threshold: led::THRESHOLD,
            glow,
            spill_strength,
        };

        let px_per_cell = 8;
        let out_w = r.width * px_per_cell;
        let out_h = r.height * px_per_cell;
        let record = record_bytes(&params.record([0.0, 0.0, 1.0, 1.0]));
        let out = render_wgsl_quad_io(
            &gpu, &source(), &record, r.width, r.height, out_w, out_h, &input,
        );

        for gy in 0..r.height {
            for gx in 0..r.width {
                let lit = r.alpha[(gy * r.width + gx) as usize] >= 128;
                let c = gx * px_per_cell + px_per_cell / 2;
                let row = gy * px_per_cell + px_per_cell / 2;
                let px = out[px_index(out_w, c, row)];
                let expected = if lit { colors.lit } else { colors.dim };
                let tol = 0.08;
                assert!(
                    (px[0] - expected.r).abs() < tol
                        && (px[1] - expected.g).abs() < tol
                        && (px[2] - expected.b).abs() < tol,
                    "text={text:?} powered={powered} bright={bright} cell=({gx},{gy}) lit={lit}: \
                     gpu={:?} expected~={expected:?}",
                    &px[0..3]
                );
            }
        }
    }
}
