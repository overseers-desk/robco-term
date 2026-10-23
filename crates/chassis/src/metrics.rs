//! The two measure contracts the bank composes: what a shell declares about its
//! fixed furniture, and how a display quantises its own width. A shell's
//! measures are constants, so they are a plain struct; a display's are
//! functions of the live settings, so they are a trait a display kit
//! implements.
//!
//! A `ShellMetrics` value exists whole or not at all -- there is no
//! partial-load state to guard against. Two of its fields are real design
//! contract rather than defensive fallback, though: `top_padding`/
//! `bottom_padding` fall back to `bank_padding` when a shell splits nothing
//! evenly ([`ShellMetrics::new`]), and `track_x` falls back to `bank_padding`
//! for a shell whose chassis carries no rail ([`crate::bank::BankGeometry`]).
//!
//! # One home per measure group
//!
//! Each measure group has one home, and this module holds only the
//! *composed* shape the bank reads:
//!
//! - a shell's metrics are owned by `crate::shells::{annunciator,
//!   slide_rule,switchboard}`'s `metrics` module -- bare constants, read
//!   directly by the drawing recipes beside them. [`shells::annunciator`]
//!   and its two siblings here build a [`ShellMetrics`] by reading those
//!   constants, not by restating them.
//! - a display's metrics are owned by `crate::displays::led::metrics` and
//!   `crate::displays::tape::metrics` -- free functions over the display's
//!   own settings-sourced parameters. [`LedMetrics`] and [`TapeMetrics`]'s
//!   [`DisplayMetrics`] impls call straight through to those functions with
//!   their own fields; the kits' free functions are the arithmetic, these
//!   impls are the swappable-at-runtime shape the bank, the seam drag and
//!   the strip raster all need `dyn DisplayMetrics` for.
//! - a shell's pager measures are a different concern, so they get a
//!   different home: `crate::shells::{annunciator,slide_rule,switchboard}`'s
//!   `pager` module, holding the measures the bank's foot needs.
//!   [`ShellMetrics`]'s pager fields read those, on the same terms as the
//!   `metrics` ones.
//!
//! `tests/suite/metrics_homes.rs` pins each single home's composed shape against
//! its constants/functions as a regression check, so a shell or kit
//! silently regaining a second, drifting copy of its metrics would fail a
//! test rather than a code review.

use crate::js;

/// A colour as 0..1 floats, the range a shader uniform takes. Every one of
/// this type's own call sites (the shells' casting, bezel and ridge
/// colours) is a literal hex value, not a runtime-parsed string, so
/// [`Rgb::from_hex_literal`] divides by 255 rather than 256. See
/// `crate::color`'s module doc for the rule and
/// [`crate::color::str_to_color`] for the other kind of site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    /// Divides by 255: every caller is a hex-literal colour (the shells'
    /// casting, bezel and ridge colours), never a runtime-parsed string.
    /// Delegates to [`crate::color::hex_literal_to_color`] rather than a third
    /// copy of the divisor.
    pub fn from_hex_literal(s: &str) -> Self {
        let c = crate::color::hex_literal_to_color(s);
        Rgb {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

/// A shell's fixed furniture: every measure the bank reads off a shell,
/// plus the casting pair the frame and chassis metals share (the light
/// direction / colour tail each shell's metrics module declares).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellMetrics {
    /// The bank's left shoulder.
    pub bank_padding: i32,
    /// Headroom above row 1.
    pub top_padding: i32,
    /// Plate below the pager.
    pub bottom_padding: i32,
    /// Plate between the strips' right edge and the frame's moulding; zero
    /// where the strips run to the boundary.
    pub right_padding: i32,
    /// Real, not int: a fractional row pitch carries its fraction here or
    /// fourteen rows drift three pixels by the bank's foot.
    pub row_spacing: f64,
    pub column_gap: i32,
    pub numeral_width: i32,
    pub strip_padding: i32,
    pub min_row_height: i32,
    /// The rail's width as the shell declares it; the bank zeroes it under a
    /// non-pointer indicator law.
    pub track_width: i32,
    /// True for a shell that draws the rail as fixed chassis furniture
    /// inside its own padding; the bank then cuts no second lane out of the
    /// rows.
    pub chassis_carries_track: bool,
    /// Where that rail runs. Meaningful only when `chassis_carries_track`;
    /// otherwise the bank uses `bank_padding`.
    pub track_x: i32,
    /// The light the frame and chassis castings share.
    pub casting_light_dir: [f32; 2],
    /// The metal the frame and chassis castings share.
    pub casting_color: Rgb,
    /// The pager's height at the group's natural size: the footprint the
    /// pager takes off the bank's foot before the row-count settle divides
    /// what is left. It is a shell's pager measure rather than its general
    /// metrics, which is why it and the pager fields below read from a
    /// second module per shell.
    pub pager_natural_height: f64,
    /// The content width a shell's pager is drawn at full size for. A pager
    /// narrower than this squeezes proportionally; `0.0` is a shell whose
    /// pager never squeezes, which is the other two.
    pub pager_squeeze_span: f64,
    /// Fixed pixels the pager adds after the squeeze.
    pub pager_extra: i32,
    /// The content width under which a shell's pager stacks its labels over
    /// its keys and stands `pager_stacked_height` tall instead; `0.0` is a
    /// shell whose pager never stacks, which is all but the annunciator.
    pub pager_stack_below: f64,
    pub pager_stacked_height: f64,
}

impl ShellMetrics {
    /// The height the pager takes at the bank's foot, for a bank whose rows
    /// have `content_width` to stand in.
    ///
    /// Two pagers read that width. The slide rule's group is stated at a
    /// natural span and every measure in it scales by `min(1, width / span)`;
    /// the annunciator's stacks its labels over its keys in a lane too narrow
    /// for one line. The switchboard declares a constant and ignores the
    /// argument.
    pub fn pager_height(&self, content_width: i32) -> i32 {
        if (content_width as f64) < self.pager_stack_below {
            return js::round_i32(self.pager_stacked_height) + self.pager_extra;
        }
        let squeeze = if self.pager_squeeze_span > 0.0 && content_width > 0 {
            (content_width as f64 / self.pager_squeeze_span).min(1.0)
        } else {
            1.0
        };
        js::round_i32(self.pager_natural_height * squeeze) + self.pager_extra
    }
}

impl ShellMetrics {
    /// A shell that splits its padding unevenly states both ends; one that
    /// says nothing gets the even air.
    pub fn new(bank_padding: i32) -> Self {
        ShellMetrics {
            bank_padding,
            top_padding: bank_padding,
            bottom_padding: bank_padding,
            right_padding: 0,
            row_spacing: 0.0,
            column_gap: 0,
            numeral_width: 0,
            strip_padding: 0,
            min_row_height: 0,
            track_width: 0,
            chassis_carries_track: false,
            track_x: 0,
            casting_light_dir: [0.0, -1.0],
            casting_color: Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            },
            pager_natural_height: 0.0,
            pager_squeeze_span: 0.0,
            pager_extra: 0,
            pager_stack_below: 0.0,
            pager_stacked_height: 0.0,
        }
    }
}

/// A display kit's width-quantisation contract: a strip grows and shrinks in
/// whole characters, and these are the measures of one.
///
/// The bank composes the height pair one way only: ask the display what band
/// the shell's punched hole holds, then ask it back for the strip's height,
/// so the two functions are never called apart.
pub trait DisplayMetrics {
    /// One character's width on the panel, unrounded: the seam drag divides
    /// by this to find the count nearest the hand.
    fn unit_width(&self) -> f64;

    /// The fewest characters the display will hold; the seam drag's floor.
    fn min_units(&self) -> i32;

    /// A strip's width for `n` visible characters.
    fn width_for_units(&self, n: i32) -> i32;

    /// A strip's height for a band of `pad_cells` unlit rows.
    fn height_for_pad_cells(&self, pad_cells: i32) -> i32;

    /// The band that fills a punched hole of `hole_height` px beyond the glyphs.
    fn pad_cells_for_hole(&self, hole_height: i32) -> i32;
}

/// The LED panel's measures.
///
/// `lamp_cell_width`/`lamp_cell_height` are the profile lamp font's own metrics
/// (`max(1, round(advance_width))` and `max(1, ceil(height))`), so they
/// arrive measured rather than computed here: the geometry stays testable
/// without a font stack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedMetrics {
    pub lamp_cell_width: i32,
    pub lamp_cell_height: i32,
    /// Default 1.5.
    pub dot_pitch: f64,
    /// Default 8.
    pub min_characters: i32,
    /// Default 4.
    pub pad_cells: i32,
    /// Default 1.
    pub side_pad_cells: i32,
}

impl DisplayMetrics for LedMetrics {
    /// Arithmetic owned by [`crate::displays::led::metrics::unit_width`].
    fn unit_width(&self) -> f64 {
        crate::displays::led::metrics::unit_width(self.lamp_cell_width.max(0) as u32, self.dot_pitch)
    }

    fn min_units(&self) -> i32 {
        self.min_characters
    }

    /// Arithmetic owned by [`crate::displays::led::metrics::width_for_units`].
    fn width_for_units(&self, n: i32) -> i32 {
        crate::displays::led::metrics::width_for_units(
            self.lamp_cell_width.max(0) as u32,
            n.max(0) as u32,
            self.side_pad_cells.max(0) as u32,
            self.dot_pitch,
        ) as i32
    }

    /// Arithmetic owned by
    /// [`crate::displays::led::metrics::height_for_pad_cells`].
    fn height_for_pad_cells(&self, pad_cells: i32) -> i32 {
        crate::displays::led::metrics::height_for_pad_cells(
            self.lamp_cell_height.max(0) as u32,
            pad_cells.max(0) as u32,
            self.dot_pitch,
        ) as i32
    }

    /// Arithmetic owned by
    /// [`crate::displays::led::metrics::pad_cells_for_hole`].
    fn pad_cells_for_hole(&self, hole_height: i32) -> i32 {
        crate::displays::led::metrics::pad_cells_for_hole(
            self.lamp_cell_height.max(0) as u32,
            hole_height as f64,
            self.dot_pitch,
            self.pad_cells.max(0) as u32,
        ) as i32
    }
}

impl Default for LedMetrics {
    /// The defaults for everything that is not the lamp font's business,
    /// over a nominal 5x8 lamp cell. The cell pair is a fixture, not a claim
    /// about any bundled face: whoever measures the profile's font
    /// overwrites it.
    fn default() -> Self {
        LedMetrics {
            lamp_cell_width: 5,
            lamp_cell_height: 8,
            dot_pitch: 1.5,
            min_characters: 8,
            pad_cells: 4,
            side_pad_cells: 1,
        }
    }
}

/// The punched-tape display's measures.
///
/// The letters are a fixed size the kit carries with it, unlike the LED
/// strip's, which follows the profile's chosen lamp font: a punch wheel
/// stamps the one size of letter into every tape it ever cuts. `unit_width`
/// is therefore one measured advance of Departure Mono at
/// `letter_pixel_size`, taken by the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapeMetrics {
    /// One advance of Departure Mono at `letter_pixel_size`, measured by
    /// whoever owns the font stack.
    pub unit_width: f64,
    /// The blank the wheel leaves at each end.
    pub end_pad: i32,
    pub min_characters: i32,
}

/// The stamped letters' size.
pub const TAPE_LETTER_PIXEL_SIZE: i32 = 20;
/// The well the kit cuts when the fixture names no height.
pub const TAPE_NATURAL_HEIGHT: i32 = 44;

impl DisplayMetrics for TapeMetrics {
    fn unit_width(&self) -> f64 {
        self.unit_width
    }

    fn min_units(&self) -> i32 {
        self.min_characters
    }

    /// Arithmetic owned by
    /// [`crate::displays::tape::metrics::width_for_units`].
    fn width_for_units(&self, n: i32) -> i32 {
        crate::displays::tape::metrics::width_for_units(
            self.unit_width,
            n.max(0) as u32,
            self.end_pad.max(0) as u32,
        ) as i32
    }

    /// Arithmetic owned by
    /// [`crate::displays::tape::metrics::height_for_pad_cells`]. Tape has
    /// nothing to count, so the hole's own height travels through both
    /// halves of the pair and the tape lies in exactly the well the shell
    /// punched.
    fn height_for_pad_cells(&self, pad_cells: i32) -> i32 {
        crate::displays::tape::metrics::height_for_pad_cells(pad_cells as i64) as i32
    }

    /// Arithmetic owned by
    /// [`crate::displays::tape::metrics::pad_cells_for_hole`].
    fn pad_cells_for_hole(&self, hole_height: i32) -> i32 {
        crate::displays::tape::metrics::pad_cells_for_hole(hole_height as f64) as i32
    }
}

/// The three shells' fixed measures, composed from each shell's own
/// `metrics` module -- and, for the pager fields alone, its `pager` module.
///
/// Only the geometry the bank frame composes lives here; the furniture each
/// shell draws around it (row plates, pagers, selector tracks, switch wells)
/// is a separate concern and stays with the shell work. The pager is the one
/// exception, and only for its height: the bank's foot takes that off before
/// the row-count settle divides what is left, so the composed shape has to
/// carry it. It reads a second home per shell, `shells::<shell>::pager`,
/// beside the `metrics` one -- a different concern gets a different module,
/// not a literal restated here.
pub mod shells {
    use super::{Rgb, ShellMetrics};
    use crate::shells::{annunciator, slide_rule, switchboard};

    /// The amber appliance: a raised plate carrying windows at a 45px
    /// pitch, cut close at the sides. Every field reads
    /// [`crate::shells::annunciator::metrics`]; nothing here restates a
    /// number.
    pub fn annunciator() -> ShellMetrics {
        use annunciator::metrics as m;
        use annunciator::pager as p;
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            track_width: m::TRACK_WIDTH,
            // This shell declares no dedicated rail, so the bank takes the
            // lane off the content ground and `track_x` falls back to
            // `bank_padding`; that fallback is this composer's own
            // business, not the bare metrics module's.
            chassis_carries_track: false,
            track_x: m::BANK_PADDING,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: Rgb::from_hex_literal(m::CASTING_COLOR),
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            pager_stack_below: p::STACK_BELOW,
            pager_stacked_height: p::STACKED_HEIGHT,
        }
    }

    /// The blue appliance: windows punched straight into the chassis at a
    /// 60.8px pitch, with a full-height carrier rail the chassis itself
    /// draws. Reads [`crate::shells::slide_rule::metrics`].
    pub fn slide_rule() -> ShellMetrics {
        use slide_rule::metrics as m;
        use slide_rule::pager as p;
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            track_width: m::TRACK_WIDTH,
            chassis_carries_track: m::CHASSIS_CARRIES_TRACK,
            track_x: m::TRACK_X,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: Rgb::from_hex_literal(m::CASTING_COLOR),
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            // This pager never stacks.
            pager_stack_below: 0.0,
            pager_stacked_height: 0.0,
        }
    }

    /// Fifteen rows of heavy toggle switches in recessed wells. No selector
    /// rail anywhere on the panel: the thrown switch is the whole mark, so
    /// there is no track constant to read. Reads
    /// [`crate::shells::switchboard::metrics`].
    pub fn switchboard() -> ShellMetrics {
        use switchboard::metrics as m;
        use switchboard::pager as p;
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            // No track anywhere on this panel: the thrown switch is the
            // mark. `m::TRACK_WIDTH` does not exist -- the bare module
            // declares no such constant for this shell either.
            track_width: 0,
            chassis_carries_track: false,
            track_x: m::BANK_PADDING,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: Rgb::from_hex_literal(m::CASTING_COLOR),
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            // This pager never stacks.
            pager_stack_below: 0.0,
            pager_stacked_height: 0.0,
        }
    }

    /// The pager rail across the bank's foot, the one shell measure the
    /// row-count settle needs that is not the bank's own. Restates
    /// [`crate::shells::switchboard::metrics::PAGER_HEIGHT`] under the name
    /// this composer's callers use.
    pub const SWITCHBOARD_PAGER_HEIGHT: i32 = switchboard::metrics::PAGER_HEIGHT;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_literal_divides_by_255() {
        // Every caller is a hex-literal colour, parsed at /255, so 0xff
        // reaches exactly 1.0 -- unlike `str_to_color`'s /256
        // (`crate::color`'s module doc states the rule).
        let c = Rgb::from_hex_literal("#ffffff");
        assert_eq!(c.r, 1.0);
        assert_eq!(Rgb::from_hex_literal("#000000").r, 0.0);
        // The switchboard's casting metal, sampled #232830 at (300, 26).
        let m = Rgb::from_hex_literal("#232830");
        assert_eq!(m.r, 0x23 as f32 / 255.0);
        assert_eq!(m.g, 0x28 as f32 / 255.0);
        assert_eq!(m.b, 0x30 as f32 / 255.0);
    }

    #[test]
    fn led_metrics_match_the_defining_formulas() {
        let led = LedMetrics::default();
        // unit_width = lamp_cell_width * dot_pitch = 5 * 1.5.
        assert_eq!(led.unit_width(), 7.5);
        // width_for_units(12) = round(5 * (12 + 2) * 1.5) = round(105) = 105.
        assert_eq!(led.width_for_units(12), 105);
        // A fractional case that pins the rounding: round(5 * 9 * 1.5) = round(67.5) = 68.
        assert_eq!(led.width_for_units(7), 68);
        // height_for_pad_cells(4) = round((8 + 4) * 1.5) = 18.
        assert_eq!(led.height_for_pad_cells(4), 18);
        // pad_cells_for_hole(35) = max(4, round(35 / 1.5) - 8) = max(4, 23 - 8) = 15.
        assert_eq!(led.pad_cells_for_hole(35), 15);
        // ...and the floor bites when the hole is shallow: max(4, round(12/1.5) - 8) = max(4, 0).
        assert_eq!(led.pad_cells_for_hole(12), 4);
    }

    #[test]
    fn tape_metrics_match_the_defining_formulas() {
        // The measurement the kit was sized against: twelve characters of
        // Departure Mono at 20px with 12px of blank tape at either end fill
        // the switchboard's 177px label glass to the pixel.
        let advance = (177.0 - 2.0 * 12.0) / 12.0;
        let tape = TapeMetrics {
            unit_width: advance,
            end_pad: 12,
            min_characters: 8,
        };
        assert_eq!(tape.width_for_units(12), 177);
        // The height pair is a pass-through in both directions.
        assert_eq!(tape.pad_cells_for_hole(44), 44);
        assert_eq!(tape.height_for_pad_cells(44), 44);
        // A negative hole clamps at zero rather than inverting the well.
        assert_eq!(tape.pad_cells_for_hole(-8), 0);
    }
}
