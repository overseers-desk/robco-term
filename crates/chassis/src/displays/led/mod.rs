//! The LED strip kit. A channel title read off the bundled pixel font, one
//! image pixel per lamp, drawn by `shaders/wgsl/led_matrix.wgsl`.
//!
//! [`metrics`] is the width/height quantisation contract. This module is
//! the state-to-appearance mapping: the function that turns
//! `powered`/`bright`/the profile's font colour into the shader's
//! lit/dim/panel triple ([`window_colors`]), the grid geometry a title's
//! cell count implies, and the fixed shader constants pinned here (as
//! opposed to the shader's own `#pragma parameter` defaults, which a bare
//! mount of the `.slangp` would otherwise fall back to). Assembling the
//! padded glyph canvas from that grid is plumbing, not appearance, and stays
//! out; a done-test mounts the raw raster directly (one grid cell per raster
//! pixel), which is what "compose the proven raster with the shader" means
//! at this module's scope.

pub mod metrics;

use crate::color::{scale_color, Rgba};

pub const LED_DOT_PITCH: f64 = 1.5;
pub const MIN_LED_CHARACTERS: u32 = 8;
pub const LED_PAD_CELLS: u32 = 4;
pub const LED_SIDE_PAD_CELLS: u32 = 1;
/// The schema default (`robco-config`'s `GeneralSettings::led_characters`).
pub const DEFAULT_LED_CHARACTERS: u32 = 12;
/// The schema default (`ScreenSettings` names its own default font; this is
/// the *bank's* own, user-level default, distinct from a screen's
/// `font_name`).
pub const DEFAULT_LED_FONT_NAME: &str = "UNSCII_8_SCALED";

/// The LED cell a chosen font implies, from the advance and scaled metrics
/// of `"M"` at the font's own catalogue pixel size. `(lamp_cell_width,
/// lamp_cell_height)`.
pub fn cell_metrics(font_data: &[u8], pixel_size: u32) -> (u32, u32) {
    let advance = term::fonts::metrics::char_advance_px(font_data, 'M', pixel_size).unwrap_or(0.0);
    let scaled = term::fonts::metrics::scaled_metrics(font_data, pixel_size);
    let height = scaled.map(|m| m.height()).unwrap_or(0.0);
    let lamp_cell_width = (advance.round() as i64).max(1) as u32;
    let lamp_cell_height = (height.ceil() as i64).max(1) as u32;
    (lamp_cell_width, lamp_cell_height)
}

/// The three panel colours a window is struck from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub lit: Rgba,
    pub dim: Rgba,
    pub panel: Rgba,
}

/// Every window colour struck from the profile's font colour, normalised to
/// full brightness first (the "peak" step) so a dim phosphor still lights
/// its own window at full strength.
pub fn window_colors(font_color: Rgba, powered: bool, bright: bool) -> Colors {
    let peak = font_color.r.max(font_color.g).max(font_color.b);
    let full = scale_color(font_color, if peak > 0.0 { 1.0 / peak } else { 1.0 });
    Colors {
        lit: if bright {
            full
        } else {
            scale_color(full, 0.45)
        },
        dim: scale_color(full, if powered { 0.20 } else { 0.13 }),
        panel: scale_color(full, if powered { 0.09 } else { 0.045 }),
    }
}

/// A ghost's window: the title lit below an open window off the air, on an
/// unpowered panel. It throws no spill, being no channel yet.
pub fn ghost_colors(font_color: Rgba) -> Colors {
    let off = window_colors(font_color, false, true);
    Colors {
        lit: scale_color(off.lit, 0.30),
        ..off
    }
}

/// The window's spill-glow strength.
pub fn spill_strength(powered: bool, bright: bool) -> f32 {
    if !powered {
        0.0
    } else if bright {
        1.0
    } else {
        0.3
    }
}

/// The matrix glow strength.
pub fn glow(bright: bool) -> f32 {
    if bright {
        0.55
    } else {
        0.3
    }
}

/// Fixed shader constants -- pinned at the call site, not the shader's own
/// own defaults (`dot_radius 0.35`, `threshold 0.5` in
/// `led_matrix.wgsl`).
pub const DOT_RADIUS: f32 = 0.50;
pub const THRESHOLD: f32 = 0.4;
/// No dead band: the throw starts at the window's lip.
pub const SPILL_DEAD: (f32, f32) = (0.0, 0.0);

/// The lamp grid a title's cell count and padding imply.
pub fn grid_size(
    lamp_cell_width: u32,
    lamp_cell_height: u32,
    characters: u32,
    pad_cells_left: u32,
    pad_cells_right: u32,
    pad_cells_y: u32,
) -> (u32, u32) {
    let grid_w = lamp_cell_width * (characters + pad_cells_left + pad_cells_right);
    let grid_h = lamp_cell_height + pad_cells_y;
    (grid_w, grid_h)
}

/// Both spill margins are struck from the strip's own height (not width).
pub fn spill_margins(implicit_height: f64) -> (i64, i64) {
    (
        (implicit_height * 1.1).round() as i64,
        (implicit_height * 0.8).round() as i64,
    )
}

/// How long a hovered title holds each step of its roll: one character.
pub const MARQUEE_STEP: std::time::Duration = std::time::Duration::from_millis(250);

/// A dark or empty slot shows nothing, and a title longer than the window
/// reads from its first letter. `marquee` is how many characters a hovered
/// title has rolled left; the roll stops with the last letter at the
/// window's right edge and holds there.
pub fn visible_text(text: &str, powered: bool, characters: usize, marquee: usize) -> &str {
    if !powered || text.is_empty() || characters == 0 {
        return "";
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.len() <= characters {
        return text;
    }
    let start = marquee.min(chars.len() - characters);
    let end = chars
        .get(start + characters)
        .map_or(text.len(), |&(i, _)| i);
    &text[chars[start].0..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_colors_amber_powered_bright() {
        // Default Amber's font colour, robco-config's ScreenSettings preset:
        // "#ff8100" -> Rgba(255/256, 129/256, 0, 1).
        let amber = crate::color::str_to_color("#ff8100");
        let c = window_colors(amber, true, true);
        // peak = r = 255/256, full = amber/peak = (1.0, 129/255, 0.0, 1.0)
        assert!((c.lit.r - 1.0).abs() < 1e-3);
        assert!((c.lit.g - 129.0 / 255.0).abs() < 1e-3);
        assert_eq!(c.lit.b, 0.0);
        // dim = full * 0.20 (powered)
        assert!((c.dim.r - 0.20).abs() < 1e-3);
        // panel = full * 0.09 (powered)
        assert!((c.panel.r - 0.09).abs() < 1e-3);
    }

    #[test]
    fn window_colors_unpowered_is_darker_than_powered_dark() {
        let amber = crate::color::str_to_color("#ff8100");
        let off = window_colors(amber, false, false);
        let on_dark = window_colors(amber, true, false);
        assert!(off.dim.r < on_dark.dim.r);
        assert!(off.panel.r < on_dark.panel.r);
        // bright is meaningless once unpowered, but the formula still runs:
        // lit == full*0.45 either way since `bright` is false in both calls.
        assert_eq!(off.lit, on_dark.lit);
    }

    #[test]
    fn a_ghost_title_is_dimmer_than_an_open_windows_and_brighter_than_its_unlit_lamps() {
        let amber = crate::color::str_to_color("#ff8100");
        let ghost = ghost_colors(amber);
        assert!(ghost.lit.r > window_colors(amber, true, false).dim.r);
        assert!(ghost.lit.r < window_colors(amber, true, false).lit.r);
        assert_eq!(ghost.panel, window_colors(amber, false, false).panel);
    }

    #[test]
    fn spill_strength_and_glow_match_the_defining_formulas() {
        assert_eq!(spill_strength(false, false), 0.0);
        assert_eq!(spill_strength(false, true), 0.0);
        assert_eq!(spill_strength(true, false), 0.3);
        assert_eq!(spill_strength(true, true), 1.0);
        assert_eq!(glow(false), 0.3);
        assert_eq!(glow(true), 0.55);
    }

    #[test]
    fn grid_size_matches_the_defining_formula() {
        // UNSCII_8_SCALED-shaped cell (8x9-ish), 12 characters, 1 side pad
        // each side, 4 pad rows.
        let (w, h) = grid_size(8, 9, 12, 1, 1, 4);
        assert_eq!(w, 8 * 14);
        assert_eq!(h, 13);
    }

    #[test]
    fn spill_margins_both_come_from_height() {
        let (x, y) = spill_margins(20.0);
        assert_eq!(x, 22); // round(20 * 1.1)
        assert_eq!(y, 16); // round(20 * 0.8)
    }

    #[test]
    fn visible_text_reads_from_the_first_letter_and_rolls_to_the_last() {
        assert_eq!(visible_text("session/deep/path", true, 4, 0), "sess");
        assert_eq!(visible_text("session/deep/path", true, 4, 3), "sion");
        assert_eq!(visible_text("session/deep/path", true, 4, 99), "path");
        assert_eq!(visible_text("hi", true, 4, 5), "hi");
        assert_eq!(visible_text("hi", false, 4, 0), "");
        assert_eq!(visible_text("", true, 4, 0), "");
        assert_eq!(visible_text("hi", true, 0, 0), "");
    }

    #[test]
    fn cell_metrics_are_at_least_one_pixel() {
        let entry =
            term::fonts::font_by_name(DEFAULT_LED_FONT_NAME, term::fonts::FontSource::Bundled)
                .unwrap();
        let (w, h) = cell_metrics(entry.data(), entry.pixel_size);
        assert!(w >= 1);
        assert!(h >= 1);
    }
}
