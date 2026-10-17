//! The glyph raster the LED strip and the tape label read.
//!
//! The text is drawn one image pixel per font pixel with antialiasing off,
//! on the CPU, so the lamp grid reads the font's own bitmap regardless of the
//! screen's scale factor. Rasterising at device resolution instead would hand
//! the strip grey-edged glyphs on scaled screens, lighting lamps the font
//! never inked.
//!
//! The raster itself is the return value: a coverage byte per pixel, which
//! `chassis` turns into premultiplied white (`[a, a, a, a]`) on the way to
//! the upload.
//!
//! Parity, measured against the golden rasters (`tests/suite/font_parity.rs`):
//! every low-resolution face is pixel-identical, which is the case that
//! matters, since the LED and tape displays choose from the low-resolution
//! list alone. The scalable faces differ on a few hundred pixels out of tens
//! of thousands (worst RMSE 0.167, IBM 3278), where FreeType's autohinted
//! monochrome rasteriser thickens a diagonal stem to three pixels that the
//! coverage threshold here leaves at two.
//!
//! Geometry:
//!   * width  = round(sum of the fractional advances), floored at 1;
//!   * height = round(ascent) + round(descent), floored at 1;
//!   * baseline at round(ascent), glyphs placed at the rounded running
//!     advance.

use swash::scale::ScaleContext;
use swash::FontRef;
use ttf_parser::Face;

use super::raster;

use super::metrics::{char_advance_26_6, scaled_metrics_for};

/// A one-byte-per-pixel coverage raster. With antialiasing off every byte is
/// 0 or 255, which is what makes the lamp grid honest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedRaster {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

/// Keyed on the face's bytes rather than on a family name, since there is
/// no font database here to look a family up in.
pub fn led_text_image(data: &[u8], pixel_size: u32, text: &str) -> Option<LedRaster> {
    if text.is_empty() || pixel_size == 0 {
        return None;
    }
    let face = Face::parse(data, 0).ok()?;
    let font = FontRef::from_index(data, 0)?;

    let metrics = scaled_metrics_for(&face, pixel_size);
    let ascent = metrics.ascent_int();
    let height = metrics.height_int().max(1) as u32;

    // The fractional advances are summed first and rounded once at the end,
    // not rounded individually and then summed.
    let total_26_6: i32 = text
        .chars()
        .map(|c| char_advance_26_6(&face, c, pixel_size))
        .sum();
    let width = (((total_26_6 + 32) >> 6).max(1)) as u32;

    let mut alpha = vec![0u8; (width * height) as usize];

    let mut ctx = ScaleContext::new();
    let mut scaler = ctx.builder(font).size(pixel_size as f32).hint(true).build();
    let charmap = font.charmap();

    let mut pen_26_6 = 0i32;
    for c in text.chars() {
        let gid = charmap.map(c);
        // The strike is asked for first, and `raster` is where that rule and
        // its reasons live: the terminal's glyph atlas takes the same one.
        if let Some(image) = raster::glyph_mask(&mut scaler, gid) {
            // Each glyph is placed at the truncated running advance; only the
            // string's total width is rounded.
            let x0 = (pen_26_6 >> 6) + image.placement.left;
            let y0 = ascent - image.placement.top;
            blit(
                &mut alpha,
                width,
                height,
                x0,
                y0,
                &image.data,
                image.placement.width,
                image.placement.height,
            );
        }
        pen_26_6 += char_advance_26_6(&face, c, pixel_size);
    }

    Some(LedRaster {
        width,
        height,
        alpha,
    })
}

/// Antialiasing off: any coverage at or past half a pixel inks the lamp,
/// nothing else does.
fn threshold(coverage: u8) -> u8 {
    if coverage >= 128 {
        255
    } else {
        0
    }
}

#[allow(clippy::too_many_arguments)]
fn blit(dst: &mut [u8], dw: u32, dh: u32, x0: i32, y0: i32, src: &[u8], sw: u32, sh: u32) {
    for sy in 0..sh as i32 {
        let dy = y0 + sy;
        if dy < 0 || dy >= dh as i32 {
            continue;
        }
        for sx in 0..sw as i32 {
            let dx = x0 + sx;
            if dx < 0 || dx >= dw as i32 {
                continue;
            }
            let v = threshold(src[(sy as u32 * sw + sx as u32) as usize]);
            if v > 0 {
                dst[(dy as u32 * dw + dx as u32) as usize] = v;
            }
        }
    }
}
