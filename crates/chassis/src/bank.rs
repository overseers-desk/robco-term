//! The channel bank's geometry: the column of furniture set into the chassis,
//! left of the screen well.
//!
//! Only the measures are here. The bank's other half (paging, the chord
//! input, which slot the press lands on) is channel state and belongs with
//! the channel state machines, not with the chassis.
//!
//! The one fact the rest of the window turns on is [`BankGeometry::implicit_width`]:
//! the bank's width follows the LED strip settings, fitted by
//! `chassis::Cabinet::measure` to what the window can spare beside the screen
//! well's floor when the configured strip does not fit -- the fit never
//! widens a bank the user set narrow, and never below the display's own
//! least strip. That fit is screen-fit only; it is never written back to the
//! settings, so a wider window later re-fits from the same configured count
//! rather than from whatever the narrow window last drew. Everything else in
//! the layout is measured from whatever width this settles on.

use crate::metrics::{DisplayMetrics, ShellMetrics};

/// How the profile marks the channel that is on screen.
///
/// Only [`ChannelIndicator::Pointer`] changes the bank's geometry; it is the one
/// law that stands a mechanical selector down the left edge and so takes a lane
/// off the rows. The other two mark the row itself and cost no width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelIndicator {
    /// A mechanical selector rides a rail down the left edge.
    Pointer,
    /// The row furniture's thrown toggle is the whole mark.
    Switch,
    /// The window of the channel on screen runs hotter than the rest.
    #[default]
    Glow,
}

impl ChannelIndicator {
    /// The two named laws are matched exactly and everything else (including
    /// a profile naming a law this build does not know) falls to the glow.
    pub fn from_setting(s: &str) -> Self {
        match s {
            "pointer" => ChannelIndicator::Pointer,
            "switch" => ChannelIndicator::Switch,
            _ => ChannelIndicator::Glow,
        }
    }
}

/// The bank's measures for one (shell, display, character count, indicator law).
///
/// Every field is derived once at construction and never updates itself in
/// place: rebuild the value when any input moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BankGeometry {
    /// What the shell's punched hole holds beyond the text: the window's
    /// outer height less the bezel it keeps at each end.
    pub window_hole_height: i32,
    /// Unlit lamp rows above and below the glyphs, counted together, so the
    /// lamp field runs to the window's lips.
    pub pad_cells_y: i32,
    pub strip_width: i32,
    pub strip_height: i32,
    /// The fewest characters the display will hold; the seam drag's floor and
    /// the fit's.
    pub min_units: i32,
    /// The character count these measures were cut for, which is the count
    /// the strips are drawn at.
    ///
    /// It is the configured `general.led_characters` in a window with room
    /// for it, and the fit's answer in one without
    /// ([`crate::seam::fitted_characters`]). Whoever draws a strip reads it
    /// here rather than from the settings, so the lamps and the width they
    /// were measured for cannot disagree.
    pub characters: i32,
    /// A row is at least the shell's window, and grows if the strip inside
    /// it does.
    pub row_height: i32,
    /// Zero under any law but the pointer's.
    pub track_width: i32,
    pub lane_in_chassis: bool,
    /// Where the selector rail runs, in bank coordinates.
    pub track_x: i32,
    pub track_gap: i32,
    /// The ground left to the rows once the selector has taken its lane.
    pub content_x: i32,
    /// The bank's whole footprint.
    pub implicit_width: i32,
    /// Carried through for the row-count settle.
    pub row_spacing: f64,
    /// Carried through for the row-count settle.
    pub top_padding: i32,
    pub bottom_padding: i32,
    /// Carried through for the selector's own rect.
    pub bank_padding: i32,
}

impl BankGeometry {
    /// Evaluate the bank's measure chain for one set of inputs.
    pub fn new(
        shell: &ShellMetrics,
        display: &dyn DisplayMetrics,
        led_characters: i32,
        indicator: ChannelIndicator,
    ) -> Self {
        // The hole is what the window's outer height leaves inside the bezel.
        let window_hole_height = shell.min_row_height - 2 * shell.strip_padding;
        // The fixture names the hole, the display answers in its own units,
        // and the answer comes straight back as the strip's height.
        let pad_cells_y = display.pad_cells_for_hole(window_hole_height);
        let strip_width = display.width_for_units(led_characters);
        let strip_height = display.height_for_pad_cells(pad_cells_y);
        // A strip taller than the shell's window pushes the row open.
        let row_height = shell
            .min_row_height
            .max(strip_height + 2 * shell.strip_padding);

        // A shell always declares a track width; only the pointer law spends it.
        let pointer = indicator == ChannelIndicator::Pointer;
        let track_width = if pointer { shell.track_width } else { 0 };
        // A shell whose chassis carries the rail has the lane inside its own
        // padding already and names where it runs; the bank cuts no second
        // one out of the rows, and asks for no gap beside it either.
        let lane_in_chassis = shell.chassis_carries_track;
        let track_x = if lane_in_chassis {
            shell.track_x
        } else {
            shell.bank_padding
        };
        let track_gap = if pointer && !lane_in_chassis {
            shell.column_gap
        } else {
            0
        };

        let content_x =
            shell.bank_padding + if lane_in_chassis { 0 } else { track_width } + track_gap;
        // No padding of the bank's own stands before the frame's moulding on
        // the right; the moulding is the strips' right frame.
        let implicit_width =
            content_x + shell.numeral_width + shell.column_gap + strip_width + shell.right_padding;

        BankGeometry {
            window_hole_height,
            pad_cells_y,
            strip_width,
            strip_height,
            min_units: display.min_units(),
            characters: led_characters,
            row_height,
            track_width,
            lane_in_chassis,
            track_x,
            track_gap,
            content_x,
            implicit_width,
            row_spacing: shell.row_spacing,
            top_padding: shell.top_padding,
            bottom_padding: shell.bottom_padding,
            bank_padding: shell.bank_padding,
        }
    }

    /// The rows' ground, which runs to the bank's right edge. `width` is the
    /// bank item's own width: [`Self::implicit_width`] in every standing
    /// layout, since the window gives the bank exactly what it asks.
    pub fn content_width(&self, width: i32) -> i32 {
        width - self.content_x
    }

    /// How many rows a bank of this height shows, given the height the
    /// shell's pager takes at its foot.
    ///
    /// This is measured rather than bound to a live value, because a live
    /// count would reflow the bank on every frame of a window drag; the
    /// caller re-measures 150 ms after the drag stops instead. The debounce
    /// is the caller's business; the arithmetic is here.
    ///
    /// Returns `None` for the pre-load beat, before the metrics exist: a
    /// measurement against a zero row height would divide by nothing and
    /// explode the count.
    pub fn rows_visible(&self, height: i32, pager_height: i32) -> Option<i32> {
        if self.row_height <= 0 {
            return None;
        }
        let rows_height = (height - self.top_padding - self.bottom_padding - pager_height) as f64
            - self.row_spacing;
        let pitch = self.row_height as f64 + self.row_spacing;
        Some(1.max(((rows_height + self.row_spacing) / pitch).floor() as i32))
    }

    /// The character count whose bank width sits nearest `w`, measured from
    /// the current width so no layout arithmetic is repeated.
    ///
    /// The result is deliberately unclamped: clamping here would hide which
    /// of the drag's two bounds bit. [`crate::seam`] holds the clamped form.
    pub fn characters_for_width(&self, w: f64, led_characters: i32, unit_width: f64) -> i32 {
        // A display that has not loaded reports one pixel per character
        // rather than zero, so the divide is always defined.
        let per_char = if unit_width > 0.0 { unit_width } else { 1.0 };
        // Saturating, because `w` is a pointer position and a pointer position
        // can be wild: a drag across a multi-monitor desktop, or a synthetic
        // event, puts a number here that no window ever held. The arithmetic
        // runs in doubles and cannot overflow; the `i32` this returns can,
        // and the caller's clamps would have brought the answer back into
        // range anyway.
        led_characters.saturating_add(crate::js::round_i32(
            (w - self.implicit_width as f64) / per_char,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{shells, LedMetrics, TapeMetrics};

    /// The stock appliance: amber shell, LED strips, twelve characters, glow.
    fn annunciator_led() -> BankGeometry {
        BankGeometry::new(
            &shells::annunciator(),
            &LedMetrics::default(),
            12,
            ChannelIndicator::Glow,
        )
    }

    #[test]
    fn annunciator_led_measures_follow_the_defining_formula_chain() {
        let g = annunciator_led();
        // 43px window less 4px of bezel at each end.
        assert_eq!(g.window_hole_height, 43 - 2 * 4);
        // max(4, round(35/1.5) - 8).
        assert_eq!(g.pad_cells_y, 15);
        // round(5 * (12 + 2) * 1.5).
        assert_eq!(g.strip_width, 105);
        // round((8 + 15) * 1.5) = round(34.5) = 35, which is the punched hole
        // exactly: the lamp field runs to the window's lips.
        assert_eq!(g.strip_height, 35);
        assert_eq!(g.strip_height, g.window_hole_height);
        // max(43, 35 + 8) = 43. The strip fits, so the shell's pitch holds.
        assert_eq!(g.row_height, 43);
        // Glow law, so no lane and no gap; content_x is the padding.
        assert_eq!(g.track_width, 0);
        assert_eq!(g.track_gap, 0);
        assert_eq!(g.content_x, 3);
        // 3 + 46 + 16 + 105 + 14.
        assert_eq!(g.implicit_width, 184);
    }

    #[test]
    fn the_pointer_law_is_the_only_one_that_costs_width() {
        let shell = shells::annunciator();
        let led = LedMetrics::default();
        let glow = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Glow);
        let switch = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Switch);
        let pointer = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Pointer);

        assert_eq!(glow.implicit_width, switch.implicit_width);
        // The annunciator's 14px rail plus a 16px column gap.
        assert_eq!(pointer.implicit_width, glow.implicit_width + 14 + 16);
        assert_eq!(pointer.content_x, 3 + 14 + 16);
        // No chassis rail, so the lane sits at the bank's own padding.
        assert_eq!(pointer.track_x, 3);
    }

    #[test]
    fn a_chassis_carried_rail_costs_the_rows_nothing() {
        // The slide rule draws its rail as fixed chassis furniture inside
        // its own 76px shoulder, so switching to the pointer law must not
        // move the content ground at all.
        let shell = shells::slide_rule();
        let led = LedMetrics::default();
        let glow = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Glow);
        let pointer = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Pointer);

        assert_eq!(glow.implicit_width, pointer.implicit_width);
        assert_eq!(glow.content_x, pointer.content_x);
        assert_eq!(pointer.content_x, 76);
        assert_eq!(pointer.track_gap, 0);
        // The carriage rides where the chassis put it, not at the padding.
        assert_eq!(pointer.track_x, 29);
        assert!(pointer.lane_in_chassis);
        // 76 + 36 + 8 + 105 + 8.
        assert_eq!(pointer.implicit_width, 233);
    }

    #[test]
    fn switchboard_over_tape_matches_the_mock_it_was_measured_from() {
        // Measured against the mock: the switchboard's label glass is x
        // 196..373 of the mock, 177px, and twelve characters of tape fill it
        // to the pixel.
        let tape = TapeMetrics {
            unit_width: (177.0 - 24.0) / 12.0,
            end_pad: 12,
            min_characters: 8,
        };
        let g = BankGeometry::new(&shells::switchboard(), &tape, 12, ChannelIndicator::Switch);
        assert_eq!(g.strip_width, 177);
        // The label well's 48px outer less its 2px bevel ring at each end.
        assert_eq!(g.window_hole_height, 44);
        // The tape height pair is a pass-through, so the strip is the hole
        // and the row stays at the shell's own 48.
        assert_eq!(g.strip_height, 44);
        assert_eq!(g.row_height, 48);
        // 8 + 170 + 18 + 177 + 29. The mock's plate runs x 8..402, and the
        // bank's right edge lands on the screen well's trough at 402.
        assert_eq!(g.implicit_width, 402);
    }

    #[test]
    fn a_strip_taller_than_the_window_pushes_the_row_open() {
        // Coarsen the dot pitch until the lamp field outgrows the shell's
        // punched hole; the strip height is then the binding term, not the
        // shell's minimum row height.
        let led = LedMetrics {
            dot_pitch: 4.0,
            ..LedMetrics::default()
        };
        let g = BankGeometry::new(&shells::annunciator(), &led, 12, ChannelIndicator::Glow);
        // pad_cells_for_hole(35) = max(4, round(35/4) - 8) = max(4, 1) = 4.
        assert_eq!(g.pad_cells_y, 4);
        // height_for_pad_cells(4) = round((8 + 4) * 4) = 48, over the 43px window.
        assert_eq!(g.strip_height, 48);
        assert_eq!(g.row_height, 48 + 2 * 4);
        assert!(g.row_height > 43);
    }

    #[test]
    fn rows_visible_measures_the_pitch_not_the_row() {
        let g = annunciator_led();
        // rows_height = h - 61 - 8 - pager - spacing; the count is
        // floor((rows_height + spacing) / (row_height + spacing)).
        // At h = 768, pager 0: (768 - 61 - 8 - 2 + 2) / 45 = 699/45 = 15.5 -> 15.
        assert_eq!(g.rows_visible(768, 0), Some(15));
        // The pager's own height comes straight off the top of the count.
        assert_eq!(g.rows_visible(768, 104), Some(13));
        // The floor is one row, never zero, however little height is left.
        assert_eq!(g.rows_visible(0, 0), Some(1));
        assert_eq!(g.rows_visible(240, 200), Some(1));
    }

    #[test]
    fn rows_visible_refuses_the_pre_load_beat() {
        // A zero row height is the beat before the metrics exist, and
        // dividing by it would explode the count rather than yield one.
        let shell = ShellMetrics::new(0);
        let led = LedMetrics {
            lamp_cell_height: 0,
            pad_cells: 0,
            dot_pitch: 0.0,
            ..LedMetrics::default()
        };
        let g = BankGeometry::new(&shell, &led, 12, ChannelIndicator::Glow);
        assert_eq!(g.row_height, 0);
        assert_eq!(g.rows_visible(768, 0), None);
    }

    #[test]
    fn characters_for_width_is_measured_from_the_standing_width() {
        let g = annunciator_led();
        let unit = LedMetrics::default().unit_width(); // 7.5 px per character
                                                       // At the standing width the count is unchanged.
        assert_eq!(g.characters_for_width(184.0, 12, unit), 12);
        // One character's worth further right is one character more.
        assert_eq!(g.characters_for_width(184.0 + 7.5, 12, unit), 13);
        // ...and to the left, one fewer.
        assert_eq!(g.characters_for_width(184.0 - 7.5, 12, unit), 11);
        // The half-character step is where JS rounding is visible: -0.5 rounds
        // toward +Infinity, so a hand exactly half a character left of the edge
        // still reads as the standing count (`js::round`).
        assert_eq!(g.characters_for_width(184.0 - 3.75, 12, unit), 12);
        assert_eq!(g.characters_for_width(184.0 + 3.75, 12, unit), 13);
    }

    #[test]
    fn characters_for_width_survives_a_display_that_measures_nothing() {
        // The pre-load display reports one pixel per character rather than
        // zero, so the divide is defined and the count merely useless.
        let g = annunciator_led();
        assert_eq!(g.characters_for_width(200.0, 12, 0.0), 12 + 16);
    }
}
