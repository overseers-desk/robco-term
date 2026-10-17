//! Scaled font metrics, computed as exact 26.6 fixed-point arithmetic.
//!
//! [`compute_base_width`] and [`led_text_image`](super::led::led_text_image)
//! both need metrics at a pixel size, and their results are baked into the
//! catalogue and into every LED raster. The arithmetic has to be exact, not
//! merely close.
//!
//! The rule, established against the golden metrics table for all 24
//! bundled faces (`tests/fixtures/golden-fonts.json`, checked by
//! `tests/suite/font_parity.rs`) and fitting candidate arithmetics to it:
//!
//! ```text
//! metric_26_6 = floor(design_units * pixel_size * 64 / units_per_em)
//! ```
//!
//! exact-rational, floored to a 26.6 fixed-point pixel: *not* FreeType's
//! `FT_MulFix` rounding, and not float rounding, both of which are off by
//! 1/64 on several faces in either direction.
//!
//! Ascent and descent have one override: when the face carries an embedded
//! bitmap strike (`EBLC`/`CBLC`) whose vertical ppem equals the pixel size,
//! that strike's own line metrics are reported instead of the outline's.
//! That is what makes Terminess at 12px read 10/2 rather than the outline's
//! 10.53/2.55. Advances always come from `hmtx`, strike or no strike.
//!
//! All 24 catalogue entries match on ascent, descent and the advance of "M".

use ttf_parser::{Face, Tag};

/// A face's metrics at one pixel size, in 26.6 fixed point (1/64 pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaledMetrics {
    pub ascent_26_6: i32,
    pub descent_26_6: i32,
}

impl ScaledMetrics {
    pub fn ascent(&self) -> f64 {
        self.ascent_26_6 as f64 / 64.0
    }
    pub fn descent(&self) -> f64 {
        self.descent_26_6 as f64 / 64.0
    }
    /// Ascent + descent.
    pub fn height(&self) -> f64 {
        self.ascent() + self.descent()
    }
    /// The integer ascent metric, rounded from the fractional one. The
    /// integer height is the sum of the two *separately* rounded halves,
    /// which is why it can exceed `ceil(height())`.
    pub fn ascent_int(&self) -> i32 {
        round_26_6(self.ascent_26_6)
    }
    pub fn descent_int(&self) -> i32 {
        round_26_6(self.descent_26_6)
    }
    pub fn height_int(&self) -> i32 {
        self.ascent_int() + self.descent_int()
    }
}

fn round_26_6(v: i32) -> i32 {
    // Round-to-int by adding half a pixel and truncating.
    (v + 32) >> 6
}

/// The scaling rule above, for one design-unit value.
fn scale_26_6(design_units: i32, pixel_size: u32, units_per_em: u16) -> i32 {
    let n = design_units as i64 * pixel_size as i64 * 64;
    let d = units_per_em as i64;
    // Floor division (not truncation), so negative values (a descender, as
    // `hhea` stores it) round toward negative infinity, matching the floor
    // rule stated in this module's doc comment.
    let q = n.div_euclid(d);
    q as i32
}

fn hhea_ascender_descender(face: &Face) -> (i32, i32) {
    // Deliberately the `hhea` values and not ttf-parser's `Face::ascender()`,
    // which may prefer OS/2 typo or win metrics: horizontal face metrics
    // come from `hhea`.
    match face.raw_face().table(Tag::from_bytes(b"hhea")) {
        Some(t) if t.len() >= 8 => (
            i16::from_be_bytes([t[4], t[5]]) as i32,
            i16::from_be_bytes([t[6], t[7]]) as i32,
        ),
        _ => (face.ascender() as i32, face.descender() as i32),
    }
}

/// Embedded-bitmap strike line metrics at an exact vertical ppem, if the face
/// has such a strike. `EBLC` and `CBLC` share the layout.
fn strike_line_metrics(face: &Face, pixel_size: u32) -> Option<(i32, i32)> {
    for tag in [b"EBLC", b"CBLC"] {
        let Some(t) = face.raw_face().table(Tag::from_bytes(tag)) else {
            continue;
        };
        if t.len() < 8 {
            continue;
        }
        let num_sizes = u32::from_be_bytes([t[4], t[5], t[6], t[7]]) as usize;
        for i in 0..num_sizes {
            // bitmapSizeTable is 48 bytes: 16 bytes of offsets, hori and vert
            // sbitLineMetrics (12 bytes each), then the glyph range and the
            // ppem/bit-depth tail.
            let off = 8 + i * 48;
            if off + 48 > t.len() {
                break;
            }
            let ppem_y = t[off + 45] as u32;
            if ppem_y != pixel_size {
                continue;
            }
            let hori = off + 16;
            let ascender = t[hori] as i8 as i32;
            let descender = t[hori + 1] as i8 as i32;
            return Some((ascender, -descender));
        }
    }
    None
}

/// Metrics for a face at a pixel size, in 26.6 fixed point.
pub fn scaled_metrics(data: &[u8], pixel_size: u32) -> Option<ScaledMetrics> {
    let face = Face::parse(data, 0).ok()?;
    Some(scaled_metrics_for(&face, pixel_size))
}

pub(crate) fn scaled_metrics_for(face: &Face, pixel_size: u32) -> ScaledMetrics {
    if let Some((asc, desc)) = strike_line_metrics(face, pixel_size) {
        return ScaledMetrics {
            ascent_26_6: asc * 64,
            descent_26_6: desc * 64,
        };
    }
    let upem = face.units_per_em();
    let (asc, desc) = hhea_ascender_descender(face);
    ScaledMetrics {
        ascent_26_6: scale_26_6(asc, pixel_size, upem),
        descent_26_6: scale_26_6(-desc, pixel_size, upem),
    }
}

/// Advance of one character at a pixel size, in 26.6 fixed point. From
/// `hmtx`, whether or not the face has a strike at this size.
pub(crate) fn char_advance_26_6(face: &Face, c: char, pixel_size: u32) -> i32 {
    let gid = face.glyph_index(c).unwrap_or(ttf_parser::GlyphId(0));
    let adv = face.glyph_hor_advance(gid).unwrap_or(0) as i32;
    scale_26_6(adv, pixel_size, face.units_per_em())
}

/// Advance of one character at a pixel size, in fractional pixels: the
/// value chassis's LED-cell and tape-letter width contracts both read
/// exactly.
/// `char_advance_26_6` already computes it; this is the only externally
/// visible door to that arithmetic; it stays `pub(crate)` because
/// [`led_text_image`](super::led::led_text_image) sums it across a string on
/// the hot path and does not want the `f64` round-trip.
pub fn char_advance_px(data: &[u8], c: char, pixel_size: u32) -> Option<f64> {
    let face = Face::parse(data, 0).ok()?;
    Some(char_advance_26_6(&face, c, pixel_size) as f64 / 64.0)
}

/// The cell aspect a low-resolution face wants, as a multiple of its
/// height, clamped to `0.25..=2.0`.
pub fn compute_base_width(data: &[u8], pixel_size: u32, fallback: f64) -> f64 {
    let Ok(face) = Face::parse(data, 0) else {
        return fallback;
    };
    let glyph_width = char_advance_26_6(&face, 'M', pixel_size) as f64 / 64.0;
    let glyph_height = scaled_metrics_for(&face, pixel_size).height();
    if glyph_width <= 0.0 || glyph_height <= 0.0 {
        return fallback;
    }
    const TARGET_RATIO: f64 = 0.5;
    (TARGET_RATIO * glyph_height / glyph_width).clamp(0.25, 2.0)
}

/// The family name the face reports.
pub fn family_name(data: &[u8]) -> Option<String> {
    let face = Face::parse(data, 0).ok()?;
    // Typographic family (name ID 16) when present, else the legacy family
    // (ID 1): the pair a font database picks between, and the reason
    // "Terminess Nerd Font Mono" rather than "Terminess Nerd Font" is the
    // catalogue's family.
    let mut legacy = None;
    for name in face.names() {
        if !name.is_unicode() {
            continue;
        }
        match name.name_id {
            ttf_parser::name_id::TYPOGRAPHIC_FAMILY => {
                if let Some(s) = name.to_string() {
                    return Some(s);
                }
            }
            ttf_parser::name_id::FAMILY => {
                if legacy.is_none() {
                    legacy = name.to_string();
                }
            }
            _ => {}
        }
    }
    legacy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{font_by_name, FontSource};

    #[test]
    fn char_advance_px_matches_the_26_6_value_it_wraps() {
        let entry = font_by_name("DEPARTURE_MONO_SCALED", FontSource::Bundled).unwrap();
        let face = Face::parse(entry.data(), 0).unwrap();
        let want = char_advance_26_6(&face, 'M', 20) as f64 / 64.0;
        let got = char_advance_px(entry.data(), 'M', 20).unwrap();
        assert_eq!(got, want);
        assert!(got > 0.0);
    }

    #[test]
    fn char_advance_px_is_none_for_garbage_bytes() {
        assert_eq!(char_advance_px(b"not a font", 'M', 20), None);
    }
}
