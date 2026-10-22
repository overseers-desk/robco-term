//! The CPU-side contracts, without a GPU: the clock, the degauss curve, and the
//! arithmetic between a setting and the uniform it becomes.
//!
//! These are the parts a pixel test cannot pin down. A render test can say the
//! picture got brighter; only this can say it got brighter by
//! `lint(0.5, 1.5, brightness)` and not by some other curve that happens to
//! rise.

use std::time::{Duration, Instant};

use config::{Config, Rasterization};
use crt::degauss::{DURATION, PEAK_BRIGHTNESS, PEAK_SCALE_Y};
use crt::{Degauss, DegaussState, Geometry, Pacing, Params, Structure};

fn params_for(cfg: &Config, geom: &Geometry) -> Params {
    let mut pacing = Pacing::new(Instant::now());
    let t = pacing.tick_by(Duration::from_millis(16));
    Params::build(cfg, geom, t, DegaussState::IDLE)
}

#[test]
fn the_clock_measures_wall_time_and_starts_with_no_delta() {
    let epoch = Instant::now();
    let mut pacing = Pacing::new(epoch);

    let first = pacing.tick(epoch);
    assert_eq!(first.index, 0);
    assert_eq!(first.elapsed, 0.0);
    assert_eq!(
        first.delta, 0.0,
        "the first frame has no previous frame to measure a delta against"
    );

    let second = pacing.tick(epoch + Duration::from_millis(16));
    assert_eq!(second.index, 1);
    assert!((second.elapsed - 0.016).abs() < 1e-6);
    assert!((second.delta - 0.016).abs() < 1e-6);

    // A clock that ran backwards must not hand out a negative delta: the
    // burn-in decay is a product of it, and a negative one would brighten the
    // ghost instead of fading it.
    let backwards = pacing.tick(epoch);
    assert_eq!(backwards.delta, 0.0);
    assert_eq!(backwards.elapsed, 0.0);
}

#[test]
fn the_degauss_curve_is_the_mockups_keyframe() {
    let epoch = Instant::now();
    let mut d = Degauss::new();

    assert_eq!(d.sample(epoch), DegaussState::IDLE);

    d.trigger(epoch);
    let peak = d.sample(epoch);
    assert_eq!(peak.brightness, PEAK_BRIGHTNESS);
    assert_eq!(peak.scale_y, PEAK_SCALE_Y);
    assert_eq!(PEAK_BRIGHTNESS, 2.6);
    assert_eq!(PEAK_SCALE_Y, 0.97);
    assert_eq!(DURATION, Duration::from_millis(200));

    // The `OutQuad` ease-out curve is `1 - (1-t)^2`, so at the halfway point
    // the transient is already three quarters of the way home. A linear ramp would
    // read 0.5 here, which is the mistake this asserts against.
    let half = d.sample(epoch + Duration::from_millis(100));
    let eased = 0.75;
    assert!((half.brightness - (2.6 + (1.0 - 2.6) * eased)).abs() < 1e-5);
    assert!((half.scale_y - (0.97 + (1.0 - 0.97) * eased)).abs() < 1e-5);

    // At the duration it is over, and it stays over.
    assert_eq!(d.sample(epoch + DURATION), DegaussState::IDLE);
    assert!(!d.is_running(epoch + DURATION));
    assert_eq!(
        d.sample(epoch + Duration::from_secs(10)),
        DegaussState::IDLE
    );

    // Triggering during a transient restarts it, as `degauss.restart()` does.
    d.trigger(epoch);
    d.trigger(epoch + Duration::from_millis(150));
    assert_eq!(d.sample(epoch + Duration::from_millis(150)).brightness, 2.6);

    // And it can be cut short.
    d.cancel();
    assert_eq!(
        d.sample(epoch + Duration::from_millis(160)),
        DegaussState::IDLE
    );
}

#[test]
fn shader_uniform_arithmetic_matches_the_defining_formulas() {
    let mut cfg = Config::default();
    let geom = Geometry {
        output_width: 1024.0,
        output_height: 1024.0,
        virtual_width: 128.0,
        virtual_height: 128.0,
        total_font_scaling: 2.0,
        device_pixel_ratio: 1.0,
    };

    cfg.screen.brightness = 0.5;
    cfg.screen.bloom = 0.4;
    cfg.screen.rgb_shift = 1.0;
    cfg.screen.jitter = 0.5;
    cfg.screen.horizontal_sync = 0.5;
    cfg.screen.glowing_line = 0.5;
    cfg.screen.screen_curvature = 0.5;
    cfg.screen.frame_size = 0.1;
    cfg.screen.rasterization = Rasterization::SubpixelRasterization;
    cfg.screen.font_color = "#ffbf00".into();
    cfg.screen.background_color = "#000000".into();

    let p = params_for(&cfg, &geom);
    let get = |name: &str| p.get(name).unwrap_or_else(|| panic!("no uniform {name}"));

    // `lint(0.5, 1.5, 0.5)` = 1.0.
    assert!((get("ScreenBrightness") - 1.0).abs() < 1e-6);
    // `bloom * 2.5`.
    assert!((get("Bloom") - 1.0).abs() < 1e-6);
    // `glowingLine * 0.2`.
    assert!((get("GlowingLine") - 0.1).abs() < 1e-6);
    // `lint(0.05, 0.35, 0.5)` = 0.2.
    assert!((get("HorizontalSyncStrength") - 0.2).abs() < 1e-6);
    // `(0.007 * jitter, 0.002 * jitter)`.
    assert!((get("JitterX") - 0.0035).abs() < 1e-6);
    assert!((get("JitterY") - 0.001).abs() < 1e-6);
    // `rgbShift * (4.0 / width) * totalFontScaling`.
    assert!((get("RgbShift") - (4.0 / 1024.0 * 2.0)).abs() < 1e-9);

    // `screenCurvature * screenCurvatureSize * normalizedScreenScale`, where
    // the scale is `1024 / (0.5*w + 0.5*h)` = 1.0 on a square 1024 window.
    assert!((get("ScreenCurvature") - 0.5 * 0.6).abs() < 1e-6);

    // `frameSize` scales to 5% of its raw value, times the same screen
    // scale. The raw value is the chassis's while a chassis stands, which
    // the shipped default does, so the screen's own 0.1 above is *not* what
    // reaches the shader: the chassis's 0.3 is.
    assert!(cfg.general.chassis_shown);
    assert!((get("FrameSize") - 0.3 * 0.05).abs() < 1e-6);
    // The frame pass takes the same number under its own spelling.
    assert!((get("frameSize") - get("FrameSize")).abs() < 1e-9);
    cfg.general.chassis_shown = false;
    assert!((params_for(&cfg, &geom).get("FrameSize").unwrap() - 0.1 * 0.05).abs() < 1e-6);
    cfg.general.chassis_shown = true;

    // `screenRadius = lint(4, 120, _screenRadius)`, in pixels, and
    // chassis-governed for the same reason.
    assert!((get("screenRadius") - (4.0 + 116.0 * 0.2)).abs() < 1e-4);

    // `frameShininess` is halved from its raw value, over the same split
    // again. The frame body reads it as `frameAlpha = 1.0 - frameShininess *
    // 0.4`, so the halving is worth a byte of alpha, not just a name.
    let shininess = cfg.chassis.frame_shininess;
    assert!((get("FrameShininess") - (shininess * 0.5) as f32).abs() < 1e-6);
    assert!((get("frameShininess") - get("FrameShininess")).abs() < 1e-9);
    cfg.screen.frame_shininess = 0.8;
    cfg.general.chassis_shown = false;
    assert!(
        (params_for(&cfg, &geom).get("FrameShininess").unwrap() - 0.4).abs() < 1e-6,
        "a bare tube takes the screen's own shininess, halved"
    );
    cfg.general.chassis_shown = true;

    // `radius = lint(16, 64, bloomQuality)`.
    assert!((get("radius") - 40.0).abs() < 1e-6);

    // `smoothstep(2.0, 4.0, _screenDensity)` at a density of 8 saturates.
    assert!((get("RasterizationIntensity") - 1.0).abs() < 1e-6);
    assert_eq!(get("RasterMode"), 3.0);

    // What reaches a shader is never the stored hex: the font colour is
    // desaturated towards white by `saturationColor`, and then the two
    // profile colours are mixed into each other by `contrast`. Both are at
    // the shipped defaults here (0.2 and 0.8), and the arithmetic below is
    // hand-evaluated from the defining formula, not this expression written
    // twice:
    //
    //   strToColor("#ffbf00") = (255, 191, 0) / 256 = (0.99609375, 0.74609375, 0)
    //   strToColor("#FFFFFF") = 0.99609375 in every channel
    //   saturatedColor = mix(font, white, 0.2 * 0.5)
    //                  = (0.99609375, 0.77109375, 0.099609375)
    //   0.7 + contrast * 0.3 = 0.94
    //   fontColor       = mix(black, saturated, 0.94) = saturated * 0.94
    //   backgroundColor = mix(saturated, black, 0.94) = saturated * 0.06
    //
    // Assert the whole of both, because the failure this replaces was a set of
    // pins that agreed with a straight `#rrggbb / 255` unpack -- which is the
    // shape a wrong answer takes here.
    // Kept at double precision: two of the three are not f32-exact, and
    // rounding them before the multiply would make the pin a re-derivation of
    // whatever f32 does rather than of the defining formula.
    let saturated = [0.99609375_f64, 0.77109375, 0.099609375];
    for (i, channel) in ["R", "G", "B"].iter().enumerate() {
        let font = get(&format!("FontColor{channel}"));
        let bg = get(&format!("BackgroundColor{channel}"));
        let want_font = (saturated[i] * 0.94) as f32;
        let want_bg = (saturated[i] * 0.06) as f32;
        assert!(
            (font - want_font).abs() < 1e-6,
            "FontColor{channel} is {font}, the formula sends {want_font}"
        );
        assert!(
            (bg - want_bg).abs() < 1e-6,
            "BackgroundColor{channel} is {bg}, the formula sends {want_bg}"
        );
    }
    // The background is emphatically not black at a contrast below 1.0, which
    // is what the old pin said it was.
    assert!(get("BackgroundColorR") > 0.0);

    // The item opacity, at the profile's own `windowOpacity` of 1.0.
    assert!((get("WindowOpacity") - 1.0).abs() < 1e-6);

    // The noise tile scale: `(width * 0.75) / (512 * windowScaling *
    // totalFontScaling)`.
    assert!((get("ScaleNoiseX") - (1024.0 * 0.75) / (512.0 * 1.0 * 2.0)).abs() < 1e-6);
}

#[test]
fn the_rasterization_fade_follows_the_screen_density() {
    let cfg = Config::default();
    let at = |density: f32| {
        let geom = Geometry {
            output_width: 128.0 * density,
            output_height: 128.0 * density,
            virtual_width: 128.0,
            virtual_height: 128.0,
            total_font_scaling: 1.0,
            device_pixel_ratio: 1.0,
        };
        params_for(&cfg, &geom)
            .get("RasterizationIntensity")
            .unwrap()
    };
    // Off below 2x, on above 4x, and rising in between: rasterization is
    // faded out rather than switched, because a hard cutover would flash as
    // the virtual resolution crossed the physical display's.
    assert_eq!(at(1.5), 0.0);
    assert_eq!(at(2.0), 0.0);
    assert!(at(3.0) > 0.4 && at(3.0) < 0.6);
    assert_eq!(at(4.0), 1.0);
    assert_eq!(at(8.0), 1.0);
}

#[test]
fn the_only_burn_in_uniform_left_is_the_composite_switch() {
    // An earlier chain carried three burn-in uniforms while the accumulator
    // was a placeholder: a geometric `BurnInDecay`, and the `BurnInTime` /
    // `BurnInLastUpdate` pair the shader extrapolated a fade from. The real
    // accumulator replaced all three with one number computed per frame from
    // the real frame delta, and none of them may linger: a uniform nothing
    // reads is a rate someone sets later and then wonders why the ghost
    // ignores it.
    for dead in ["BurnInDecay", "BurnInTime", "BurnInLastUpdate"] {
        assert_eq!(
            params_for(&Config::default(), &Geometry::default()).get(dead),
            None,
            "`{dead}` outlived the pass that read it"
        );
    }

    // What is left is whether burn-in is on at all, as a uniform: the
    // dynamic pass composites the ghost only above zero, and the value is
    // the setting itself with no arithmetic on it.
    let mut cfg = Config::default();
    for v in [0.0, 0.25, 1.0] {
        cfg.screen.burn_in = v;
        assert_eq!(
            params_for(&cfg, &Geometry::default()).get("BurnIn"),
            Some(v as f32)
        );
    }

    // The fade time those settings mean is `crt::burn_in`'s to say now, still
    // on the 0.16 s and 1.6 s bounds it always had.
    assert!((crt::burn_in::decay::fade_time_seconds(0.0) - 0.16).abs() < 1e-12);
    assert!((crt::burn_in::decay::fade_time_seconds(1.0) - 1.6).abs() < 1e-12);
}

#[test]
fn only_structural_keys_move_the_structure() {
    let base = Config::default();
    let s = Structure::from_config(&base);

    // Parameter-level keys, one from each group the shaders read.
    let mut param_change = base.clone();
    param_change.screen.brightness = 0.9;
    param_change.screen.burn_in = 0.7;
    param_change.screen.bloom = 0.2;
    param_change.screen.rasterization = Rasterization::PixelRasterization;
    assert_eq!(
        Structure::from_config(&param_change),
        s,
        "a parameter-level change moved the chain structure, which would cost a \
         rebuild and reset the burn-in ghost"
    );

    // And the three that do rebuild it.
    for mutate in [
        (|c: &mut Config| c.general.window_scaling = 2.0) as fn(&mut Config),
        |c: &mut Config| c.general.bloom_quality = 0.25,
        |c: &mut Config| c.general.burn_in_quality = 0.25,
    ] {
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(Structure::from_config(&changed), s);
    }
}

#[test]
fn a_parameter_can_be_overridden_after_it_is_built() {
    let cfg = Config::default();
    let mut p = params_for(&cfg, &Geometry::default());
    p.set("ScreenBrightness", 0.42);
    assert_eq!(p.get("ScreenBrightness"), Some(0.42));
    // A name the settings do not produce is appended rather than dropped, which
    // is what lets a caller push a uniform this crate does not know about.
    p.set("SomethingAPassAdds", 7.0);
    assert_eq!(p.get("SomethingAPassAdds"), Some(7.0));
}

/// The shipped profile's own colours, end to end.
///
/// The Default Amber screen is what the terminal starts wearing, so these
/// three numbers are the recorded golden values for this renderer's default
/// look. They come out of the profile's `font_color` `#ff8100`,
/// `saturation_color` 0.2 and `contrast` 0.8 evaluated by hand from the
/// defining formula -- the *preset's* saturation, not the 0.25 a bare
/// default would carry before a screen profile overrides it.
#[test]
fn the_shipped_profile_colour_matches_the_recorded_endpoints() {
    let cfg = Config::default();
    assert_eq!(cfg.screen.name, "Default Amber");
    assert_eq!(cfg.screen.font_color, "#ff8100");
    assert_eq!(cfg.screen.saturation_color, 0.2);
    assert_eq!(cfg.screen.contrast, 0.8);

    let p = params_for(&cfg, &Geometry::default());

    //   strToColor("#ff8100")  = (255, 129, 0) / 256
    //   saturatedColor         = mix(that, white, 0.1)
    //                          = (0.99609375, 0.553125, 0.099609375)
    //   fontColor              = saturatedColor * (0.7 + 0.8 * 0.3)
    for (name, want) in [
        ("FontColorR", 0.996_093_75 * 0.94),
        ("FontColorG", 0.553_125 * 0.94),
        ("FontColorB", 0.099_609_375 * 0.94),
    ] {
        let got = p.get(name).expect("the uniform is pushed");
        assert!(
            (got - want as f32).abs() < 1e-6,
            "{name} is {got}, the recorded value is {want}"
        );
    }

    // The moulding is tinted from the *mixed* pair too, not the raw stored
    // hex, so a frame colour computed from the raw hex is a second place the
    // same bug hides.
    let expected = crt::color::frame_base_color(
        crt::color::str_to_color(&cfg.chassis.frame_color),
        crt::color::Rgba::new(
            (0.996_093_75 * 0.94) as f32,
            (0.553_125 * 0.94) as f32,
            (0.099_609_375 * 0.94) as f32,
            1.0,
        ),
        crt::color::Rgba::new(
            (0.996_093_75 * 0.06) as f32,
            (0.553_125 * 0.06) as f32,
            (0.099_609_375 * 0.06) as f32,
            1.0,
        ),
        cfg.screen.ambient_light as f32,
    );
    for (name, want) in [
        ("frameColorR", expected.r),
        ("frameColorG", expected.g),
        ("frameColorB", expected.b),
    ] {
        let got = p.get(name).expect("the uniform is pushed");
        assert!((got - want).abs() < 1e-6, "{name} is {got}, want {want}");
    }
}

/// `contrast` and `saturationColor` are settings the rebuild used to drop on
/// the floor: the colours were pushed as raw hex, so both sliders moved
/// nothing at all. A regression here is silent -- the picture still draws --
/// so the test is that moving each one moves a uniform.
#[test]
fn contrast_and_saturation_reach_the_shader() {
    let mut cfg = Config::default();
    let geom = Geometry::default();
    let base = params_for(&cfg, &geom);

    cfg.screen.contrast = 0.2;
    let dull = params_for(&cfg, &geom);
    assert!(
        dull.get("FontColorR").unwrap() < base.get("FontColorR").unwrap(),
        "lowering contrast pulls the font colour towards the background"
    );
    assert!(
        dull.get("BackgroundColorR").unwrap() > base.get("BackgroundColorR").unwrap(),
        "...and pulls the background towards the font colour, by the same weight"
    );

    cfg = Config::default();
    cfg.screen.saturation_color = 1.0;
    let washed = params_for(&cfg, &geom);
    assert!(
        washed.get("FontColorB").unwrap() > base.get("FontColorB").unwrap(),
        "saturationColor mixes the phosphor towards white, which an amber \
         screen shows first in the blue channel it has none of"
    );
}

/// One setting, two places: the item opacity multiplies the finished
/// picture, and the degauss flood is written at the same translucency
/// rather than at full strength.
#[test]
fn window_opacity_reaches_the_output_and_the_flood() {
    let mut cfg = Config::default();
    let geom = Geometry::default();
    assert_eq!(cfg.screen.window_opacity, 1.0);
    assert!((params_for(&cfg, &geom).get("WindowOpacity").unwrap() - 1.0).abs() < 1e-6);

    // The floor is 0.7, not 0: `windowOpacity * 0.3 + 0.7`, so a fully
    // see-through profile still shows most of the picture.
    cfg.screen.window_opacity = 0.0;
    let clear = params_for(&cfg, &geom);
    assert!((clear.get("WindowOpacity").unwrap() - 0.7).abs() < 1e-6);

    // And the flood: `PEAK_BRIGHTNESS` is a multiplier, so what the
    // translucency scales is the part of it above 1.0.
    let flooded = Params::build(
        &cfg,
        &geom,
        Pacing::new(Instant::now()).tick_by(Duration::from_millis(16)),
        DegaussState {
            brightness: PEAK_BRIGHTNESS,
            scale_y: PEAK_SCALE_Y,
        },
    );
    let want = 1.0 + (PEAK_BRIGHTNESS - 1.0) * 0.7;
    assert!((flooded.get("DegaussBrightness").unwrap() - want).abs() < 1e-6);
}

/// The output pass writes the opacity into the alpha as well as the colour.
///
/// The uniform test above pins the value and stops there, which is how the
/// output came to be `vec4(colour * WindowOpacity, 1.0)`: opacity in the
/// colour alone is a dim towards black, not a composite. The window this
/// renders into is transparent, so what a translucent picture actually
/// blends with is whatever sits behind it, and the two only agree where
/// that background is black.
///
/// Premultiplied, so the surface decides what it means: `Opaque` -- every
/// backend in use today -- discards the alpha and gets the picture that was
/// always drawn, and a transparent surface composites. Written the other way
/// round (straight alpha) the same line would need the colour un-multiplied
/// and would change today's picture, which is why this pins the shape and not
/// just the presence of an alpha.
#[test]
fn the_outputs_alpha_carries_the_window_opacity_premultiplied() {
    let pass = crt::preset::PASSES
        .iter()
        .find(|p| p.file == "terminal_dynamic.slang")
        .expect("the chain's output pass");
    // The assignment, however many lines it is written over.
    let mut output = String::new();
    for line in pass
        .source
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("FragColor"))
    {
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(line);
        if line.ends_with(';') {
            break;
        }
    }
    assert!(
        !output.is_empty(),
        "the pass has no FragColor assignment at all"
    );
    assert!(
        !output.ends_with(", 1.0);"),
        "the output still hard-codes an opaque alpha: {output}"
    );
    // Both halves through the same factors, which is what premultiplied means
    // here: the screen mask and the opacity.
    let factors = "inside * global.WindowOpacity";
    assert_eq!(
        output.matches(factors).count(),
        2,
        "colour and alpha must carry the same factors: {output}"
    );
    assert!(
        output.ends_with(&format!("{factors});")),
        "the alpha is not the second of them: {output}"
    );
}

/// The frame-source slot's second occupant takes its whole uniform set.
///
/// With `chassis_shown` -- the shipped default -- the slot carries
/// `frame_metal.slang`, which names 36 parameters. Five are shared by name with
/// `terminal_frame` and were the only five the chain pushed; the other 31 sat
/// at their `#pragma` defaults, which are not the shipped shell's casting.
#[test]
fn the_shells_bezel_is_pushed_whole_when_it_holds_the_slot() {
    let mut cfg = Config::default();
    assert!(cfg.general.chassis_shown);
    let geom = Geometry::default();
    let layout =
        chassis::WindowLayout::new(geom.output_width as f64, geom.output_height as f64, 0.0);
    let whole = chassis::frame::frame_params(
        &chassis::frame_style(&cfg),
        &chassis::shell_metrics(&cfg),
        &cfg,
        &layout,
    );
    assert_eq!(whole.len(), 36);

    let p = params_for(&cfg, &geom);
    for (name, want) in &whole {
        let got = p
            .get(name)
            .unwrap_or_else(|| panic!("the chain never pushes {name}"));
        assert!(
            (got - want).abs() < 1e-5,
            "{name} reaches the shader as {got}, the shell casts it at {want}"
        );
    }

    // The five shared by name keep this crate's own derivation rather than
    // being overwritten by the chassis's, so a uniform two modules both derive
    // has one answer whatever order they run in.
    assert!((p.get("frameSize").unwrap() - p.get("FrameSize").unwrap()).abs() < 1e-9);
    assert!((p.get("frameShininess").unwrap() - p.get("FrameShininess").unwrap()).abs() < 1e-9);

    // Bare, the slot carries `terminal_frame`, which names none of them.
    cfg.general.chassis_shown = false;
    let bare = params_for(&cfg, &geom);
    assert!(bare.get("bezelColorR").is_none());
    assert!(bare.get("wellDepth").is_none());
    assert!(
        bare.get("frameColorR").is_some(),
        "the screen's own moulding still takes its tint"
    );
}

/// The frame's `viewportSize` is the *item* size over `windowScaling` --
/// logical pixels -- and the shader can only get there from `OutputSize`,
/// which is a physical-pixel framebuffer the preset has already scaled by
/// `windowScaling`, if it is told both factors. It is told them as one
/// folded divisor rather than two separate uniforms.
#[test]
fn the_frame_is_told_the_window_scaling_and_the_pixel_ratio_it_was_sized_by() {
    let mut cfg = Config::default();
    let geom = Geometry::default();
    assert!((params_for(&cfg, &geom).get("windowScaling").unwrap() - 1.0).abs() < 1e-6);
    cfg.general.window_scaling = 2.0;
    assert!((params_for(&cfg, &geom).get("windowScaling").unwrap() - 2.0).abs() < 1e-6);

    // The DPR is the other half of the divisor, and it multiplies: a 2x
    // display at 2x window scaling divides by four.
    let hidpi = Geometry {
        device_pixel_ratio: 2.0,
        ..Geometry::default()
    };
    assert!((params_for(&cfg, &hidpi).get("windowScaling").unwrap() - 4.0).abs() < 1e-6);
    cfg.general.window_scaling = 1.0;
    assert!((params_for(&cfg, &hidpi).get("windowScaling").unwrap() - 2.0).abs() < 1e-6);
}

/// The ruler itself, run through the shader's own division: `OutputSize /
/// windowScaling` has to land on the well's *logical* size, because every
/// pixel count the frame pass measures against it (`screenRadius`, the
/// shells' bezel margins, `outerRadius`, `wellDepth`) is stated in logical
/// pixels. Wrong by the DPR and a 2x display gets half the corner radius it
/// should, silently.
#[test]
fn the_frames_ruler_is_the_wells_logical_size_at_any_ratio() {
    // One well, 1024x768 logical, seen at three ratios; the physical render
    // target the chain draws into is that size times the ratio.
    for &dpr in &[1.0f32, 1.5, 2.0] {
        for &scaling in &[0.5f64, 1.0, 2.0] {
            let mut cfg = Config::default();
            cfg.general.window_scaling = scaling;
            let geom = Geometry {
                output_width: 1024.0,
                output_height: 768.0,
                device_pixel_ratio: dpr,
                ..Geometry::default()
            };
            let divisor = params_for(&cfg, &geom).get("windowScaling").unwrap();
            // `scale_type = original`, `scale = windowScaling`: the pass's
            // framebuffer is the physical target scaled by the window scaling.
            let output = 1024.0 * dpr * scaling as f32;
            assert!(
                (output / divisor - 1024.0).abs() < 1e-3,
                "ruler {} at dpr {dpr}, scaling {scaling}",
                output / divisor
            );
        }
    }
}

/// The short colour string, all the way down the path a config file takes.
///
/// `Config::load` does not parse colours: the hex sits in the schema as a
/// `String` and only becomes an `Rgba` here, when a frame's uniforms are
/// built. So a three-digit `background_color` in a config file loaded
/// cleanly and then took the process down on the first frame it was asked
/// to paint -- and again on every live reload that read it back. The parse
/// must clamp rather than throw on a short read, so it degrades to a colour
/// instead.
#[test]
fn a_short_hex_colour_from_a_config_builds_uniforms_instead_of_aborting() {
    for hex in ["#000", "", "#", "#12345", "#0000000"] {
        let mut cfg = Config::default();
        cfg.screen.background_color = hex.to_string();
        cfg.screen.font_color = hex.to_string();
        cfg.chassis.frame_color = hex.to_string();
        // The assertion is that this returns at all; a panic here is the bug.
        let params = params_for(&cfg, &Geometry::default());
        assert!(
            params.iter().count() > 0,
            "{hex:?} built no uniforms at all"
        );
    }

    // And the degraded colour is the zero the parser's `unwrap_or(0)` already
    // chose, not a wrong picture painted confidently.
    assert_eq!(
        crt::color::str_to_color("#000"),
        crt::color::Rgba::new(0.0, 0.0, 0.0, 1.0)
    );
}
