//! Every uniform the chain takes, and the arithmetic between a setting and it.
//!
//! Every structural rebuild's alternative funnels through here: all 56
//! shader variants collapse into values written here, once per frame, at
//! 83 ns each. Nothing in this module touches the GPU; it turns a `Config`, a
//! [`FrameTime`](crate::pacing::FrameTime), a
//! [`DegaussState`](crate::degauss::DegaussState) and the frame's geometry into
//! a flat list of named floats, and [`Chain`](crate::chain::Chain) pushes them.
//!
//! The arithmetic is the part that earns its own module rather than living
//! inline at the shader source: several uniforms are not settings but
//! products of settings (`bloom * 2.5`, `lint(0.5, 1.5, brightness)`, the
//! rasterization fade, the noise tile scale), and each expression sits here
//! at the uniform it feeds.

use config::{Config, Rasterization, ScreenSettings};

use crate::color::{self, lint, smoothstep, Rgba};
use crate::degauss::DegaussState;
use crate::pacing::FrameTime;

/// The fixed curvature scale a screen-curvature setting of 1.0 maps to.
const SCREEN_CURVATURE_SIZE: f32 = 0.6;
/// The noise texture is 512x512.
const NOISE_TEXTURE_SIZE: f32 = 512.0;
/// The type size, in logical pixels, the shaders' displacement constants
/// were tuned at.
pub const REFERENCE_FONT_SIZE: f32 = 32.0;

/// The frame's measurements, which the settings alone do not give.
///
/// **`output_*` is in logical pixels**, not physical ones. That is not a free
/// choice: [`Geometry::normalized_screen_scale`] divides 1024 by the
/// *logical* size of the screen region, so feeding it physical pixels would
/// halve `ScreenCurvature` and `FrameSize` on a 2x display, and the noise
/// tile and RGB shift derive from the same logical size. The one measurement
/// taken in device pixels is the screen density, which is why
/// [`Geometry::device_pixel_ratio`] is carried separately rather than being
/// baked into the size.
///
/// `virtual_*` is neither: it is a count of raster pixels, which is
/// device-independent by construction. `total_font_scaling` is the type size
/// over [`REFERENCE_FONT_SIZE`].
///
/// The app fills this from its window and its font stack; the tests fill it
/// with whatever makes an assertion legible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// The chain's output, and the glass region on screen, in **logical**
    /// pixels.
    pub output_width: f32,
    pub output_height: f32,
    /// The terminal grid's own resolution: columns and rows in raster pixels,
    /// before any integer scale.
    pub virtual_width: f32,
    pub virtual_height: f32,
    pub total_font_scaling: f32,
    /// The one measurement taken in device pixels rather than logical ones:
    /// multiplying the logical size by this ratio gives the physical
    /// resolution the rasterization fade is measured against. 1.0 on an
    /// ordinary display.
    pub device_pixel_ratio: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            output_width: 1024.0,
            output_height: 768.0,
            virtual_width: 640.0,
            virtual_height: 480.0,
            total_font_scaling: 1.0,
            device_pixel_ratio: 1.0,
        }
    }
}

impl Geometry {
    /// Curvature and frame size are given in units of a 1024-pixel screen,
    /// so a bigger window curves by the same visible amount rather than the
    /// same pixel count. Logical pixels, per this type's doc: dividing by a
    /// physical size here would fold the device pixel ratio in as well.
    fn normalized_screen_scale(&self) -> f32 {
        1024.0 / (0.5 * self.output_width + 0.5 * self.output_height).max(1.0)
    }

    /// How many *device* pixels there are per virtual pixel, on the tighter
    /// axis. The device pixel is the point of it -- the mask is faded out as
    /// the virtual resolution approaches the physical display's, where it
    /// would alias -- so this is the one derivation that puts the pixel
    /// ratio back in.
    fn screen_density(&self) -> f32 {
        let ratio = self.device_pixel_ratio.max(f32::EPSILON);
        (self.output_width * ratio / self.virtual_width.max(1.0))
            .min(self.output_height * ratio / self.virtual_height.max(1.0))
    }
}

/// One named uniform.
pub type Param = (&'static str, f32);

/// The whole uniform set for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    values: Vec<Param>,
}

impl Params {
    /// Build the frame's uniforms.
    pub fn build(cfg: &Config, geom: &Geometry, time: FrameTime, degauss: DegaussState) -> Self {
        let s = &cfg.screen;

        let normalized = geom.normalized_screen_scale();
        // The setting is a shape; the size constant and the screen scale
        // turn it into the number the shader distorts by.
        let screen_curvature = s.screen_curvature as f32 * SCREEN_CURVATURE_SIZE * normalized;
        // The frame size setting scales to 5% of its raw value before the
        // screen scale is applied; which setting is "raw" depends on
        // whether the chassis is standing (its own frame) or not (the
        // screen's own moulding) -- see `raw_frame_size` below.
        //
        // The same number appears one crate up as
        // `app::settings::unscaled_frame_size * normalized_screen_scale`,
        // which is what `term::distortion` inverts for the pointer. They are
        // not one function because the crates cannot see each other; they are
        // held to one answer by a test in `crates/app`, because a pointer that
        // un-bends a different warp from the one the shader drew is a defect no
        // pixel test can see.
        let frame_size = cfg.raw_frame_size() as f32 * 0.05 * normalized;

        // Shininess is halved before it reaches the frame's own uniform,
        // over the same chassis-or-screen split `frameSize` takes. The
        // halving is not decorative: the frame body reads it as
        // `frameAlpha = 1.0 - frameShininess * 0.4`, so this is what keeps a
        // raw 0.3 from darkening the moulding more than the shipped look
        // calls for. `chassis::frame` applies the same halving for the
        // bezel's own uniform set.
        let frame_shininess = cfg.raw_frame_shininess() as f32 * 0.5;

        // `bloom * 2.5`, and only when there is a bloom source at all. There
        // always is one here (the pass is in the chain unconditionally), so the
        // setting alone decides.
        let bloom = s.bloom as f32 * 2.5;

        // `lint(0.5, 1.5, brightness)`: the slider's 0..1 is a 0.5x..1.5x
        // multiplier, not a brightness in its own right.
        let screen_brightness = lint(0.5, 1.5, s.brightness as f32);

        // `rgbShift * (4.0 / width) * totalFontScaling`: the setting is a
        // displacement in font-sized units, the uniform is in texture
        // coordinates.
        let rgb_shift =
            s.rgb_shift as f32 * (4.0 / geom.output_width.max(1.0)) * geom.total_font_scaling;

        // Rasterization is faded out as the virtual resolution approaches the
        // physical one, where the mask would alias against the pixel grid.
        // `smoothstep(2.0, 4.0, _screenDensity)`.
        let rasterization_intensity = smoothstep(2.0, 4.0, geom.screen_density());

        // The noise texture is tiled at three quarters of the output size,
        // divided out by the scaling so the grain stays the same size on screen
        // when the window or the font grows.
        let noise_divisor =
            NOISE_TEXTURE_SIZE * cfg.general.window_scaling as f32 * geom.total_font_scaling;
        let scale_noise_x = (geom.output_width * 0.75) / noise_divisor.max(f32::EPSILON);
        let scale_noise_y = (geom.output_height * 0.75) / noise_divisor.max(f32::EPSILON);

        let jitter = s.jitter as f32;
        let horizontal_sync = s.horizontal_sync as f32;
        let glowing_line = s.glowing_line as f32 * 0.2;

        // The setting itself, and nothing derived from it. The fade *rate* is
        // not a uniform of this chain: `crate::burn_in::DecayClock` turns the same
        // setting into the per-frame `BURNIN_DECAY_STEP` the accumulator
        // subtracts, from the real frame delta, and `crt::chain` pushes it. All
        // this value does is tell the dynamic pass whether to composite the
        // ghost at all.
        let burn_in = s.burn_in as f32;

        // Not the stored hex: the two profile colours are mixed into each
        // other before either reaches a shader. See [`screen_colors`].
        let (font_color, background_color) = screen_colors(s);

        // The opacity setting maps to a 0.3x..1.0 range at a 0.7 floor
        // (`windowOpacity * 0.3 + 0.7`) and multiplies every pass's
        // contribution on the way out, so it is applied once, as a single
        // multiplier at the end of the chain's last pass.
        let window_opacity = s.window_opacity as f32 * 0.3 + 0.7;

        // The blur radius is `lint(16, 64, bloomQuality)`, in the pixels of
        // the item being blurred -- which is the terminal downscaled to
        // `bloomQuality`, and here is the bloom passes' own framebuffer. The
        // quality itself is structural (it sizes that framebuffer); the
        // radius derived from it is not, so it rides here with everything
        // else.
        let bloom_radius = lint(16.0, 64.0, cfg.general.bloom_quality as f32);

        // The frame pass's own uniforms, using lower-case names because the
        // pass is fully procedural and its oracle test measures those exact
        // parameter names. Three of them are the same quantities the
        // terminal passes take under their own naming (curvature, frame
        // size, shininess), and they are pushed twice rather than being
        // renamed on one side, because renaming either pass would break its
        // own oracle test's naming contract.
        //
        // `ambientLight` reaches only the frame, and raw. The dynamic pass
        // declares a scaled `ambientLight * 0.2` but leaves it out of that
        // shader's uniform buffer, so it stays dead there: there is nothing
        // to wire up, not a divergence to fix.
        let frame_color = color::frame_base_color(
            // The chassis-or-screen split, through the same /256 parse
            // every colour setting here goes through.
            color::str_to_color(cfg.raw_frame_color()),
            // The frame takes the *mixed* colours -- the same pair the
            // terminal passes take -- not the profile's stored hex.
            font_color,
            background_color,
            s.ambient_light as f32,
        );
        // In pixels, chassis-or-screen governed exactly as the frame size is.
        let screen_radius = {
            let (lo, hi) = config::SCREEN_RADIUS_PX;
            lint(lo as f32, hi as f32, cfg.raw_screen_radius() as f32)
        };

        // Whether the frame is drawn at all. The pass is always in the
        // chain; this is the uniform that zeroes its contribution when
        // there is nothing to draw, rather than rebuilding the chain to
        // drop the pass.
        let frame_enabled =
            if s.ambient_light > 0.0 || cfg.raw_frame_size() > 0.0 || s.screen_curvature > 0.0 {
                1.0
            } else {
                0.0
            };

        let values = vec![
            // Pass 3, static.
            ("ScreenCurvature", screen_curvature),
            ("RgbShift", rgb_shift),
            ("FrameShininess", frame_shininess),
            ("FrameSize", frame_size),
            ("ScreenBrightness", screen_brightness),
            ("Bloom", bloom),
            // Pass 4, dynamic.
            ("Time", time.elapsed),
            ("VirtualResolutionX", geom.virtual_width),
            ("VirtualResolutionY", geom.virtual_height),
            ("RasterizationIntensity", rasterization_intensity),
            ("RasterMode", raster_mode(s.rasterization)),
            // The ghost composite, and the whole of what pass 0 gets from the
            // settings: whether burn-in is on at all, as a uniform rather
            // than a chain variant. The accumulator's own decay
            // is wall-clock and `crt::chain` pushes it per frame.
            ("BurnIn", burn_in),
            ("StaticNoise", s.static_noise as f32),
            ("GlowingLine", glowing_line),
            ("ChromaColor", s.chroma_color as f32),
            ("JitterX", 0.007 * jitter),
            ("JitterY", 0.002 * jitter),
            ("Jitter", jitter),
            ("HorizontalSync", horizontal_sync),
            ("HorizontalSyncStrength", lint(0.05, 0.35, horizontal_sync)),
            ("Flickering", s.flickering as f32),
            ("ScaleNoiseX", scale_noise_x),
            ("ScaleNoiseY", scale_noise_y),
            ("FrameEnabled", frame_enabled),
            ("FontColorR", font_color.r),
            ("FontColorG", font_color.g),
            ("FontColorB", font_color.b),
            ("BackgroundColorR", background_color.r),
            ("BackgroundColorG", background_color.g),
            ("BackgroundColorB", background_color.b),
            ("WindowOpacity", window_opacity),
            // The flood's brightness scales with window opacity rather than
            // picking it up automatically, because the flood is not an
            // overlay composited on top of the picture but a multiplier
            // fixed at `degauss::PEAK_BRIGHTNESS`; what scales here is the
            // amount by which that multiplier exceeds 1.0, at the same
            // translucency the picture itself uses.
            (
                "DegaussBrightness",
                1.0 + (degauss.brightness - 1.0) * window_opacity,
            ),
            ("DegaussScaleY", degauss.scale_y),
            // Passes 1 and 2, the two halves of the bloom.
            ("radius", bloom_radius),
            // Pass 3, the frame. Lower-case: see the note above.
            ("screenCurvature", screen_curvature),
            ("frameSize", frame_size),
            ("frameShininess", frame_shininess),
            ("frameColorR", frame_color.r),
            ("frameColorG", frame_color.g),
            ("frameColorB", frame_color.b),
            ("frameColorA", frame_color.a),
            ("screenRadius", screen_radius),
            ("ambientLight", s.ambient_light as f32),
            // The frame pass's own `viewportSize`, as a divisor rather than
            // a size: the shader divides `OutputSize` by this uniform and
            // must land on a **logical**-pixel item size (this type's own
            // doc on which pixel, and why `device_pixel_ratio` is carried
            // apart from the size).
            //
            // Two factors stand between the two, so the value pushed is the
            // product of both and not `windowScaling` alone:
            //
            // - `windowScaling`, because the preset scales this pass's
            //   framebuffer by it (`preset::Scale::WindowScaling` over a
            //   `scale_type = original`), so `OutputSize` is the chain's input
            //   times the window scaling;
            // - the device pixel ratio, because that input is the render
            //   target, and a render target is physical pixels. `OutputSize /
            //   windowScaling` alone is therefore the *physical* well, which
            //   on a 2x display is twice the ruler every other uniform
            //   measured against `viewportSize` assumes: `screenRadius`, the
            //   bezel margins, `outerRadius`, the well depth.
            //
            // Folded here rather than sent as a second uniform so the
            // shader's one division stays one division. `app::chrome`
            // divides the well it declares to `chassis_metal` by the same
            // DPR, which is what keeps the bezel's grain and the bank
            // casting's one field across the seam at any ratio.
            (
                "windowScaling",
                cfg.general.window_scaling.max(0.01) as f32 * geom.device_pixel_ratio.max(0.01),
            ),
        ];

        let mut params = Self { values };

        // The rest of the shell's casting, when the shell's bezel is the
        // body in the frame-source slot (`crt::preset::CHASSIS_FRAME`,
        // switched on `general.chassis_shown`). `frame_metal.slang` names 36
        // uniforms; five of them are shared by name with `terminal_frame`
        // and are pushed above, and without this the other 31 -- bezel,
        // ridge and chassis colours, the plate's margins, the well, the
        // grain, mottle and scratches -- sit at their `#pragma parameter`
        // defaults, which are not the shipped shell's numbers.
        //
        // `chassis::frame::frame_params` builds the whole set, the five
        // included; only what is not already here is taken from it, so a
        // uniform two modules both derive keeps one derivation rather than
        // whichever ran last. The layout handed to it is the glass region
        // itself with no bank, because `normalized_screen_scale` measures
        // the screen well and `geom` already *is* the well.
        if cfg.general.chassis_shown {
            let layout = chassis::WindowLayout::new(
                geom.output_width as f64,
                geom.output_height as f64,
                0.0,
            );
            let bezel = chassis::frame_style(cfg);
            let shell = chassis::shell_metrics(cfg);
            for (name, value) in chassis::frame::frame_params(&bezel, &shell, cfg, &layout) {
                if params.get(name).is_none() {
                    params.set(name, value);
                }
            }
        }

        params
    }

    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.values.iter()
    }

    /// The `screen.burn_in` setting, on the scale `crate::burn_in` takes it.
    ///
    /// Not a uniform lookup for its own sake: [`Chain`](crate::chain::Chain)
    /// hands this to `BurnInPass::set_burn_in` on every parameter push. The
    /// rate changes and the ghost already on screen keeps fading from where
    /// it is, at the new rate, with no chain rebuild.
    pub fn burn_in(&self) -> f64 {
        self.get("BurnIn").unwrap_or(0.0) as f64
    }

    /// One uniform's value, for tests and for logging a frame that looked wrong.
    pub fn get(&self, name: &str) -> Option<f32> {
        self.values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    }

    /// Overwrite one uniform. The escape hatch for a caller that has a value
    /// the settings alone cannot express.
    pub fn set(&mut self, name: &'static str, value: f32) {
        match self.values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => self.values.push((name, value)),
        }
    }
}

/// The rasterization mode constants, in their declared order.
fn raster_mode(r: Rasterization) -> f32 {
    match r {
        Rasterization::NoRasterization => 0.0,
        Rasterization::ScanlineRasterization => 1.0,
        Rasterization::PixelRasterization => 2.0,
        Rasterization::SubpixelRasterization => 3.0,
        Rasterization::ModernRasterization => 4.0,
    }
}

/// The two colours every shader in the chain is given, from the two the
/// profile stores:
///
/// ```text
/// saturated  = mix(fontColor, white,      saturationColor * 0.5)
/// font       = mix(backgroundColor, saturated, 0.7 + contrast * 0.3)
/// background = mix(saturated, backgroundColor, 0.7 + contrast * 0.3)
/// ```
///
/// `fontColor`/`backgroundColor` on the right are the stored strings
/// (`screen.font_color`, `screen.background_color` here); `font`/`background`
/// on the left are what every pass in the chain is actually given, and what
/// tints the moulding. Pushing the stored hex instead is not a rounding
/// error: it leaves `contrast` and `saturationColor` moving nothing at all,
/// and it sends the font colour a fifth too bright at the shipped contrast
/// of 0.8.
///
/// Two notes on the arithmetic:
///
/// * The divisor is 256, not 255. That is the parser every one of the three
///   lines above goes through ([`color::str_to_color`] ports it and says
///   so). It is *not* a genuine /255 conversion; the shells' bezel and
///   casting colours are that kind of parse and belong on a /255 path.
///   Nothing in this module takes one, so there is one parser here.
/// * This stays in float throughout the two mixes, rather than round
///   -tripping the intermediate `saturated` colour through an 8-bit-per
///   -channel hex string between them, which would quantise it. The shipped
///   calibration figures were computed against the float path.
fn screen_colors(s: &ScreenSettings) -> (Rgba, Rgba) {
    let saturated = color::mix(
        color::str_to_color(&s.font_color),
        color::str_to_color("#FFFFFF"),
        s.saturation_color as f32 * 0.5,
    );
    let background = color::str_to_color(&s.background_color);
    let t = 0.7 + s.contrast as f32 * 0.3;
    (
        color::mix(background, saturated, t),
        color::mix(saturated, background, t),
    )
}
