//! The tape-label kit. A channel title stamped as embossed punch tape,
//! drawn by `shaders/wgsl/tape_label.wgsl`. Unlike the LED
//! strip, tape reads nothing from the profile: font, colours and light
//! direction are the kit's own -- tape is paint, not light.
//!
//! [`metrics`] is the width/height quantisation contract. This module is
//! the state-to-appearance mapping: the `powered` gate, truncation, letter
//! scale, the raster-at-double-size trick, the glyph rectangle, the fixed
//! shader constants -- plus the well the label lies in ([`well_chrome`]),
//! which is vector painting rather than a pass and arrived alongside the
//! furniture module.

pub mod metrics;

use crate::color::{darker, lighter, hex_literal_to_color, Rgba};
use crate::layout::Rect;
use crate::paint::{Painting, RectOp, Stop};

pub const LETTER_PIXEL_SIZE: u32 = 20;
pub const END_PAD: u32 = 12;
pub const NATURAL_HEIGHT: u32 = 44;
/// Catalogue name of the face this kit hardcodes ("DepartureMono Nerd Font
/// Mono"), independent of any profile's chosen font.
pub const FONT_NAME: &str = "DEPARTURE_MONO_SCALED";

/// Fixed shader constants (the ones left at the shader's own default rather
/// than overridden).
pub const BEVEL_PX: f32 = 1.4;
pub const DILATE_PX: f32 = 0.6;
pub const SHEEN_AMOUNT: f32 = 1.0;
pub const GRAIN_AMOUNT: f32 = 0.10;
pub const SEED: f32 = 0.37;

/// The switchboard's room key, overriding the shader's own default of
/// `(0.8, -0.6)`.
pub const DISPLAY_LIGHT_DIR: (f32, f32) = (-0.4, -0.9);

/// A hex colour literal (/255), not `str_to_color`'s divisor. See
/// `crate::color`'s module doc.
pub fn tape_color() -> Rgba {
    hex_literal_to_color("#0d0e15")
}

/// Same rule as [`tape_color`].
pub fn letter_color() -> Rgba {
    hex_literal_to_color("#f0ece0")
}

/// The slot floor, near-black and a shade colder than the tape's own
/// plastic, so a bare well never reads as a tape with nothing stamped on it.
pub fn slot_color() -> Rgba {
    hex_literal_to_color("#0a0c10")
}

/// The recessed well the tape lies in.
///
/// The punched-panel vocabulary the blue shell's rows already speak: dark
/// under the top lip where the room cannot reach, the far wall catching the
/// light along the bottom, the left wall falling dark with the key leaning
/// that way and the right one catching a little of it. The three walls are
/// children of a `clip: true` floor, so each is cut at the floor's rounded
/// bounds.
///
/// `strip` is the well's rectangle; the painting is in its own coordinates.
pub fn well_chrome(strip: Rect) -> Painting {
    let mut p = Painting::new();
    if strip.width <= 0.0 || strip.height <= 0.0 {
        return p;
    }
    let floor = Rect::new(0.0, 0.0, strip.width, strip.height);
    let radius = 3.0;
    let dark = slot_color();
    let clear = Rgba::new(0.0, 0.0, 0.0, 0.0);
    let shade = |a: f32| Rgba::new(0.0, 0.0, 0.0, a);

    // The floor's own vertical gradient, darkest at the top lip.
    p.rect(RectOp::gradient(
        floor,
        radius,
        vec![
            Stop::new(0.00, darker(dark, 1.7)),
            Stop::new(0.50, dark),
            Stop::new(0.90, lighter(dark, 1.8)),
            Stop::new(1.00, lighter(dark, 3.4)),
        ],
    ));
    // The shadow the top lip throws down the near wall.
    p.rect(
        RectOp::gradient(
            Rect::new(0.0, 0.0, floor.width, 4.0),
            0.0,
            vec![Stop::new(0.0, shade(0.75)), Stop::new(1.0, clear)],
        )
        .clipped_to(floor, radius),
    );
    // The left wall dark, the right one catching a little.
    p.rect(
        RectOp::horizontal_gradient(
            Rect::new(0.0, 0.0, 5.0, floor.height),
            0.0,
            vec![Stop::new(0.0, shade(0.65)), Stop::new(1.0, clear)],
        )
        .clipped_to(floor, radius),
    );
    p.rect(
        RectOp::horizontal_gradient(
            Rect::new(floor.width - 3.0, 0.0, 3.0, floor.height),
            0.0,
            vec![Stop::new(0.0, clear), Stop::new(1.0, lighter(dark, 3.0))],
        )
        .at_opacity(0.5)
        .clipped_to(floor, radius),
    );
    p
}

/// Identical truncation rule to the LED strip's, gated by `powered`.
pub fn visible_text(text: &str, powered: bool, characters: usize, marquee: usize) -> &str {
    super::led::visible_text(text, powered, characters, marquee)
}

/// The well's implicit width/height. `unit_width` comes from
/// [`metrics::unit_width`].
pub fn implicit_width(unit_width: f64, characters: u32) -> i64 {
    metrics::width_for_units(unit_width, characters, END_PAD)
}

pub fn implicit_height(pad_cells_y: u32) -> u32 {
    if pad_cells_y > 0 {
        pad_cells_y
    } else {
        NATURAL_HEIGHT
    }
}

/// The kit's one letter size, stated as a share of whatever height the well
/// ended up.
pub fn letter_scale(height: f64) -> f64 {
    LETTER_PIXEL_SIZE as f64 / height.max(1.0)
}

/// The glyph raster is always rasterised at *double* the label's displayed
/// size, so the shader downscales into soft-edged strokes rather than
/// reading a raw 1:1 pixel-font blit.
pub fn raster_size(height: f64, letter_scale: f64) -> u32 {
    ((height * letter_scale * 2.0).round() as i64).max(8) as u32
}

/// The glyph box centres cap letters a hair high (the raster carries the
/// font's descent); this seats them on the strip.
pub fn glyph_y_nudge(height: f64) -> f64 {
    height * 0.04
}

/// The on-screen pixel box the shader samples the (half-size, downscaled)
/// raster into, centred in the label.
pub fn glyph_rect_px(width: f64, height: f64, glyph_w: f64, glyph_h: f64) -> (f32, f32, f32, f32) {
    let x = (width - glyph_w) / 2.0;
    let y = (height - glyph_h) / 2.0 + glyph_y_nudge(height);
    (
        x as f32,
        y as f32,
        glyph_w.max(1.0) as f32,
        glyph_h.max(1.0) as f32,
    )
}

/// `ceil(glyph_w + 2*end_padding)`.
pub fn tape_label_implicit_width(glyph_w: f64, end_padding: f64) -> i64 {
    (glyph_w + 2.0 * end_padding).ceil() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_text_reuses_the_led_truncation_rule() {
        assert_eq!(visible_text("session/deep/path", true, 4, 0), "sess");
        assert_eq!(visible_text("hi", false, 4, 0), "");
    }

    #[test]
    fn implicit_height_falls_back_to_natural_height() {
        assert_eq!(implicit_height(0), NATURAL_HEIGHT);
        assert_eq!(implicit_height(30), 30);
    }

    #[test]
    fn letter_scale_and_raster_size_cancel_the_height() {
        // The height cancels out of the round trip: whatever height the
        // well settles at, the raster is always rasterised at 2x the fixed
        // letter pixel size (a tape carries one letter size, not the
        // well's).
        for height in [22.0, 32.0, 44.0, 100.0] {
            let scale = letter_scale(height);
            let size = raster_size(height, scale);
            assert_eq!(size, 2 * LETTER_PIXEL_SIZE, "height={height}");
        }
    }

    #[test]
    fn raster_size_floors_at_eight() {
        // A degenerate letter_scale near zero must not starve the raster.
        assert_eq!(raster_size(1.0, 0.001), 8);
    }

    #[test]
    fn glyph_rect_centres_in_the_label() {
        let (x, y, z, w) = glyph_rect_px(100.0, 44.0, 60.0, 24.0);
        assert_eq!(x, 20.0); // (100-60)/2
        assert_eq!(y, (44.0 - 24.0) / 2.0 + 44.0 * 0.04); // +glyphYNudge
        assert_eq!(z, 60.0);
        assert_eq!(w, 24.0);
    }

    #[test]
    fn colors_match_the_recorded_hex_constants() {
        // Both are hex-literal colours, parsed at /255, not run through
        // `str_to_color`.
        let t = tape_color();
        let l = letter_color();
        assert!((t.r - 0x0d as f32 / 255.0).abs() < 1e-6);
        assert!((l.r - 0xf0 as f32 / 255.0).abs() < 1e-6);
    }
}
