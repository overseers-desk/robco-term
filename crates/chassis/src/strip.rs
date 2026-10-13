//! The data seam between the channel model and the bank's furniture: what one
//! row of the bank shows, and what the whole visible page of rows shows.
//!
//! A row's whole contract with the model: the row reads nothing from the
//! model; the bank feeds it and takes the press back. The types here are
//! that feed, written down.
//!
//! # Who owns which half
//!
//! This module is **only** the shape. Nothing here reads a session, a page or a
//! channel: the channel model (`app::channels`, and the pager in `app::bank`)
//! fills these in, and the furniture (the shells' numerals, mouldings and the
//! LED/tape strips) draws from them. Neither side needs the other's code, which
//! is the point of putting the struct here: `chassis` is the crate both already
//! depend on.
//!
//! The measures a row is drawn at are *not* here either; they are
//! [`crate::BankGeometry`], which the same drawing code already has. A
//! [`StripRow`] carries what changes when a channel changes, and nothing that
//! changes when the window is resized.
//!
//! # How it reaches the display kits
//!
//! [`StripRow::open`] and [`StripRow::bright`] are exactly the `powered` and
//! `bright` a display item binds, so they go straight into
//! [`crate::displays::led::window_colors`] and
//! [`crate::displays::led::spill_strength`] / [`crate::displays::tape`]'s
//! `visible_text`. [`StripRow::title`] is the display's `text`.

use crate::bank::{BankGeometry, ChannelIndicator};

/// One slot of the bank as it is drawn: the numeral printed in the plastic
/// beside the window, and what the window is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StripRow {
    /// The absolute slot behind the numeral: `page_base + index + 1`. The
    /// press reaches this channel; the numeral does not name it.
    pub channel: u32,
    /// The numeral as engraved, counted within the bank page: `index + 1`.
    /// The numerals read 1..N on every page, the way a car stereo reuses
    /// its preset keys across FM1/FM2/FM3.
    pub label: u32,
    /// [`StripRow::label`] as the panel printer stamped it: two digits
    /// always.
    pub numeral: String,
    /// What the window reads. Empty on a dark slot.
    pub title: String,
    /// A session stands here. A dark slot's press starts one.
    pub open: bool,
    /// This is the channel on the air, on the page the bank is showing.
    pub current: bool,
    /// The display's `bright`: the current row under the glow law, or
    /// *every* row under the other two, where the mark is the selector's
    /// or the toggle's to carry rather than the lamp's.
    pub bright: bool,
    /// What a drag would leave here, shown before the drop: the window
    /// reads dimmer than any channel standing in the bank.
    pub ghost: bool,
}

impl StripRow {
    pub fn numeral_text(label: u32) -> String {
        if label < 10 {
            format!("0{label}")
        } else {
            label.to_string()
        }
    }
}

/// Which windows run at full current, which is [`StripRow::bright`]'s whole
/// law.
///
/// `bright: current || indicator != Glow`. So under the glow the channel on
/// the air is the only bright window, and under the other two laws every
/// open window is bright and the mark is made elsewhere -- by the selector
/// carriage on the rail, or by the thrown lever. That is the whole of what
/// `chassis.channel_indicator` does to a *strip*; what it does to the bank's
/// geometry is [`BankGeometry`]'s lane.
///
/// It lives here rather than at the one builder that calls it because the
/// furniture used to decide it a second time from its own copy of the
/// indicator; one law, one home.
pub fn bright(current: bool, indicator: ChannelIndicator) -> bool {
    current || indicator != ChannelIndicator::Glow
}

impl BankStrips {
    /// The page the appliance boots showing: `rows` engraved keys, the first
    /// slot open and on the air, the rest dark, and no titles yet.
    ///
    /// Not a placeholder -- it is a state the running appliance really
    /// passes through: a fresh local channel starts with an empty title,
    /// and the strip shows a lit but blank window until the session's own
    /// OSC names it. It exists as a constructor so the furniture and its
    /// mount can be measured with no channel model and no pty at all.
    pub fn cold_start(rows: usize) -> Self {
        BankStrips {
            rows: (0..rows)
                .map(|i| {
                    let label = i as u32 + 1;
                    StripRow {
                        channel: label,
                        label,
                        numeral: StripRow::numeral_text(label),
                        title: String::new(),
                        open: i == 0,
                        current: i == 0,
                        bright: bright(i == 0, ChannelIndicator::Glow),
                        ghost: false,
                    }
                })
                .collect(),
            page_index: 0,
            page_count: 1,
            indicator: ChannelIndicator::Glow,
            current_row: (rows > 0).then_some(0),
            pointer_shown: rows > 0,
        }
    }
}

/// The whole bank page the furniture draws: its rows, where the pager stands,
/// and where the selector rides.
#[derive(Clone, Debug, PartialEq)]
pub struct BankStrips {
    /// Top to bottom, one per engraved key on this page. At most
    /// `rows_visible`, and fewer on the last page, which stops at the
    /// channel cap: no key is engraved for a slot that no channel can ever
    /// take.
    pub rows: Vec<StripRow>,
    /// Which bank page is showing, and how many there are, for the pager's
    /// own readout.
    pub page_index: i32,
    pub page_count: i32,
    /// How the profile marks the channel on screen, which decides whether
    /// the selector is drawn at all.
    pub indicator: ChannelIndicator,
    /// The row the selector stands beside, counted from the top of this page,
    /// or `None` when the channel on the air is not among these rows. Feed it
    /// to [`BankGeometry::pointer_y`].
    pub current_row: Option<i32>,
    /// Whether the carriage is on the rail at all. The carriage stands only
    /// where the air is on the viewed page's machine; parked at the pager it
    /// means "further along these slots", which is a lie when the air is a
    /// whole page away, so there it leaves the rail instead.
    pub pointer_shown: bool,
}

impl BankStrips {
    /// The slot behind a page numeral, or 0 where this page has no such row.
    /// The chord commits through `app::bank::BankPager::absolute_slot`,
    /// which can see the model; this is the drawing side's copy, waiting
    /// for the pager furniture.
    pub fn absolute_slot(&self, page_slot: u32) -> u32 {
        self.rows
            .iter()
            .find(|r| r.label == page_slot)
            .map(|r| r.channel)
            .unwrap_or(0)
    }
}

impl BankGeometry {
    /// Where the selector stands, in the track's own coordinates. Beside
    /// the row of the channel on screen, or down by the pager when that
    /// channel is on a page this one is not showing.
    ///
    /// `pager_centre_y` is the middle of the pager item in the bank's
    /// coordinates, which is the drawing code's own measure and not one this
    /// crate has: the pager is a shell component whose height only the shell
    /// knows.
    pub fn pointer_y(&self, current_row: Option<i32>, pager_centre_y: f64) -> f64 {
        match current_row {
            Some(row) => {
                f64::from(self.top_padding - self.bank_padding)
                    + f64::from(row) * (f64::from(self.row_height) + self.row_spacing)
                    + f64::from(self.row_height) / 2.0
            }
            None => pager_centre_y - f64::from(self.bank_padding),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{shells, LedMetrics};

    fn geometry() -> BankGeometry {
        BankGeometry::new(
            &shells::annunciator(),
            &LedMetrics::default(),
            12,
            ChannelIndicator::Pointer,
        )
    }

    #[test]
    fn a_numeral_is_two_digits_as_the_panel_printer_stamped_it() {
        assert_eq!(StripRow::numeral_text(1), "01");
        assert_eq!(StripRow::numeral_text(9), "09");
        assert_eq!(StripRow::numeral_text(10), "10");
        assert_eq!(StripRow::numeral_text(99), "99");
    }

    #[test]
    fn the_selector_rides_the_row_pitch_and_parks_at_the_pager() {
        let g = geometry();
        // The annunciator: 61px top padding, 3px bank padding, 43px rows,
        // 2px spacing.
        let first = g.pointer_y(Some(0), 700.0);
        assert_eq!(first, (61 - 3) as f64 + 43.0 / 2.0);
        let second = g.pointer_y(Some(1), 700.0);
        assert_eq!(second - first, f64::from(g.row_height) + g.row_spacing);
        // Off the page, it leaves the rail and stands by the pager.
        assert_eq!(g.pointer_y(None, 700.0), 700.0 - 3.0);
    }

    #[test]
    fn a_page_numeral_resolves_to_the_slot_behind_it() {
        let strips = BankStrips {
            rows: (1..=3)
                .map(|label| StripRow {
                    channel: 10 + label,
                    label,
                    numeral: StripRow::numeral_text(label),
                    title: String::new(),
                    open: false,
                    current: false,
                    bright: false,
                    ghost: false,
                })
                .collect(),
            page_index: 1,
            page_count: 4,
            indicator: ChannelIndicator::Glow,
            current_row: None,
            pointer_shown: false,
        };
        assert_eq!(strips.absolute_slot(1), 11);
        assert_eq!(strips.absolute_slot(3), 13);
        assert_eq!(strips.absolute_slot(4), 0, "no key is engraved for it");
        assert_eq!(strips.absolute_slot(0), 0);
    }
}
