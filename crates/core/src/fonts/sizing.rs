//! The sizing seam: a catalogue row plus the user's knobs in, the renderer's
//! permissions out.
//!
//! A low-resolution face is rasterised at its design size and *nowhere
//! else*; its magnification is applied to geometry, never to the rasteriser.
//! Re-rasterising Terminess larger instead of scaling its geometry
//! corrupts 3960 pixels while leaving Pet Me looking
//! fine. That magnification is one whole number covering the requested size
//! and the display's density together, so a fractional density still lands
//! every texel on whole device pixels.
//!
//! The catalogue in the parent module is the metadata half of the seam; this
//! is the arithmetic half. Nothing downstream (atlas, pipeline, DPR handling)
//! needs to see a [`FontEntry`] again once it holds a [`ResolvedFont`].

use super::FontEntry;

/// The user-facing knobs. `font_size` is in logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct SizingRequest {
    pub font_size: u32,
    pub line_spacing: f64,
    pub font_width: f64,
    pub device_pixel_ratio: f64,
}

impl Default for SizingRequest {
    fn default() -> Self {
        Self {
            font_size: 24,
            line_spacing: 0.1,
            font_width: 1.0,
            device_pixel_ratio: 1.0,
        }
    }
}

/// What the renderer is allowed to do once the metadata has spoken.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    /// The size handed to the rasteriser, in device pixels. A low-resolution
    /// face's design size, whatever the request.
    pub raster_pixel_size: u32,
    /// Magnification applied as geometry; 1 for a scalable face.
    pub integer_scale: u32,
    /// Antialiasing turns off exactly when the face is low-resolution.
    pub antialias: bool,
    /// Horizontal squeeze, `baseWidth * fontWidth`.
    ///
    /// The renderer only honors this when it keeps the cell
    /// an integer number of pixels wide; see [`is_pixel_exact_width`].
    pub font_width: f64,
    /// Extra pixels between baselines, in unscaled raster pixels.
    pub line_spacing: i32,
    /// The face's family, and the family that covers its gaps.
    pub family: String,
    pub fallback_family: String,
}

pub fn resolve(entry: &FontEntry, req: &SizingRequest) -> ResolvedFont {
    let target = req.font_size.max(1) as f64 * req.device_pixel_ratio;
    let (raster_pixel_size, integer_scale) = if entry.low_resolution {
        let scale = (target / entry.pixel_size as f64).round() as u32;
        (entry.pixel_size, scale.max(1))
    } else {
        (target.round() as u32, 1)
    };

    let fallback_family = super::font_by_name(entry.fallback_name, super::FontSource::Bundled)
        .filter(|f| f.name != entry.name)
        .map(|f| f.family.clone())
        .unwrap_or_default();

    ResolvedFont {
        raster_pixel_size,
        integer_scale,
        antialias: !entry.low_resolution,
        font_width: entry.base_width * req.font_width,
        line_spacing: (raster_pixel_size as f64 * req.line_spacing).round() as i32,
        family: if entry.family.is_empty() {
            entry.name.to_string()
        } else {
            entry.family.clone()
        },
        fallback_family,
    }
}

/// Is this cell width pixel-exact, i.e. does the horizontal squeeze land the
/// cell on a whole number of pixels?
///
/// The `fontWidth` answer: a low-resolution face is only
/// squeezed by ratios that keep the cell an integer number of pixels wide.
/// A continuous horizontal scale of a bitmap glyph is a resample by
/// definition, and a resampled pixel font is the one thing this renderer
/// exists to avoid; a scalable face has no such constraint and is squeezed
/// freely.
///
/// `cell_width_px` is the unscaled advance in raster pixels.
pub fn is_pixel_exact_width(cell_width_px: u32, resolved: &ResolvedFont) -> bool {
    if resolved.antialias {
        return true;
    }
    let scaled = cell_width_px as f64 * resolved.integer_scale as f64 * resolved.font_width;
    (scaled - scaled.round()).abs() < 1e-9 && scaled >= 1.0
}

/// The nearest width factor at or below `wanted` that [`is_pixel_exact_width`]
/// accepts, for the renderer to snap a user's continuous setting to.
pub fn snap_font_width(cell_width_px: u32, integer_scale: u32, wanted: f64) -> f64 {
    let denom = (cell_width_px * integer_scale) as f64;
    if denom <= 0.0 {
        return wanted;
    }
    let target = (wanted * denom).round().max(1.0);
    target / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{font_by_name, FontSource};

    fn at(name: &str, font_size: u32, dpr: f64) -> ResolvedFont {
        let entry = font_by_name(name, FontSource::Bundled).unwrap();
        let req = SizingRequest {
            font_size,
            device_pixel_ratio: dpr,
            ..Default::default()
        };
        resolve(entry, &req)
    }

    #[test]
    fn low_resolution_face_never_changes_raster_size() {
        for size in [6, 12, 24, 31, 128] {
            for dpr in [1.0, 1.5, 2.0, 3.0] {
                let r = at("TERMINESS_SCALED", size, dpr);
                assert_eq!(r.raster_pixel_size, 12, "size {size} dpr {dpr}");
                assert!(!r.antialias);
                assert!(r.integer_scale >= 1);
            }
        }
    }

    #[test]
    fn scalable_face_rasterises_at_size_times_dpr() {
        for (size, dpr, raster) in [(24, 1.0, 24), (24, 2.0, 48), (30, 1.5, 45), (13, 1.25, 16)] {
            let r = at("HACK", size, dpr);
            assert_eq!(r.raster_pixel_size, raster, "size {size} dpr {dpr}");
            assert_eq!(r.integer_scale, 1);
            assert!(r.antialias);
        }
    }

    #[test]
    fn magnification_is_the_nearest_whole_number_and_at_least_one() {
        assert_eq!(at("EXCELSIOR_SCALED", 24, 1.0).integer_scale, 2);
        assert_eq!(at("GOHU_11_SCALED", 24, 1.0).integer_scale, 2);
        assert_eq!(at("TERMINESS_SCALED", 24, 1.5).integer_scale, 3);
        assert_eq!(at("TERMINESS_SCALED", 24, 2.0).integer_scale, 4);
        assert_eq!(at("EXCELSIOR_SCALED", 6, 1.0).integer_scale, 1);
    }

    #[test]
    fn line_spacing_is_a_fraction_of_the_raster_height() {
        assert_eq!(at("TERMINESS_SCALED", 24, 1.0).line_spacing, 1);
        assert_eq!(at("HACK", 24, 1.0).line_spacing, 2);
        assert_eq!(at("HACK", 24, 2.0).line_spacing, 5);
    }

    #[test]
    fn font_width_is_accepted_only_on_whole_pixel_cells() {
        let entry = font_by_name("UNSCII_8_SCALED", FontSource::Bundled).unwrap();
        let mut r = resolve(entry, &SizingRequest::default());
        // baseWidth 0.5 on an 8px cell: 4 whole pixels, exact.
        assert!(is_pixel_exact_width(8, &r));
        r.font_width = 0.5 * 0.7;
        assert!(!is_pixel_exact_width(8, &r));
        let snapped = snap_font_width(8, r.integer_scale, r.font_width);
        r.font_width = snapped;
        assert!(is_pixel_exact_width(8, &r));
    }

    #[test]
    fn a_scalable_face_is_squeezed_freely() {
        let mut r = at("HACK", 24, 1.0);
        r.font_width = 0.813;
        assert!(is_pixel_exact_width(19, &r));
    }
}
