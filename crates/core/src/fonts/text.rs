//! One line of antialiased text raster at a pixel size, with absolute
//! letter spacing and synthetic bold.
//!
//! The sibling of [`super::led`], and deliberately not the same function. The
//! LED strip's raster exists to feed a lamp grid, so it thresholds: every byte
//! is 0 or 255, one image pixel per font pixel, whatever the screen's scale
//! factor ([`super::led`]'s module doc has the reasoning). The chassis's
//! *plate* text is the opposite case. It is painted into the panel at device
//! resolution, antialiased, and the grey edges are the picture rather than a
//! defect in it: a struck numeral whose edges are hard reads as a stencil.
//!
//! So the two share [`super::raster::glyph_mask`] (the strike-first rule, one
//! home) and part company at the threshold.
//!
//! # What this text stack covers
//!
//! Only what the chassis furniture's text needs, which is a short list: one
//! line, one face, a pixel size, absolute letter spacing (pixels added to
//! every glyph's advance), a bold flag, and a horizontal alignment inside a
//! given width. There is no shaping, no bidi and no fallback: every string
//! painted through here is ASCII (`"01".."99"`, `"PREV"`, `"NEXT"`) or one
//! geometric arrow, and each is a single run in a single face.
//!
//! Geometry:
//!   * each glyph advances by its own fractional advance **plus the letter
//!     spacing**, added to every glyph including the last, so the spacing is
//!     in the line's natural width too;
//!   * width = round(that sum), floored at 1;
//!   * height = round(ascent) + round(descent), the implicit height a
//!     one-line raster reports;
//!   * baseline at round(ascent), glyphs placed at the truncated running pen.
//!
//! # Synthetic bold
//!
//! A bold request on a family with no bold face (which is every face this is
//! asked for, since the catalogue ships one weight each) is faked:
//! emboldening the *outline* by `units_per_EM / 24` and leaving the advance
//! alone, so a bold string occupies exactly the width the regular one did.
//! [`embolden`] is that, done to the coverage mask instead of the outline: a
//! horizontal max-filter of the same width. The stem thickens and the line
//! does not reflow, which is the behaviour that matters; the difference from
//! emboldening the curve is confined to the glyph's own antialiased edge.
//!
//! # Grey or subpixel
//!
//! Whether the coverage comes back as one value per pixel or three is not
//! this module's decision: it is the host's, read out of fontconfig by
//! [`super::subpixel`], which also owns the filter. Everything about the
//! *line* is the same either way -- the advances, the width an alignment is
//! measured against, the raster's bounds, the baseline -- because an LCD
//! sampling mode changes how a glyph is sampled inside its pixels and nothing
//! about where those pixels are. That is why the layout below runs once and
//! the choice appears only in how wide the sample buffer is.

use swash::scale::ScaleContext;
use swash::FontRef;
use ttf_parser::Face;

use super::metrics::{char_advance_26_6, scaled_metrics_for};
use super::raster;
use super::subpixel::{self, Layout};

/// Where a line sits in the width it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One text-raster request.
#[derive(Clone, Copy, Debug)]
pub struct TextSpec<'a> {
    /// The face's bytes, as [`super::FontEntry::data`] hands them out.
    pub data: &'a [u8],
    pub pixel_size: u32,
    pub text: &'a str,
    /// Absolute letter spacing: pixels added to every glyph's advance.
    pub letter_spacing: f64,
    /// Bold. Synthetic, per the module doc.
    pub bold: bool,
}

/// How much of each pixel a line covers: 0 where the paper is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// One value per pixel, which is what a host with no subpixel geometry
    /// configured renders and what this module produced before there was a
    /// second variant.
    Grey(Vec<u8>),
    /// Three values per pixel, always in red, green, blue order however the
    /// panel's stripes are ordered: [`Layout::Bgr`] is folded to this order by
    /// [`subpixel::fold`], so that everything downstream can read a colour off
    /// it without knowing the panel.
    Stripes(Vec<[u8; 3]>),
}

impl Coverage {
    /// The pixel's coverage as three channels, the grey case answering the
    /// same number three times.
    pub fn channels(&self, index: usize) -> [u8; 3] {
        match self {
            Coverage::Grey(a) => [a[index]; 3],
            Coverage::Stripes(rgb) => rgb[index],
        }
    }

    /// Pixels, not bytes.
    pub fn len(&self) -> usize {
        match self {
            Coverage::Grey(a) => a.len(),
            Coverage::Stripes(rgb) => rgb.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An antialiased coverage raster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRaster {
    pub width: u32,
    pub height: u32,
    /// Distance from the top of the raster to the baseline, in pixels.
    pub baseline: i32,
    pub coverage: Coverage,
}

/// The natural width of a line, in fractional pixels: what an alignment is
/// measured against.
pub fn natural_width(spec: &TextSpec) -> f64 {
    let Ok(face) = Face::parse(spec.data, 0) else {
        return 0.0;
    };
    natural_width_26_6(&face, spec) as f64 / 64.0
}

fn natural_width_26_6(face: &Face, spec: &TextSpec) -> i32 {
    let spacing = (spec.letter_spacing * 64.0).round() as i32;
    spec.text
        .chars()
        .map(|c| char_advance_26_6(face, c, spec.pixel_size) + spacing)
        .sum()
}

/// Where a line's first glyph starts inside `width`, for an alignment.
pub fn align_offset(align: Align, width: f64, natural: f64) -> f64 {
    match align {
        Align::Left => 0.0,
        Align::Center => (width - natural) / 2.0,
        Align::Right => width - natural,
    }
}

/// The raster for one line, sampled for `layout`'s stripes. `None` for empty
/// text or a zero pixel size, the same two refusals
/// [`super::led::led_text_image`] makes.
///
/// `layout` is a parameter and not something read off the machine in here so
/// that this stays a function of what it is given: the host's answer is read
/// once, at the one call site that paints (`chassis::paint`), where the caveat
/// that comes with it can be stated next to the thing it qualifies.
pub fn text_image(spec: &TextSpec, layout: Layout) -> Option<TextRaster> {
    if spec.text.is_empty() || spec.pixel_size == 0 {
        return None;
    }
    let face = Face::parse(spec.data, 0).ok()?;
    let font = FontRef::from_index(spec.data, 0)?;

    let metrics = scaled_metrics_for(&face, spec.pixel_size);
    let baseline = metrics.ascent_int();
    let height = metrics.height_int().max(1) as u32;

    let spacing = (spec.letter_spacing * 64.0).round() as i32;
    let total_26_6 = natural_width_26_6(&face, spec);
    // The bold dilation grows the last glyph past the advance it did not
    // change, so the raster carries the room for it; the *line's* width is
    // still the one alignment is measured against.
    let bulge = if spec.bold {
        embolden_width(spec.pixel_size)
    } else {
        0
    };
    let width = ((((total_26_6 + 32) >> 6).max(1)) as u32) + bulge;

    // One layout, two sampling rates. Under an LCD filter the glyphs are
    // rasterised into a buffer three times as wide -- one column per stripe --
    // and everything below that counts in x counts in thirds of a pixel: the
    // pen lands on whole pixels either way, since a glyph is positioned at
    // the truncated advance and the stripes are inside the pixel it lands on.
    let thirds = if layout.is_subpixel() { 3 } else { 1 };
    let samples_w = width * thirds;
    let mut samples = vec![0u8; (samples_w * height) as usize];

    let mut ctx = ScaleContext::new();
    let mut scaler = ctx
        .builder(font)
        .size(spec.pixel_size as f32)
        .hint(true)
        .build();
    let charmap = font.charmap();

    let mut pen_26_6 = 0i32;
    for c in spec.text.chars() {
        let glyph = if layout.is_subpixel() {
            raster::glyph_mask_thirds(&mut scaler, charmap.map(c))
        } else {
            raster::glyph_mask(&mut scaler, charmap.map(c))
        };
        if let Some(image) = glyph {
            // The fake bold widens the stem by the same fraction of a pixel
            // whatever the sampling rate: three columns of the wide buffer are
            // one column of the raster.
            let (mask, mw, mh) = if spec.bold {
                embolden(
                    &image.data,
                    image.placement.width,
                    image.placement.height,
                    bulge * thirds,
                )
            } else {
                (
                    image.data.clone(),
                    image.placement.width,
                    image.placement.height,
                )
            };
            // A glyph is placed at the truncated running advance; only the
            // line's total width is rounded.
            blit(
                &mut samples,
                samples_w,
                height,
                (pen_26_6 >> 6) * thirds as i32 + image.placement.left,
                baseline - image.placement.top,
                &mask,
                mw,
                mh,
            );
        }
        pen_26_6 += char_advance_26_6(&face, c, spec.pixel_size) + spacing;
    }

    let coverage = if layout.is_subpixel() {
        Coverage::Stripes(subpixel::fold(&samples, width, height, layout))
    } else {
        Coverage::Grey(samples)
    };

    Some(TextRaster {
        width,
        height,
        baseline,
        coverage,
    })
}

/// The fake-bold strength, in whole pixels: `units_per_EM / 24` scaled to
/// the size, which is the pixel size over 24. Floored at one, since a bold
/// that widens nothing is not bold.
fn embolden_width(pixel_size: u32) -> u32 {
    ((pixel_size as f64 / 24.0).round() as u32).max(1)
}

/// A horizontal max-filter `bulge` pixels wide: the coverage-mask stand-in for
/// emboldening the outline.
fn embolden(src: &[u8], w: u32, h: u32, bulge: u32) -> (Vec<u8>, u32, u32) {
    if w == 0 || h == 0 {
        return (src.to_vec(), w, h);
    }
    let out_w = w + bulge;
    let mut out = vec![0u8; (out_w * h) as usize];
    for y in 0..h {
        for x in 0..out_w {
            let mut best = 0u8;
            for k in 0..=bulge {
                let sx = x as i64 - k as i64;
                if sx >= 0 && sx < w as i64 {
                    best = best.max(src[(y * w + sx as u32) as usize]);
                }
            }
            out[(y * out_w + x) as usize] = best;
        }
    }
    (out, out_w, h)
}

/// Coverage, not ink: the greater of what is there and what arrives, so two
/// glyphs whose masks touch do not punch each other out.
#[allow(clippy::too_many_arguments)]
fn blit(dst: &mut [u8], dw: u32, dh: u32, x0: i32, y0: i32, src: &[u8], sw: u32, sh: u32) {
    for sy in 0..sh as i32 {
        let dy = y0 + sy;
        if dy < 0 || dy >= dh as i32 {
            continue;
        }
        for sx in 0..sw as i32 {
            let dx = x0 + sx;
            if dx < 0 || dx >= dw as i32 {
                continue;
            }
            let v = src[(sy as u32 * sw + sx as u32) as usize];
            let slot = &mut dst[(dy as u32 * dw + dx as u32) as usize];
            *slot = (*slot).max(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iosevka() -> &'static [u8] {
        super::super::font_by_name("IOSEVKA", super::super::FontSource::Bundled)
            .expect("Iosevka is bundled")
            .data()
    }

    fn spec<'a>(text: &'a str, letter_spacing: f64, bold: bool) -> TextSpec<'a> {
        TextSpec {
            data: iosevka(),
            pixel_size: 34,
            text,
            letter_spacing,
            bold,
        }
    }

    /// Every channel of every pixel, whichever variant the coverage came back
    /// in.
    fn all_channels(r: &TextRaster) -> Vec<u8> {
        (0..r.coverage.len())
            .flat_map(|i| r.coverage.channels(i))
            .collect()
    }

    #[test]
    fn the_numeral_raster_is_grey_at_its_edges_where_the_lamp_raster_is_not() {
        // The whole reason this module is not `led_text_image`: a struck
        // numeral's antialiased edge is the picture. The LED path thresholds
        // the same glyph to 0/255.
        let aa = text_image(&spec("08", -1.0, false), Layout::None).expect("a raster");
        assert!(
            all_channels(&aa).iter().any(|&a| a > 0 && a < 255),
            "no intermediate coverage: this is a thresholded raster"
        );
        let led = super::super::led::led_text_image(iosevka(), 34, "08").expect("a raster");
        assert!(led.alpha.iter().all(|&a| a == 0 || a == 255));
    }

    #[test]
    fn letter_spacing_lands_in_the_width_alignment_measures_against() {
        // Absolute spacing adds to every glyph's advance, the last one
        // included, so two characters at -1 are two pixels narrower than the
        // same two at 0. Alignment is measured against exactly this number.
        let plain = natural_width(&spec("08", 0.0, false));
        let tight = natural_width(&spec("08", -1.0, false));
        assert!((plain - tight - 2.0).abs() < 1e-9, "{plain} vs {tight}");

        // Right alignment puts the line's right edge on the box's.
        assert!((align_offset(Align::Right, 46.0, tight) + tight - 46.0).abs() < 1e-9);
        assert_eq!(align_offset(Align::Left, 46.0, tight), 0.0);
    }

    #[test]
    fn synthetic_bold_thickens_the_stroke_without_moving_the_line() {
        let plain = text_image(&spec("01", -1.0, false), Layout::None).expect("a raster");
        let bold = text_image(&spec("01", -1.0, true), Layout::None).expect("a raster");
        // The advance is untouched, which is the fake bold's rule: the
        // *line* is the same width, and only the raster carries the bulge.
        assert_eq!(
            natural_width(&spec("01", -1.0, false)),
            natural_width(&spec("01", -1.0, true))
        );
        assert_eq!(bold.width, plain.width + embolden_width(34));
        assert_eq!(bold.height, plain.height);
        // ...and there is strictly more ink.
        let ink = |r: &TextRaster| all_channels(r).iter().map(|&a| a as u64).sum::<u64>();
        assert!(ink(&bold) > ink(&plain));
    }

    #[test]
    fn the_baseline_is_where_the_face_says_and_empty_text_has_no_raster() {
        let r = text_image(&spec("07", -1.0, false), Layout::None).expect("a raster");
        assert!(r.baseline > 0 && (r.baseline as u32) < r.height);
        assert_eq!(r.coverage.len() as u32, r.width * r.height);
        assert!(text_image(&spec("", -1.0, false), Layout::None).is_none());
        assert!(text_image(
            &TextSpec {
                pixel_size: 0,
                ..spec("07", -1.0, false)
            },
            Layout::None
        )
        .is_none());
    }

    /// The off/on comparison, on a real numeral rather than a fixture: the
    /// same line, the same box, and the only difference is what the host's
    /// fontconfig says the panel is.
    ///
    /// What must *not* change is the geometry -- the LCD sampling mode
    /// changes sampling, not layout -- and what must change is that the
    /// numeral now carries colour it did not have, red down the right-hand
    /// side of its strokes under `rgb`, since red is the stripe nearest a
    /// stroke standing to its left.
    #[test]
    fn the_lcd_filter_tints_the_numeral_without_moving_it() {
        let grey = text_image(&spec("08", -1.0, false), Layout::None).expect("a raster");
        let lcd = text_image(&spec("08", -1.0, false), Layout::Rgb).expect("a raster");

        assert_eq!((grey.width, grey.height), (lcd.width, lcd.height));
        assert_eq!(grey.baseline, lcd.baseline);
        assert!(matches!(grey.coverage, Coverage::Grey(_)));
        assert!(matches!(lcd.coverage, Coverage::Stripes(_)));

        // The grey raster has no colour in it at all, by construction, and the
        // filtered one has plenty.
        assert!((0..grey.coverage.len()).all(|i| {
            let [r, g, b] = grey.coverage.channels(i);
            r == g && g == b
        }));
        let tinted = (0..lcd.coverage.len())
            .filter(|&i| {
                let [r, _, b] = lcd.coverage.channels(i);
                r != b
            })
            .count();
        assert!(tinted > 0, "the filter produced no chroma at all");

        // Walk each row; wherever the coverage stops, the pixel past the last
        // inked one is the stroke's right-hand edge, and under `rgb` that edge
        // is red. (Reading a stroke's *inside* would not do: a stem two
        // pixels wide has a left edge too, which goes the other way.)
        let mut right_edges = 0;
        for y in 0..lcd.height as usize {
            for x in 1..lcd.width as usize {
                let i = y * lcd.width as usize + x;
                let [r, g, b] = lcd.coverage.channels(i);
                let left = lcd.coverage.channels(i - 1);
                let grey_here = grey.coverage.channels(i)[0];
                let grey_left = grey.coverage.channels(i - 1)[0];
                // A pixel the grey raster leaves blank, immediately right of
                // one it inks: the ink ended inside the pixel to the left.
                if grey_here == 0 && grey_left > 128 && r + g + b > 0 {
                    assert!(
                        r > b,
                        "the right edge of a stroke is not red at ({x}, {y}): \
                         [{r}, {g}, {b}], the pixel left of it {left:?}"
                    );
                    right_edges += 1;
                }
            }
        }
        assert!(right_edges > 0, "no stroke edge found to check");
    }

    /// The bulge in a bold line is a whole pixel wide however finely the
    /// coverage under it is sampled: a subpixel raster is the same size as the
    /// grey one it replaces, bold or not.
    #[test]
    fn the_subpixel_raster_is_the_same_size_as_the_grey_one() {
        for bold in [false, true] {
            let grey = text_image(&spec("88", -1.0, bold), Layout::None).expect("a raster");
            let lcd = text_image(&spec("88", -1.0, bold), Layout::Rgb).expect("a raster");
            assert_eq!(
                (grey.width, grey.height, grey.baseline),
                (lcd.width, lcd.height, lcd.baseline),
                "bold={bold}"
            );
            assert_eq!(lcd.coverage.len() as u32, lcd.width * lcd.height);
        }
    }
}
