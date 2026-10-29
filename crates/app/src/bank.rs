//! The bank's state half: which slots the engraved numerals name right now.
//!
//! The measures are `chassis::bank`'s and the strips' shape is
//! `chassis::strip`'s; what is here is the paging, which is channel state and
//! belongs with the channel state machines, exactly as `chassis::bank`'s
//! module doc says.
//!
//! The rows are a fixed-size window onto the slot space, paged: the numerals
//! read 1..N on every page, the way a car stereo reuses its preset keys across
//! FM1/FM2/FM3. The pager walks one flattened space over every bank the model
//! holds, home's slots first and then each attachment's, so the same two keys
//! are the band switch and the preset scroll. Stepping within one bank's own
//! screenfuls views a page without stealing the air: the channel on screen
//! stays put until a switch is pressed. The arithmetic here never moves the
//! air at all -- crossing into another bank's stretch is a band switch, and
//! what that means for the channel on the air is
//! [`crate::window::TerminalSurface::step_bank`]'s.

use chassis::{BankGeometry, BankStrips, ChannelIndicator, StripRow};

use crate::channels::{BankId, Channels, CHANNEL_CAP};

/// Where the pager stands, resolved against the model as it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BankView {
    pub bank: BankId,
    /// Where its stretch of slots begins there.
    pub base: u32,
    pub page_count: i32,
    /// The last page stops at the cap.
    pub rows_on_page: u32,
}

/// The bank's own state: how many rows fit, and which page of the flattened
/// slot space is showing.
#[derive(Clone, Debug)]
pub struct BankPager {
    /// Measured rather than bound: a live count would reflow the bank on
    /// every frame of a window drag.
    rows_visible: i32,
    page_index: i32,
    /// The view as it stood when [`BankPager::refresh`] last looked, so a view
    /// that moved for any reason can be noticed once, in one place.
    last_view: Option<(BankId, u32)>,
}

impl Default for BankPager {
    fn default() -> Self {
        Self::new()
    }
}

impl BankPager {
    pub fn new() -> Self {
        BankPager {
            // Every slot on one page until a drawn bank measures how many
            // of them fit, so a chord typed at a set with no bank on show
            // names the channel it says rather than a key on a page.
            rows_visible: CHANNEL_CAP as i32,
            page_index: 0,
            last_view: None,
        }
    }

    pub fn rows_visible(&self) -> i32 {
        self.rows_visible
    }

    pub fn page_index(&self) -> i32 {
        self.page_index
    }

    /// A reflow that leaves the row count where it was leaves the page there
    /// too, so a hand-picked page survives the window being nudged. Returns
    /// whether the count moved.
    ///
    /// `height` and `pager_height` are the bank item's own and its pager's, in
    /// the logical pixels [`BankGeometry`] is measured in. `None` from
    /// [`BankGeometry::rows_visible`] is the pre-load beat, and settles nothing.
    pub fn settle<S>(
        &mut self,
        geometry: &BankGeometry,
        height: i32,
        pager_height: i32,
        channels: &Channels<S>,
    ) -> bool {
        let Some(measured) = geometry.rows_visible(height, pager_height) else {
            return false;
        };
        if measured == self.rows_visible {
            return false;
        }
        self.rows_visible = measured;
        self.ensure_visible(channels);
        true
    }

    /// Resolves the flattened index against the model as it stands.
    pub fn view<S>(&self, channels: &Channels<S>) -> BankView {
        let table = channels.view_pages(self.rows_visible);
        // At least one page, however empty the model is.
        let page_count = table.iter().map(|v| v.count).sum::<i32>().max(1);
        // The page index only needs clamping here, at resolve time: nothing
        // else reads or reacts to it directly, so a second clamp elsewhere
        // would just repeat this one.
        let mut index = self.page_index.clamp(0, page_count - 1);
        let mut resolved = (0, 0u32, self.rows_visible);
        for view in &table {
            if index < view.count {
                resolved = (view.bank, (index * view.rows).max(0) as u32, view.rows);
                break;
            }
            index -= view.count;
        }
        let (bank, base, rows) = resolved;
        BankView {
            bank,
            base,
            page_count,
            rows_on_page: (rows.max(0) as u32).min(CHANNEL_CAP.saturating_sub(base)),
        }
    }

    /// The band switch and the preset scroll are the same two keys.
    pub fn step<S>(&mut self, direction: i32, channels: &Channels<S>) {
        let count = self.view(channels).page_count;
        self.page_index = (self.page_index + direction).clamp(0, count - 1);
    }

    /// The bank turns to the channel on the air: its bank's stretch of the
    /// flattened space, then the page of its slot.
    pub fn ensure_visible<S>(&mut self, channels: &Channels<S>) {
        let table = channels.view_pages(self.rows_visible);
        let mut flat = 0;
        for view in &table {
            if view.bank == channels.current_bank() {
                let local = (channels.current_channel().saturating_sub(1) as i32)
                    .div_euclid(view.rows);
                self.page_index = flat + local.clamp(0, view.count - 1);
                return;
            }
            flat += view.count;
        }
    }

    /// Whether the stretch of slots on view has moved since this was last
    /// called.
    ///
    /// Whatever re-labels the numerals abandons the digits typed against the
    /// old labels: they are never committed against the new ones. A flip of
    /// the pager is the ordinary way that happens, but not the only one. A
    /// bank collapsing under a detach shortens the flattened space, and the
    /// same index then falls in another bank's stretch of it, crossing a
    /// band with no key pressed at all; what the chord watches is therefore
    /// the stretch on view rather than the index that picked it.
    ///
    /// The bank and its base slot are two derived values, and this treats
    /// them as one question asked once. The surface calls it after anything
    /// that could have moved either, and cancels its chord when it answers
    /// true.
    pub fn refresh<S>(&mut self, channels: &Channels<S>) -> bool {
        let view = self.view(channels);
        let now = (view.bank, view.base);
        let moved = self.last_view.is_some_and(|was| was != now);
        self.last_view = Some(now);
        moved
    }

    /// The rows the furniture draws, and where the selector rides.
    pub fn strips<S>(&self, channels: &Channels<S>, indicator: ChannelIndicator) -> BankStrips {
        let view = self.view(channels);
        let on_view = view.bank == channels.current_bank();
        let rows = (1..=view.rows_on_page)
            .map(|label| {
                let channel = view.base + label;
                let title = channels.slot_title(view.bank, channel);
                let current = on_view && channels.current_channel() == channel;
                StripRow {
                    channel,
                    label,
                    numeral: StripRow::numeral_text(label),
                    open: title.is_some(),
                    title: title.map(String::from).unwrap_or_default(),
                    // The brightness rule has one home: `chassis::strip`.
                    bright: chassis::strip::bright(current, indicator),
                    current,
                    ghost: false,
                    marquee: 0,
                }
            })
            .collect();
        // The selector stands beside the row of the channel on screen, when
        // that row is one of these.
        let current = channels.current_channel();
        let current_row =
            (on_view && current > view.base && current <= view.base + view.rows_on_page)
                .then(|| (current - view.base - 1) as i32);
        BankStrips {
            rows,
            band: channels.bank_identity(view.bank),
            page_index: self.page_index.clamp(0, view.page_count - 1),
            page_count: view.page_count,
            indicator,
            current_row,
            pointer_shown: on_view,
        }
    }

    /// A store targets any slot on the page, dark included; a select waits
    /// only on open ones.
    pub fn slot_prefix_exists<S>(&self, channels: &Channels<S>, buf: &str, store: bool) -> bool {
        let view = self.view(channels);
        if store {
            return (1..=view.rows_on_page).any(|n| {
                let s = n.to_string();
                s.len() > buf.len() && s.starts_with(buf)
            });
        }
        channels.page_slot_prefix_exists(view.bank, buf, view.base, view.rows_on_page)
    }

    /// A page slot as the chord and the numerals read it, to its absolute
    /// slot; 0 where this page has no such row.
    pub fn absolute_slot<S>(&self, channels: &Channels<S>, page_slot: u32) -> u32 {
        let view = self.view(channels);
        if page_slot >= 1 && page_slot <= view.rows_on_page {
            view.base + page_slot
        } else {
            0
        }
    }

    /// The press of a preset: a dark slot starts a session on it, an open one
    /// comes to the screen. The press lands on the bank on view, so this is
    /// where a viewed bank takes the air.
    pub fn press<S>(
        &self,
        channels: &mut Channels<S>,
        channel: u32,
        session: impl FnOnce() -> Option<S>,
    ) {
        let bank = self.view(channels).bank;
        if channels.slot_title(bank, channel).is_none() {
            channels.open_channel(bank, channel, session);
        } else {
            channels.select_channel(bank, channel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Channels<u32> {
        let mut set = Channels::new();
        set.start(|| Some(1));
        set
    }

    fn pager(rows: i32, channels: &Channels<u32>) -> BankPager {
        let mut pager = BankPager::new();
        pager.rows_visible = rows;
        pager.ensure_visible(channels);
        pager
    }

    #[test]
    fn the_numerals_restart_at_one_on_every_page() {
        let mut set = model();
        for slot in 2..=12 {
            set.open_channel(0, slot, || Some(slot));
        }
        let mut bank = pager(5, &set);
        bank.page_index = 0;
        let first = bank.strips(&set, ChannelIndicator::Glow);
        assert_eq!(first.rows[0].label, 1);
        assert_eq!(first.rows[0].channel, 1);
        assert_eq!(first.rows[4].channel, 5);

        bank.step(1, &set);
        let second = bank.strips(&set, ChannelIndicator::Glow);
        assert_eq!(second.rows[0].label, 1, "the keys are reused");
        assert_eq!(second.rows[0].channel, 6, "the slot behind them is not");
        assert_eq!(second.rows[0].numeral, "01");
        // Twelve open slots and the next free one is 13, so three pages of five.
        assert_eq!(second.page_count, 3);
    }

    #[test]
    fn the_last_page_stops_at_the_channel_cap() {
        let mut set = model();
        // A slot high enough that the flattened space runs five pages of
        // twenty; the last of them, based at 80, has 19 keys before the cap.
        set.open_channel(0, 95, || Some(95));
        let mut bank = pager(20, &set);
        bank.page_index = 4;
        let view = bank.view(&set);
        assert_eq!(view.base, 80);
        assert_eq!(view.rows_on_page, 19);
        assert_eq!(bank.strips(&set, ChannelIndicator::Glow).rows.len(), 19);
        assert_eq!(bank.absolute_slot(&set, 19), 99);
        assert_eq!(bank.absolute_slot(&set, 20), 0);
    }

    /// Within one bank's stretch, which is the whole of what this arithmetic
    /// decides: a step that crosses into another bank's is a band switch, and
    /// that is [`crate::window::TerminalSurface::step_bank`]'s to make.
    #[test]
    fn stepping_the_pager_within_a_bank_views_a_page_without_stealing_the_air() {
        let mut set = model();
        set.open_channel(0, 7, || Some(7));
        let mut bank = pager(3, &set);
        assert_eq!(set.banks().len(), 1, "one bank, so the step stays in it");
        let before = (set.current_bank(), set.current_channel());
        bank.step(-1, &set);
        assert_eq!(bank.view(&set).bank, before.0);
        assert_eq!((set.current_bank(), set.current_channel()), before);
        // ...and the marked row leaves with it.
        let strips = bank.strips(&set, ChannelIndicator::Glow);
        assert!(strips.rows.iter().all(|r| !r.current));
        assert_eq!(strips.current_row, None);
    }

    #[test]
    fn the_bank_turns_to_the_channel_on_the_air() {
        let mut set = model();
        set.open_channel(0, 8, || Some(8));
        let bank = pager(3, &set);
        // Slot 8 is on the third page of three rows: (8-1)/3 = 2.
        assert_eq!(bank.page_index(), 2);
        assert_eq!(bank.view(&set).base, 6);
        let strips = bank.strips(&set, ChannelIndicator::Glow);
        assert_eq!(strips.current_row, Some(1));
        assert!(strips.rows[1].current && strips.rows[1].bright);
        assert!(!strips.rows[0].bright, "the glow law marks by light alone");
    }

    #[test]
    fn the_other_two_laws_drive_every_open_window_at_full_a_carrier_included() {
        let mut set = model();
        set.open_channel(0, 2, || Some(2));
        set.attach(0, 1, "prime");
        let bank = pager(4, &set);
        for law in [ChannelIndicator::Pointer, ChannelIndicator::Switch] {
            let strips = bank.strips(&set, law);
            assert!(strips.rows.iter().all(|r| r.bright), "{law:?}");
            assert_eq!(strips.rows[0].title, "tmux -CC # @prime", "{law:?}");
        }
    }

    #[test]
    fn a_press_opens_a_dark_slot_and_selects_an_open_one() {
        let mut set = model();
        let bank = pager(5, &set);
        bank.press(&mut set, 4, || Some(44));
        assert_eq!(set.current_channel(), 4);
        assert_eq!(set.current().unwrap().session, 44);
        bank.press(&mut set, 1, || Some(0));
        assert_eq!(set.current_channel(), 1);
        assert_eq!(set.len(), 2, "an open slot is selected, not reopened");
    }

    #[test]
    fn a_store_chord_may_name_a_dark_slot_and_a_select_chord_may_not() {
        let mut set = model();
        set.open_channel(0, 12, || Some(12));
        let bank = pager(20, &set);
        // A select waits on "1" because slot 12 is open and 12 begins with 1.
        assert!(bank.slot_prefix_exists(&set, "1", false));
        // Nothing beginning with 2 is open, so a select chord commits at once.
        assert!(!bank.slot_prefix_exists(&set, "2", false));
        // A store waits on it anyway: key 20 is engraved on this page, and a
        // store may land on a dark slot.
        assert!(bank.slot_prefix_exists(&set, "2", true));
        assert!(
            !bank.slot_prefix_exists(&set, "20", true),
            "nothing is deeper"
        );
    }

    #[test]
    fn the_stretch_on_view_is_what_a_chord_watches() {
        let mut set = model();
        set.open_channel(0, 6, || Some(6));
        let mut bank = pager(3, &set);
        assert!(
            !bank.refresh(&set),
            "the first look establishes the baseline"
        );
        assert!(!bank.refresh(&set));
        assert_eq!(bank.strips(&set, ChannelIndicator::Glow).band, None);
        assert_eq!(bank.view(&set).rows_on_page, 3);
        bank.step(-1, &set);
        assert!(bank.refresh(&set), "the pager flipped");

        // A bank collapsing shortens the flattened space, and the same index
        // then falls in an attachment's stretch of it, with no key pressed.
        set.select_channel(0, 1);
        let attachment = set.attach(0, 1, "prime").unwrap();
        let window = tmux_cc::WindowId::parse("@1").unwrap();
        let pane = tmux_cc::PaneId::parse("%1").unwrap();
        set.open_tmux_window(attachment, &window, &pane, "vim", None, || Some(1));
        bank.ensure_visible(&set);
        bank.refresh(&set);
        assert_eq!(bank.view(&set).bank, attachment);
        // A managed bank's band costs its page a row; home carries none.
        assert_eq!(bank.strips(&set, ChannelIndicator::Glow).band, set.bank_identity(attachment));
        assert_eq!(bank.view(&set).rows_on_page, 2);
        set.collapse_bank(attachment, None);
        assert!(bank.refresh(&set), "the band changed under the numerals");
    }

    #[test]
    fn a_reflow_that_keeps_the_row_count_keeps_the_hand_picked_page() {
        let mut set = model();
        for slot in 2..=9 {
            set.open_channel(0, slot, || Some(slot));
        }
        let geometry = BankGeometry::new(
            &chassis::metrics::shells::annunciator(),
            &chassis::LedMetrics::default(),
            12,
            ChannelIndicator::Glow,
        );
        let mut bank = BankPager::new();
        // 43px rows on a 2px pitch: 240px of bank holds three of them.
        assert!(bank.settle(&geometry, 240, 0, &set));
        let rows = bank.rows_visible();
        bank.page_index = 1;
        // A nudge too small to change the count leaves the page alone.
        assert!(!bank.settle(&geometry, 244, 0, &set));
        assert_eq!(bank.rows_visible(), rows);
        assert_eq!(bank.page_index(), 1);
        // A drag that does change it turns the bank back to the air.
        assert!(bank.settle(&geometry, 768, 0, &set));
        assert_eq!(bank.page_index(), 0);
    }
}
