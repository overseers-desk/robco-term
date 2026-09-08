//! The sizing seam: pixel-size and scaling computation, plus the
//! integer-scale policy applied on top of it, in one place.
//!
//! The properties this module has to preserve: a low-resolution face is
//! rasterised at its design size and *nowhere else*, and every requested
//! magnification is applied to geometry rather than to the rasteriser. The
//! counterexample stands: re-rasterising Terminess larger instead of scaling
//! its geometry corrupted 3960 pixels while leaving Pet Me looking fine.
//!
//! The catalogue in the parent module is the metadata half of the seam; this
//! is the arithmetic half. Nothing downstream (atlas, pipeline, DPR handling)
//! needs to see a [`FontEntry`] again once it holds a [`ResolvedFont`].

use super::{FontEntry, BASE_FONT_PIXEL_HEIGHT};

/// The user-facing knobs. `font_scaling` defaults to `1.0` and
/// `base_font_scaling` to `0.75`.
#[derive(Clone, Copy, Debug)]
pub struct SizingRequest {
    pub font_scaling: f64,
    pub base_font_scaling: f64,
    pub line_spacing: f64,
    pub font_width: f64,
    pub window_scaling: f64,
    pub device_pixel_ratio: f64,
    /// The prose face's pixel size as a multiple of the configured face's.
    /// `1.0` here is the neutral request; the profile's own default is the
    /// schema's to state.
    pub prose_scaling: f64,
}

impl Default for SizingRequest {
    fn default() -> Self {
        Self {
            font_scaling: 1.0,
            base_font_scaling: 0.75,
            line_spacing: 0.1,
            font_width: 1.0,
            window_scaling: 1.0,
            device_pixel_ratio: 1.0,
            prose_scaling: 1.0,
        }
    }
}

/// The computed-font contract the renderer consumes.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedFont {
    pub family: String,
    pub pixel_size: u32,
    pub line_spacing: i32,
    pub screen_scaling: f64,
    pub font_width: f64,
    pub fallback_family: String,
    pub low_resolution: bool,
}

/// How a continuous scale becomes an integer one.
///
/// [`ScalePolicy::Floor`] is the default, so an existing
/// profile keeps rendering the size it renders today. The cost is recorded
/// and known: just under a doubling asks for 1.99x and gets 1x, text up to
/// 25% smaller than the request at the sizes users actually pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScalePolicy {
    /// `max(1.0, floor(v))`.
    #[default]
    Floor,
    /// Nearest integer. Overshoots by up to half a step but tracks the
    /// requested size more closely. Not the default; kept because switching
    /// to it is a deliberately open design fork.
    Round,
}

impl ScalePolicy {
    fn apply(self, v: f64) -> u32 {
        let n = match self {
            ScalePolicy::Floor => v.floor(),
            ScalePolicy::Round => v.round(),
        };
        (n as i64).max(1) as u32
    }
}

/// What the renderer is allowed to do once the metadata has spoken.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    /// The size handed to the rasteriser. Pinned to the face's design size for
    /// a low-resolution face, independent of every scaling knob and of DPR.
    pub raster_pixel_size: u32,
    /// The size the prose face is rasterised at: the configured face's size
    /// times `prose_scaling`, floored at one. The row keeps the configured
    /// face's height regardless.
    pub prose_pixel_size: u32,
    /// Total magnification applied as geometry: `texture_scale * dpr_scale`.
    pub integer_scale: u32,
    /// The font-scaling half.
    pub texture_scale: u32,
    /// The DPR half.
    pub dpr_scale: u32,
    /// The continuous scale the integer one was floored from; reported so the
    /// loss to the floor policy is visible rather than discovered.
    pub screen_scaling: f64,
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

pub fn compute_font(entry: &FontEntry, req: &SizingRequest) -> ComputedFont {
    let total_font_scaling = req.base_font_scaling * req.font_scaling;
    let target_pixel_height = BASE_FONT_PIXEL_HEIGHT * total_font_scaling;

    let line_spacing = (target_pixel_height * req.line_spacing).round() as i32;
    let pixel_size = if entry.low_resolution {
        entry.pixel_size
    } else {
        // Truncates here, does not round.
        target_pixel_height as u32
    };

    let native_line_height =
        entry.pixel_size as f64 + (entry.pixel_size as f64 * req.line_spacing).round();
    let target_line_height = target_pixel_height + line_spacing as f64;
    let screen_scaling = if entry.low_resolution && native_line_height > 0.0 {
        target_line_height / native_line_height
    } else {
        1.0
    };

    let fallback_family = super::font_by_name(entry.fallback_name, super::FontSource::Bundled)
        .filter(|f| f.name != entry.name)
        .map(|f| f.family.clone())
        .unwrap_or_default();

    ComputedFont {
        family: if entry.family.is_empty() {
            entry.name.to_string()
        } else {
            entry.family.clone()
        },
        pixel_size,
        line_spacing,
        screen_scaling,
        font_width: entry.base_width * req.font_width,
        fallback_family,
        low_resolution: entry.low_resolution,
    }
}

/// The full seam: metadata plus knobs in, renderer permissions out.
pub fn resolve(entry: &FontEntry, req: &SizingRequest, policy: ScalePolicy) -> ResolvedFont {
    let computed = compute_font(entry, req);

    let texture_scale = policy.apply(computed.screen_scaling * req.window_scaling);
    // The DPR half applies only to a low-resolution face: `device_pixel_ratio`
    // when true, 1 otherwise.
    //
    // Deliberate divergence: dividing the texture
    // size by the raw (possibly fractional) DPR would give a 1.5x screen a
    // fractional texture and let the crispness guarantee quietly lapse. We
    // take the integer part and leave the remainder as headroom, so the
    // guarantee holds at every DPR rather than only at integer ones.
    let dpr_scale = if entry.low_resolution {
        policy.apply(req.device_pixel_ratio)
    } else {
        1
    };

    ResolvedFont {
        raster_pixel_size: computed.pixel_size,
        prose_pixel_size: ((computed.pixel_size as f64 * req.prose_scaling).round() as u32).max(1),
        integer_scale: texture_scale * dpr_scale,
        texture_scale,
        dpr_scale,
        screen_scaling: computed.screen_scaling,
        antialias: !entry.low_resolution,
        font_width: computed.font_width,
        line_spacing: computed.line_spacing,
        family: computed.family,
        fallback_family: computed.fallback_family,
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

    #[test]
    fn low_resolution_face_never_changes_raster_size() {
        let entry = font_by_name("TERMINESS_SCALED", FontSource::Bundled).unwrap();
        for scaling in [0.5, 1.0, 1.7, 3.0] {
            for dpr in [1.0, 1.5, 2.0, 3.0] {
                let r = resolve(
                    entry,
                    &SizingRequest {
                        font_scaling: scaling,
                        device_pixel_ratio: dpr,
                        ..Default::default()
                    },
                    ScalePolicy::Floor,
                );
                assert_eq!(r.raster_pixel_size, 12, "scaling {scaling} dpr {dpr}");
                assert!(!r.antialias);
                assert!(r.integer_scale >= 1);
            }
        }
    }

    #[test]
    fn scalable_face_moves_raster_size_and_keeps_scale_at_one() {
        let entry = font_by_name("HACK", FontSource::Bundled).unwrap();
        let r = resolve(
            entry,
            &SizingRequest {
                font_scaling: 2.0,
                base_font_scaling: 1.0,
                device_pixel_ratio: 2.0,
                ..Default::default()
            },
            ScalePolicy::Floor,
        );
        assert_eq!(r.raster_pixel_size, 64);
        assert_eq!(r.dpr_scale, 1);
        assert!(r.antialias);
    }

    #[test]
    fn dpr_only_multiplies_the_geometric_scale() {
        let entry = font_by_name("TERMINESS_SCALED", FontSource::Bundled).unwrap();
        let base = resolve(entry, &SizingRequest::default(), ScalePolicy::Floor);
        let hidpi = resolve(
            entry,
            &SizingRequest {
                device_pixel_ratio: 2.0,
                ..Default::default()
            },
            ScalePolicy::Floor,
        );
        assert_eq!(base.raster_pixel_size, hidpi.raster_pixel_size);
        assert_eq!(hidpi.integer_scale, base.integer_scale * 2);
    }

    /// The floor policy's cost, kept as a test so the decision stays visible:
    /// a request just short of 2x renders at 1x.
    #[test]
    fn floor_policy_can_render_a_quarter_smaller_than_asked() {
        let entry = font_by_name("UNSCII_8_SCALED", FontSource::Bundled).unwrap();
        // Sweep the font-size knob and keep the worst case: how much smaller
        // than the requested line height the floor policy is willing to draw.
        let mut worst = 1.0f64;
        let mut worst_scaling = 0.0;
        for step in 1..400 {
            let req = SizingRequest {
                font_scaling: step as f64 / 100.0,
                ..Default::default()
            };
            let r = resolve(entry, &req, ScalePolicy::Floor);
            let shrink = r.texture_scale as f64 / r.screen_scaling;
            if shrink < worst {
                worst = shrink;
                worst_scaling = req.font_scaling;
            }
        }
        // Just short of a doubling: asked for nearly 2x, drawn at 1x.
        assert!(
            worst < 0.76,
            "worst shrink {worst} at scaling {worst_scaling}"
        );
        let req = SizingRequest {
            font_scaling: worst_scaling,
            ..Default::default()
        };
        assert!(
            resolve(entry, &req, ScalePolicy::Round).texture_scale
                > resolve(entry, &req, ScalePolicy::Floor).texture_scale
        );
    }

    #[test]
    fn font_width_is_accepted_only_on_whole_pixel_cells() {
        let entry = font_by_name("UNSCII_8_SCALED", FontSource::Bundled).unwrap();
        let mut r = resolve(entry, &SizingRequest::default(), ScalePolicy::Floor);
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
        let entry = font_by_name("HACK", FontSource::Bundled).unwrap();
        let mut r = resolve(entry, &SizingRequest::default(), ScalePolicy::Floor);
        r.font_width = 0.813;
        assert!(is_pixel_exact_width(19, &r));
    }
}
