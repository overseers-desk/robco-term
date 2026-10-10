//! The workspace's colour math, in its one home. The chrome
//! ([`displays::led`](crate::displays::led),
//! [`displays::tape`](crate::displays::tape)) maps state to appearance
//! through [`scale_color`]; the glass chain builds its uniforms through the
//! same functions, re-exported as `crt::color`. The dependency points the
//! way it already did (the chain depends on this crate for its shells and
//! shaders), so "the chain stops at the glass" stays true of the graph.
//!
//! # The divisor rule, stated once
//!
//! A `"#rrggbb"` string reaches this crate's `f32` colours by exactly one of
//! two paths, and the two divide differently:
//!
//! - **A hex colour literal** (a shell's fixed casting/bezel/ridge/plate
//!   colours, and the tape kit's fixed colours, each written straight into
//!   its own module as a string, including one such colour assigned from
//!   another): reads each byte as `byte / 255`. [`hex_literal_to_color`]
//!   holds that divisor; every such site is cited where it calls in.
//! - **A profile's stored colour** (the profile's `font_color`/
//!   `background_color`/`frame_color` and the derivations built from them):
//!   divides each byte by 256, not 255. [`str_to_color`] holds that divisor.
//!
//! The two land a third of a level apart in eight bits, small enough that
//! nothing visible depends on picking the wrong one, which is exactly why a
//! per-site citation matters more than a shared default: nothing except the
//! citation would ever catch the swap. `crate::metrics::Rgb::from_hex_literal`
//! is the same rule again for the one type this module does not itself
//! produce (`chassis::metrics`'s composed shell colours); it delegates to
//! [`hex_literal_to_color`] rather than restating the divisor a third time.

/// A straight (non-premultiplied) RGBA colour in `0.0..=1.0` per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

fn clamp(x: f32, min: f32, max: f32) -> f32 {
    if x <= min {
        return min;
    }
    if x >= max {
        return max;
    }
    x
}

/// RGB scaled by `value` and clamped, alpha passed through its own clamp
/// unscaled -- deliberate, not a typo.
pub fn scale_color(c: Rgba, value: f32) -> Rgba {
    Rgba::new(
        clamp(c.r * value, 0.0, 1.0),
        clamp(c.g * value, 0.0, 1.0),
        clamp(c.b * value, 0.0, 1.0),
        clamp(c.a, 0.0, 1.0),
    )
}

/// `colorA * (1 - alpha) + colorB * alpha`, per channel.
///
/// The third function of the deliberate duplicate this module's doc explains.
/// It is here because the LED strip reads the derived font colour, not the
/// stored hex ([`crate::furniture::font_color`]).
pub fn mix(a: Rgba, b: Rgba, alpha: f32) -> Rgba {
    let t = clamp(alpha, 0.0, 1.0);
    Rgba::new(
        a.r * (1.0 - t) + b.r * t,
        a.g * (1.0 - t) + b.g * t,
        a.b * (1.0 - t) + b.b * t,
        a.a * (1.0 - t) + b.a * t,
    )
}

/// Linear interpolation. Named `lint`, not a typo for "lerp" worth silently
/// renaming away from since other tests already refer to it by this name.
pub fn lint(a: f32, b: f32, t: f32) -> f32 {
    (1.0 - t) * a + t * b
}

/// The GLSL-style scalar smoothstep, not `clamp`'s twin despite the similar
/// shape -- min/max order matters here the way it does in GLSL (reversed
/// order does not simply mirror the curve).
pub fn smoothstep(min: f32, max: f32, value: f32) -> f32 {
    let x = ((value - min) / (max - min)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// `"#rrggbb"` to `Rgba`, alpha forced to 1.0. Divides by 256 (not 255),
/// which is why `#ffffff` does not read back as exactly `1.0`. Only for a
/// profile's stored colour string (see the module doc's divisor rule); a
/// hex colour literal wants [`hex_literal_to_color`] instead.
pub fn str_to_color(s: &str) -> Rgba {
    let r = i32::from_str_radix(substring(s, 1, 3), 16).unwrap_or(0) as f32 / 256.0;
    let g = i32::from_str_radix(substring(s, 3, 5), 16).unwrap_or(0) as f32 / 256.0;
    let b = i32::from_str_radix(substring(s, 5, 7), 16).unwrap_or(0) as f32 / 256.0;
    Rgba::new(r, g, b, 1.0)
}

/// A clamping substring, the shared foundation both hex-colour readers need.
///
/// The three channel reads clamp rather than panic: on `"#000"` they answer
/// `"00"`, `"0"` and `""`, so the parse degrades to `0` and the function
/// still returns a colour. Rust's `&s[1..3]` panics instead, so without this
/// a config carrying a three-digit hex colour took the process down at the
/// first frame it was asked to paint. The `unwrap_or(0)` at each call site
/// was already the answer for a channel that will not parse; this makes the
/// slicing that feeds it total as well.
///
/// `get` rather than direct slicing for the last inch: an index landing
/// inside a multi-byte character is not a range a well-formed hex string can
/// produce, but it is one a config file can.
fn substring(s: &str, from: usize, to: usize) -> &str {
    let to = to.min(s.len());
    let from = from.min(to);
    s.get(from..to).unwrap_or("")
}

/// `"#rrggbb"` to `Rgba`, alpha forced to 1.0. Divides by 255, so `#ffffff`
/// reads back as exactly `1.0` (unlike [`str_to_color`]). For a hex colour
/// literal (see the module doc's divisor rule), not a profile's stored
/// colour string.
///
/// Slices through `substring` for the same reason its neighbour does: an
/// invalid colour should answer a colour rather than panicking, and while
/// most callers here hand over a literal from this repository,
/// [`crate::metrics`] and `shells::common::rgb` pass a string through from a
/// file.
pub fn hex_literal_to_color(s: &str) -> Rgba {
    let r = i32::from_str_radix(substring(s, 1, 3), 16).unwrap_or(0) as f32 / 255.0;
    let g = i32::from_str_radix(substring(s, 3, 5), 16).unwrap_or(0) as f32 / 255.0;
    let b = i32::from_str_radix(substring(s, 5, 7), 16).unwrap_or(0) as f32 / 255.0;
    Rgba::new(r, g, b, 1.0)
}

/// Per-channel addition, clamped.
pub fn sum(a: Rgba, b: Rgba) -> Rgba {
    Rgba::new(
        clamp(a.r + b.r, 0.0, 1.0),
        clamp(a.g + b.g, 0.0, 1.0),
        clamp(a.b + b.b, 0.0, 1.0),
        clamp(a.a + b.a, 0.0, 1.0),
    )
}

/// The appliance's plastic: the profile's frame colour lifted toward the
/// screen's own glow as ambient light rises.
///
/// The bank's plastic is bound to exactly this call, and every piece of
/// painted furniture takes its shade from that one property, so this is the
/// root of the whole column's colour. The glass chain's frame uniform comes
/// from the same function, through `crt::color`.
pub fn frame_base_color(
    frame_color: Rgba,
    font_color: Rgba,
    background_color: Rgba,
    ambient_light: f32,
) -> Rgba {
    mix(
        scale_color(mix(font_color, background_color, 0.2), 0.2),
        sum(frame_color, Rgba::new(0.1, 0.1, 0.1, 1.0)),
        0.125 + 0.750 * ambient_light,
    )
}

/// Lightens by `factor`, scaling the HSV *value* rather than the RGB
/// channels.
///
/// The difference is the whole point: lightening the plastic in RGB would
/// wash its hue out, where scaling the value up leaves hue and saturation
/// where they were, until the value saturates, at which point it starts
/// spending saturation instead, which is what turns a hard-lit moulding lip
/// pale rather than clipping it to the hue's own ceiling. This crate's
/// mouldings lean on both halves: the annunciator's rim asks for `1.9 + 0.2
/// * jitter` off a near-black plastic.
pub fn lighter(c: Rgba, factor: f32) -> Rgba {
    if factor <= 0.0 {
        return c;
    }
    if factor < 1.0 {
        return darker(c, 1.0 / factor);
    }
    let (h, mut s, v) = to_hsv(c);
    let mut v = v * factor;
    if v > 1.0 {
        // The overflow comes off the saturation, in the same units the
        // value overflowed by.
        s = (s - (v - 1.0)).max(0.0);
        v = 1.0;
    }
    from_hsv(h, s, v, c.a)
}

/// Darkens by `factor`: the HSV value divided, hue and saturation untouched.
pub fn darker(c: Rgba, factor: f32) -> Rgba {
    if factor <= 0.0 {
        return c;
    }
    if factor < 1.0 {
        return lighter(c, 1.0 / factor);
    }
    let (h, s, v) = to_hsv(c);
    from_hsv(h, s, v / factor, c.a)
}

/// Hue in `0.0..1.0`, and `-1.0` for the achromatic case.
fn to_hsv(c: Rgba) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (-1.0, 0.0, max);
    }
    let mut hue = if c.r >= max {
        (c.g - c.b) / delta
    } else if c.g >= max {
        2.0 + (c.b - c.r) / delta
    } else {
        4.0 + (c.r - c.g) / delta
    } * 60.0;
    if hue < 0.0 {
        hue += 360.0;
    }
    (hue / 360.0, delta / max, max)
}

/// The inverse of [`to_hsv`]: HSV back to RGB.
fn from_hsv(h: f32, s: f32, v: f32, a: f32) -> Rgba {
    let v = clamp(v, 0.0, 1.0);
    if s <= 0.0 || h < 0.0 {
        return Rgba::new(v, v, v, a);
    }
    let h6 = (h - h.floor()) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Rgba::new(r, g, b, a)
}

/// A colour from explicit channels.
pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba::new(r, g, b, a)
}

/// The same colour at another opacity.
pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    Rgba::new(c.r, c.g, c.b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighter_and_darker_are_an_hsv_pair_not_an_rgb_scale() {
        // The annunciator's plastic, roughly: a dark warm grey.
        let c = hex_literal_to_color("#4a3f34");
        let up = lighter(c, 1.9);
        let (h0, s0, _) = to_hsv(c);
        let (h1, s1, _) = to_hsv(up);
        // Hue and saturation ride through untouched; only the value moves.
        assert!((h0 - h1).abs() < 1e-4, "{h0} vs {h1}");
        assert!((s0 - s1).abs() < 1e-4, "{s0} vs {s1}");
        assert!((to_hsv(up).2 - to_hsv(c).2 * 1.9).abs() < 1e-5);

        // ...and the pair inverts, which is the property every moulding's
        // lit-lip/dark-cut pair is built on.
        let back = darker(up, 1.9);
        assert!((back.r - c.r).abs() < 1e-5 && (back.b - c.b).abs() < 1e-5);

        // A factor below 1 crosses over, matching `lighter`'s own
        // delegation to `darker(1.0 / factor)` for that range.
        assert_eq!(lighter(c, 0.5), darker(c, 2.0));
        assert_eq!(darker(c, 0.5), lighter(c, 2.0));
    }

    #[test]
    fn a_value_that_overflows_is_paid_for_out_of_saturation() {
        // The clamp is not a clamp: past full value this spends saturation,
        // so a hard-lit lip goes pale rather than sticking at the hue's
        // ceiling. An RGB scale would do neither.
        let c = hex_literal_to_color("#804020");
        let hot = lighter(c, 4.0);
        let (_, s, v) = to_hsv(hot);
        assert!((v - 1.0).abs() < 1e-6, "value should saturate: {v}");
        assert!(s < to_hsv(c).1, "saturation should have paid: {s}");
        // Pale, not the pure hue: every channel has lifted off its floor.
        assert!(hot.b > c.b * 2.0);
    }

    #[test]
    fn a_black_plastic_lightens_to_black_the_way_the_panel_gradient_expects() {
        // The punched hole's own near-black, `#0d0700`, lightened by 1.6 and
        // 3.2 for the far wall's catch. Value scaling on a near-black is a
        // small absolute step, which is why the window's bottom lip is a
        // hint rather than a highlight.
        let panel = hex_literal_to_color("#0d0700");
        let lit = lighter(panel, 3.2);
        assert!(lit.r > panel.r && lit.r < 0.2, "{lit:?}");
        // A pure black has no hue to keep and stays black under a value
        // scale.
        let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
        assert_eq!(lighter(black, 4.0), black);
    }

    #[test]
    fn scale_color_matches_js() {
        let c = Rgba::new(0.5, 0.5, 0.5, 0.5);
        let s = scale_color(c, 3.0);
        assert_eq!(s, Rgba::new(1.0, 1.0, 1.0, 0.5));

        let c2 = Rgba::new(0.2, 0.4, 0.6, 1.0);
        let s2 = scale_color(c2, 0.5);
        assert_eq!(s2, Rgba::new(0.1, 0.2, 0.3, 1.0));
    }

    /// The short-string case, which used to abort the process.
    ///
    /// The three channel reads clamp rather than throwing: on `"#000"` they
    /// answer `"00"`, `"0"` and `""`, so the parse degrades to the zero
    /// `unwrap_or(0)` already chose, and above all the function returns.
    #[test]
    fn str_to_color_degrades_on_short_input_the_way_substring_does() {
        // Nothing here may panic; the assertion is that control returns.
        for s in ["", "#", "#0", "#00", "#000", "#0000", "#00000", "#123456"] {
            let c = str_to_color(s);
            assert_eq!(c.a, 1.0, "{s:?} lost its alpha");
        }
        // "#000": substring(1,3) = "00" -> 0, (3,5) = "0" -> 0, (5,7) = "".
        assert_eq!(str_to_color("#000"), Rgba::new(0.0, 0.0, 0.0, 1.0));
        // "#ff11" clamps to two whole channels and an empty third, exactly as
        // substring would hand them over.
        let c = str_to_color("#ff11");
        assert!((c.r - 255.0 / 256.0).abs() < 1e-6);
        assert!((c.g - 17.0 / 256.0).abs() < 1e-6);
        assert_eq!(c.b, 0.0);
        // A multi-byte character is not a boundary a well-formed hex string
        // can produce, but it is one a config file can.
        assert_eq!(str_to_color("#\u{e9}\u{e9}\u{e9}").a, 1.0);
    }

    #[test]
    fn str_to_color_matches_js() {
        // This divides by 256, so "#ff0000" is NOT exactly (1,0,0): 255/256.
        let c = str_to_color("#ff0000");
        assert!((c.r - 255.0 / 256.0).abs() < 1e-6);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 1.0);

        let c2 = str_to_color("#33cc99");
        assert!((c2.r - 0x33 as f32 / 256.0).abs() < 1e-6);
        assert!((c2.g - 0xcc as f32 / 256.0).abs() < 1e-6);
        assert!((c2.b - 0x99 as f32 / 256.0).abs() < 1e-6);
    }

    #[test]
    fn hex_literal_to_color_divides_by_255() {
        // This divides by 255, so "#ffffff" is exactly (1,1,1), unlike
        // `str_to_color`'s 255/256.
        let c = hex_literal_to_color("#ffffff");
        assert_eq!(c.r, 1.0);
        assert_eq!(c.g, 1.0);
        assert_eq!(c.b, 1.0);
        assert_eq!(c.a, 1.0);

        // The switchboard's casting metal, sampled #232830 at (300, 26).
        let m = hex_literal_to_color("#232830");
        assert_eq!(m.r, 0x23 as f32 / 255.0);
        assert_eq!(m.g, 0x28 as f32 / 255.0);
        assert_eq!(m.b, 0x30 as f32 / 255.0);
    }
}
