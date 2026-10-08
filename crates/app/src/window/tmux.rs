//! The tmux control-mode plumbing: everything between a gateway's wire and
//! the channel model.
//!
//! It all runs synchronously, on every pump. By the time [`TerminalSurface`]
//! has attached, the model is already wired to the gateway whose bootstrap is
//! in flight, so no late attachment can race it.
//!
//! The protocol is [`crate::tmux::Gateway`]'s and the slot arithmetic is
//! [`crate::channels`]'s; what is here is the wiring between them.
//!
//! Fields touched: `banks`, whose record holds each attachment's client half;
//! `channels`, which the gateway's events move; `sessions`, the register of
//! every session seen on every server, keyed by socket, server pid and id;
//! and `viewport` and `session_config`, which size and configure the clients
//! this surface starts for sessions it finds.

use std::time::{Duration, Instant};

use term::{ChannelSession, TmuxPane};
use tmux_cc::{PaneId, SessionId};

use crate::channels::{BankId, Manager};
use crate::tmux::{server_is_local, tmux_binary, Gateway, GatewayEvent};

use super::{spawn, TerminalSurface};

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
        // Detection: a PTY channel's program entered control mode. The
        // channel transports to a new bank and its PTY becomes the
        // attachment's wire.
        let mut detected: Vec<(BankId, u32)> = Vec::new();
        for row in self.channels.rows_mut() {
            if let Some(tap) = row.session.tap_mut() {
                if tap.take_detected() {
                    detected.push((row.bank, row.channel));
                }
            }
        }
        for (bank, channel) in detected {
            // A gateway row on a standing bank is a client this surface started.
            let banked = self
                .banks
                .get(&bank)
                .is_some_and(|runtime| runtime.gateway.is_some());
            if channel == 1 && !banked && self.is_tmux(bank) {
                if !self.start_gateway(bank) {
                    self.collapse_bank(bank, false);
                }
            } else {
                self.attach(bank, channel);
            }
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

    /// One detection on a home channel: raise the bank over it, then the gateway.
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

    /// Dup a bank's gateway wire, raise the client half, and tell every
    /// attachment the glass's grid (the client-size law, `crate::channels`).
    fn start_gateway(&mut self, bank: BankId) -> bool {
        let writer = self
            .channels
            .rows_mut()
            .find(|r| r.bank == bank && r.channel == 1)
            .and_then(|r| r.session.control_writer());
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
                log::error!("tmux: no wire for the attachment: the slot has none");
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
            up += usize::from(matches!(b.manager, Manager::Tmux { home: None, .. }));
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
            // Host name empty until the client's own bootstrap names it.
            let Some(raised) = self.channels.attach_spawned("", || spawn(&config, size)) else {
                log::warn!("tmux: no client for session {name} on {socket}");
                continue;
            };
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
        // gateway's own tap.
        let drained = self
            .channels
            .rows_mut()
            .find(|r| r.bank == bank && r.channel == 1)
            .and_then(|r| r.session.tap_mut())
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
        // they reach here, by `key_input` and `write`: a gateway's are
        // swallowed, a window's land in its `TmuxPane`.
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
                GatewayEvent::WindowAdded { window, pane, name } => {
                    let size = self.viewport.term_size();
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
                        gateway.attach_window(&window, &pane);
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

    /// Detach or gateway death: the model collapses the bank
    /// (`channels::Channels::collapse_bank`), and a protocol lost without an
    /// `ST` also forces the gateway's parsers out of the envelope no one will
    /// ever close (`term::Session::leave_control_mode`).
    fn collapse_bank(&mut self, bank: BankId, lost_protocol: bool) {
        let home = self.channels.collapse_bank(bank);
        self.forget_bank(bank, false);
        // A bank that never held a home slot left no row to tell.
        if lost_protocol {
            if let Some((home_bank, home_slot)) = home {
                let row = self
                    .channels
                    .rows_mut()
                    .find(|r| r.bank == home_bank && r.channel == home_slot);
                if let Some(row) = row {
                    row.session.leave_control_mode();
                }
            }
        }
        log::info!("tmux: bank {bank} collapsed (protocol lost: {lost_protocol})");
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

    /// The gateway's keyboard, which is the whole of what an attached
    /// channel does with typed input.
    ///
    /// Every key is accepted and dropped, and the bare Enter is turned into
    /// the empty line tmux's control mode reads as "detach". The glass under
    /// it is still a picture to read and copy from; it is no longer a
    /// surface to type at, because that pty *is* the protocol's wire.
    ///
    /// **The one subtlety, and it is protocol hygiene.** Writing the `\r`
    /// onto the wire directly would only work if a stray reply could be
    /// discarded as unsolicited; this build's codec instead pairs replies by
    /// command id off its own send queue, so a line it did not send is a
    /// block it cannot attribute. The detach therefore goes out as
    /// [`Gateway::detach`]'s `detach-client`, which is the same ask, paired,
    /// and answered by the same `%exit` coming back up the same wire to
    /// collapse the bank. Nothing reaches the gateway's pty except
    /// through the codec (`crate::tmux`).
    ///
    /// winit folds the two Enter keys (the main one and the keypad's) into
    /// one `NamedKey::Enter` told apart only by `KeyEvent::location`, so
    /// matching the named key covers both.
    ///
    /// Answers whether the key was the gateway's, which on a gateway is
    /// always.
    pub(super) fn gateway_key(&mut self, logical: &winit::keyboard::Key) -> bool {
        use winit::keyboard::{Key, NamedKey};

        if !self.is_gateway_on_air() {
            return false;
        }
        let bank = self.channels.current_bank();
        if matches!(logical, Key::Named(NamedKey::Enter)) {
            match self.gateway_mut(bank) {
                Some(gateway) => gateway.detach(),
                // The row is a gateway with no client standing only between a
                // teardown and the collapse that follows it; the bank is going
                // home already.
                None => log::debug!("the gateway's Enter found no client on bank {bank}"),
            }
        }
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
