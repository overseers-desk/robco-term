//! The bank frame: the bezel the screen well is set into, and the chassis metal
//! that continues it leftwards under the bank.
//!
//! Both the bezel and the chassis metal are shaded surfaces over the metals
//! so this module turns a window layout and a profile into the named
//! parameters `shaders/metal/frame_metal.slang` and
//! `shaders/wgsl/chassis_metal.wgsl` take, and whoever owns the device mounts
//! the pass. The two live in different languages because they sit on
//! different sides of the chain: the bezel is inside the curvature and the
//! casting is drawn after it.
//!
//! Two things about the frame are easy to get backwards.
//!
//! **The chassis frame ignores the profile's frame colour.** The bare screen
//! bezel (the one a screen wears when no chassis stands around it) tints
//! itself from the profile's frame colour. The shell frames do not: their
//! bezel, ridge and chassis colours are the shell's own casting, because a
//! cabinet is one poured piece and its metal is not a per-profile tint. What
//! the profile still moves is the *light* on it: ambient light, shininess,
//! frame size, radius and curvature all come from the settings.
//!
//! **The frame is not chrome over the glass; it is drawn under the curvature.**
//! The frame renders into a texture the dynamic pass samples, feeding the
//! frame-source slot, and the frame shader distorts its own coordinates by
//! the same `distortCoordinates` the glass uses (`frame_metal.frag`, and the
//! `robco-shader-oracle` crate's `distort_coordinates` here). That is how the bezel hugs a
//! curved tube instead of standing square around it. The rule that chassis
//! chrome composites over the glass rather than through the chain is about
//! the *bank column*, which is flat and square and to the left of
//! everything; the bezel is the one piece of chassis that belongs in the
//! frame-source slot. Wiring it there in place of `terminal_frame` when a
//! chassis is shown is the presentation layer's business, and the two are
//! drop-in alternatives for the same slot.

use crate::layout::WindowLayout;
use crate::metrics::{Rgb, ShellMetrics};
use crate::params::{ChassisMetalParams, MetalParams};
use config::Config;

/// One named uniform, the same shape `crt::params::Param` uses.
pub type Param = (&'static str, f32);

/// `crt-render/src/params.rs:23` and `term/src/distortion.rs:102` hold this
/// same 0.6: a fixed constant rather than a setting, and the frame has to
/// distort by exactly the number the glass distorts by or the bezel and the
/// tube part company at the rim.
pub const SCREEN_CURVATURE_SIZE: f32 = 0.6;

fn lint(a: f32, b: f32, t: f32) -> f32 {
    (1.0 - t) * a + t * b
}

/// A shell's bezel: everything fixed about it that is not read from the
/// settings or from the shell's own metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameStyle {
    /// The plate's own face colour.
    pub bezel_color: Rgb,
    /// The worn bright edge along the moulding.
    pub ridge_color: Rgb,
    /// Bezel plate edge insets in px: left, top, right, bottom.
    pub bezel_margins: [f32; 4],
    pub outer_radius: f32,
    pub well_depth: f32,
    pub well_floor: f32,
    pub ridge_gain: f32,
    pub trough_gain: f32,
    pub grain_amount: f32,
    pub mottle_amount: f32,
    pub scratch_amount: f32,
    pub vignette_strength: f32,
    pub fill_gain: f32,
    /// The face-band law: a plate lit only in a band along its own moulding,
    /// with the rim light standing on the well's wall around the opening.
    /// Zeros switch it off, which is how the annunciator declares it: every
    /// uniform the shader names gets a value, so the block is written whole.
    pub face_band_px: f32,
    pub rim_dist_px: f32,
    pub rim_gain: f32,
}

/// The chassis metal under the bank. The casting's light and colour are the
/// shell's, shared with the frame; only the surface amounts are the
/// chassis's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChassisStyle {
    pub grain_amount: f32,
    pub mottle_amount: f32,
    pub scratch_amount: f32,
    pub vignette_strength: f32,
}

/// The three shells' bezels and chassis surfaces.
pub mod styles {
    use super::{ChassisStyle, FrameStyle};
    use crate::metrics::Rgb;

    /// A slim bezel in aged gunmetal, low-key room light from high and
    /// slightly left, meeting the bank flush on its left edge.
    pub fn annunciator_frame() -> FrameStyle {
        FrameStyle {
            bezel_color: Rgb::from_hex_literal("#26211c"),
            ridge_color: Rgb::from_hex_literal("#6e5c48"),
            bezel_margins: [0.0, 12.0, 10.0, 9.0],
            outer_radius: 60.0,
            well_depth: 9.0,
            well_floor: 0.4,
            ridge_gain: 0.7,
            trough_gain: 0.35,
            grain_amount: 0.16,
            mottle_amount: 0.5,
            scratch_amount: 0.15,
            vignette_strength: 0.42,
            fill_gain: 0.95,
            // The face-band law, switched off for this shell.
            face_band_px: 0.0,
            rim_dist_px: 0.0,
            rim_gain: 0.0,
        }
    }

    pub fn annunciator_chassis() -> ChassisStyle {
        ChassisStyle {
            grain_amount: 0.16,
            mottle_amount: 0.4,
            scratch_amount: 0.08,
            vignette_strength: 0.42,
        }
    }

    /// A deep barrel-mouthed bezel in aged dark bronze, a bright ridge along
    /// the outer moulding, a 45px well dropping to the glass.
    pub fn slide_rule_frame() -> FrameStyle {
        FrameStyle {
            bezel_color: Rgb::from_hex_literal("#2e2820"),
            ridge_color: Rgb::from_hex_literal("#8c8068"),
            bezel_margins: [6.0, 10.0, 14.0, 12.0],
            outer_radius: 45.0,
            well_depth: 45.0,
            well_floor: 0.16,
            ridge_gain: 0.45,
            trough_gain: 0.0,
            grain_amount: 0.35,
            mottle_amount: 1.0,
            scratch_amount: 0.55,
            vignette_strength: 0.35,
            fill_gain: 0.35,
            face_band_px: 12.0,
            rim_dist_px: 14.0,
            rim_gain: 1.6,
        }
    }

    pub fn slide_rule_chassis() -> ChassisStyle {
        ChassisStyle {
            grain_amount: 0.4,
            mottle_amount: 1.35,
            scratch_amount: 0.7,
            vignette_strength: 0.3,
        }
    }

    /// A slim band of the chassis's own near-neutral gunmetal, dropping
    /// through a shallow dark well to a big round-cornered glass.
    pub fn switchboard_frame() -> FrameStyle {
        FrameStyle {
            bezel_color: Rgb::from_hex_literal("#20242b"),
            ridge_color: Rgb::from_hex_literal("#737a83"),
            bezel_margins: [0.0, 6.0, 10.0, 10.0],
            outer_radius: 26.0,
            well_depth: 30.0,
            well_floor: 0.18,
            ridge_gain: 0.4,
            trough_gain: 0.0,
            grain_amount: 0.35,
            mottle_amount: 0.8,
            scratch_amount: 0.45,
            vignette_strength: 0.4,
            fill_gain: 0.35,
            face_band_px: 10.0,
            rim_dist_px: 12.0,
            rim_gain: 1.3,
        }
    }

    pub fn switchboard_chassis() -> ChassisStyle {
        ChassisStyle {
            grain_amount: 0.45,
            mottle_amount: 0.7,
            scratch_amount: 0.45,
            vignette_strength: 0.22,
        }
    }
}

/// The four settings-derived numbers the frame is drawn with.
///
/// The derivations are the point of this type: each one sits between the
/// stored value `robco-config` holds and the shader uniform. Reading a
/// stored value straight into a uniform gives the frame twenty times the
/// moulding it should have.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameScale {
    /// The frame-size setting times 0.05, times the screen scale.
    pub frame_size: f32,
    /// `lint(4.0, 120.0, screen_radius)`. In pixels, not a fraction.
    pub screen_radius: f32,
    /// The frame-shininess setting times 0.5.
    pub frame_shininess: f32,
    /// The curvature setting, the size constant and the screen scale
    /// together.
    pub screen_curvature: f32,
}

impl FrameScale {
    /// Derive the four from a profile and a window layout.
    ///
    /// The chassis half of the settings is what a shell frame reads, always:
    /// a shell frame is only ever drawn when the chassis is shown, so it
    /// picks the chassis copies of these keys rather than switching on the
    /// flag itself. Curvature and ambient light have no chassis copy and
    /// come from the screen half.
    pub fn build(cfg: &Config, layout: &WindowLayout) -> Self {
        let normalized = layout.normalized_screen_scale() as f32;
        FrameScale {
            frame_size: cfg.chassis.frame_size as f32 * 0.05 * normalized,
            screen_radius: lint(4.0, 120.0, cfg.chassis.screen_radius as f32),
            frame_shininess: cfg.chassis.frame_shininess as f32 * 0.5,
            screen_curvature: cfg.screen.screen_curvature as f32
                * SCREEN_CURVATURE_SIZE
                * normalized,
        }
    }
}

/// The uniform set for `shaders/metal/frame_metal.slangp`.
pub fn frame_params(
    style: &FrameStyle,
    shell: &ShellMetrics,
    cfg: &Config,
    layout: &WindowLayout,
) -> Vec<Param> {
    let scale = FrameScale::build(cfg, layout);
    let light = shell.casting_light_dir;
    let chassis = shell.casting_color;
    vec![
        ("screenCurvature", scale.screen_curvature),
        ("frameSize", scale.frame_size),
        ("screenRadius", scale.screen_radius),
        ("ambientLight", cfg.screen.ambient_light as f32),
        ("frameShininess", scale.frame_shininess),
        ("lightDirX", light[0]),
        ("lightDirY", light[1]),
        ("bezelColorR", style.bezel_color.r),
        ("bezelColorG", style.bezel_color.g),
        ("bezelColorB", style.bezel_color.b),
        ("bezelColorA", 1.0),
        ("chassisColorR", chassis.r),
        ("chassisColorG", chassis.g),
        ("chassisColorB", chassis.b),
        ("chassisColorA", 1.0),
        ("ridgeColorR", style.ridge_color.r),
        ("ridgeColorG", style.ridge_color.g),
        ("ridgeColorB", style.ridge_color.b),
        ("ridgeColorA", 1.0),
        ("bezelMarginsX", style.bezel_margins[0]),
        ("bezelMarginsY", style.bezel_margins[1]),
        ("bezelMarginsZ", style.bezel_margins[2]),
        ("bezelMarginsW", style.bezel_margins[3]),
        ("outerRadius", style.outer_radius),
        ("wellDepth", style.well_depth),
        ("wellFloor", style.well_floor),
        ("ridgeGain", style.ridge_gain),
        ("grainAmount", style.grain_amount),
        ("mottleAmount", style.mottle_amount),
        ("scratchAmount", style.scratch_amount),
        ("vignetteStrength", style.vignette_strength),
        ("fillGain", style.fill_gain),
        ("troughGain", style.trough_gain),
        ("faceBandPx", style.face_band_px),
        ("rimDistPx", style.rim_dist_px),
        ("rimGain", style.rim_gain),
    ]
}

/// The parameter block for `shaders/wgsl/chassis_metal.wgsl`, which the host
/// completes with the well's logical size
/// ([`ChassisMetalParams::record`]) and writes into its own buffer.
///
/// The three field uniforms are what make the bank's metal and the bezel's one
/// casting rather than two: the chassis is drawn in the frame's coordinates,
/// scaled and offset into them, so grain, mottling and vignette run
/// continuously across the boundary.
pub fn chassis_params(
    style: &ChassisStyle,
    shell: &ShellMetrics,
    layout: &WindowLayout,
) -> ChassisMetalParams {
    let (_viewport, field_scale, field_offset) = layout.chassis_field();
    let light = shell.casting_light_dir;
    let color = shell.casting_color;
    ChassisMetalParams {
        field_scale,
        field_offset,
        light_dir: light,
        chassis_color: [color.r, color.g, color.b],
        metal: MetalParams {
            grain_amount: style.grain_amount,
            mottle_amount: style.mottle_amount,
            scratch_amount: style.scratch_amount,
        },
        vignette_strength: style.vignette_strength,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::shells;

    fn param(params: &[Param], name: &str) -> f32 {
        params
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no uniform named {name}"))
            .1
    }

    #[test]
    fn the_derivations_between_the_stored_value_and_the_shader() {
        let cfg = Config::default();
        // The stock chassis profile, as `robco-config` stores it.
        assert_eq!(cfg.chassis.frame_size, 0.3);
        assert_eq!(cfg.chassis.screen_radius, 0.2);
        assert_eq!(cfg.chassis.frame_shininess, 0.3);

        // A well of exactly 1024x1024 normalises to 1.0, so the scale factor
        // drops out and the derivations stand alone.
        let unit = WindowLayout::new(1024.0, 1024.0, 0.0);
        assert_eq!(unit.normalized_screen_scale(), 1.0);
        let s = FrameScale::build(&cfg, &unit);
        // 0.3 * 0.05.
        assert!((s.frame_size - 0.015).abs() < 1e-6);
        // lint(4, 120, 0.2) = 4 + 116 * 0.2.
        assert!((s.screen_radius - 27.2).abs() < 1e-4);
        // 0.3 * 0.5.
        assert!((s.frame_shininess - 0.15).abs() < 1e-6);
        // 0.2 * 0.6 (the stock screen's curvature).
        assert!((s.screen_curvature - cfg.screen.screen_curvature as f32 * 0.6).abs() < 1e-6);
    }

    #[test]
    fn frame_size_and_curvature_follow_the_well_not_the_window() {
        let cfg = Config::default();
        // The same window, once bare and once with the stock 184px bank. The
        // bank narrows the well, which raises the screen scale, which is what
        // keeps the moulding the same visible thickness rather than the same
        // pixel count.
        let bare = FrameScale::build(&cfg, &WindowLayout::new(1024.0, 768.0, 0.0));
        let banked = FrameScale::build(&cfg, &WindowLayout::new(1024.0, 768.0, 184.0));
        assert!(banked.frame_size > bare.frame_size);
        assert!(banked.screen_curvature > bare.screen_curvature);
        // The radius is in pixels and takes no scale at all.
        assert_eq!(banked.screen_radius, bare.screen_radius);
    }

    #[test]
    fn frame_params_carry_the_shells_casting_not_the_profiles_colour() {
        let cfg = Config::default();
        let layout = WindowLayout::new(1024.0, 768.0, 184.0);
        let shell = shells::annunciator();
        let p = frame_params(&styles::annunciator_frame(), &shell, &cfg, &layout);

        // #16130f, the shell's own casting colour, by way of the frame's
        // chassis-colour uniform. The profile's own frame colour (#001735 in
        // the stock chassis) reaches nothing here. The casting colour is a
        // hex-literal colour, parsed at /255 (`rgb`'s own doc,
        // `chassis::color`'s module doc), not the /256 runtime parser.
        assert_eq!(cfg.chassis.frame_color, "#001735");
        assert_eq!(param(&p, "chassisColorR"), 0x16 as f32 / 255.0);
        assert_eq!(param(&p, "chassisColorG"), 0x13 as f32 / 255.0);
        assert_eq!(param(&p, "chassisColorB"), 0x0f as f32 / 255.0);
        // The plate meets the bank flush on the left, so the left inset is
        // zero and the other three are not.
        assert_eq!(param(&p, "bezelMarginsX"), 0.0);
        assert_eq!(param(&p, "bezelMarginsY"), 12.0);
        // This shell switches the face-band law off, in full.
        assert_eq!(param(&p, "faceBandPx"), 0.0);
        assert_eq!(param(&p, "rimDistPx"), 0.0);
        assert_eq!(param(&p, "rimGain"), 0.0);
        // Every uniform the shader names gets exactly one value.
        assert_eq!(p.len(), 36);
        let mut names: Vec<_> = p.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate uniform in the frame set");
    }

    #[test]
    fn the_two_shells_with_a_lit_face_band_declare_it_whole() {
        // The slide rule and the switchboard both light only a band along the
        // moulding; all three of the law's uniforms have to be non-zero
        // together or the shader lights a band of nothing.
        for style in [styles::slide_rule_frame(), styles::switchboard_frame()] {
            assert!(style.face_band_px > 0.0);
            assert!(style.rim_dist_px > 0.0);
            assert!(style.rim_gain > 0.0);
        }
    }

    #[test]
    fn the_chassis_metal_is_drawn_in_the_frames_coordinates() {
        let layout = WindowLayout::new(1024.0, 768.0, 184.0);
        let p = chassis_params(
            &styles::annunciator_chassis(),
            &shells::annunciator(),
            &layout,
        );
        // The bank stands left of the frame's origin, covering 184/840 of
        // its field.
        assert_eq!(p.field_scale, [184.0 / 840.0, 1.0]);
        assert_eq!(p.field_offset, [-(184.0 / 840.0), 0.0]);
        // The same casting the frame reads, so the metal is continuous
        // across the boundary rather than two pieces that happen to meet.
        let frame = frame_params(
            &styles::annunciator_frame(),
            &shells::annunciator(),
            &Config::default(),
            &layout,
        );
        for (i, c) in ["chassisColorR", "chassisColorG", "chassisColorB"]
            .iter()
            .enumerate()
        {
            assert_eq!(p.chassis_color[i], param(&frame, c));
        }
        assert_eq!(p.light_dir[0], param(&frame, "lightDirX"));
        assert_eq!(p.light_dir[1], param(&frame, "lightDirY"));

        // The block the mount writes is the WGSL struct's own field order,
        // with the well's logical size in the slot the shader reads it from.
        let record = p.record([840.0, 768.0]);
        assert_eq!(record[6], 840.0);
        assert_eq!(record[7], 768.0);
        assert_eq!(record[11], 1.0);
        assert_eq!(record[15], p.vignette_strength);
    }
}
