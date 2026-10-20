//! The tmux control-mode plumbing: everything between a gateway's wire and
//! the channel model.
//!
//! It all runs synchronously, on every pump. By the time [`TerminalSurface`]
//! has attached, the model is already wired to the gateway whose bootstrap is
//! queued, so no late attachment can race it.
//!
//! The protocol is [`crate::tmux::Gateway`]'s and the slot arithmetic is
//! [`crate::channels`]'s; what is here is the wiring between them.
//!
//! Fields touched: `banks`, whose record holds each attachment's client half
//! and, on a bank raised for a session found on a server, its wire;
//! `channels`, which the gateway's events move; `sessions`, the register of
//! every session seen on every server, keyed by socket, server pid and id;
//! and `viewport` and `session_config`, which size and configure the clients
//! this surface starts for sessions it finds.

use std::time::{Duration, Instant};

use term::{ChannelSession, TermSize, TmuxPane};
use tmux_cc::{PaneId, SessionId};

use crate::channels::{BankId, Manager};
use crate::tmux::{server_is_local, tmux_binary, Gateway, GatewayEvent};

use super::{spawn, AppSession, TerminalSurface};

/// How many banks this surface raises for sessions it finds on a server; a starting value, not a measurement.
const FOUND_BANK_CAP: usize = 8;

/// How long a session whose client died before the protocol opened waits for another; a starting value.
const RESPAWN_BACKOFF: Duration = Duration::from_secs(30);

/// One session on one server: a client in flight, a bank, or a client that died
/// before its envelope opened (which the next listing waits out, not restarts).
pub(super) enum SessionSlot {
    Spawning(BankId),
    Banked(BankId),
    Failed(Instant),
}

impl SessionSlot {
    fn bank(&self) -> Option<BankId> {
        match self {
            SessionSlot::Spawning(b) | SessionSlot::Banked(b) => Some(*b),
            SessionSlot::Failed(_) => None,
        }
    }

    /// Owed a client: only a failure, and only past its backoff.
    fn owed(&self, now: Instant) -> bool {
        match self {
            SessionSlot::Failed(at) => now.duration_since(*at) >= RESPAWN_BACKOFF,
            _ => false,
        }
    }
}

impl TerminalSurface {
    /// Detection, the gateways' turn, and the model transitions their events
    /// ask for.
    /// Answers what [`Self::pump_gateway`] counted, summed over the banks.
    pub(super) fn pump_gateways(&mut self) -> usize {
        // Detection: a channel's program entered control mode, and its PTY
        // becomes the wire of a new bank.
        let mut detected: Vec<(BankId, u32)> = Vec::new();
        for row in self.channels.rows_mut() {
            if row.session.tap_mut().is_some_and(|tap| tap.take_detected()) {
                detected.push((row.bank, row.channel));
            }
        }
        for (bank, channel) in detected {
            self.attach(bank, channel);
        }

        let banks: Vec<BankId> = self
            .banks
            .iter()
            .filter(|(_, runtime)| runtime.gateway.is_some())
            .map(|(bank, _)| *bank)
            .collect();
        let mut visible = 0;
        for bank in banks {
            visible += self.pump_gateway(bank);
        }
        visible
    }

    /// One detection on a channel: raise a bank for it, then the gateway.
    fn attach(&mut self, bank: BankId, channel: u32) {
        // The tmux server's hostname is the bootstrap's to resolve
        // (`host_changed`); the bank opens under the empty name briefly, until
        // it does.
        let Some(raised) = self.channels.attach(bank, channel, "") else {
            log::warn!("control mode detected on a slot that cannot attach ({bank},{channel})");
            return;
        };
        if self.start_gateway(raised) {
            log::info!("tmux: attached; bank {raised} raised over channel {channel}");
        } else {
            self.channels.collapse_bank(raised);
        }
    }

    /// The PTY carrying a bank's wire: its carrier row where a typed
    /// `tmux -CC` raised the bank, else the client this surface started.
    fn wire_mut(&mut self, bank: BankId) -> Option<&mut AppSession> {
        match self.channels.manager_of(bank)?.carrier() {
            Some(pair) => self
                .channels
                .rows_mut()
                .find(|r| (r.bank, r.channel) == pair)
                .map(|r| &mut r.session),
            None => self.banks.get_mut(&bank)?.wire.as_mut(),
        }
    }

    /// The clients this surface started for sessions it found, pumped beside
    /// the rows. One whose envelope opened gets its client half; one whose
    /// PTY closed takes its bank down, and holds off a restart if it closed
    /// before its envelope opened.
    pub(super) fn pump_wires(&mut self) {
        let (mut opened, mut dead) = (Vec::new(), Vec::new());
        for (bank, runtime) in &mut self.banks {
            let Some(wire) = runtime.wire.as_mut() else {
                continue;
            };
            let started = runtime.gateway.is_some();
            if wire.pump().eof {
                dead.push((*bank, started));
            } else if !started && wire.tap_mut().is_some_and(|tap| tap.take_detected()) {
                opened.push(*bank);
            }
        }
        for bank in opened {
            if !self.start_gateway(bank) {
                self.collapse_bank(bank, false);
            }
        }
        for (bank, started) in dead {
            if !started {
                log::warn!("tmux: bank {bank}'s client died before the protocol opened");
                self.forget_bank(bank, true);
            }
            self.collapse_bank(bank, true);
        }
    }

    /// Dup a bank's wire, raise the client half, and tell every attachment
    /// the glass's grid (the client-size law, `crate::channels`).
    fn start_gateway(&mut self, bank: BankId) -> bool {
        let writer = self.wire_mut(bank).and_then(|wire| wire.control_writer());
        match writer {
            Some(Ok(writer)) => {
                self.banks.entry(bank).or_default().gateway = Some(Gateway::new(writer));
                self.set_client_size();
                true
            }
            Some(Err(e)) => {
                log::error!("tmux: no wire for the attachment: {e}");
                false
            }
            None => {
                log::error!("tmux: no wire for the attachment: the bank has none");
                false
            }
        }
    }

    pub(super) fn is_tmux(&self, bank: BankId) -> bool {
        self.channels.manager_of(bank).is_some_and(Manager::is_tmux)
    }

    /// A gateway's session listing: every session on a local server with no bank
    /// gets one, raised here and only here; the lowest bank on a server answers
    /// for it, so two gateways on one server do not both raise the same sessions,
    /// and a typed `tmux -CC` into a banked session is honoured.
    fn bank_sessions(
        &mut self,
        bank: BankId,
        attached: SessionId,
        socket: String,
        pid: u32,
        sessions: Vec<(SessionId, String)>,
    ) {
        self.sessions
            .insert((socket.clone(), pid, attached), SessionSlot::Banked(bank));
        if !server_is_local(&socket, pid) {
            log::info!("tmux: the server on {socket} is not this machine's; its other sessions keep to themselves");
            return;
        }
        let enumerator = self
            .sessions
            .iter()
            .filter(|((s, p, _), _)| *s == socket && *p == pid)
            .filter_map(|(_, slot)| slot.bank())
            .min();
        if enumerator != Some(bank) {
            return;
        }
        let mut up = 0;
        for b in self.channels.banks() {
            up += usize::from(matches!(b.manager, Manager::Tmux { carrier: None, .. }));
        }
        let size = self.viewport.term_size();
        let now = Instant::now();
        for (id, name) in sessions {
            let key = (socket.clone(), pid, id.clone());
            if self.sessions.get(&key).is_some_and(|slot| !slot.owed(now)) {
                continue;
            }
            if up >= FOUND_BANK_CAP {
                log::warn!("tmux: {FOUND_BANK_CAP} banks already stand for sessions found on their servers; {name} gets none");
                return;
            }
            let mut config = self.session_now();
            config.program = Some(tmux_binary(pid));
            let args = ["-S", &socket, "-CC", "attach-session", "-t", id.as_str()];
            config.args = args.iter().map(|a| a.to_string()).collect();
            // Empty on purpose: a client refuses to attach from inside another tmux.
            config.env.push(("TMUX".to_string(), String::new()));
            let Some(wire) = spawn(&config, size) else {
                log::warn!("tmux: no client for session {name} on {socket}");
                continue;
            };
            // Host name empty until the client's own bootstrap names it.
            let raised = self.channels.attach_found("");
            self.banks.entry(raised).or_default().wire = Some(wire);
            log::info!("tmux: bank {raised} raised for session {name} on {socket}");
            self.sessions.insert(key, SessionSlot::Spawning(raised));
            up += 1;
        }
    }

    /// The register follows the banks; a client dead before its envelope opened
    /// leaves a failed entry that holds off a restart for [`RESPAWN_BACKOFF`].
    pub(super) fn forget_bank(&mut self, bank: BankId, failed: bool) {
        if failed {
            for slot in self.sessions.values_mut() {
                if slot.bank() == Some(bank) {
                    *slot = SessionSlot::Failed(Instant::now());
                }
            }
        } else {
            self.sessions.retain(|_, slot| slot.bank() != Some(bank));
        }
    }

    /// One gateway's turn: drain its tap, advance, apply; answers the bytes that
    /// reached the channel on the air (see [`Self::pump`]).
    fn pump_gateway(&mut self, bank: BankId) -> usize {
        let current = self.channels.on_air();
        let mut visible = 0;
        let Some(mut gateway) = self
            .banks
            .get_mut(&bank)
            .and_then(|runtime| runtime.gateway.take())
        else {
            return visible;
        };
        // The peeled envelope body, and whether an `ST` closed it, off the
        // wire's own tap.
        let drained = self
            .wire_mut(bank)
            .and_then(|wire| wire.tap_mut())
            .map(|tap| (tap.take_body(), tap.take_ended()));
        let Some((bytes, ended)) = drained else {
            // No gateway, no wire: the bank collapsed under this client.
            return visible;
        };

        let mut events = gateway.advance(&bytes);
        // `ST` closes the envelope only after an `%exit` line, which the
        // codec has already turned into a detach; an `ST` that finds the
        // gateway still attached ends the attachment here.
        if ended && gateway.attached() {
            events.extend(gateway.control_mode_ended());
        }
        // The pump's own clock: the queued wire goes out, a settled resize
        // with it, and the bootstrap watchdog reads the attachment's pulse --
        // which can end it, so what it says joins the events above.
        events.extend(gateway.poll(Instant::now()));

        // The write side of the keystroke diversion: what the window sessions
        // queued becomes `send-keys`. Which keys get queued is settled before
        // they reach here, by `key_input` and `write`: a window's land in its
        // `TmuxPane`.
        let mut inputs: Vec<(PaneId, Vec<u8>)> = Vec::new();
        for row in self.channels.rows_mut().filter(|r| r.bank == bank) {
            let Some((_, pane)) = row.tmux.clone() else {
                continue;
            };
            if let Some(pane_session) = row.session.tmux_pane_mut() {
                let input = pane_session.take_input();
                if !input.is_empty() {
                    inputs.push((pane, input));
                }
            }
        }
        for (pane, input) in inputs {
            gateway.send_keys(&pane, &input);
        }

        let mut collapse = None;
        for event in events {
            match event {
                GatewayEvent::HostChanged(host) => self.channels.host_changed(bank, &host),
                GatewayEvent::WindowAdded {
                    window,
                    pane,
                    name,
                    columns,
                    rows,
                } => {
                    let size = self.pane_size(columns, rows);
                    let scrollback = self.session_config.scrollback;
                    let clustering = self.grapheme_clustering();
                    let opened =
                        self.channels
                            .open_tmux_window(bank, &window, &pane, &name, || {
                                Some(ChannelSession::TmuxPane(TmuxPane::new(
                                    size, scrollback, clustering,
                                )))
                            });
                    if opened {
                        gateway.attach_window(&window, &pane, scrollback);
                    } else {
                        // A window added with no free slot to take it stays
                        // channelless: tmux keeps it, but nothing here draws
                        // it.
                        log::warn!("tmux: no slot for window {window} on bank {bank}");
                    }
                }
                GatewayEvent::WindowRenamed { window, name } => {
                    let channel = self.channels.channel_of_window(bank, &window);
                    if channel > 0 {
                        self.channels.set_title(bank, channel, &name);
                    }
                }
                GatewayEvent::WindowClosed { window } => {
                    self.channels.window_closed(bank, &window);
                }
                GatewayEvent::PaneResized {
                    window,
                    columns,
                    rows,
                } => {
                    let size = self.pane_size(columns, rows);
                    let row = self.channels.rows_mut().find(|r| {
                        r.bank == bank && r.tmux.as_ref().is_some_and(|(w, _)| *w == window)
                    });
                    if let Some(row) = row {
                        let on_air = (row.bank, row.channel) == current;
                        if let Some(pane_session) = row.session.tmux_pane_mut() {
                            pane_session.resize(size);
                        }
                        // A program that does not repaint on SIGWINCH sends
                        // nothing, and the glass must still show the new grid.
                        if on_air {
                            self.settle_selection_columns();
                            self.output_pending = true;
                        }
                    }
                }
                GatewayEvent::WindowPaneChanged { window, pane } => {
                    // The channel keeps its emulation and scrollback; only
                    // its routing moves. The gateway's fresh capture redraws
                    // the screen through the ordinary output path.
                    for row in self.channels.rows_mut().filter(|r| r.bank == bank) {
                        if let Some((known, showing)) = row.tmux.as_mut() {
                            if *known == window {
                                *showing = pane.clone();
                            }
                        }
                    }
                }
                GatewayEvent::Output { pane, bytes } => {
                    let row = self.channels.rows_mut().find(|r| {
                        r.bank == bank && r.tmux.as_ref().is_some_and(|(_, p)| *p == pane)
                    });
                    if let Some(row) = row {
                        let on_air = (row.bank, row.channel) == current;
                        if let Some(pane_session) = row.session.tmux_pane_mut() {
                            pane_session.feed(&bytes);
                            if on_air {
                                visible += bytes.len();
                            }
                        }
                    }
                }
                GatewayEvent::SessionsSeen {
                    attached,
                    socket,
                    server_pid,
                    sessions,
                } => self.bank_sessions(bank, attached, socket, server_pid, sessions),
                GatewayEvent::Detached { lost_protocol } => collapse = Some(lost_protocol),
            }
        }

        if let Some(lost_protocol) = collapse {
            self.collapse_bank(bank, lost_protocol);
        } else {
            self.banks.entry(bank).or_default().gateway = Some(gateway);
        }
        visible
    }

    /// Detach or a dead wire: a protocol lost without an `ST` first forces
    /// the wire's parser out of the envelope no one will ever close
    /// (`term::Session::leave_control_mode`), then the model collapses the
    /// bank (`channels::Channels::collapse_bank`).
    pub(super) fn collapse_bank(&mut self, bank: BankId, lost_protocol: bool) {
        if lost_protocol {
            if let Some(wire) = self.wire_mut(bank) {
                wire.leave_control_mode();
            }
        }
        self.channels.collapse_bank(bank);
        self.forget_bank(bank, false);
        log::info!("tmux: bank {bank} collapsed (protocol lost: {lost_protocol})");
    }

    /// A pane's screen at the size tmux gave it, in the glass's cell: the
    /// cell is the font's, which tmux knows nothing of.
    fn pane_size(&self, columns: u16, rows: u16) -> TermSize {
        let (width, height) = self.viewport.physical_cell();
        TermSize::new(usize::from(columns), usize::from(rows), width, height)
    }

    /// The client-size law: one client, one geometry, told to every
    /// attachment. A gateway told only while its bank held the air would
    /// keep a stale size that tmux would draw *other* sessions at.
    pub(super) fn set_client_size(&mut self) {
        let size = self.viewport.term_size();
        let columns = size.cols().min(u16::MAX as usize) as u16;
        let rows = size.rows().min(u16::MAX as usize) as u16;
        for runtime in self.banks.values_mut() {
            if let Some(gateway) = runtime.gateway.as_mut() {
                gateway.set_client_size(columns, rows);
            }
        }
    }

    /// `Ctrl+Shift+O`: the tmux window on the air steps to its next pane, as
    /// tmux's own `C-b o` would, and its channel follows when tmux says the
    /// active pane changed. Anywhere else the chord declines and the key
    /// reaches the program.
    pub(super) fn next_pane(&mut self) -> bool {
        let Some((bank, (window, _))) = self
            .channels
            .current()
            .and_then(|row| Some((row.bank, row.tmux.clone()?)))
        else {
            return false;
        };
        let Some(gateway) = self.gateway_mut(bank) else {
            return false;
        };
        gateway.next_pane(&window);
        true
    }

    /// A bank's tmux client, for the keys and the closes that command it.
    pub(super) fn gateway_mut(
        &mut self,
        bank: BankId,
    ) -> Option<&mut Gateway<Box<dyn std::io::Write + Send>>> {
        self.banks
            .get_mut(&bank)
            .and_then(|runtime| runtime.gateway.as_mut())
    }
}
