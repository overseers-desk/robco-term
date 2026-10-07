//! Colour resolution: rio-vt hands out `AnsiColor`, the renderer wants four
//! floats.
//!
//! The phosphor decision belongs to the CRT passes downstream, not to the
//! glyph path, so what leaves here is the colour the program asked for.
//! `Scheme::full_color` is what the appliance runs on: the chain's last pass
//! weighs each colour into one brightness and mixes the profile's two
//! colours by it, and a palette flattened before that pass would arrive as
//! one level and light every cell alike. Discarding SGR colours here would
//! also fail half the conformance suite and every `ls --color`.
//!
//! `Scheme::monochrome` is the flat scheme, one colour for glyph and plate
//! alike, which the pixel-comparison tests want and nothing else does.

use rio_vt::config::colors::{AnsiColor, ColorRgb, NamedColor};

/// Straight, non-premultiplied RGBA. Premultiplication happens in the shader,
/// once, so nothing upstream has to remember which convention it is in.
pub type Rgba = [f32; 4];

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

pub const TRANSPARENT: Rgba = [0.0, 0.0, 0.0, 0.0];

/// The colours a grid is drawn with.
#[derive(Clone, Debug)]
pub struct Scheme {
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    /// Indices 0..16 are the ANSI set, 16..232 the 6x6x6 cube, 232..256 the
    /// greys. Filled by `Scheme::xterm_palette` unless a caller overrides it.
    pub palette: [Rgba; 256],
    /// Dim (SGR 2) multiplier: matches xterm's 2/3 convention.
    pub dim_factor: f32,
}

impl Default for Scheme {
    fn default() -> Self {
        // Amber, the "Default Amber" phosphor profile every RMSE comparison
        // in this repo is taken at.
        Self::monochrome(rgb(255, 176, 0), [0.0, 0.0, 0.0, 1.0])
    }
}

impl Scheme {
    /// One colour for every glyph, one for every background. SGR colours
    /// still change *which* of the two a cell gets (through INVERSE), they
    /// just cannot introduce a third. A scheme with nothing to measure but
    /// shape, which is what the pixel comparisons ask for.
    pub fn monochrome(foreground: Rgba, background: Rgba) -> Self {
        Self {
            foreground,
            background,
            cursor: foreground,
            palette: [foreground; 256],
            dim_factor: 2.0 / 3.0,
        }
    }

    /// The ordinary 256-colour terminal palette, and what the appliance runs
    /// on. Each colour reaches the chain as itself and is weighed there, so
    /// a background arrives as a cell lit as far as that colour was bright.
    pub fn full_color(foreground: Rgba, background: Rgba) -> Self {
        Self {
            foreground,
            background,
            cursor: foreground,
            palette: xterm_palette(),
            dim_factor: 2.0 / 3.0,
        }
    }

    pub fn resolve(&self, color: AnsiColor) -> Rgba {
        match color {
            AnsiColor::Named(named) => self.named(named),
            AnsiColor::Spec(ColorRgb { r, g, b }) => rgb(r, g, b),
            AnsiColor::Indexed(i) => self.palette[i as usize],
        }
    }

    pub fn named(&self, named: NamedColor) -> Rgba {
        match named {
            NamedColor::Foreground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            // The dim and bright families are the base sixteen with a
            // multiplier or an offset; `NamedColor as usize` puts the base set
            // at 0..16 and everything else above 256.
            other => {
                let i = other as usize;
                if i < 16 {
                    self.palette[i]
                } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&i)
                {
                    dim(
                        self.palette[i - NamedColor::DimBlack as usize],
                        self.dim_factor,
                    )
                } else if i == NamedColor::DimForeground as usize {
                    dim(self.foreground, self.dim_factor)
                } else if i == NamedColor::LightForeground as usize {
                    self.foreground
                } else {
                    self.foreground
                }
            }
        }
    }
}

/// How far a glyph's colour is carried toward the phosphor before it is
/// drawn, in 0..=1. Zero draws the colour at its own weight, which puts
/// blue at four percent of the beam and off the screen; one draws every
/// glyph at full strength, which is one colour for all text.
///
/// The plate is not lifted. A background stands at the weight its colour
/// earns, which is what lets a dialog's panel read as a panel.
///
/// Blue is what the number is set against, being the darkest entry the
/// table holds: a half lift puts it at half the beam, which reads, while
/// the eight still stand in the order they were written in. Above about
/// two thirds they start to close on one colour.
pub const GLYPH_LIFT: f32 = 0.5;

/// Carry a colour `amount` of the way toward `toward`.
pub fn lift(c: Rgba, toward: Rgba, amount: f32) -> Rgba {
    let mix = |a: f32, b: f32| a + (b - a) * amount;
    [
        mix(c[0], toward[0]),
        mix(c[1], toward[1]),
        mix(c[2], toward[2]),
        c[3],
    ]
}

/// How bright a colour reaches the glass, as the chain's last pass weighs it
/// (`rgb2grey` in `terminal_dynamic.slang`).
pub fn grey(c: Rgba) -> f32 {
    0.21 * c[0] + 0.72 * c[1] + 0.04 * c[2]
}

/// SGR 1's second job. Bold picks a heavier face, and it also moves a base
/// ANSI colour to its bright twin, which is what the second eight were cut
/// for. The twins sit at 8..16 of the same table, so the move is an index
/// away.
pub fn brightened(color: AnsiColor) -> AnsiColor {
    match color {
        AnsiColor::Named(named) if (named as usize) < 8 => AnsiColor::Indexed(named as u8 + 8),
        AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
        other => other,
    }
}

pub fn dim(c: Rgba, factor: f32) -> Rgba {
    [c[0] * factor, c[1] * factor, c[2] * factor, c[3]]
}

/// The xterm 256-colour table, generated rather than transcribed: the cube and
/// the grey ramp are formulas, and a transcription of 240 hex triples is 240
/// chances to fat-finger one.
pub fn xterm_palette() -> [Rgba; 256] {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

    let mut out = [[0.0; 4]; 256];
    for (i, (r, g, b)) in BASE.iter().enumerate() {
        out[i] = rgb(*r, *g, *b);
    }
    for i in 0..216 {
        let (r, g, b) = (CUBE[i / 36], CUBE[(i / 6) % 6], CUBE[i % 6]);
        out[16 + i] = rgb(r, g, b);
    }
    for i in 0..24 {
        let v = 8 + i as u8 * 10;
        out[232 + i] = rgb(v, v, v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_landmarks_match_xterm() {
        let p = xterm_palette();
        assert_eq!(p[1], rgb(205, 0, 0));
        assert_eq!(p[16], rgb(0, 0, 0));
        assert_eq!(p[21], rgb(0, 0, 255));
        assert_eq!(p[231], rgb(255, 255, 255));
        assert_eq!(p[232], rgb(8, 8, 8));
        assert_eq!(p[255], rgb(238, 238, 238));
    }

    /// The scheme the appliance runs keeps every palette entry's own colour,
    /// so a program that paints a background gets a cell lit as far as that
    /// colour was bright and no further. Collapsing the palette to the
    /// foreground instead lights every background fully, which fills the
    /// glass and hides the text drawn on it: `whiptail`'s backdrop is this
    /// blue, and it is meant to arrive almost dark.
    ///
    /// The weighing is `rgb2grey` in the chain's last pass, so this asserts
    /// the colours reach it, not what it makes of them.
    #[test]
    fn the_appliance_scheme_carries_a_background_colour_at_its_own_brightness() {
        let white = rgb(255, 255, 255);
        let scheme = Scheme::full_color(white, [0.0, 0.0, 0.0, 1.0]);
        let blue = scheme.resolve(AnsiColor::Named(NamedColor::Blue));
        assert_eq!(blue, rgb(0, 0, 238));
        assert_ne!(blue, scheme.foreground);
    }

    /// SGR 1 moves a base colour to its bright twin. Blue is where this
    /// pays: (0, 0, 238) weighs four percent of the beam, its twin
    /// (92, 92, 255) weighs ten times that, and a bold-blue prompt path is
    /// the difference between a line and a gap.
    #[test]
    fn bold_moves_a_base_colour_to_its_bright_twin() {
        let scheme = Scheme::full_color(rgb(255, 255, 255), [0.0, 0.0, 0.0, 1.0]);
        let plain = scheme.resolve(AnsiColor::Named(NamedColor::Blue));
        let bold = scheme.resolve(brightened(AnsiColor::Named(NamedColor::Blue)));
        assert_eq!(plain, rgb(0, 0, 238));
        assert_eq!(bold, rgb(92, 92, 255));
        // The eight above the base set are already bright, and a 256-colour
        // index outside the first eight names itself.
        assert_eq!(brightened(AnsiColor::Indexed(9)), AnsiColor::Indexed(9));
        assert_eq!(brightened(AnsiColor::Indexed(200)), AnsiColor::Indexed(200));
    }

    /// The lift carries a glyph colour toward the phosphor without erasing
    /// the differences between colours: the order the eight stand in is the
    /// order they still stand in afterwards.
    #[test]
    fn the_lift_raises_a_glyph_colour_without_flattening_it() {
        let white = rgb(255, 255, 255);
        let lifted = |c: Rgba| lift(c, white, 0.5);
        assert_eq!(lifted(rgb(0, 0, 0)), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(lifted(white), white);
        let blue = lifted(rgb(0, 0, 238));
        let green = lifted(rgb(0, 205, 0));
        assert!(blue[2] > green[2] && green[1] > blue[1]);
    }

    #[test]
    fn monochrome_scheme_cannot_produce_a_third_colour() {
        let fg = rgb(255, 176, 0);
        let scheme = Scheme::monochrome(fg, [0.0, 0.0, 0.0, 1.0]);
        for i in 0..=255u8 {
            assert_eq!(scheme.resolve(AnsiColor::Indexed(i)), fg);
        }
        assert_eq!(scheme.resolve(AnsiColor::Named(NamedColor::Red)), fg);
        // A truecolour SGR is the one escape that can still name its own
        // colour; the phosphor is enforced by the pass chain, not here.
        assert_eq!(
            scheme.resolve(AnsiColor::Spec(ColorRgb { r: 1, g: 2, b: 3 })),
            rgb(1, 2, 3)
        );
    }
}
