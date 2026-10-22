//! The channel model: what a window's sessions are numbered, which one is on
//! the air, and what the tube does when that changes.
//!
//! A bank is a visual collection of rows of names: a window name under tmux, a
//! shell's title otherwise. A bank is managed by this program itself, by one
//! `tmux -CC` attachment, or by one SSH connection this program owns, whose
//! channels multiplex over the one wire. An attachment's wire is the pty of
//! the channel `tmux -CC` was typed in, its carrier, which stays on its own
//! bank and takes neither the air nor a key while attached; the windows fill
//! the attachment's bank from slot 1. Each tmux session gets a bank of its
//! own, and so does each SSH connection. Within a bank a slot whose session has exited goes dark and stays dark: no
//! renumbering, and a new channel takes the first free slot above the one on
//! the air (`Channels::next_slot`). Rows beyond what the theme shows keep the
//! pager behaviour (`crate::bank`).
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
//! home. The grid is a request: tmux sizes each window from every client
//! attached to it, and a pane's channel is drawn at the size tmux reports
//! (`crate::tmux::GatewayEvent::PaneResized`). The request goes to every
//! window as well as to the client, because under `window-size latest` a
//! window shared with another client follows whichever was last active on
//! it, and this client's `send-keys` never counts; a per-window size caps the
//! window at the glass whichever client it follows.
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
        /// The hostname the tmux server reported, which names the bank. As
        /// often this machine as another: `tmux -CC` is run here at least as
        /// often as over ssh.
        host: String,
        /// The row whose pty carries the attachment's wire: the channel
        /// `tmux -CC` was typed in, on the home bank or an SSH bank. `None`
        /// on a bank the terminal raised for a session it found, whose wire
        /// the surface holds.
        carrier: Option<(BankId, u32)>,
        /// The slot of a window this bank asked tmux for, still on its way:
        /// asking for a channel puts you in it, so the next window to arrive
        /// lands there and takes the air while the ones tmux volunteers do
        /// not.
        new_window_pending: Option<u32>,
    },
    /// One SSH connection this program owns. Channels are the connection's
    /// own multiplexed channels and fill from slot 1.
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

    /// The row carrying this bank's wire, if a typed `tmux -CC` raised it.
    pub fn carrier(&self) -> Option<(BankId, u32)> {
        match self {
            Manager::Tmux { carrier, .. } => *carrier,
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
    /// its first slot.
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
    /// The numbered rows one of its pages holds.
    pub rows: i32,
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

    /// `Component.onCompleted`: open channel 1 and arm the tube.
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

    /// The tmux bank whose wire the row at `(bank, channel)` carries. Read
    /// off the managers, never stored on the row.
    pub fn carried_bank(&self, bank: BankId, channel: u32) -> Option<BankId> {
        let pair = Some((bank, channel));
        self.banks.iter().find(|b| b.manager.carrier() == pair).map(|b| b.id)
    }

    pub fn is_carrier(&self, bank: BankId, channel: u32) -> bool {
        self.carried_bank(bank, channel).is_some()
    }

    /// What a bank is called: `tmux -CC # @host` for an attachment,
    /// `user@host` for a connection, and nothing for the home bank.
    pub fn bank_identity(&self, bank: BankId) -> Option<String> {
        match self.manager_of(bank)? {
            Manager::Home => None,
            Manager::Tmux { host, .. } => Some(gateway_title(host)),
            Manager::Ssh { user, host, .. } => Some(ssh_title(user, host)),
        }
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

    /// The lowest free slot of a bank. Zero when the bank is full to the cap.
    pub fn first_free(&self, bank: BankId) -> u32 {
        let mut taken = [false; CHANNEL_CAP as usize + 1];
        for row in self.rows.iter().filter(|r| r.bank == bank) {
            if row.channel <= CHANNEL_CAP {
                taken[row.channel as usize] = true;
            }
        }
        (1..=CHANNEL_CAP).find(|n| !taken[*n as usize]).unwrap_or(0)
    }

    /// The slot `Ctrl+Shift+T` takes: the first free one above the slot the
    /// bank is on, so a dark slot left before a group of channels stays dark
    /// while the group past it grows, wrapping to the lowest free one when
    /// nothing above is. A bank standing on a dark slot, at no signal, takes
    /// that slot. Zero when the bank is full.
    pub fn next_slot(&self, bank: BankId) -> u32 {
        let from = self.bank_of(bank).map_or(1, |i| self.banks[i].channel.max(1));
        (from..=CHANNEL_CAP)
            .find(|n| self.row_of(bank, *n).is_none())
            .unwrap_or_else(|| self.first_free(bank))
    }

    pub fn highest_open(&self, bank: BankId) -> u32 {
        self.rows
            .iter()
            .filter(|r| r.bank == bank)
            .map(|r| r.channel)
            .max()
            .unwrap_or(0)
    }

    /// The pager walks one flattened space over every bank: each
    /// unrolls into as many pages as its slots need, every open slot reachable
    /// and so is the slot a new channel will take. A bank
    /// with an identity heads each page with its band, which costs a row.
    pub fn view_pages(&self, rows_visible: i32) -> Vec<ViewPage> {
        self.banks
            .iter()
            .map(|b| {
                let rows = (rows_visible - i32::from(self.bank_identity(b.id).is_some())).max(1);
                let span = self.highest_open(b.id).max(self.next_slot(b.id)).max(1) as i32;
                ViewPage {
                    bank: b.id,
                    rows,
                    // JS `Math.ceil(span / rows)` on two positive integers.
                    count: (span + rows - 1) / rows,
                }
            })
            .collect()
    }

    /// True when `buf` is a strict prefix of some *open* slot of
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
    /// clearing the flag. Turning the knob makes the tube flinch,
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
    /// first open row when that slot has gone dark since; never on a
    /// carrier. False only for a bank this set does not hold: one with no
    /// open row takes the air too, on the slot it remembers or, where a
    /// carrier stands there, its first free one, the strip the user paged to
    /// and the glass having to agree (`window::TerminalSurface::no_signal`
    /// draws the rest).
    pub fn select_bank(&mut self, bank: BankId) -> bool {
        let Some(i) = self.bank_of(bank) else {
            return false;
        };
        let remembered = self.banks[i].channel;
        let open = |c: u32| self.row_of(bank, c).is_some() && !self.is_carrier(bank, c);
        let channel = if open(remembered) {
            remembered
        } else {
            match self.rows.iter().find(|r| r.bank == bank && open(r.channel)) {
                Some(row) => row.channel,
                None if self.is_carrier(bank, remembered) => self.first_free(bank),
                None => remembered,
            }
        };
        self.set_current(bank, channel);
        true
    }

    // ---- opening -----------------------------------------------------

    /// PTY shells live on the home bank alone: an attachment's channels are
    /// tmux's to give. `session` is called only once the slot is known to be
    /// free, so a refused open costs no pty.
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
        if self.row_of(bank, channel).is_some() {
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

    pub fn open_first_free(&mut self, session: impl FnOnce() -> Option<S>) -> bool {
        let slot = self.first_free(0);
        if slot == 0 {
            return false;
        }
        self.open_channel(0, slot, session)
    }

    /// What `Ctrl+Shift+T` asks for. A new channel goes to `view`,
    /// the bank the user is looking at: on home the slot `next_slot` names,
    /// with a shell in it, on an attachment another window of that session,
    /// which is the gateway's to give. Returns the bank whose gateway must be asked, or
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
        if self.ask_for_window(view, self.next_slot(view)) {
            return Some(view);
        }
        let slot = self.next_slot(0);
        if slot > 0 {
            self.open_channel(0, slot, session);
        }
        None
    }

    /// Ask the bank the air is on for another window, by name
    /// rather than by where the pager stands.
    pub fn new_tmux_window(&mut self) -> Option<BankId> {
        let bank = self.current_bank;
        self.ask_for_window(bank, self.next_slot(bank)).then_some(bank)
    }

    /// The bank owes itself a window from its gateway, to land on `slot`.
    /// False where this program manages the bank, there being no gateway to
    /// ask.
    pub fn ask_for_window(&mut self, bank: BankId, slot: u32) -> bool {
        let Some(Manager::Tmux {
            new_window_pending, ..
        }) = self.manager_mut(bank)
        else {
            return false;
        };
        *new_window_pending = Some(slot);
        true
    }

    /// A tmux window becomes an ordinary channel of its bank. Windows the
    /// attach lists and windows tmux volunteers line up on the lowest free
    /// slot without taking the air; a window this set asked for lands on the
    /// slot it was asked for, unless one has filled it since, and takes the
    /// air outright. The first window needs no greeting: the air of a user
    /// standing on a bank with no row is already on slot 1, where that window
    /// lands.
    pub fn open_tmux_window(
        &mut self,
        bank: BankId,
        window: &WindowId,
        pane: &PaneId,
        name: &str,
        session: impl FnOnce() -> Option<S>,
    ) -> bool {
        let Some(Manager::Tmux {
            new_window_pending, ..
        }) = self.manager_mut(bank)
        else {
            return false;
        };
        // Whatever arrives next answers the request, and nothing after it does.
        let asked = new_window_pending.take();
        let channel = match asked {
            Some(slot) if self.row_of(bank, slot).is_none() => slot,
            _ => self.first_free(bank),
        };
        if channel < 1 {
            return false;
        }
        let Some(session) = session() else {
            return false;
        };
        self.insert_row(Row {
            bank,
            channel,
            title: normalize_title(name),
            tmux: Some((window.clone(), pane.clone())),
            session,
        });
        if asked.is_some() {
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

    /// A row whose bank or channel was rewritten in place slides to
    /// where the sort order wants it.
    fn resort(&mut self) {
        self.rows.sort_by_key(|r| (r.bank, r.channel));
    }

    // ---- closing -----------------------------------------------------

    /// What the user's close asks for, by what the row is. A carrier's is
    /// nothing: `Alt+Shift+D` is the way out of an attachment.
    pub fn close_channel(&mut self, bank: BankId, channel: u32) -> Close {
        let Some(index) = self.row_of(bank, channel) else {
            return Close::Nothing;
        };
        if self.is_carrier(bank, channel) {
            return Close::Nothing;
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

    /// A row's own program died, the ordinary end of a channel. A carrier
    /// takes the attachment it carried down first.
    pub fn session_died(&mut self, bank: BankId, channel: u32) -> Close {
        if let Some(carried) = self.carried_bank(bank, channel) {
            self.collapse_bank(carried);
        }
        if self.row_of(bank, channel).is_none() {
            return Close::Nothing;
        }
        if self.anchored_rows() <= 1 {
            return Close::CloseWindow;
        }
        self.remove_row(bank, channel);
        Close::Removed
    }

    /// Home's rows and every SSH channel (an SSH bank stands on the user's
    /// own ask): the last of these going is the last channel going out,
    /// whatever else stands.
    fn anchored_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.bank == 0 || self.manager_of(r.bank).is_some_and(Manager::is_ssh))
            .count()
    }

    /// Where a single row goes. The nearest surviving row of the same bank
    /// takes the air when the removed one had it. An SSH bank whose last row
    /// goes goes with it: there is nothing for an empty one to stand for.
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

    /// The row nearest the hole a removed row left, its own bank's rows
    /// first: the one that slid into its place, else the nearest one before
    /// it, else whatever another bank still holds. Never a carrier.
    fn nearest_row(&self, index: usize, bank: BankId) -> Option<usize> {
        let at = index.min(self.rows.len());
        let order = || {
            (at..self.rows.len())
                .chain((0..at).rev())
                .filter(|i| !self.is_carrier(self.rows[*i].bank, self.rows[*i].channel))
        };
        order().find(|i| self.rows[*i].bank == bank).or_else(|| order().next())
    }

    // ---- selecting and storing ---------------------------------------

    /// A dark slot is not selectable, and neither is a carrier, whose pty is
    /// an attachment's wire.
    pub fn select_channel(&mut self, bank: BankId, channel: u32) -> bool {
        if self.row_of(bank, channel).is_none() || self.is_carrier(bank, channel) {
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
        let from = self.current_channel();
        self.row_of(bank, from).is_some()
            && !self.is_carrier(bank, from)
            && !self.is_carrier(bank, channel)
    }

    /// The session on screen keeps its screen; its slot number
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

    /// Cycling walks the current bank's selectable rows: the other banks'
    /// channels are a pager step away, not a knob turn.
    pub fn cycle_open(&mut self, direction: i32) {
        let slots: Vec<u32> = self
            .rows
            .iter()
            .filter(|r| r.bank == self.current_bank && !self.is_carrier(r.bank, r.channel))
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

    /// A PTY shell's title is its own; on an attachment the model owns it,
    /// and a tmux window's title is tmux's to give.
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

    /// A channel's program has entered tmux control mode: a new bank is
    /// raised for the attachment, and the channel stays where it stands as
    /// its carrier. The air, if it was on the carrier, crosses to the new
    /// bank, which shows no signal until its first window lands. Returns the
    /// new bank's id.
    pub fn attach(&mut self, bank: BankId, channel: u32, host: &str) -> Option<BankId> {
        self.row_of(bank, channel)?;
        if self.manager_of(bank).is_some_and(Manager::is_tmux) || self.is_carrier(bank, channel) {
            return None;
        }
        let id = self.push_bank(host, Some((bank, channel)));
        if self.on_air() == (bank, channel) {
            self.set_current(id, 1);
        }
        Some(id)
    }

    /// A bank the terminal raised for a session it found: no carrier, no
    /// row, and nothing takes the air.
    pub fn attach_found(&mut self, host: &str) -> BankId {
        self.push_bank(host, None)
    }

    /// A bank for an SSH connection the user asked for: its first channel
    /// takes slot 1 and the air with it. The ask is the user's, so the
    /// arrival is a channel change like any other.
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
            title: ssh_title(user, host),
            tmux: None,
            session,
        });
        self.set_current(id, 1);
        Some(id)
    }

    /// Another channel on an SSH bank: the connection's next multiplexed
    /// channel, on `channel`, taking the air the way the asked window of a
    /// tmux bank does. False when the bank is not an SSH one or the slot is
    /// not a free one.
    pub fn open_ssh_channel(
        &mut self,
        bank: BankId,
        channel: u32,
        make: impl FnOnce() -> Option<S>,
    ) -> bool {
        let Some(Manager::Ssh { host, user, .. }) = self.manager_of(bank) else {
            return false;
        };
        let title = ssh_title(user, host);
        if !(1..=CHANNEL_CAP).contains(&channel) || self.row_of(bank, channel).is_some() {
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

    /// A new bank under a tmux attachment, with no row on it yet.
    fn push_bank(&mut self, host: &str, carrier: Option<(BankId, u32)>) -> BankId {
        let id = self.next_bank_id;
        self.next_bank_id += 1;
        self.banks.push(Bank {
            id,
            manager: Manager::Tmux {
                host: host.to_string(),
                carrier,
                new_window_pending: None,
            },
            channel: 1,
        });
        id
    }

    /// Detach or a dead wire: the bank goes with its window rows. A user
    /// standing on it lands on the carrier where one stands, else on the
    /// nearest surviving row; anywhere else the air stays put. Answers the
    /// carrier's pair.
    pub fn collapse_bank(&mut self, bank: BankId) -> Option<(BankId, u32)> {
        let i = self.bank_of(bank)?;
        let carrier = self.banks[i].manager.carrier();
        let at = self.rows.iter().position(|r| r.bank == bank).unwrap_or(0);
        let was_here = self.current_bank == bank;
        self.rows.retain(|r| r.bank != bank);
        self.banks.remove(i);
        if was_here {
            let landing = carrier
                .filter(|(b, c)| self.row_of(*b, *c).is_some())
                .or_else(|| self.nearest_row(at, 0).map(|n| (self.rows[n].bank, self.rows[n].channel)));
            if let Some((b, c)) = landing {
                self.set_current(b, c);
            }
        }
        carrier
    }

    /// The tmux server's hostname can resolve after the handshake: the
    /// bank's identity follows it.
    pub fn host_changed(&mut self, bank: BankId, host: &str) {
        if let Some(Manager::Tmux { host: known, .. }) = self.manager_mut(bank) {
            *known = host.to_string();
        }
    }

    pub fn channel_of_window(&self, bank: BankId, window: &WindowId) -> u32 {
        self.rows
            .iter()
            .find(|r| r.bank == bank && r.tmux.as_ref().is_some_and(|(w, _)| w == window))
            .map(|r| r.channel)
            .unwrap_or(0)
    }

    /// tmux says a window closed; its row goes.
    pub fn window_closed(&mut self, bank: BankId, window: &WindowId) {
        let channel = self.channel_of_window(bank, window);
        if channel > 0 {
            self.remove_row(bank, channel);
        }
    }
}

fn normalize_title(raw: &str) -> String {
    raw.trim().to_string()
}

/// What a tmux bank is called.
fn gateway_title(host: &str) -> String {
    format!("tmux -CC # @{host}")
}

/// What an SSH bank and its channels are called.
fn ssh_title(user: &str, host: &str) -> String {
    format!("{user}@{host}")
}

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
    fn a_new_channel_takes_the_first_free_slot_above_the_one_on_view() {
        let mut set = channels();
        open(&mut set, 0, 2, 2);
        open(&mut set, 0, 5, 5);
        // From 5, slot 3 stays the dark gap it was left as.
        assert_eq!(set.new_channel(0, || Some(6)), None);
        assert_eq!(set.current_channel(), 6);
        // From 2 the first free slot above is the gap.
        set.select_channel(0, 2);
        assert_eq!(set.new_channel(0, || Some(3)), None);
        assert_eq!(set.current_channel(), 3);
        // Nothing free above wraps to the lowest free slot.
        open(&mut set, 0, CHANNEL_CAP, 99);
        assert_eq!(set.next_slot(0), 4);
    }

    #[test]
    fn an_ssh_channel_lands_on_the_slot_asked_and_refuses_a_taken_one() {
        let mut set = channels();
        let ssh = set
            .open_ssh_bank("overseer", "vault", 22, || Some(500))
            .unwrap();
        assert!(!set.open_ssh_channel(ssh, 1, || Some(501)));
        assert!(set.open_ssh_channel(ssh, 4, || Some(504)));
        assert_eq!(set.on_air(), (ssh, 4));
        assert_eq!(set.slot_title(ssh, 2), None);
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
    fn view_pages_unroll_every_bank_far_enough_to_reach_its_next_slot() {
        let mut set = channels();
        // One channel: one page, which still has to reach slot 2.
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, rows: 4, count: 1 }]);
        open(&mut set, 0, 4, 4);
        // Slots 1 and 4 open with 4 on the air: the next channel takes 5,
        // which a second page has to reach.
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, rows: 4, count: 2 }]);
        // From slot 1 the next is 2, and one page of four holds it all.
        set.select_channel(0, 1);
        assert_eq!(set.view_pages(4), vec![ViewPage { bank: 0, rows: 4, count: 1 }]);
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
    fn attaching_raises_a_bank_and_leaves_the_carrier_where_it_stands() {
        let mut set = channels();
        open(&mut set, 0, 3, 33);
        set.set_title(0, 3, "~/src");
        let bank = set.attach(0, 3, "prime").unwrap();
        let row = set.rows().iter().find(|r| r.channel == 3).unwrap();
        assert_eq!((row.bank, row.session, row.title.as_str()), (0, 33, "~/src"));
        assert!(set.is_carrier(0, 3));
        assert!(set.rows().iter().all(|r| r.bank == 0), "the new bank holds no row");
        assert_eq!(set.on_air(), (bank, 1), "the air crossed with the user");
        assert!(set.current().is_none(), "and finds no signal there yet");
        assert_eq!(set.first_free(0), 2);
        assert_eq!(set.bank_identity(bank).as_deref(), Some("tmux -CC # @prime"));
    }

    #[test]
    fn an_attachments_windows_fill_from_slot_one_and_only_an_asked_one_takes_the_air() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        assert!(window(&mut set, bank, 1, "vim", 1));
        assert_eq!(set.on_air(), (bank, 1));
        assert_eq!(set.current().unwrap().session, 1, "the first window is on the air");
        // The ones tmux volunteers after it line up without taking the air.
        assert!(window(&mut set, bank, 2, "logs", 2));
        assert_eq!(set.slot_title(bank, 2), Some("logs"));
        assert_eq!(set.current_channel(), 1);
        // A volunteered window landing on a freed slot 1 does not either.
        set.select_channel(bank, 2);
        set.window_closed(bank, &WindowId::parse("@1").unwrap());
        assert!(window(&mut set, bank, 4, "volunteered", 4));
        assert_eq!(set.slot_title(bank, 1), Some("volunteered"));
        assert_eq!(set.current_channel(), 2);
        // ...but one this set asked for takes it outright.
        assert_eq!(set.new_tmux_window(), Some(bank));
        assert!(window(&mut set, bank, 3, "asked", 3));
        assert_eq!(set.current_channel(), 3);
        // A window asked for a dark slot lands on that slot.
        assert!(set.ask_for_window(bank, 7));
        assert!(window(&mut set, bank, 5, "pressed", 5));
        assert_eq!(set.on_air(), (bank, 7));
        assert_eq!(set.slot_title(bank, 7), Some("pressed"));
    }

    /// Leaving a bank and returning finds the channel it was showing.
    #[test]
    fn a_bank_comes_back_to_the_channel_it_was_left_on() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = set.attach_found("prime");
        window(&mut set, bank, 1, "vim", 1);
        window(&mut set, bank, 2, "logs", 2);
        assert_eq!(set.on_air(), (0, 2), "the found bank took no air");

        // A bank that has never been on the air comes up on its first slot.
        assert!(set.select_bank(bank));
        assert_eq!(set.on_air(), (bank, 1));
        set.select_channel(bank, 2);

        // Home is where it was left, and so is the attachment on the way back.
        assert!(set.select_bank(0));
        assert_eq!(set.on_air(), (0, 2));
        assert!(set.select_bank(bank));
        assert_eq!(set.on_air(), (bank, 2));

        // A bank this set does not hold is nowhere to land.
        assert!(!set.select_bank(77));
        assert_eq!(set.on_air(), (bank, 2));
    }

    /// The remembered slot died while the bank was off the air: the return
    /// lands on what the bank still has.
    #[test]
    fn a_bank_whose_remembered_slot_went_dark_comes_back_on_its_first_open_row() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        open(&mut set, 0, 3, 33);
        set.select_channel(0, 2);
        let bank = set.attach_found("prime");
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

    /// The first-run path: the only channel there was carries the wire, so
    /// home takes the air on no row at all.
    #[test]
    fn a_bank_whose_only_row_is_a_carrier_takes_the_air_on_no_row() {
        let mut set = channels();
        set.attach(0, 1, "prime").unwrap();
        assert!(set.select_bank(0));
        assert_eq!(set.on_air(), (0, 2));
        assert!(set.current().is_none(), "and nothing to put on the glass");
        assert!(set.open_first_free(|| Some(77)));
        assert_eq!(set.on_air(), (0, 2));
    }

    #[test]
    fn a_collapse_lands_on_the_carrier_and_lights_it_again() {
        let mut set = channels();
        open(&mut set, 0, 5, 55);
        let bank = set.attach(0, 5, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        assert_eq!(set.collapse_bank(bank), Some((0, 5)));
        assert_eq!(set.on_air(), (0, 5));
        assert_eq!(set.current().unwrap().session, 55);
        assert!(!set.is_carrier(0, 5));
        assert_eq!(set.len(), 2, "the window went with the bank");
        assert_eq!(set.banks().len(), 1);
    }

    #[test]
    fn a_tmux_typed_on_an_ssh_channel_carries_from_its_own_bank() {
        let mut set = channels();
        let ssh = set
            .open_ssh_bank("overseer", "vault", 22, || Some(500))
            .unwrap();
        assert!(set.open_ssh_channel(ssh, 2, || Some(501)));
        assert_eq!(set.bank_identity(ssh).as_deref(), Some("overseer@vault"));
        let tmux = set.attach(ssh, 2, "remote").unwrap();
        assert_eq!(set.on_air(), (tmux, 1));
        assert_eq!(set.first_free(ssh), 3, "the carrier still stands");
        window(&mut set, tmux, 1, "vim", 1);
        assert_eq!(set.collapse_bank(tmux), Some((ssh, 2)));
        assert_eq!(set.on_air(), (ssh, 2));
        assert_eq!(set.current().unwrap().session, 501);
        assert_eq!(set.first_free(ssh), 3);
    }

    #[test]
    fn a_carriers_death_collapses_its_bank_and_goes_dark_like_any_shell() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = set.attach(0, 2, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        assert_eq!(set.session_died(0, 2), Close::Removed);
        assert_eq!(set.len(), 1, "only home's slot 1 survives");
        assert_eq!(set.on_air(), (0, 1));
        assert_eq!(set.banks().len(), 1);

        // Over SSH the emptied connection's bank goes too.
        let ssh = set
            .open_ssh_bank("overseer", "vault", 22, || Some(500))
            .unwrap();
        let tmux = set.attach(ssh, 1, "remote").unwrap();
        window(&mut set, tmux, 1, "vim", 1);
        set.session_died(ssh, 1);
        assert!(set.rows().iter().all(|r| r.bank == 0));
        assert_eq!(set.banks().len(), 1, "only home stands");
    }

    #[test]
    fn a_carrier_is_neither_selected_stored_onto_nor_closed() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = set.attach(0, 1, "prime").unwrap();
        window(&mut set, bank, 1, "vim", 1);
        set.select_channel(bank, 1);
        // A store never crosses banks.
        assert!(!set.move_current_to(0, 4));
        assert!(!set.select_channel(0, 1));
        assert_eq!(set.on_air(), (bank, 1));
        set.select_channel(0, 2);
        assert!(!set.can_store(0, 1));
        assert_eq!(set.close_channel(0, 1), Close::Nothing);
        // The cycle steps over it.
        set.cycle_open(1);
        assert_eq!(set.on_air(), (0, 2));
    }

    #[test]
    fn a_close_of_a_window_row_is_tmuxs_to_do_and_the_row_stays_until_it_says() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        window(&mut set, bank, 7, "vim", 1);
        let seven = WindowId::parse("@7").unwrap();
        assert_eq!(
            set.close_channel(bank, 1),
            Close::KillWindow {
                bank,
                window: seven.clone()
            }
        );
        assert_eq!(set.len(), 2, "the row waits for %window-close");
        set.window_closed(bank, &seven);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn ctrl_shift_t_acts_on_the_bank_on_view() {
        let mut set = channels();
        let bank = set.attach(0, 1, "prime").unwrap();
        // The air is on an attachment, but the pager has stepped back to home:
        // the new channel is a PTY shell, not a tmux window, beside the carrier.
        assert_eq!(set.new_channel(0, || Some(7)), None);
        assert_eq!(set.slot_title(0, 2), Some(""));
        // Viewing the attachment, the same key asks its gateway instead.
        assert_eq!(set.new_channel(bank, || Some(8)), Some(bank));
        assert!(matches!(
            set.manager_of(bank),
            Some(Manager::Tmux {
                new_window_pending: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn a_bank_raised_for_a_found_session_takes_no_home_slot_no_row_and_no_air() {
        let mut set = channels();
        let free = set.first_free(0);
        let _ = set.take_degauss();
        let bank = set.attach_found("prime");
        assert_eq!(set.first_free(0), free, "nothing is held at home for it");
        assert_eq!(set.on_air(), (0, 1), "the user asked for none of it");
        assert!(!set.take_degauss(), "and the tube holds steady");
        assert!(set.rows().iter().all(|r| r.bank == 0));
        assert_eq!(set.bank_identity(bank).as_deref(), Some("tmux -CC # @prime"));
        assert!(window(&mut set, bank, 1, "vim", 1));
        assert_eq!(set.slot_title(bank, 1), Some("vim"));
    }

    #[test]
    fn collapsing_a_bank_raised_for_a_found_session_takes_nothing_home() {
        let mut set = channels();
        open(&mut set, 0, 2, 22);
        let bank = set.attach_found("prime");
        window(&mut set, bank, 1, "vim", 1);
        set.select_channel(bank, 1);
        assert_eq!(set.collapse_bank(bank), None, "no carrier stood for it");
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
        let bank = set.attach_found("prime");
        window(&mut set, bank, 1, "vim", 1);
        // Home's one channel is the last channel, whatever stands beside it.
        assert_eq!(set.close_channel(0, 1), Close::CloseWindow);
        assert_eq!(set.collapse_bank(bank), None);
        assert_eq!(set.banks().len(), 1);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn the_tmux_host_can_resolve_after_the_handshake() {
        let mut set = channels();
        let bank = set.attach(0, 1, "").unwrap();
        assert_eq!(set.bank_identity(bank).as_deref(), Some("tmux -CC # @"));
        set.host_changed(bank, "prime");
        assert_eq!(set.bank_identity(bank).as_deref(), Some("tmux -CC # @prime"));
        assert_eq!(set.bank_identity(0), None);
    }
}
