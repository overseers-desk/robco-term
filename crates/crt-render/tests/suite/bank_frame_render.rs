//! Done-test: the bank frame drawn, at sampled window sizes, with the uniforms
//! the geometry produces.
//!
//! `robco-chassis`'s `bank_frame_geometry.rs` proves the arithmetic. This
//! proves the arithmetic reaches the glass: the frame is rendered offscreen
//! through this crate's own measurement rig with the uniform set
//! [`chassis::frame::frame_params`] builds from a real [`WindowLayout`], and
//! the screen opening it leaves is measured off the readback and compared
//! against what the distortion formula says it should be.
//!
//! Why the opening is predictable without reimplementing the shader:
//!
//! - `frame_metal.frag`'s screen mask is a rounded-rect SDF over the *distorted*
//!   coordinates, taken against the unit rect (`oracle::frame_metal`, and
//!   `oracle::rounded_rect_sdf_pixels` under it). Outside that rect and away
//!   from a corner the SDF reduces to `(|local| - halfSize) - radius`, and
//!   `halfSize + radius` is exactly half the viewport, so the mask's zero
//!   crossing sits at distorted coordinate 0 whatever the corner radius is.
//! - `distortCoordinates` pads before it bends: `padded = coords * (1 + 2 *
//!   frameSize) - frameSize` (`oracle::distort_coordinates`). At zero curvature
//!   the bend is the identity, so the crossing lands at
//!   `frameSize / (1 + 2 * frameSize)` of the well's width -- the moulding's
//!   thickness, in one closed form, with no shader constants in it.
//!
//! So the measurement below reads the moulding off rendered pixels and checks
//! it against that expression, which carries `frameSize`, which carries the
//! screen scale, which carries the bank's width. A bank width that drifted, or
//! a derivation dropped between the stored setting and the uniform, moves this
//! edge.

use chassis::frame::{self, FrameScale};
use chassis::metrics::shells;
use chassis::{BankGeometry, ChannelIndicator, LedMetrics, WindowLayout};
use config::Config;
use crt::harness::render_single_pass;
use gpu::harness::{px_index, Locked};

/// The stock appliance's bank: amber shell, LED strips, twelve characters,
/// glow. 184 px, and it does not move with the window.
fn stock_bank_width() -> f64 {
    BankGeometry::new(
        &shells::annunciator(),
        &LedMetrics::default(),
        12,
        ChannelIndicator::Glow,
    )
    .implicit_width as f64
}

/// The uniform set and the oracle's parameter struct, built from the same
/// inputs by the two independent paths. Building both here is the point: a
/// misspelled uniform name in `frame_params` reaches the shader as nothing at
/// all, and only a comparison against the oracle notices.
fn params_pair(
    cfg: &Config,
    layout: &WindowLayout,
) -> (Vec<(&'static str, f32)>, oracle::FrameMetalParams) {
    let shell = shells::annunciator();
    let style = frame::styles::annunciator_frame();
    let scale = FrameScale::build(cfg, layout);
    let gpu = frame::frame_params(&style, &shell, cfg, layout);
    let cpu = oracle::FrameMetalParams {
        screen_curvature: scale.screen_curvature,
        frame_size: scale.frame_size,
        screen_radius: scale.screen_radius,
        ambient_light: cfg.screen.ambient_light as f32,
        frame_shininess: scale.frame_shininess,
        light_dir: shell.casting_light_dir,
        bezel_color: [
            style.bezel_color.r,
            style.bezel_color.g,
            style.bezel_color.b,
        ],
        chassis_color: [
            shell.casting_color.r,
            shell.casting_color.g,
            shell.casting_color.b,
        ],
        ridge_color: [
            style.ridge_color.r,
            style.ridge_color.g,
            style.ridge_color.b,
        ],
        bezel_margins: style.bezel_margins,
        outer_radius: style.outer_radius,
        well_depth: style.well_depth,
        well_floor: style.well_floor,
        ridge_gain: style.ridge_gain,
        metal: oracle::MetalParams {
            grain_amount: style.grain_amount,
            mottle_amount: style.mottle_amount,
            scratch_amount: style.scratch_amount,
        },
        vignette_strength: style.vignette_strength,
        fill_gain: style.fill_gain,
        trough_gain: style.trough_gain,
        face_band_px: style.face_band_px,
        rim_dist_px: style.rim_dist_px,
        rim_gain: style.rim_gain,
    };
    (gpu, cpu)
}

/// Where the moulding gives way to the glass, along the row nearest the well's
/// waist. Returned in pixels from the well's left edge.
///
/// The frame's alpha is the discriminator, and it is not opacity: it is
/// `mix(1 - shininess * 0.4, ambient * 0.3, inScreen)` (`oracle::frame_metal`),
/// so it sits near 0.94 on metal and near 0.09 on glass with a one-pixel band
/// between. Anything either side of the midpoint is unambiguous.
fn moulding_thickness(pixels: &[[f32; 4]], w: u32, h: u32) -> u32 {
    let row = h / 2;
    for c in 0..w {
        if pixels[px_index(w, c, row)][3] < 0.5 {
            return c;
        }
    }
    panic!("the frame left no opening at all in a {w}x{h} well");
}

/// The three window sizes the frame is rendered at. Kept to sizes a desktop
/// actually hands the window, and no larger: each one is a full offscreen
/// render plus a readback.
const WINDOW_SIZES: [(f64, f64); 3] = [(640.0, 480.0), (1024.0, 768.0), (1440.0, 900.0)];

#[test]
fn the_moulding_lands_where_the_geometry_puts_it() {
    let mut cfg = Config::default();
    // A flat tube, so `distortCoordinates` reduces to its padding step and the
    // opening has a closed form. Curvature gets its own test below.
    cfg.screen.screen_curvature = 0.0;

    let bank = stock_bank_width();
    let gpu = Locked::new().expect("headless wgpu device");
    let preset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../chassis/shaders/metal/frame_metal.slangp");

    for (win_w, win_h) in WINDOW_SIZES {
        let layout = WindowLayout::new(win_w, win_h, bank);
        let w = layout.crt.width as u32;
        let h = layout.crt.height as u32;
        let (params, oracle_params) = params_pair(&cfg, &layout);

        let input = vec![0u8; (w * h * 4) as usize];
        let out = render_single_pass(&gpu, &preset, &params, w, h, &input);

        // The uniform, derived here by hand rather than read back off the
        // thing under test: the stored frame-size setting of the stock
        // profile (0.3), the `frame_size = frame_size_setting * 0.05`
        // formula, and the screen scale. Asserting it against what the code
        // produced is what makes the measurement below a check on the
        // derivation and not merely on the shader's use of whatever number
        // it was handed.
        let fs = 0.3 * 0.05 * layout.normalized_screen_scale() as f32;
        assert!(
            (oracle_params.frame_size - fs).abs() < 1e-6,
            "frameSize derivation: {} vs {fs}",
            oracle_params.frame_size
        );
        // The closed form: frameSize / (1 + 2 * frameSize) of the well's width.
        let expected = (fs / (1.0 + 2.0 * fs)) * w as f32;
        let measured = moulding_thickness(&out, w, h) as f32;

        // Two pixels: the mask's own one-pixel smoothstep band, plus the half
        // pixel between a texel centre and the edge it straddles.
        assert!(
            (measured - expected).abs() <= 2.0,
            "in a {win_w}x{win_h} window the moulding measured {measured} px \
             where the geometry puts it at {expected:.2} px \
             (well {w}x{h}, frameSize {fs})"
        );
        // ...and it is a moulding, not a hairline or the whole plate.
        assert!(
            measured > 4.0 && measured < w as f32 * 0.2,
            "implausible moulding of {measured} px in a {w} px well"
        );
    }
}

#[test]
fn the_drawn_frame_agrees_with_the_oracle_on_uniforms_the_geometry_chose() {
    // The uniform set reaches the shader whole: a name `frame_params` spells
    // differently from the shader is silently dropped, and the only thing that
    // notices is a pixel that no longer matches the formula.
    let cfg = Config::default();
    let layout = WindowLayout::new(1024.0, 768.0, stock_bank_width());
    let w = layout.crt.width as u32; // 840
    let h = layout.crt.height as u32; // 768
    let (params, oracle_params) = params_pair(&cfg, &layout);

    let gpu = Locked::new().expect("headless wgpu device");
    let preset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../chassis/shaders/metal/frame_metal.slangp");
    let input = vec![0u8; (w * h * 4) as usize];
    let out = render_single_pass(&gpu, &preset, &params, w, h, &input);

    // A corner (chassis beyond the plate), the moulding at the waist, the
    // glass, and a point on the plate's lit top band.
    for &(c, r) in &[(2u32, 2u32), (10, h / 2), (w / 2, h / 2), (w / 2, 6)] {
        let uv = [(c as f32 + 0.5) / w as f32, (r as f32 + 0.5) / h as f32];
        let (color, alpha) = oracle::frame_metal(uv, [w as f32, h as f32], &oracle_params);
        let px = out[px_index(w, c, r)];
        let tol = 0.015;
        assert!(
            (px[0] - color[0]).abs() < tol
                && (px[1] - color[1]).abs() < tol
                && (px[2] - color[2]).abs() < tol,
            "color mismatch at ({c},{r}): gpu={:?} oracle={color:?}",
            &px[0..3]
        );
        assert!(
            (px[3] - alpha).abs() < 0.01,
            "alpha mismatch at ({c},{r}): gpu={} oracle={alpha}",
            px[3]
        );
    }
}

#[test]
fn a_wider_bank_thickens_the_moulding_it_leaves() {
    // The chain the seam drag pulls: a wider bank narrows the well, a
    // narrower well raises the normalized screen scale, a higher scale
    // raises `frame_size`, and the moulding grows. It is the reason the
    // seam moves the frame's look and not only its width, and it is
    // measured here on rendered pixels rather than argued.
    let mut cfg = Config::default();
    cfg.screen.screen_curvature = 0.0;

    let gpu = Locked::new().expect("headless wgpu device");
    let preset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../chassis/shaders/metal/frame_metal.slangp");

    let mut measured = Vec::new();
    // The stock bank, and the bank at roughly twice and four times the
    // characters, in a window whose size does not change.
    for bank in [184.0, 334.0, 634.0] {
        let layout = WindowLayout::new(1440.0, 900.0, bank);
        let w = layout.crt.width as u32;
        let h = layout.crt.height as u32;
        let (params, p) = params_pair(&cfg, &layout);
        let input = vec![0u8; (w * h * 4) as usize];
        let out = render_single_pass(&gpu, &preset, &params, w, h, &input);
        let px = moulding_thickness(&out, w, h) as f32;
        // Each one still lands on its own closed form.
        let expected = (p.frame_size / (1.0 + 2.0 * p.frame_size)) * w as f32;
        assert!(
            (px - expected).abs() <= 2.0,
            "bank {bank}: moulding {px} px against {expected:.2} px"
        );
        measured.push((bank, w, px, p.frame_size));
    }

    // The claim, as a fraction of the well: a narrower well wears a
    // proportionally heavier moulding.
    let fractions: Vec<f32> = measured
        .iter()
        .map(|(_, w, px, _)| px / *w as f32)
        .collect();
    assert!(
        fractions[0] < fractions[1] && fractions[1] < fractions[2],
        "the moulding did not thicken as the bank grew: {measured:?}"
    );
}

#[test]
fn curvature_pushes_the_glass_edge_out_under_the_moulding() {
    // With the tube bent, `distortCoordinates` moves a point below the centre
    // further from it, so the mask's zero crossing needs a larger padded
    // coordinate to reach: the glass retreats and the moulding reads thicker.
    // The stock profile's curvature is small, so this is a one-sided bound
    // rather than a figure -- what it catches is a curvature uniform that
    // never arrived, or arrived with the sign flipped.
    let gpu = Locked::new().expect("headless wgpu device");
    let preset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../chassis/shaders/metal/frame_metal.slangp");
    let layout = WindowLayout::new(1440.0, 900.0, stock_bank_width());
    let w = layout.crt.width as u32;
    let h = layout.crt.height as u32;
    let input = vec![0u8; (w * h * 4) as usize];

    let mut thickness = Vec::new();
    for curvature in [0.0, 0.2, 0.7] {
        let mut cfg = Config::default();
        // 0.7 is the highest curvature any bundled screen preset carries.
        cfg.screen.screen_curvature = curvature;
        let (params, _) = params_pair(&cfg, &layout);
        let out = render_single_pass(&gpu, &preset, &params, w, h, &input);
        thickness.push(moulding_thickness(&out, w, h));
    }

    assert!(
        thickness[0] < thickness[1] && thickness[1] < thickness[2],
        "curvature did not retreat the glass: {thickness:?}"
    );
}
