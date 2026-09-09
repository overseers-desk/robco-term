//! The channel commands and the bank in front of them: opening, closing,
//! cycling and moving a channel, the digit chord that names one, and
//! everything that follows the pair on the air moving.
//!
//! The paging arithmetic is [`crate::bank::BankPager`]'s and the chord's own
//! state machine is [`crate::chord`]'s; what is here is what those two mean
//! for this window's channels.
//!
//! Fields touched: `channels`, the model every command moves; `pager`, which
//! resolves the numerals against it; `chord`, the digits typed against those
//! numerals, and `chord_modifier`, whose release commits them; `on_air`, the
//! pair the glass was last showing, so a switch's work runs on a switch;
//! `degauss`, the flinch a switch triggers; `selection`, which a switch
//! clears; `scroll`, which re-reads the channel arriving; `find`, which
//! comes down with the switch; and `cabinet`, which the pager keys ask
//! about because a page nobody can see is not worth turning.

use std::time::Instant;

use crate::channels::{Close, Manager};
use crate::chord::Chord;

use super::{spawn, TerminalSurface};

impl TerminalSurface {
    /// `Ctrl+Shift+T`. A new channel is another of whatever the bank on view
    /// is: on an SSH bank another channel of that connection, on a tmux
    /// attachment another window of that session, which is its gateway's to
    /// give, and on a bank this program manages itself whatever a fresh
    /// window would open on -- the `[ssh]` table's default connection where
    /// there is one, dialled on a bank of its own, and otherwise a shell in
    /// the lowest free slot. The default is what "where sessions start"
    /// means, and it means the same thing for the second session as for the
    /// first.
    pub fn new_channel(&mut self) {
        // On an SSH bank, another of what you are looking at is another
        // channel of that connection, resolved locally: channel numbering
        // is the client's, so unlike a tmux window there is no round trip
        // to wait on.
        let view = self.pager.view(&self.channels).bank;
        if self.channels.manager_of(view).is_some_and(Manager::is_ssh) {
            self.open_ssh_channel(view);
            self.channel_changed();
            return;
        }
        // A tmux attachment answers for its own bank, so the default has no
        // say there: the channel asked for below is a window of that
        // session. Only where this program is the manager is the question
        // "what does a session start as" the config's to answer -- and a
        // default naming a vanished row, or none at all, is a local shell.
        if !self.is_tmux(view) {
            if let Some(req) = crate::ssh::default_request(&self.live_config()) {
                self.connect_ssh(&req);
                self.channel_changed();
                return;
            }
        }
        let (config, size) = (self.session_now(), self.viewport.term_size());
        if let Some(bank) = self.channels.new_channel(view, || spawn(&config, size)) {
            // The model set the bank's `new_window_pending` flag; the window
            // tmux answers with will take the air when it lands
            // (`open_tmux_window`).
            match self.gateway_mut(bank) {
                Some(gateway) => gateway.new_window(),
                None => log::warn!("bank {bank} asked for a window with no gateway standing"),
            }
        }
        self.channel_changed();
    }

    /// `Ctrl+Shift+W`.
    pub fn close_channel(&mut self) {
        let (bank, channel) = self.channels.on_air();
        match self.channels.close_channel(bank, channel) {
            // The last channel anywhere switches the appliance off, which
            // for this surface is the same end its child's exit has.
            Close::CloseWindow => self.eof = true,
            // A gateway detaches its bank: tmux keeps the session, the channel
            // comes home when `%exit` echoes back through the pump.
            Close::Detach(bank) => {
                if let Some(gateway) = self.gateway_mut(bank) {
                    gateway.detach();
                }
            }
            // A tmux window is tmux's to kill; its row goes when the close
            // notification lands, not here.
            Close::KillWindow { bank, window } => {
                if let Some(gateway) = self.gateway_mut(bank) {
                    gateway.kill_window(&window);
                }
            }
            Close::Removed | Close::Nothing => {}
        }
        self.channel_changed();
    }

    /// `Ctrl+PgUp` / `Ctrl+PgDown`.
    pub fn cycle_channel(&mut self, direction: i32) {
        self.channels.cycle_open(direction);
        self.channel_changed();
    }

    /// `Ctrl+Shift+Left` / `Ctrl+Shift+Right`. The session on screen takes
    /// the slot beside its own and swaps with whoever sits there, which is
    /// the `Alt+Shift+<digit>` store aimed at a neighbour rather than at a
    /// numeral. The ends of the bank are walls: slot 1 has nothing to its
    /// left and the cap has nothing to its right, and a step into either
    /// leaves the bank as it stands.
    ///
    /// Unlike the chord, this names no numeral, so it stands whether or not
    /// the bank is on show.
    pub fn move_channel(&mut self, direction: i32) {
        let (bank, channel) = self.channels.on_air();
        // Slot 0 does not exist, and the cap is the model's to enforce:
        // `move_current_to` answers false for either and nothing moves.
        let Some(target) = channel.checked_add_signed(direction) else {
            return;
        };
        self.channels.move_current_to(bank, target);
        self.channel_changed();
    }

    /// Whether this window has a bank at all. The two pager shortcuts are
    /// the only keys that need one: a page nobody can see is not worth
    /// turning. Every other shortcut stands whether the chassis is drawn or
    /// not, the digit chord included.
    fn has_bank(&self) -> bool {
        self.cabinet.as_ref().is_some_and(|c| c.is_shown())
    }

    /// The bank folds away and the well takes the whole window, or it
    /// comes back at its configured count. This window's runtime state,
    /// like fullscreen: no setting is written, and a reload leaves it as it
    /// stands. Answers whether there was a cabinet to fold.
    pub fn toggle_bank_fold(&mut self) -> bool {
        let Some(cabinet) = self.cabinet.as_mut() else {
            return false;
        };
        let folded = !cabinet.is_folded();
        cabinet.set_folded(folded);
        self.relayout();
        self.announce_bank_width();
        true
    }

    /// `Alt+PgUp` / `Alt+PgDown`: within one bank's stretch the pager views
    /// a page without stealing the air; landing on another bank's stretch
    /// brings back the channel that bank last had on the air
    /// ([`crate::channels::Channels::select_bank`] owns that rule).
    /// Answers whether there was a bank to step.
    pub fn step_bank(&mut self, direction: i32) -> bool {
        if !self.has_bank() {
            return false;
        }
        let before = self.pager.view(&self.channels).bank;
        self.pager.step(direction, &self.channels);
        let landed = self.pager.view(&self.channels).bank;
        if landed == before {
            self.settle_bank();
            return true;
        }
        self.channels.select_bank(landed);
        // `channel_changed` settles the bank and turns it to the channel it
        // just put on the air.
        self.channel_changed();
        true
    }

    /// One digit of a chord, and whatever it commits. The bank is drawn for
    /// the eye and the mouse; a chord names a channel with or without it.
    pub fn chord_digit(&mut self, digit: u8, store: bool) -> bool {
        self.chord_modifier = true;
        let (pager, channels) = (&self.pager, &self.channels);
        let committed = self
            .chord
            .feed_digit(digit, store, Instant::now(), |buf, store| {
                pager.slot_prefix_exists(channels, buf, store)
            });
        self.apply_chord(committed);
        true
    }

    /// The chord modifier came up, or the window went away under it: either
    /// way the chord commits.
    pub(super) fn commit_chord(&mut self) {
        let committed = self.chord.commit();
        self.apply_chord(committed);
    }

    /// The chord names a key on the page the bank is showing, as the
    /// numerals engraved beside those keys read; the pager turns it into a
    /// slot.
    pub(super) fn apply_chord(&mut self, committed: Option<Chord>) {
        let Some(chord) = committed else { return };
        let view = self.pager.view(&self.channels);
        let slot = self.pager.absolute_slot(&self.channels, chord.slot());
        match chord {
            Chord::Select(_) => {
                self.channels.select_channel(view.bank, slot);
            }
            Chord::Store(_) => {
                self.channels.move_current_to(view.bank, slot);
            }
        }
        self.channel_changed();
    }

    /// Everything that follows the current pair moving, in order: the tube
    /// flinches, the bank turns to the channel on the air, and a stretch of
    /// numerals that moved under the chord abandons its digits.
    pub(super) fn channel_changed(&mut self) {
        if self.channels.take_degauss() {
            self.degauss.trigger(Instant::now());
        }
        let on_air = self.channels.on_air();
        if self.on_air != on_air {
            self.on_air = on_air;
            // A mark is a region of one grid, so it does not survive the
            // glass turning to another: it clears with the switch, the way
            // the drag state below does. Restoring it on the way back was
            // weighed and ruled out by the owner (GitHub issue #32), not
            // left over from the old parking scheme. What the mark had
            // already copied is safe regardless -- the release wrote it to
            // PRIMARY, and a middle click on any channel reads the string,
            // not the mark.
            self.selection.clear();
            // The view offset's authority is rio-vt's own `display_offset`,
            // which is the channel's, not the window's: `ScrollPosition` is a
            // mirror of it, so the channel coming to the screen brings its own
            // place in its own scrollback and this re-reads it at once rather
            // than a tick later.
            self.scroll.cancel_glide();
            if let Some(session) = self.channels.session() {
                self.scroll.sync(session.term());
            }
            // The renderer holds the cells of the channel that just left,
            // and the channel arriving damaged nothing while it was off the
            // air, so without this the glass keeps the old picture and moves
            // only the cursor into it. It also covers the view the line
            // above just re-read: `ScrollPosition::sync` is what tells the
            // renderer the view moved, and it has already been told here.
            if let Some(glass) = self.glass.as_mut() {
                glass.renderer.invalidate();
            }
            // The find line takes every key while it stands, so one left
            // behind on a channel nobody is looking at is a mode with no way
            // out: its own channel is the only one whose Escape it answers.
            // It comes down here, and the query it held is kept so it comes
            // back when the line is raised again.
            self.close_find();
            // A composition belongs to the channel it was being typed into.
            // Carried across, it would commit into whatever program the new
            // channel is running.
            self.ime.abandon();
            // The gesture the pointer is in the middle of belongs to the
            // grid it started on, exactly as the mark does. This is what the
            // focus-loss path clears for the same reason (`on_focus_changed`).
            self.dragging = false;
            self.secondary_press = false;
            self.last_click = None;
            // And the memory the next gesture reads: the cell the pointer
            // was last over is a cell of the grid that just left, and a
            // motion event on the new one must not be swallowed for matching
            // it; the wheel's banked trackpad pixels were travel over that
            // same grid.
            self.pointer_cell = (0, 0);
            self.wheel_pixels = 0.0;
            // And the link the pointer was over, or had pressed: cells of
            // that grid too.
            self.link_press = None;
            self.set_hover(None);
            // This must run only when the air moves and at no other time:
            // `channel_changed` is also called from the pump, every 8ms, and
            // `ensure_visible` recomputes `page_index` from the channel on the
            // air with no memory of a manual step. Without this guard it put
            // the bank back on the air's own page within a frame of the user
            // paging away from it, which is Alt+PageUp doing nothing and a
            // chord spanning two pages never surviving to be committed.
            self.pager.ensure_visible(&self.channels);
        }
        // Outside the guard, deliberately: this is not a switch's work. The
        // bank on view is the one `Ctrl+Shift+T` acts on however it came to be
        // showing, and `BankPager::refresh` already answers true only when the
        // view really moved, so a pump that changed nothing cancels no chord.
        self.settle_bank();
    }

    /// The half of the above that a pager step also needs: a stretch of
    /// numerals that moved abandons the digits typed against the old ones.
    pub(super) fn settle_bank(&mut self) {
        if self.pager.refresh(&self.channels) {
            self.chord.cancel();
        }
    }

    /// What the bank's furniture draws: one strip per engraved key of the page
    /// on view -- the reason
    /// [`chassis::strip`] is a type and not a private struct.
    /// Every frame draws this, so it reads the one setting it needs rather
    /// than taking a snapshot to get at it: `handle.current()` clones the
    /// whole `Config` under the settings mutex, and `redraw` has already
    /// cloned that same snapshot one call earlier in `apply_live_settings`.
    ///
    /// The no-handle arm reads `base` and not `Config::default()`, for the
    /// same reason `live_config` does: under `--default-settings --profile`
    /// the resolved profile is in `base` and nowhere else, and reading a
    /// default here showed the bank an indicator the profile had not asked
    /// for. It also built a whole `Config` per frame to do it.
    pub fn bank_strips(&self) -> chassis::BankStrips {
        let indicator = match self.settings.as_ref() {
            Some(handle) => handle.with(chassis::channel_indicator),
            None => chassis::channel_indicator(&self.base),
        };
        self.pager.strips(&self.channels, indicator)
    }
}
