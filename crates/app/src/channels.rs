//! The channel model: what a window's sessions are numbered, which one is on
//! the air, and what the tube does when that changes.
//!
//! A bank is a visual collection of rows of names: a window name under tmux, a
//! shell's title otherwise. A bank is managed by this program itself, by one
//! `tmux -CC` attachment -- its gateway, the channel carrying the control
//! stream, standing at slot 1, controlling every channel of its bank -- or by
//! one SSH connection this program owns, whose channels multiplex over the
//! one wire with no gateway row. Each tmux session gets a bank of its own,
//! and so does each SSH connection. Within
//! a bank a slot whose session has exited goes dark and stays dark: no
//! renumbering, and a new channel takes the lowest free slot. Rows beyond what
//! the theme shows keep the pager behaviour (`crate::bank`).
//!
//! # What is here and what is not
//!
//! Everything in this file is the state machine: slot arithmetic, the current
//! pair, and the transitions a tmux attachment puts it through. What is *not*
//! here is a gateway client. A row does not hold one; that client is
//! [`crate::tmux::Gateway`], held by the surface (`window::TerminalSurface`),
//! and where this model would otherwise call it it returns a [`Close`] or a
//! [`BankId`] and lets the surface do it. One gateway law lives with the
//! surface too: the glass's grid has to reach *every* gateway on attach and on
//! resize, because a per-bank publish silently corrupts the sizes tmux draws
//! other sessions at, and the surface's `set_client_size` is that law's one
//! home.
//!
//! # One deliberate choice
//!
//! Every query walks [`Channels::rows`], at most 99 entries per bank, rather
//! than a derived cache kept in sync by hand: a cache is only as sound as a
//! rule remembered at every call site that touches the state, and walking the
//! small, bounded row set removes the rule instead of keeping it.

use std::mem;

use tmux_cc::{PaneId, WindowId};

/// Slots are two engraved digits, so 99 is the last one a panel can name.
pub const CHANNEL_CAP: u32 = 99;

/// A bank's id. Bank 0 is the one this program manages and never leaves; tmux
/// attachments and SSH connections take 1, 2, ... in the order they arrive,
/// and ids only ever grow, so sorting rows by `(bank, channel)` puts the home
/// bank first and the rest behind it in arrival order.
pub type BankId = u32;

/// Who manages a bank, and everything true only under one manager: the
/// attachment state under tmux, the destination under SSH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Manager {
    /// This program, which spawns the shells itself.
    Home,
    /// One `tmux -CC` attachment.
    Tmux {
        /// The hostname the tmux server reported, shown in the gateway row's
        /// title. As often this machine as another: `tmux -CC` is run here at
        /// least as often as over ssh.
        host: String,
        /// The slot the gateway holds dark behind it while attached: the
        /// home-bank slot it was typed in, or an SSH bank's slot for a
        /// `tmux -CC` typed on a remote shell. `None` on a bank the
        /// terminal raised itself, which holds nothing at home.
        home: Option<(BankId, u32)>,
        /// A window this bank asked tmux for, still on its way: asking for a
        /// channel puts you in it, so the next window to arrive takes the air
        /// while the ones tmux volunteers do not.
        new_window_pending: bool,
        /// The attachment's first window has arrived and the greeting is
        /// spent. Counted rather than read off the slot number: the first
        /// window lands on slot 2, but so does a window another client
        /// volunteers *after* the user has closed the one that was standing
        /// there, and that second slot-2 window must not take the air.
        attach_done: bool,
    },
    /// One SSH connection this program owns. Channels are the connection's
    /// own multiplexed channels and fill from slot 1: there is no gateway
    /// row, because the wire has no on-screen carrier.
    Ssh {
        /// Where the connection goes, as the user spelled it.
        host: String,
        user: String,
        port: u16,
    },
}

impl Manager {
    pub fn is_tmux(&self) -> bool {
        matches!(self, Manager::Tmux { .. })
    }

    pub fn is_ssh(&self) -> bool {
        matches!(self, Manager::Ssh { .. })
    }

    /// The slot held for this bank's gateway, if it came from one.
    fn home(&self) -> Option<(BankId, u32)> {
        match self {
            Manager::Tmux { home, .. } => *home,
            Manager::Home | Manager::Ssh { .. } => None,
        }
    }
}

/// One bank.
#[derive(Clone, Debug)]
pub struct Bank {
    pub id: BankId,
    pub manager: Manager,
    /// The channel this bank last had on the air, the set's current channel
    /// being the current bank's entry: a band selector comes back to the
    /// station it was left on. A bank that has never taken the air holds 1,
    /// its gateway or first slot.
    pub channel: u32,
}

/// One channel slot with a session in it.
#[derive(Clone, Debug)]
pub struct Row<S> {
    pub bank: BankId,
    pub channel: u32,
    pub title: String,
    /// Where tmux draws this row: the window and the pane showing in it.
    pub tmux: Option<(WindowId, PaneId)>,
    /// What the slot holds. `app` puts a `term::Session` here; a test puts
    /// whatever it can assert on.
    pub session: S,
}

/// A bank's stretch of the pager's flattened space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewPage {
    pub bank: BankId,
    /// How many pages this bank unrolls into.
    pub count: i32,
}

/// What a close or a death asks the surface for: the surface is where the
/// gateway call or the window close actually happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Close {
    /// No such row; nothing happened.
    Nothing,
    /// The row is gone.
    Removed,
    /// That was the last channel anywhere: the appliance switches off.
    CloseWindow,
    /// A gateway: detach the bank's session. tmux keeps it, the channel comes
    /// home, and the row goes when the detach echoes back.
    Detach(BankId),
    /// A tmux window is tmux's to kill; the row goes when `%window-close`
    /// comes back.
    KillWindow { bank: BankId, window: WindowId },
}

/// The per-window collection of channels and the banks they are numbered on.
pub struct Channels<S> {
    banks: Vec<Bank>,
    /// Kept sorted ascending by `(bank, channel)`.
    rows: Vec<Row<S>>,
    next_bank_id: BankId,
    current_bank: BankId,
    /// The set only flinches once it is on: bringing up the first channel is
    /// not a channel change.
    degauss_armed: bool,
    degauss_pending: bool,
    /// `channelStored` acknowledgements the bank has not blinked yet.
    stored: Vec<u32>,
}

impl<S> Default for Channels<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Channels<S> {
    /// An empty set with the home bank and nothing on it, degauss disarmed:
    /// the state before anything has been opened.
    pub fn new() -> Self {
        Channels {
            banks: vec![Bank {
                id: 0,
                manager: Manager::Home,
                channel: 1,
            }],
            rows: Vec::new(),
            next_bank_id: 1,
            current_bank: 0,
            degauss_armed: false,
            degauss_pending: false,
            stored: Vec::new(),
        }
    }

    /// `Component.onCompleted` (`:698-701`): open channel 1 and arm the tube.
    /// The first channel is not a channel change, so nothing flinches.
    pub fn start(&mut self, session: impl FnOnce() -> Option<S>) {
        self.open_channel(0, 1, session);
        self.started();
    }

    /// The set has come up on its first channel, however that channel was
    /// made: the tube arms only now, so the arrival was not a channel
    /// change and nothing flinched.
    pub fn started(&mut self) {
        self.degauss_armed = true;
        self.degauss_pending = false;
    }

    // ---- reading the state ------------------------------------------

    pub fn rows(&self) -> &[Row<S>] {
        &self.rows
    }

    /// Every row, for the two things the surface writes on one directly:
    /// pumping its session and taking the title that session's escapes set.
    ///
    /// `bank` and `channel` are the sort key and are not the surface's to
    /// write: the transition that moves them re-sorts, and a write here would
    /// not.
    pub fn rows_mut(&mut self) -> impl Iterator<Item = &mut Row<S>> {
        self.rows.iter_mut()
    }

    pub fn banks(&self) -> &[Bank] {
        &self.banks
    }

    pub fn current_bank(&self) -> BankId {
        self.current_bank
    }

    /// The channel of the bank on the air, read off that bank. Zero while the
    /// current bank id names no bank -- the beat between a bank being taken
    /// out of the list and the air landing somewhere else -- which is the
    /// answer the readers below already gave for a pair naming no row.
    pub fn current_channel(&self) -> u32 {
        self.bank_of(self.current_bank)
            .map_or(0, |i| self.banks[i].channel)
    }

    /// The pair on the air, which is how most callers want it: nothing reads
    /// one of these two without reading the other.
    pub fn on_air(&self) -> (BankId, u32) {
        (self.current_bank, self.current_channel())
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Slot 1 of a bank a tmux attachment manages: the channel whose PTY runs
    /// `tmux -CC` and carries the control stream. Read off the bank and the
    /// slot, never stored.
    pub fn is_gateway(&self, row: &Row<S>) -> bool {
        row.channel == 1 && self.manager_of(row.bank).is_some_and(Manager::is_tmux)
    }

    pub fn manager_of(&self, bank: BankId) -> Option<&Manager> {
        self.bank_of(bank).map(|i| &self.banks[i].manager)
    }

    fn row_of(&self, bank: BankId, channel: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.bank == bank && r.channel == channel)
    }

    fn bank_of(&self, bank: BankId) -> Option<usize> {
        self.banks.iter().position(|b| b.id == bank)
    }

    /// A bank's manager, for the transitions that write its state.
    fn manager_mut(&mut self, bank: BankId) -> Option<&mut Manager> {
        let i = self.bank_of(bank)?;
        Some(&mut self.banks[i].manager)
    }

    /// The title standing at a slot, or `None` for a dark one. Computed
    /// rather than cached (see the module doc).
    pub fn slot_title(&self, bank: BankId, channel: u32) -> Option<&str> {
        self.row_of(bank, channel)
            .map(|i| self.rows[i].title.as_str())
    }

    /// What the window title reads. `None` is where the caller falls back to
    /// the application's own name, which is the surface's string to supply.
    pub fn current_title(&self) -> Option<&str> {
        match self.slot_title(self.current_bank, self.current_channel()) {
            Some(t) if !t.is_empty() => Some(t),
            _ => None,
        }
    }

    pub fn current(&self) -> Option<&Row<S>> {
        self.row_of(self.current_bank, self.current_channel())
            .map(|i| &self.rows[i])
    }

    pub fn current_mut(&mut self) -> Option<&mut Row<S>> {
        self.row_of(self.current_bank, self.current_channel())
            .map(|i| &mut self.rows[i])
    }

    /// Every session, current or not. Every channel's shell keeps running
    /// whether or not it is the one shown; a channel whose shell keeps
    /// printing while another is on the air is still printing when you come
    /// back to it.
    pub fn sessions_mut(&mut self) -> impl Iterator<Item = &mut S> {
        self.rows.iter_mut().map(|r| &mut r.session)
    }

    /// The session on the air, which is the one the glass is showing.
    pub fn session(&self) -> Option<&S> {
        self.current().map(|r| &r.session)
    }

    pub fn session_mut(&mut self) -> Option<&mut S> {
        self.current_mut().map(|r| &mut r.session)
    }

    /// `:138-145`. A transported channel's home slot stays dark and held while
    /// it is attached; the hold lives on the bank, not on any row.
    fn slot_held(&self, bank: BankId, channel: u32) -> bool {
        self.banks
            .iter()
            .any(|b| b.manager.home() == Some((bank, channel)))
    }

    /// Whether a hold on `bank` stands in a slot's way. It does while the bank
    /// lights a row; on one gone entirely dark it does not, or the user's next
    /// channel there would open at 02 with 01 dark beside it and unopenable.
    fn holds_bind(&self, bank: BankId) -> bool {
        self.rows.iter().any(|r| r.bank == bank)
    }

    /// `:149-168`. The lowest free slot of a bank. On home the held slots
    /// count as taken ([`Self::holds_bind`]); on an attachment the gateway
    /// holds 1, so windows fill from 2. Zero when the bank is full to the cap.
    pub fn first_free(&self, bank: BankId) -> u32 {
        let mut taken = [false; CHANNEL_CAP as usize + 1];
        for row in self.rows.iter().filter(|r| r.bank == bank) {
            if row.channel <= CHANNEL_CAP {
                taken[row.channel as usize] = true;
            }
        }
        for (held_bank, slot) in self.banks.iter().filter_map(|b| b.manager.home()) {
            if held_bank == bank && slot <= CHANNEL_CAP && self.holds_bind(bank) {
                taken[slot as usize] = true;
            }
        }
        (1..=CHANNEL_CAP).find(|n| !taken[*n as usize]).unwrap_or(0)
    }

    /// `:170-178`.
    pub fn highest_open(&self, bank: BankId) -> u32 {
        self.rows
            .iter()
            .filter(|r| r.bank == bank)
            .map(|r| r.channel)
            .max()
            .unwrap_or(0)
    }

    /// `:184-195`. The pager walks one flattened space over every bank: each
    /// unrolls into as many pages as its slots need, every open slot reachable
    /// and so is the next free one, the slot a new channel will take.
    pub fn view_pages(&self, rows_visible: i32) -> Vec<ViewPage> {
        let rows = rows_visible.max(1);
        self.banks
            .iter()
            .map(|b| {
                let span = self.highest_open(b.id).max(self.first_free(b.id)).max(1) as i32;
                ViewPage {
                    bank: b.id,
                    // JS `Math.ceil(span / rows)` on two positive integers.
                    count: (span + rows - 1) / rows,
                }
            })
            .collect()
    }

    /// `:504-514`. True when `buf` is a strict prefix of some *open* slot of
    /// the named bank's stretch rooted at `base`: the chord keeps waiting only
    /// for digits that can still land.
    pub fn page_slot_prefix_exists(&self, bank: BankId, buf: &str, base: u32, count: u32) -> bool {
        (1..=count).any(|n| {
            if self.slot_title(bank, base + n).is_none() {
                return false;
            }
            let s = n.to_string();
            s.len() > buf.len() && s.starts_with(buf)
        })
    }

    // ---- the tube ----------------------------------------------------

    /// Whether a channel change since the last call asked the tube to flinch,
    /// clearing the flag. `:706-713`: turning the knob makes the tube flinch,
    /// and turning it to another bank no less; re-selecting the
    /// current channel never reaches there, because nothing changed.
    pub fn take_degauss(&mut self) -> bool {
        mem::take(&mut self.degauss_pending)
    }

    /// The `channelStored` acknowledgements the bank has not blinked yet. At
    /// most two, because each store clears the last one's: an acknowledgement
    /// nobody drains is not queued either, and a blink for a store two stores
    /// ago would land on the wrong panel anyway.
    ///
    /// **No caller yet, deliberately left unconsumed.** The eventual consumer
    /// is a per-row blink, a display state animated over time, and neither
    /// display kit has a clock: `chassis::displays::{led,tape}` map a slot's
    /// `open`/`bright` to an appearance, and the strips are rebuilt whole per
    /// frame from [`chassis::BankStrips`]. A drain that dropped the
    /// acknowledgements on the floor would be worse than none -- the model
    /// would look wired -- and the list is bounded at two by
    /// [`Channels::move_current_to`]'s own clear, so leaving it undrained costs
    /// nothing. Whoever gives a strip a clock takes this on too.
    pub fn take_stored(&mut self) -> Vec<u32> {
        mem::take(&mut self.stored)
    }

    /// The one writer of the current pair, and so the one place the tube is
    /// asked to flinch. The channel is written through to its bank; a bank
    /// already gone from the list keeps nothing, the air being on its way
    /// elsewhere anyway.
    fn set_current(&mut self, bank: BankId, channel: u32) {
        let moved = bank != self.current_bank || channel != self.current_channel();
        self.current_bank = bank;
        if let Some(i) = self.bank_of(bank) {
            self.banks[i].channel = channel;
        }
        if moved && self.degauss_armed {
            self.degauss_pending = true;
        }
    }

    /// The air lands on the channel the bank was left showing, or on its
    /// first open row when that slot has gone dark since. False only for a
    /// bank this set does not hold: one with no open row takes the air too, on
    /// the slot it remembers, the strip the user paged to and the glass having
    /// to agree (`window::TerminalSurface::no_signal` draws the rest).
    pub fn select_bank(&mut self, bank: BankId) -> bool {
        let Some(i) = self.bank_of(bank) else {
            return false;
        };
        let remembered = self.banks[i].channel;
        let channel = if self.row_of(bank, remembered).is_some() {
            remembered
        } else {
            match self.rows.iter().find(|r| r.bank == bank) {
                Some(row) => row.channel,
                None => remembered,
            }
        };
        self.set_current(bank, channel);
        true
    }

    // ---- opening -----------------------------------------------------

    /// `:213-225`. PTY shells live on the home bank alone: an attachment's
    /// channels are tmux's to give. A held slot refuses while its bank binds
    /// ([`Self::holds_bind`]); it is a transported channel's berth. `session`
    /// is called only once the slot is known to be takeable, so a refused open
    /// costs no pty.
    pub fn open_channel(
        &mut self,
        bank: BankId,
        channel: u32,
        session: impl FnOnce() -> Option<S>,
    ) -> bool {
        if bank != 0 {
            return false;
        }
        if !(1..=CHANNEL_CAP).contains(&channel) {
            return false;
        }
        if self.row_of(bank, channel).is_some()
            || (self.slot_held(bank, channel) && self.holds_bind(bank))
        {
            return false;
        }
        let Some(session) = session() else {
            return false;
        };
        self.insert_row(Row {
            bank: 0,
            channel,
            title: String::new(),
            tmux: None,
            session,
        });
        self.set_current(0, channel);
        true
    }

    /// `:284-288`.
    pub fn open_first_free(&mut self, session: impl FnOnce() -> Option<S>) -> bool {
        let slot = self.first_free(0);
        if slot == 0 {
            return false;
        }
        self.open_channel(0, slot, session)
    }

    /// `:292-301`: what `Ctrl+Shift+T` asks for. A new channel goes to `view`,
    /// the bank the user is looking at: on home the lowest free slot with a
    /// shell in it, on an attachment another window of that session, which is
    /// the gateway's to give. Returns the bank whose gateway must be asked, or
    /// `None` when the shell was opened here.
    ///
    /// The view is the pager's (`crate::bank::BankPager::view`) and is passed
    /// in rather than held: it is derived from this model plus how many rows
    /// the window fits, and a copy kept here would be a second answer to
    /// re-synchronise.
    pub fn new_channel(
        &mut self,
        view: BankId,
        session: impl FnOnce() -> Option<S>,
    ) -> Option<BankId> {
        if self.ask_for_window(view) {
            return Some(view);
        }
        self.open_first_free(session);
        None
    }

    /// `:307-313`. Ask the bank the air is on for another window, by name
    /// rather than by where the pager stands.
    pub fn new_tmux_window(&mut self) -> Option<BankId> {
        let bank = self.current_bank;
        self.ask_for_window(bank).then_some(bank)
    }

    /// The bank owes itself a window from its gateway. False where this
    /// program manages the bank, there being no gateway to ask.
    fn ask_for_window(&mut self, bank: BankId) -> bool {
        let Some(Manager::Tmux {
            new_window_pending, ..
        }) = self.manager_mut(bank)
        else {
            return false;
        };
        *new_window_pending = true;
        true
    }

    /// `:232-258`. A tmux window becomes an ordinary channel on the lowest free
    /// slot of its bank, from 2 up: the gateway holds 1. Windows the attach
    /// lists line up behind the gateway without taking the air; a window this
    /// set asked for is the exception and takes it outright.
    pub fn open_tmux_window(
        &mut self,
        bank: BankId,
        window: &WindowId,
        pane: &PaneId,
        name: &str,
        session: impl FnOnce() -> Option<S>,
    ) -> bool {
        let channel = self.first_free(bank);
        if channel < 1 {
            return false;
        }
        let on_gateway = self.on_air() == (bank, 1);
        let Some(Manager::Tmux {
            new_window_pending,
            attach_done,
            ..
        }) = self.manager_mut(bank)
        else {
            return false;
        };
        // Whatever arrives next answers the request, and nothing after it does.
        let asked = mem::take(new_window_pending);
        // An attach's first window also takes the air: the user who typed
        // `tmux -CC` meets a live prompt, not the gateway's own frozen glass.
        let greets = !*attach_done && on_gateway;
        let Some(session) = session() else {
            return false;
        };
        // The greeting is spent by the window arriving, wherever the air
        // happened to be standing: a first window that arrived while the user
        // was on another bank does not leave the greeting owing to the second.
        *attach_done = true;
        self.insert_row(Row {
            bank,
            channel,
            title: normalize_title(name),
            tmux: Some((window.clone(), pane.clone())),
            session,
        });
        if asked || greets {
            self.set_current(bank, channel);
        }
        true
    }

    fn insert_row(&mut self, row: Row<S>) {
        let dest = self
            .rows
            .iter()
            .position(|r| !less_than(r, &row))
            .unwrap_or(self.rows.len());
        self.rows.insert(dest, row);
    }

    /// `:273-282`. A row whose bank or channel was rewritten in place slides to
    /// where the sort order wants it.
    fn resort(&mut self) {
        self.rows.sort_by_key(|r| (r.bank, r.channel));
    }

    // ---- closing -----------------------------------------------------

    /// `:320-341`. What the user's close asks for, by what the row is.
    pub fn close_channel(&mut self, bank: BankId, channel: u32) -> Close {
        let Some(index) = self.row_of(bank, channel) else {
            return Close::Nothing;
        };
        if self.is_gateway(&self.rows[index]) {
            return Close::Detach(bank);
        }
        if let Some((window, _)) = self.rows[index].tmux.clone() {
            return Close::KillWindow { bank, window };
        }
        if self.anchored_rows() <= 1 {
            return Close::CloseWindow;
        }
        self.remove_row(bank, channel);
        Close::Removed
    }

    /// `:348-361`. A row's own program died. For a PTY shell that is the
    /// ordinary end of a channel; for a gateway it is the tmux client dying
    /// under the session, and the bank collapses.
    pub fn session_died(&mut self, bank: BankId, channel: u32) -> Close {
        let Some(index) = self.row_of(bank, channel) else {
            return Close::Nothing;
        };
        if self.is_gateway(&self.rows[index]) {
            return self.gateway_died(bank);
        }
        if self.anchored_rows() <= 1 {
            return Close::CloseWindow;
        }
        self.remove_row(bank, channel);
        Close::Removed
    }

    /// `:366-379`. The gateway's own shell died: collapse the bank as a detach
    /// would, which lands its row home, then remove that row, there being no
    /// live process to come home to.
    fn gateway_died(&mut self, bank: BankId) -> Close {
        if let Some((home_bank, home_slot)) = self.collapse_bank(bank) {
            self.remove_row(home_bank, home_slot);
        }
        // Nothing survived it: the appliance has no channel left to show.
        if self.anchored_rows() == 0 {
            Close::CloseWindow
        } else {
            Close::Removed
        }
    }

    /// Home's rows, the gateways that came from a home slot, and every SSH
    /// channel (an SSH bank stands on the user's own ask, with no home row
    /// behind it): the last of these going is the last channel going out,
    /// whatever else stands.
    fn anchored_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| {
                r.bank == 0
                    || (r.channel == 1
                        && self
                            .manager_of(r.bank)
                            .is_some_and(|m| m.home().is_some()))
                    || self.manager_of(r.bank).is_some_and(Manager::is_ssh)
            })
            .count()
    }

    /// `:384-399`. Where a single row goes. The nearest surviving row of the
    /// same bank takes the air when the removed one had it. An SSH bank
    /// whose last row goes goes with it: with no home slot and no gateway
    /// there is nothing for an empty one to stand for.
    fn remove_row(&mut self, bank: BankId, channel: u32) {
        let Some(index) = self.row_of(bank, channel) else {
            return;
        };
        let was_current = self.on_air() == (bank, channel);
        self.rows.remove(index);
        if was_current {
            if let Some(next) = self.nearest_row(index, bank) {
                let (b, c) = (self.rows[next].bank, self.rows[next].channel);
                self.set_current(b, c);
            }
        }
        if self.manager_of(bank).is_some_and(Manager::is_ssh)
            && !self.rows.iter().any(|r| r.bank == bank)
        {
            self.banks.retain(|b| b.id != bank);
        }
    }

    /// `:404-416`. The row nearest the hole a removed row left, its own bank's
    /// rows first: the one that slid into its place, else the nearest one
    /// before it, else whatever another bank still holds.
    fn nearest_row(&self, index: usize, bank: BankId) -> Option<usize> {
        if let Some(after) = (index..self.rows.len()).find(|i| self.rows[*i].bank == bank) {
            return Some(after);
        }
        if let Some(before) = (0..index.min(self.rows.len()))
            .rev()
            .find(|i| self.rows[*i].bank == bank)
        {
            return Some(before);
        }
        if !self.rows.is_empty() {
            return Some(index.min(self.rows.len() - 1));
        }
        None
    }

    // ---- selecting and storing ---------------------------------------

    /// `:418-426`. A dark slot is not selectable; the bank opens one instead.
    pub fn select_channel(&mut self, bank: BankId, channel: u32) -> bool {
        if self.row_of(bank, channel).is_none() {
            return false;
        }
        self.set_current(bank, channel);
        true
    }

    /// Whether [`Channels::move_current_to`] would take the session on screen
    /// to `channel` of `bank`. A drag asks it before the drop, to show only
    /// a store that will land.
    pub fn can_store(&self, bank: BankId, channel: u32) -> bool {
        // The pager can be viewing one bank while another holds the air, and a
        // store chord lands on the bank on view: there is no slot on it the
        // session could take without leaving its own bank.
        if bank != self.current_bank {
            return false;
        }
        if !(1..=CHANNEL_CAP).contains(&channel) || channel == self.current_channel() {
            return false;
        }
        let Some(from) = self.row_of(bank, self.current_channel()) else {
            return false;
        };
        if self.is_gateway(&self.rows[from]) || self.slot_held(bank, channel) {
            return false;
        }
        !self
            .row_of(bank, channel)
            .is_some_and(|i| self.is_gateway(&self.rows[i]))
    }

    /// `:433-480`. The session on screen keeps its screen; its slot number
    /// moves. An occupied slot swaps the two sessions' numbers.
    ///
    /// The LED blink is the store's acknowledgement, so the tube holds steady
    /// throughout: the degauss is disarmed for the move and re-armed after
    /// it.
    pub fn move_current_to(&mut self, bank: BankId, channel: u32) -> bool {
        if !self.can_store(bank, channel) {
            return false;
        }
        let origin = self.current_channel();
        let Some(from) = self.row_of(bank, origin) else {
            return false;
        };
        let to = self.row_of(bank, channel);

        let armed = self.degauss_armed;
        self.degauss_armed = false;
        self.stored.clear();
        // Swapping the two rows' channel numbers, on a list kept sorted by
        // `(bank, channel)`, is exactly an exchange of the two numbers.
        self.rows[from].channel = channel;
        if let Some(to) = to {
            self.rows[to].channel = origin;
        }
        self.resort();
        self.set_current(bank, channel);
        self.degauss_armed = armed;
        self.stored.push(channel);
        // A swap lands two sessions, and the displaced one gets its own say.
        if to.is_some() {
            self.stored.push(origin);
        }
        true
    }

    /// `:484-499`. Cycling walks the current bank: the other banks' channels
    /// are a pager step away, not a knob turn.
    pub fn cycle_open(&mut self, direction: i32) {
        let slots: Vec<u32> = self
            .rows
            .iter()
            .filter(|r| r.bank == self.current_bank)
            .map(|r| r.channel)
            .collect();
        let current = self.current_channel();
        let Some(pos) = slots.iter().position(|c| *c == current) else {
            return;
        };
        if slots.is_empty() {
            return;
        }
        let len = slots.len() as i32;
        let next = ((pos as i32 + direction) % len + len) % len;
        let bank = self.current_bank;
        self.select_channel(bank, slots[next as usize]);
    }

    /// `:516-522`. A PTY shell's title is its own; on an attachment the model
    /// owns it, and a tmux window's title is tmux's to give (`:750-754`).
    pub fn set_title(&mut self, bank: BankId, channel: u32, raw: &str) -> bool {
        let Some(index) = self.row_of(bank, channel) else {
            return false;
        };
        let title = normalize_title(raw);
        if self.rows[index].title == title {
            return false;
        }
        self.rows[index].title = title;
        true
    }

    // ---- the tmux transitions ----------------------------------------

    /// `:530-559`. A channel's program has entered tmux control mode: the
    /// channel transports to slot 1 of a new bank, titled for the host the
    /// tmux server reported, and its home slot is held dark behind it. The row
    /// mutates in place, never removes, so the glass keeps the screen it was
    /// showing. Transport is a renumbering, not a channel change, so the tube
    /// holds steady. Returns the new bank's id.
    pub fn attach(&mut self, bank: BankId, channel: u32, host: &str) -> Option<BankId> {
        let index = self.row_of(bank, channel)?;
        if self.manager_of(bank).is_some_and(Manager::is_tmux) {
            return None;
        }
        let id = self.push_bank(host, Some((bank, channel)));
        let was_current = self.on_air() == (bank, channel);
        let armed = self.degauss_armed;
        self.degauss_armed = false;
        self.rows[index].bank = id;
        self.rows[index].channel = 1;
        self.rows[index].title = gateway_title(host);
        self.resort();
        if was_current {
            self.set_current(id, 1);
        }
        self.degauss_armed = armed;
        Some(id)
    }

    /// A bank the terminal raised for a session it found: a gateway at slot 1 of
    /// a bank with no home slot; nothing takes the air, nothing degausses.
    pub fn attach_spawned(
        &mut self,
        host: &str,
        make: impl FnOnce() -> Option<S>,
    ) -> Option<BankId> {
        let gateway = make()?;
        let id = self.push_bank(host, None);
        self.insert_row(Row {
            bank: id,
            channel: 1,
            title: gateway_title(host),
            tmux: None,
            session: gateway,
        });
        Some(id)
    }

    /// A bank for an SSH connection the user asked for: its first channel
    /// takes slot 1 and the air with it. The `attach_spawned` shape, but the
    /// ask is the user's, so the arrival is a channel change like any other.
    pub fn open_ssh_bank(
        &mut self,
        user: &str,
        host: &str,
        port: u16,
        make: impl FnOnce() -> Option<S>,
    ) -> Option<BankId> {
        let session = make()?;
        let id = self.next_bank_id;
        self.next_bank_id += 1;
        self.banks.push(Bank {
            id,
            manager: Manager::Ssh {
                host: host.to_string(),
                user: user.to_string(),
                port,
            },
            channel: 1,
        });
        self.insert_row(Row {
            bank: id,
            channel: 1,
            title: format!("{user}@{host}"),
            tmux: None,
            session,
        });
        self.set_current(id, 1);
        Some(id)
    }

    /// Another channel on an SSH bank: the connection's next multiplexed
    /// channel, on the lowest free slot, taking the air the way the asked
    /// window of a tmux bank does. False when the bank is not an SSH one
    /// or is full to the cap.
    pub fn open_ssh_channel(&mut self, bank: BankId, make: impl FnOnce() -> Option<S>) -> bool {
        let Some(Manager::Ssh { host, user, .. }) = self.manager_of(bank) else {
            return false;
        };
        let title = format!("{user}@{host}");
        let channel = self.first_free(bank);
        if channel < 1 {
            return false;
        }
        let Some(session) = make() else {
            return false;
        };
        self.insert_row(Row {
            bank,
            channel,
            title,
            tmux: None,
            session,
        });
        self.set_current(bank, channel);
        true
    }

    /// A new bank under a tmux attachment; its gateway row is the caller's to place.
    fn push_bank(&mut self, host: &str, home: Option<(BankId, u32)>) -> BankId {
        let id = self.next_bank_id;
        self.next_bank_id += 1;
        self.banks.push(Bank {
            id,
            manager: Manager::Tmux {
                host: host.to_string(),
                home,
                new_window_pending: false,
                attach_done: false,
            },
            channel: 1,
        });
        id
    }

    /// `:577-608`. Detach or gateway death: the bank's window rows vanish, the
    /// gateway transports home to its berth and relights, and the user lands
    /// on it. Answers where it landed, or `None` when nothing came home: a
    /// bank that never held a slot simply goes, rows and all, with the air
    /// falling to the nearest surviving row if it was standing there.
    pub fn collapse_bank(&mut self, bank: BankId) -> Option<(BankId, u32)> {
        let i = self.bank_of(bank)?;
        let Manager::Tmux { home, .. } = &self.banks[i].manager else {
            return None;
        };
        let home = *home;
        let armed = self.degauss_armed;
        self.degauss_armed = false;
        let Some((home_bank, home_slot)) = home else {
            let at = self.rows.iter().position(|r| r.bank == bank).unwrap_or(0);
            let was_here = self.current_bank == bank;
            self.rows.retain(|r| r.bank != bank);
            self.banks.remove(i);
            if was_here {
                if let Some(next) = self.nearest_row(at, 0) {
                    let (b, c) = (self.rows[next].bank, self.rows[next].channel);
                    self.set_current(b, c);
                }
            }
            self.degauss_armed = armed;
            return None;
        };
        self.rows.retain(|r| r.bank != bank || r.channel == 1);
        // The berth was the gateway's alone while nothing could take a held
        // slot. Now a dark bank offers it, so a taken berth sends the returning
        // gateway to the next free slot, and a full one sends it out.
        let berth = if self.row_of(home_bank, home_slot).is_some() {
            self.first_free(home_bank)
        } else {
            home_slot
        };
        if let Some(gateway) = self.rows.iter().position(|r| r.bank == bank) {
            if berth == 0 {
                self.rows.remove(gateway);
            } else {
                self.rows[gateway].bank = home_bank;
                self.rows[gateway].channel = berth;
                self.resort();
            }
        }
        self.banks.remove(i);
        self.set_current(home_bank, if berth == 0 { home_slot } else { berth });
        self.degauss_armed = armed;
        (berth != 0).then_some((home_bank, berth))
    }

    /// `:669-681`. The tmux server's hostname can resolve after the handshake:
    /// the bank and its gateway row's title follow it.
    pub fn host_changed(&mut self, bank: BankId, host: &str) {
        let Some(Manager::Tmux { host: known, .. }) = self.manager_mut(bank) else {
            return;
        };
        *known = host.to_string();
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.bank == bank && r.channel == 1)
        {
            row.title = gateway_title(host);
        }
    }

    /// `:683-690`.
    pub fn channel_of_window(&self, bank: BankId, window: &WindowId) -> u32 {
        self.rows
            .iter()
            .find(|r| r.bank == bank && r.tmux.as_ref().is_some_and(|(w, _)| w == window))
            .map(|r| r.channel)
            .unwrap_or(0)
    }

    /// `:656-660`. tmux says a window closed; its row goes.
    pub fn window_closed(&mut self, bank: BankId, window: &WindowId) {
        let channel = self.channel_of_window(bank, window);
        if channel > 0 {
            self.remove_row(bank, channel);
        }
    }
}

/// `:112-117`.
fn normalize_title(raw: &str) -> String {
    raw.trim().to_string()
}

/// `:547`. What a gateway row reads while it is attached.
fn gateway_title(host: &str) -> String {
    format!("tmux -CC # @{host}")
}

/// `:267-269`.
fn less_than<S>(a: &Row<S>, b: &Row<S>) -> bool {
    if a.bank != b.bank {
        a.bank < b.bank
    } else {
        a.channel < b.channel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel's payload in these tests: the slot it was opened on, so a
    /// swap that moved a number rather than a session would show.
    fn channels() -> Channels<u32> {
        let mut set = Channels::new();
        let mut next = 100;
        set.start(|| {
            next += 1;
            Some(next)
        });
        set
    }

    fn open(set: &mut Channels<u32>, bank: BankId, channel: u32, mark: u32) -> bool {
        set.open_channel(bank, channel, || Some(mark))
    }

    fn window(set: &mut Channels<u32>, bank: BankId, id: u32, name: &str, mark: u32) -> bool {
        let window = WindowId::parse(&format!("@{id}")).unwrap();
        let pane = PaneId::parse(&format!("%{id}")).unwrap();
        set.open_tmux_window(bank, &window, &pane, name, || Some(mark))
    }

    #[test]
    fn the_first_channel_is_slot_one_and_does_not_flinch_the_tube() {
        let mut set = channels();
        assert_eq!(set.current_channel(), 1);
        assert_eq!(set.current_bank(), 0);
        assert_eq!(set.len(), 1);
        assert!(!set.take_degauss(), "the set only flinches once it is on");
    }

    #[test]
    fn new_channels_take_the_lowest_free_slot() {
        let mut set = channels();
        assert!(set.open_first_free(|| Some(2)));
        assert_eq!(set.current_channel(), 2);
        assert!(set.open_first_free(|| Some(3)));
        assert_eq!(set.current_channel(), 3);
        // A hole left by a close is the lowest free slot.
        set.close_channel(0, 2);
        assert_eq!(set.first_free(0), 2);
        assert!(set.open_first_free(|| Some(9)));
        assert_eq!(set.current_channel(), 2);
    }

    #[test]
    fn an_exited_slot_goes_dark_and_stays_dark() {
        let mut set = channels();
        open(&mut set, 0, 2, 2);
        open(&mut set, 0, 3, 3);
        set.select_channel(0, 3);
        set.close_channel(0, 2);
        // No renumbering: 3 is still 3.
        assert!(set.slot_title(0, 2).is_none());
        assert_eq!(set.current_channel(), 3);
        assert_eq!(set.current().unwrap().session, 3);
    }

    #[test]
    fn a_taken_slot_refuses_a_second_shell() {
        let mut set = channels();
        assert!(!open(&mut set, 0, 1, 9));
        assert_eq!(set.len(), 1);
        // ...and so does a slot outside the engraved range.
        assert!(!open(&mut set, 0, 0, 9));
        assert!(!open(&mut set, 0, CHANNEL_CAP + 1, 9));
    }

    #[test]
    fn a_channel_switch_asks_the_tube_to_flinch_and_a_re_selection_does_not() {
        let mut set = channels();
        open(&mut set, 0, 2, 2);
        assert!(set.take_degauss(), "opening 2 moved the air onto it");
        set.select_channel(0, 2);
        assert!(!set.take_degauss(), "nothing changed");
        set.select_channel(0, 1);
        assert!(set.take_degauss());
        set.select_channel(0, 42);
        assert!(!set.take_degauss(), "a dark slot is not selectable");
    }

    #[test]
    fn a_store_holds_the_tube_steady_and_blinks_the_lamp_instead() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let _ = set.take_degauss();
        assert!(set.move_current_to(0, 7));
        assert_eq!(set.current_channel(), 7);
        assert_eq!(
            set.current().unwrap().session,
            22,
            "the session moved with it"
        );
        assert!(!set.take_degauss(), "a store is not a channel change");
        assert_eq!(set.take_stored(), vec![7]);
    }

    #[test]
    fn a_store_onto_an_open_slot_swaps_the_two_sessions() {
        let mut set = channels();
        // Slot 1 carries 101 from `start`; open 2 with a mark of its own.
        let one = set.slot_title(0, 1).map(|_| set.rows()[0].session).unwrap();
        open(&mut set, 0, 2, 22);
        assert!(set.move_current_to(0, 1));
        assert_eq!(set.current_channel(), 1);
        assert_eq!(set.current().unwrap().session, 22);
        let displaced = set.rows().iter().find(|r| r.channel == 2).unwrap();
        assert_eq!(displaced.session, one);
        // Both panels acknowledge: the one stored onto and the one displaced.
        assert_eq!(set.take_stored(), vec![1, 2]);
    }

    #[test]
    fn cycling_walks_the_open_slots_of_the_current_bank_and_wraps() {
        let mut set = channels();
        open(&mut set, 0, 4, 4);
        open(&mut set, 0, 9, 9);
        set.select_channel(0, 1);
        set.cycle_open(1);
        assert_eq!(set.current_channel(), 4);
        set.cycle_open(1);
        assert_eq!(set.current_channel(), 9);
        set.cycle_open(1);
        assert_eq!(set.current_channel(), 1, "wraps");
        set.cycle_open(-1);
        assert_eq!(set.current_channel(), 9, "and backwards");
    }

    #[test]
    fn the_last_channel_closing_switches_the_appliance_off() {
        let mut set = channels();
        assert_eq!(set.close_channel(0, 1), Close::CloseWindow);
        assert_eq!(
            set.len(),
            1,
            "the row stays; the window is the surface's to close"
        );
        open(&mut set, 0, 2, 2);
        assert_eq!(set.close_channel(0, 2), Close::Removed);
        assert_eq!(set.session_died(0, 1), Close::CloseWindow);
    }

    #[test]
    fn a_title_is_trimmed_and_read_back_off_the_current_pair() {
        let mut set = channels();
        assert_eq!(set.current_title(), None);
        set.set_title(0, 1, "  ~/src  ");
        assert_eq!(set.current_title(), Some("~/src"));
        set.set_title(0, 1, "   ");
        assert_eq!(set.current_title(), None, "an empty title is the fallback");
    }

    #[test]
    fn view_pages_unroll_every_bank_far_enough_to_reach_its_next_free_slot() {
        let mut set = channels();
        // One channel: one page, which still has to reach slot 2.
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, count: 1 }]);
        open(&mut set, 0, 4, 4);
        // Slots 1..4 open, next free 2, so four rows fit one page of four.
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, count: 1 }]);
        open(&mut set, 0, 5, 5);
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, count: 2 }]);
    }

    #[test]
    fn the_prefix_predicate_waits_only_on_digits_that_can_still_land() {
        let mut set = channels();
        open(&mut set, 0, 12, 12);
        // With 1 and 12 open on a page of 20 rooted at 0: "1" is a strict
        // prefix of "12", so the chord waits.
        assert!(set.page_slot_prefix_exists(0, "1", 0, 20));
        // "12" is nobody's strict prefix, so it commits at once.
        assert!(!set.page_slot_prefix_exists(0, "12", 0, 20));
        // Nothing beginning with 3 is open.
        assert!(!set.page_slot_prefix_exists(0, "3", 0, 20));
        // ...and a page of ten rows cannot reach slot 12 at all.
        assert!(!set.page_slot_prefix_exists(0, "1", 0, 10));
    }

    // ---- the tmux transitions, without a gateway ---------------------

    #[test]
    fn attaching_transports_the_channel_to_slot_one_of_a_new_bank() {
        let mut set = channels();
        open(&mut set, 0, 3, 33);
        let _ = set.take_degauss();
        let bank = set.attach(0, 3, "prime").unwrap();
        assert_eq!((set.current_bank(), set.current_channel()), (bank, 1));
        assert_eq!(set.current().unwrap().session, 33, "the same session");
        assert!(set.is_gateway(set.current().unwrap()));
        assert_eq!(set.current_title(), Some("tmux -CC # @prime"));
        assert!(
            !set.take_degauss(),
            "transport is a renumbering, not a channel change"
        );
        // Home slot 3 is held dark behind it: nothing may take it.
        assert!(set.slot_title(0, 3).is_none());
        assert!(!open(&mut set, 0, 3, 99));
        assert_eq!(set.first_free(0), 2);
    }

    #[test]
    fn an_attachments_windows_fill_from_slot_two_and_the_first_takes_the_air() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        assert!(window(&mut set, bank, 1, "vim", 1));
        assert_eq!(
            (set.current_bank(), set.current_channel()),
            (bank, 2),
            "an attach's first window greets the user"
        );
        // The ones tmux volunteers after it line up without taking the air.
        assert!(window(&mut set, bank, 2, "logs", 2));
        assert_eq!(set.current_channel(), 2);
        assert_eq!(set.slot_title(bank, 3), Some("logs"));
        // ...but one this set asked for takes it outright.
        assert_eq!(set.new_tmux_window(), Some(bank));
        assert!(window(&mut set, bank, 3, "asked", 3));
        assert_eq!(set.current_channel(), 4);
    }

    /// The greeting is the attach's one-off, not a property of slot 2. Written
    /// as `channel == 2` it fired again every time slot 2 fell vacant, so a
    /// window another client volunteered took the air off a gateway the user
    /// was reading.
    #[test]
    fn a_volunteered_window_landing_back_on_slot_two_does_not_take_the_air() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        // The greeting, spent.
        assert!(window(&mut set, bank, 1, "vim", 1));
        assert_eq!(set.current_channel(), 2);

        // The user closes it and lands back on the gateway, leaving 2 free.
        set.window_closed(bank, &WindowId::parse("@1").unwrap());
        set.select_channel(bank, 1);
        assert_eq!(set.current_channel(), 1);
        assert_eq!(set.first_free(bank), 2);

        // Another client runs `tmux new-window`: this set never asked, so the
        // window lines up behind the gateway and the gateway keeps the air.
        assert!(window(&mut set, bank, 2, "volunteered", 2));
        assert_eq!(set.slot_title(bank, 2), Some("volunteered"));
        assert_eq!(
            set.current_channel(),
            1,
            "a volunteered window does not yank the user off the gateway"
        );

        // A window this set *did* ask for still takes it outright.
        assert_eq!(set.new_tmux_window(), Some(bank));
        assert!(window(&mut set, bank, 3, "asked", 3));
        assert_eq!(set.current_channel(), 3);
    }

    /// Leaving a bank and returning finds the channel it was showing.
    #[test]
    fn a_bank_comes_back_to_the_channel_it_was_left_on() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = spawned(&mut set, "prime", 44);
        window(&mut set, bank, 1, "vim", 1);
        window(&mut set, bank, 2, "logs", 2);
        assert_eq!(set.on_air(), (0, 2), "the found bank took no air");

        // A bank that has never been on the air comes up on its first slot,
        // which on an attachment is the gateway.
        assert!(set.select_bank(bank));
        assert_eq!(set.on_air(), (bank, 1));
        set.select_channel(bank, 3);

        // Home is where it was left, and so is the attachment on the way back.
        assert!(set.select_bank(0));
        assert_eq!(set.on_air(), (0, 2));
        assert!(set.select_bank(bank));
        assert_eq!(set.on_air(), (bank, 3));

        // A bank this set does not hold is nowhere to land.
        assert!(!set.select_bank(77));
        assert_eq!(set.on_air(), (bank, 3));
    }

    /// The remembered slot died while the bank was off the air: the return
    /// lands on what the bank still has.
    #[test]
    fn a_bank_whose_remembered_slot_went_dark_comes_back_on_its_first_open_row() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        open(&mut set, 0, 3, 33);
        set.select_channel(0, 2);
        let bank = spawned(&mut set, "prime", 44);
        window(&mut set, bank, 1, "vim", 1);
        assert!(set.select_bank(bank));

        // Home's slot 2, the one it was left on, exits while another bank has
        // the air, and slot 1 with it.
        assert_eq!(set.close_channel(0, 2), Close::Removed);
        assert_eq!(set.close_channel(0, 1), Close::Removed);
        assert_eq!(set.on_air(), (bank, 1), "neither close moved the air");

        assert!(set.select_bank(0));
        assert_eq!(set.on_air(), (0, 3), "the first row home still holds");
    }

    /// The first-run path: the only channel there was transported, so home
    /// holds nothing but the dark berth, which takes the air and then the row.
    #[test]
    fn a_bank_of_nothing_but_a_held_berth_takes_the_air_and_then_the_berth() {
        let mut set = channels();
        set.attach(0, 1, "prime").unwrap();
        assert!(set.select_bank(0), "home takes the air with no row on it");
        assert_eq!(set.on_air(), (0, 1));
        assert!(set.current().is_none(), "and nothing to put on the glass");
        assert_eq!(set.first_free(0), 1, "the berth of a dark bank is free");
        assert!(open(&mut set, 0, 1, 77));
        assert_eq!(set.on_air(), (0, 1));
    }

    /// The collision that opens: the berth is occupied by the time the gateway
    /// comes home, so it lands beside it.
    #[test]
    fn a_gateway_whose_berth_was_taken_comes_home_to_the_next_free_slot() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        set.select_bank(0);
        assert!(open(&mut set, 0, 1, 77), "the dark berth opened");
        assert_eq!(set.collapse_bank(bank), Some((0, 2)));
        assert_eq!(set.on_air(), (0, 2));
        assert_eq!(set.len(), 2, "both rows stand");
    }

    #[test]
    fn a_collapse_brings_the_gateway_home_to_the_slot_it_never_gave_up() {
        let mut set = channels();
        open(&mut set, 0, 5, 55);
        let bank = set.attach(0, 5, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        assert_eq!(set.collapse_bank(bank), Some((0, 5)));
        assert_eq!((set.current_bank(), set.current_channel()), (0, 5));
        assert_eq!(set.current().unwrap().session, 55);
        assert!(!set.is_gateway(set.current().unwrap()));
        assert_eq!(set.len(), 2, "the window went with the bank");
        assert_eq!(set.banks().len(), 1);
    }

    #[test]
    fn a_tmux_typed_on_an_ssh_channel_transports_and_comes_home_to_its_bank() {
        let mut set = channels();
        let ssh = set
            .open_ssh_bank("overseer", "vault", 22, || Some(500))
            .unwrap();
        assert!(set.open_ssh_channel(ssh, || Some(501)));

        // Control mode detected on the connection's slot 2: the row
        // transports to a tmux bank holding (ssh, 2) dark behind it.
        let tmux = set.attach(ssh, 2, "remote").unwrap();
        assert_eq!((set.current_bank(), set.current_channel()), (tmux, 1));
        assert_eq!(
            set.first_free(ssh),
            3,
            "the transported slot is held, not free"
        );
        window(&mut set, tmux, 1, "vim", 1);

        // Detach: the gateway row comes home to the SSH bank's slot 2.
        assert_eq!(set.collapse_bank(tmux), Some((ssh, 2)));
        assert_eq!((set.current_bank(), set.current_channel()), (ssh, 2));
        assert_eq!(set.current().unwrap().session, 501);
        assert_eq!(set.first_free(ssh), 3, "slots 1 and 2 stand again");
    }

    #[test]
    fn a_gateway_dying_over_ssh_sweeps_its_returned_row_and_an_emptied_bank() {
        let mut set = channels();
        let ssh = set
            .open_ssh_bank("overseer", "vault", 22, || Some(500))
            .unwrap();
        let tmux = set.attach(ssh, 1, "remote").unwrap();
        window(&mut set, tmux, 1, "vim", 1);
        // The client died: the bank collapses, the returned row is removed,
        // and the SSH bank, emptied, goes with it.
        set.session_died(tmux, 1);
        assert!(set.rows().iter().all(|r| r.bank == 0));
        assert_eq!(set.banks().len(), 1, "only home stands");
    }

    #[test]
    fn a_gateways_death_collapses_the_bank_and_takes_the_returned_row_with_it() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = set.attach(0, 2, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        assert_eq!(set.session_died(bank, 1), Close::Removed);
        // Only home's slot 1 survives.
        assert_eq!(set.len(), 1);
        assert_eq!((set.current_bank(), set.current_channel()), (0, 1));
        assert_eq!(set.banks().len(), 1);
    }

    #[test]
    fn a_store_never_crosses_banks_and_a_gateway_never_moves() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        // The air is on slot 2 of the attachment; a store aimed at home does
        // nothing at all.
        assert!(!set.move_current_to(0, 4));
        // The gateway refuses to be stored onto...
        assert!(!set.move_current_to(bank, 1));
        // ...and refuses to move when it holds the air.
        set.select_channel(bank, 1);
        assert!(!set.move_current_to(bank, 5));
    }

    #[test]
    fn a_close_of_a_window_row_is_tmuxs_to_do_and_the_row_stays_until_it_says() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        window(&mut set, bank, 7, "vim", 1);
        let seven = WindowId::parse("@7").unwrap();
        assert_eq!(
            set.close_channel(bank, 2),
            Close::KillWindow {
                bank,
                window: seven.clone()
            }
        );
        assert_eq!(set.len(), 2, "the row waits for %window-close");
        set.window_closed(bank, &seven);
        assert_eq!(set.len(), 1);
        // And a gateway's close is a detach.
        assert_eq!(set.close_channel(bank, 1), Close::Detach(bank));
    }

    #[test]
    fn ctrl_shift_t_acts_on_the_bank_on_view() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        // The air is on an attachment, but the pager has stepped back to home:
        // the new channel is a PTY shell, not a tmux window, and home being
        // dark it takes 01.
        assert_eq!(set.new_channel(0, || Some(7)), None);
        assert_eq!(set.slot_title(0, 1), Some(""));
        // Viewing the attachment, the same key asks its gateway instead.
        assert_eq!(set.new_channel(bank, || Some(8)), Some(bank));
        assert!(matches!(
            set.manager_of(bank),
            Some(Manager::Tmux {
                new_window_pending: true,
                ..
            })
        ));
    }

    /// A bank the terminal raised for a session it found.
    fn spawned(set: &mut Channels<u32>, host: &str, mark: u32) -> BankId {
        set.attach_spawned(host, || Some(mark)).expect("a bank")
    }

    #[test]
    fn a_bank_raised_for_a_found_session_takes_no_home_slot_and_no_air() {
        let mut set = channels();
        let free = set.first_free(0);
        let _ = set.take_degauss();
        let bank = spawned(&mut set, "prime", 44);
        assert_eq!(set.first_free(0), free, "nothing is held at home for it");
        assert_eq!(
            (set.current_bank(), set.current_channel()),
            (0, 1),
            "the user asked for none of it, so the air stays where it was"
        );
        assert!(!set.take_degauss(), "and the tube holds steady");
        let row = set.rows().iter().find(|r| r.bank == bank).unwrap();
        assert_eq!((row.channel, row.session), (1, 44));
        assert!(set.is_gateway(row));
        assert_eq!(set.slot_title(bank, 1), Some("tmux -CC # @prime"));
        // Its windows fill from 2, behind the gateway, like any bank's.
        assert!(window(&mut set, bank, 1, "vim", 1));
        assert_eq!(set.slot_title(bank, 2), Some("vim"));
    }

    #[test]
    fn collapsing_a_bank_raised_for_a_found_session_takes_nothing_home() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = spawned(&mut set, "prime", 44);
        window(&mut set, bank, 1, "vim", 1);
        set.select_channel(bank, 2);
        let _ = set.take_degauss();
        assert_eq!(set.collapse_bank(bank), None, "no home slot came back");
        assert_eq!(set.banks().len(), 1);
        assert_eq!(set.len(), 2, "home kept its own rows and gained none");
        assert_eq!(
            (set.current_bank(), set.current_channel()),
            (0, 2),
            "the air fell to the nearest surviving row"
        );
        assert_eq!(set.first_free(0), 3);
    }

    #[test]
    fn a_bank_raised_for_a_found_session_never_switches_the_set_off() {
        let mut set = channels();
        let bank = spawned(&mut set, "prime", 44);
        window(&mut set, bank, 1, "vim", 1);
        // Home's one channel is the last channel, whatever stands beside it.
        assert_eq!(set.close_channel(0, 1), Close::CloseWindow);
        // And that bank's gateway dying takes only that bank.
        assert_eq!(set.session_died(bank, 1), Close::Removed);
        assert_eq!(set.banks().len(), 1);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn the_greeting_is_owed_once_by_each_bank_and_not_once_by_the_set() {
        let mut set = channels();
        let typed = set.attach(0, 1, "prime").unwrap();
        let found = spawned(&mut set, "prime", 44);
        // The found bank's first window arrives while the air is on another
        // bank's gateway, so it greets nobody.
        assert!(window(&mut set, found, 1, "vim", 1));
        assert_eq!((set.current_bank(), set.current_channel()), (typed, 1));
        // The typed attachment's own first window still greets: the flag is
        // the bank's, not the set's.
        assert!(window(&mut set, typed, 2, "logs", 2));
        assert_eq!((set.current_bank(), set.current_channel()), (typed, 2));
    }

    #[test]
    fn the_tmux_host_can_resolve_after_the_handshake() {
        let mut set = channels();
        let bank = set.attach(0, 1, "").unwrap();
        assert_eq!(set.current_title(), Some("tmux -CC # @"));
        set.host_changed(bank, "prime");
        assert_eq!(set.current_title(), Some("tmux -CC # @prime"));
    }
}
