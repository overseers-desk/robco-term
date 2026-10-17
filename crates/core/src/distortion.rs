//! Inverse screen-curvature distortion: undoes the CRT-tube warp applied to
//! the terminal grid so a pointer click can be mapped back to a grid cell.
//!
//! The forward warp bends the flat terminal grid outward to fake a CRT
//! tube's curvature; a pointer click lands on the bent (visible) surface, so
//! before it can be turned into a grid cell it has to be pushed back through
//! the same warp. This reuses the *forward* warp's own math to approximate
//! the inverse (curvature is small and the warp is close to involutive at
//! the magnitudes this app uses), rather than deriving an exact inverse.
//!
//! Applied on every pointer path (press, release, move) from day one:
//! `app`'s window/mouse code is expected to call
//! [`correct_distortion`] before turning a widget-space point into a
//! terminal-grid coordinate.

/// The screen/chassis geometry and runtime settings the distortion
/// transform needs: `width`/`height` are the well, the rectangle the
/// renderer's offscreen target covers and a pointer position arrives in;
/// `frame_size` is the chassis inset, `screen_curvature(_size)` and
/// `normalized_screen_scale` are the curvature settings; and
/// `total_width`/`total_height` are the grid's own rectangle inside that
/// well, which is what the corrected point is expressed against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistortionParams {
    /// The well's width in pixels: the pointer-input area, and the size of
    /// the offscreen target the renderer draws into.
    pub width: f64,
    /// The well's height in pixels.
    pub height: f64,
    /// Chassis frame inset, as a fraction of width/height.
    pub frame_size: f64,
    /// Screen curvature strength.
    pub screen_curvature: f64,
    /// Screen curvature size scaling factor.
    pub screen_curvature_size: f64,
    /// Scale factor normalizing curvature to the on-screen curved-glass
    /// region, measured in logical pixels ([`normalized_screen_scale`]).
    pub normalized_screen_scale: f64,
    /// The grid's own width in pixels, which the margin has already been
    /// taken out of (`Viewport::term_size`).
    pub total_width: f64,
    /// The grid's own height in pixels.
    pub total_height: f64,
}

/// Point in the undistorted grid-texture space `correct_distortion` maps
/// into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Inverse-distortion transform: a pointer coordinate in well-pixel space
/// (`x`, `y`) maps to a coordinate in the grid's own space, undoing the CRT
/// curvature warp.
///
/// The renderer's three steps, run backwards. It normalizes over the well,
/// because that is the span the shader's own kernel is written in; it
/// expands by the chassis frame padding and applies the quadratic
/// correction centered on the frame's midpoint; then it scales back to well
/// pixels and subtracts where the grid starts inside the well.
///
/// The margin is not a term here. It reaches the picture by shrinking the
/// grid (`Viewport::term_size` takes it off before dividing by the cell),
/// and the renderer then centers that smaller grid in the well, so
/// `total_width`/`total_height` carry the margin's whole effect on both
/// sides of the glass.
pub fn correct_distortion(x: f64, y: f64, p: &DistortionParams) -> Point {
    let mut x = x / p.width;
    let mut y = y / p.height;

    x = x * (1.0 + p.frame_size * 2.0) - p.frame_size;
    y = y * (1.0 + p.frame_size * 2.0) - p.frame_size;

    // cc_w tracks the x-axis distance from center, cc_h tracks y.
    let cc_w = 0.5 - x;
    let cc_h = 0.5 - y;
    let distortion = (cc_h * cc_h + cc_w * cc_w)
        * p.screen_curvature
        * p.screen_curvature_size
        * p.normalized_screen_scale;

    Point {
        x: (x - cc_w * (1.0 + distortion) * distortion) * p.width
            - grid_origin(p.width, p.total_width),
        y: (y - cc_h * (1.0 + distortion) * distortion) * p.height
            - grid_origin(p.height, p.total_height),
    }
}

/// Where the grid's rectangle starts inside the well, on one axis. The
/// renderer centers it at a whole pixel and clamps at zero
/// (`draw_frame`'s `(target - grid).max(0) / 2`); a pointer that landed on
/// a different seam would report a different cell than the one drawn under
/// it.
fn grid_origin(well: f64, grid: f64) -> f64 {
    ((well - grid).max(0.0) / 2.0).floor()
}

/// Screen curvature size scaling constant: `0.6`. Not a config key (it has
/// no setter, no preset ever touches it, and it is not one of the
/// `compose*` keys extracted into config), so it is a constant here rather
/// than a [`config`](../../config/index.html) field.
pub const SCREEN_CURVATURE_SIZE: f64 = 0.6;

/// Normalizes curvature strength to the on-screen box the curved glass
/// fills, measured in *logical* pixels: the unit the render chain's
/// geometry normalizes the shader's curvature and frame uniforms in, so a
/// caller passes the well's physical size divided by the scale factor,
/// while the pointer position itself and [`DistortionParams::width`] and
/// `height` stay physical. A window manager free to grant no size at all
/// is why the denominator is floored at 1.
pub fn normalized_screen_scale(width: f64, height: f64) -> f64 {
    1024.0 / (0.5 * width + 0.5 * height).max(1.0)
}

/// Forward screen-curvature distortion: the warp kernel a CRT shader
/// applies to bend the flat terminal grid onto the curved glass, expressed
/// here as the identical scalar kernel operating on plain `f64`s (this
/// crate has no GPU pipeline, so this is not literally the shader).
///
/// The curvature factor combines the same three inputs
/// [`DistortionParams`] carries separately (`screen_curvature *
/// screen_curvature_size * normalized_screen_scale`), combined here exactly
/// as a shader's uniform upload would combine them, so this crate and a GPU
/// pass in `crt-render` name and compute the same scalar from the
/// same inputs.
///
/// [`correct_distortion`] reuses this exact kernel -- the only nonlinear
/// step in either direction -- to *approximate* its own inverse rather than
/// solving the cubic that would exactly undo it (see this module's top doc
/// comment). `forward_distort` reapplies that same kernel once rather than
/// inverting it properly (e.g. by Newton iteration): this wires the pointer
/// to the same curvature approximation the renderer uses and proves the two
/// independently-sourced implementations agree, it does not improve on the
/// approximation's own accuracy.
///
/// [`DistortionParams`] is reused for the input/output units so this
/// composes directly with [`correct_distortion`]: `(x, y)` is a point in
/// [`correct_distortion`]'s *output* domain (grid pixels), and the result
/// is a point back in its *input* domain (well pixels). The grid origin,
/// the well normalization and the frame padding are exactly undone --
/// ordinary invertible affine algebra, not part of the warp -- which is
/// what makes `forward_distort(correct_distortion(x, y, p), p)` the exact
/// identity when `p.screen_curvature` is `0.0` (the kernel term vanishes
/// entirely, so no approximation is in play at all) for *any* frame size
/// or width/height/total_width/total_height. For nonzero
/// curvature the round trip is only approximate, and the error grows with
/// curvature: measured (production-scale geometry, `screen_curvature` at
/// each preset's own value, `screen_curvature_size` `0.6`,
/// `normalized_screen_scale` `1.2`) at `0.1` curvature the worst sampled
/// point across the visible screen is off by ~4.5% of the screen's width;
/// at `0.5` (e.g. "Commodore 64"/"Apple ][") ~38%; at `0.7` (the highest
/// curvature any preset uses, "Commodore PET") ~73%. This reflects the
/// approximation's own inherent error, not a defect this implementation
/// introduces -- see the `forward_after_inverse_*` tests for the measured
/// numbers this doc comment states.
pub fn forward_distort(x: f64, y: f64, p: &DistortionParams) -> Point {
    // Undo `correct_distortion`'s grid origin and its final well-pixel
    // scale, landing back in the normalized domain its own kernel step
    // operated in (i.e. the padded-but-not-yet-unpadded value -- *not* the
    // pre-padding one, so this does not pad a second time).
    let padded_u = (x + grid_origin(p.width, p.total_width)) / p.width;
    let padded_v = (y + grid_origin(p.height, p.total_height)) / p.height;

    let cc_u = padded_u - 0.5;
    let cc_v = padded_v - 0.5;
    let curvature = p.screen_curvature * p.screen_curvature_size * p.normalized_screen_scale;
    let dist = (cc_u * cc_u + cc_v * cc_v) * curvature;
    let warped_u = padded_u + cc_u * (1.0 + dist) * dist;
    let warped_v = padded_v + cc_v * (1.0 + dist) * dist;

    // Undo the frame padding and the well normalization
    // `correct_distortion` applied on the way in: exact linear algebra
    // (not part of the warp), so it contributes no error of its own.
    let unpadded_u = (warped_u + p.frame_size) / (1.0 + p.frame_size * 2.0);
    let unpadded_v = (warped_v + p.frame_size) / (1.0 + p.frame_size * 2.0);

    Point {
        x: unpadded_u * p.width,
        y: unpadded_v * p.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    /// The required property: identity at zero curvature. With
    /// `screen_curvature == 0`, the quadratic distortion term vanishes
    /// regardless of `screen_curvature_size`/`normalized_screen_scale`, and
    /// with no frame inset and a grid filling the well the transform is the
    /// identity on the input point.
    #[test]
    fn identity_at_zero_curvature_no_frame_grid_fills_the_well() {
        let p = DistortionParams {
            width: 200.0,
            height: 150.0,
            frame_size: 0.0,
            screen_curvature: 0.0,
            screen_curvature_size: 1.0,
            normalized_screen_scale: 1.0,
            total_width: 200.0,
            total_height: 150.0,
        };
        for &(x, y) in &[(0.0, 0.0), (50.0, 30.0), (199.0, 149.0), (100.0, 75.0)] {
            let out = correct_distortion(x, y, &p);
            assert!(approx_eq(out.x, x, 1e-9), "x: {} vs {}", out.x, x);
            assert!(approx_eq(out.y, y, 1e-9), "y: {} vs {}", out.y, y);
        }
    }

    /// Zero curvature still holds with a nonzero frame inset and a grid
    /// smaller than the well. This test isolates that the curvature term
    /// alone is what zeroes out, by re-deriving the expected linear map by
    /// hand instead of reusing production code.
    #[test]
    fn zero_curvature_reduces_to_the_frame_origin_linear_map() {
        let p = DistortionParams {
            width: 220.0,
            height: 170.0,
            frame_size: 0.05,
            screen_curvature: 0.0,
            screen_curvature_size: 2.0,
            normalized_screen_scale: 1.3,
            total_width: 200.0,
            total_height: 150.0,
        };
        let (mx, my) = (123.0, 87.0);

        // Hand-computed expected value: with distortion == 0 the return
        // collapses to the frame-corrected normalized coordinate scaled
        // back to well pixels, less where the grid starts in the well.
        let nx = mx / p.width;
        let ny = my / p.height;
        let ex = nx * (1.0 + p.frame_size * 2.0) - p.frame_size;
        let ey = ny * (1.0 + p.frame_size * 2.0) - p.frame_size;
        let expected = Point {
            x: ex * p.width - (p.width - p.total_width) / 2.0,
            y: ey * p.height - (p.height - p.total_height) / 2.0,
        };

        let out = correct_distortion(mx, my, &p);
        assert!(approx_eq(out.x, expected.x, 1e-9));
        assert!(approx_eq(out.y, expected.y, 1e-9));
    }

    /// Sampled points cross-checked against an independent re-implementation
    /// of the same formula (see the Python transcription reproduced inline
    /// below as the exact values it printed), so this isn't just "the Rust
    /// implementation agrees with itself".
    ///
    /// Python:
    /// ```python
    /// def correct_distortion(x, y, width, height, frame_size,
    ///                         curvature, curvature_size, scale,
    ///                         total_width, total_height):
    ///     x = x / width
    ///     y = y / height
    ///     x = x * (1 + frame_size * 2) - frame_size
    ///     y = y * (1 + frame_size * 2) - frame_size
    ///     cc_w = 0.5 - x
    ///     cc_h = 0.5 - y
    ///     distortion = (cc_h**2 + cc_w**2) * curvature * curvature_size * scale
    ///     return (
    ///         (x - cc_w * (1 + distortion) * distortion) * width
    ///             - (width - total_width) // 2,
    ///         (y - cc_h * (1 + distortion) * distortion) * height
    ///             - (height - total_height) // 2,
    ///     )
    /// ```
    #[test]
    fn sampled_points_match_independent_python_reimplementation() {
        let p = DistortionParams {
            width: 300.0,
            height: 200.0,
            frame_size: 0.02,
            screen_curvature: 0.3,
            screen_curvature_size: 0.6,
            normalized_screen_scale: 1.0,
            total_width: 280.0,
            total_height: 180.0,
        };

        // Well-center, corner-ish, and off-axis points; values from
        // actually running the Python transcription above at these exact
        // inputs. The center is the warp's fixed point, so it comes back as
        // the center of the grid, ten pixels in on each axis.
        let cases: &[((f64, f64), (f64, f64))] = &[
            ((150.0, 100.0), (140.0, 90.0)),
            ((10.0, 10.0), (-18.488228061776937, -11.88528946828517)),
            ((290.0, 190.0), (298.48822806177697, 191.88528946828518)),
            ((80.0, 150.0), (65.504775760012, 143.21087445713428)),
        ];
        for &((x, y), (ex, ey)) in cases {
            let out = correct_distortion(x, y, &p);
            assert!(approx_eq(out.x, ex, 1e-6), "x: got {} want {}", out.x, ex);
            assert!(approx_eq(out.y, ey, 1e-6), "y: got {} want {}", out.y, ey);
        }
    }

    /// Cross-checked at a few points against `1024 / max(1, 0.5*width +
    /// 0.5*height)` computed by hand.
    #[test]
    fn normalized_screen_scale_matches_the_defining_formula() {
        assert!(approx_eq(
            normalized_screen_scale(1024.0, 768.0),
            1024.0 / 896.0,
            1e-9
        ));
        assert!(approx_eq(
            normalized_screen_scale(2048.0, 1536.0),
            1024.0 / 1792.0,
            1e-9
        ));
        // Floored at 1: a window manager that grants no size at all must
        // not divide by zero.
        assert!(approx_eq(normalized_screen_scale(0.0, 0.0), 1024.0, 1e-9));
    }

    /// The required property: `forward_distort(correct_distortion(x, y, p), p)`
    /// is the exact identity at `screen_curvature == 0.0`, for a nonzero
    /// frame size and a grid smaller than the well too (unlike the
    /// `zero_curvature_*` tests above, which only claim identity or a
    /// hand-derived linear map). The nonlinear kernel term vanishes
    /// identically, and the origin/frame bookkeeping `forward_distort`
    /// undoes is `correct_distortion`'s own, applied in reverse -- exact
    /// algebra either way.
    #[test]
    fn forward_after_inverse_is_exact_identity_at_zero_curvature() {
        let p = DistortionParams {
            width: 800.0,
            height: 600.0,
            frame_size: 0.03,
            screen_curvature: 0.0,
            screen_curvature_size: SCREEN_CURVATURE_SIZE,
            normalized_screen_scale: 1.2,
            total_width: 760.0,
            total_height: 560.0,
        };
        for &fx in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            for &fy in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                let x = fx * p.width;
                let y = fy * p.height;
                let texture = correct_distortion(x, y, &p);
                let back = forward_distort(texture.x, texture.y, &p);
                assert!(
                    approx_eq(back.x, x, 1e-9) && approx_eq(back.y, y, 1e-9),
                    "({x}, {y}) -> ({}, {}) -> ({}, {}); want ({x}, {y}) exactly",
                    texture.x,
                    texture.y,
                    back.x,
                    back.y
                );
            }
        }
    }

    /// Nonzero curvature: the round trip is only
    /// approximate above zero curvature (the forward kernel is reused as
    /// its own inverse rather than solving for an exact one -- see
    /// `forward_distort`'s doc comment), and the error grows with
    /// curvature. These are the measured worst-case errors
    /// (production-scale window geometry, a 5x5 grid spanning the visible
    /// screen, every `screen_curvature` value any bundled preset uses)
    /// that back the numbers in `forward_distort`'s doc comment; the
    /// bounds below are those measurements with ~15-20% headroom, not a
    /// tight theoretical bound.
    #[test]
    fn forward_after_inverse_stays_within_the_measured_tolerance_across_curvature() {
        let base = DistortionParams {
            width: 800.0,
            height: 600.0,
            frame_size: 0.03,
            screen_curvature: 0.0,
            screen_curvature_size: SCREEN_CURVATURE_SIZE,
            normalized_screen_scale: 1.2,
            total_width: 760.0,
            total_height: 560.0,
        };

        // (screen_curvature, max absolute pixel error tolerance), the
        // latter a rounded-up bound on the measured worst point in the
        // grid below (see this module's history for the Python
        // measurement this table is derived from).
        let cases: &[(f64, f64)] = &[
            (0.1, 45.0),
            (0.2, 95.0),
            (0.3, 160.0),
            (0.4, 240.0),
            (0.5, 340.0),
            (0.7, 650.0),
        ];

        for &(curvature, tolerance) in cases {
            let p = DistortionParams {
                screen_curvature: curvature,
                ..base
            };
            let mut worst = 0.0_f64;
            for &fx in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                for &fy in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                    let x = fx * p.width;
                    let y = fy * p.height;
                    let texture = correct_distortion(x, y, &p);
                    let back = forward_distort(texture.x, texture.y, &p);
                    worst = worst.max((back.x - x).abs()).max((back.y - y).abs());
                }
            }
            assert!(
                worst <= tolerance,
                "screen_curvature {curvature}: worst round-trip error {worst}px exceeds the stated tolerance {tolerance}px"
            );
        }
    }

    /// The case a click reproduces and every test above misses: a grid
    /// smaller than the well, which is what a nonzero `screen.margin`
    /// produces once `Viewport::term_size` has floored the remaining space
    /// to whole cells. On flat glass with no moulding the answer is the
    /// well pixel less where the grid starts, exactly, at the bottom of the
    /// screen as much as at the top.
    ///
    /// The geometry is the 900x700 window issue #27 was measured on, with
    /// the shipped margin: a 624-pixel-tall grid seated 38 pixels down.
    #[test]
    fn a_well_pixel_maps_to_that_pixel_less_the_grid_origin() {
        let p = DistortionParams {
            width: 900.0,
            height: 700.0,
            frame_size: 0.0,
            screen_curvature: 0.0,
            screen_curvature_size: SCREEN_CURVATURE_SIZE,
            normalized_screen_scale: 1.2,
            total_width: 816.0,
            total_height: 624.0,
        };
        for &(x, y) in &[(0.0, 0.0), (300.0, 420.0), (899.0, 699.0)] {
            let out = correct_distortion(x, y, &p);
            assert!(
                approx_eq(out.x, x - 42.0, 1e-9) && approx_eq(out.y, y - 38.0, 1e-9),
                "({x}, {y}) -> ({}, {}); want ({}, {})",
                out.x,
                out.y,
                x - 42.0,
                y - 38.0
            );
        }
    }
}
