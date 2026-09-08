//! The tmux control-mode gateway: one attachment's session/window policy.
//!
//! This sits on the seams the earlier waves shaped: the codec (`tmux_cc`)
//! turns lines into typed events and commands into wire text,
//! `term::tmux_cc::ControlModeTap` peels the DCS envelope, and `crate::channels`
//! holds the slot model. What is left, and what this type is, is the policy
//! the codec refuses to carry: which commands to send, what each reply
//! means, the per-pane bootstrap gate, and the window bookkeeping behind the
//! notifications.
//!
//! # Shape
//!
//! One [`Gateway`] per attachment, owning three things:
//!
//! * the codec, and the intent map naming what each in-flight command's reply
//!   is for, since the codec already pairs replies to [`CommandId`]s;
//! * the transport's write side: a handle onto the PTY that carries the
//!   control session (`term::Session::control_mode_writer`), plus the queue of what
//!   that handle would not take yet ([`Gateway::flush`]). Commands go out the
//!   moment policy decides, as far as the transport allows; the read side
//!   stays the session's, arriving here as peeled body bytes through
//!   [`Gateway::advance`];
//! * the window/pane books: known windows and their active panes (the diff
//!   base for `%window-add` re-listings), last-announced names (the rename
//!   sweep's diff base), and each attached pane's bootstrap gate.
//!
//! What it does *not* own: the channel model. Everything the surface must
//! act on comes back as a [`GatewayEvent`], and `window::TerminalSurface`
//! wires those into `channels`' transitions.
//!
//! # Design notes
//!
//! * **No bootstrap timer.** Telling the attach burst's unsolicited blocks
//!   apart from replies without any wire signal would need either a quiet
//!   period assumed safe or a hard wait before the first command goes out.
//!   tmux 3.5a marks the difference on the wire instead, the `%begin` flags
//!   field ([`tmux_cc::Block::solicited`]), so the codec discriminates and
//!   the bootstrap goes out at construction, deterministically.
//!   [`BOOTSTRAP_DEADLINE`] covers the one case that buys nothing from this:
//!   a tmux that flagged nothing solicited would leave every reply on the
//!   unsolicited path and the bootstrap unanswered forever, so a deadline
//!   measured at [`Gateway::poll`] names that shape and gives the attachment
//!   up.
//! * **The transport is queued, not assumed.** The PTY master this writes to
//!   is `O_NONBLOCK` (`teletypewriter` sets it, and the `dup` behind
//!   `control_mode_writer` shares the file-status flags), so a write under
//!   backpressure takes a prefix and refuses the rest. A `write_all` there
//!   drops the tail of a command line and the next command is read joined to
//!   its stump: one command out of two, a reply short, and the pairing off
//!   by one for the rest of the attachment. So commands queue in
//!   [`Gateway::pending`] and [`Gateway::flush`] resumes at the byte the
//!   transport stopped at.
//! * **Names decode.** `%window-renamed` names arrive `vis(3)`-escaped and the
//!   codec unescapes them; reading them verbatim would show `a\\b` for a
//!   window named `a\b` (the escaping defect recorded at D1a).

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use tmux_cc::{Codec, Command, CommandId, Event, Notification, PaneId, SessionId, WindowId};

/// The resize debounce: a drag is a burst of sizes and only the one it
/// settles on is worth a round trip.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

/// How long the bootstrap may go unanswered while the wire is plainly alive
/// before the attachment is given up. Generous on purpose: the two commands
/// go out at construction and a healthy tmux answers them in milliseconds, so
/// anything within this window is a loaded machine and anything past it is a
/// peer that is never going to answer.
const BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(10);

/// How many unsent command bytes may wait on [`Gateway::pending`] before this
/// client stops asking tmux for things.
///
/// The queue exists because the transport is `O_NONBLOCK` and a refused write
/// must not lose the tail (see [`Gateway::flush`]); it is not a place to store
/// an unbounded amount of the user's typing. `send_keys` encodes every byte as
/// hex, so the queue grows at better than twice the rate the user produces, and
/// a tmux that has stopped reading its end never takes any of it back:
/// unbounded, that is the process growing until the OOM killer arrives, the
/// read side having had [`BOOTSTRAP_DEADLINE`] all along. A megabyte is far
/// past any real backlog -- a healthy peer drains this every pump.
const PENDING_CAP: usize = 1 << 20;

/// How many `%output` bytes one pane may bank between its capture and its
/// cursor before the mount gives up on placing the cursor exactly.
///
/// The backlog is a round trip long, so a pane running `yes` fills it fast.
/// Two things go wrong unbounded: the burst is held in memory whole, and a
/// cursor reply that never arrives leaves the pane un-bootstrapped forever,
/// showing nothing and banking everything, where the watchdog cannot see it
/// (the bootstrap replies have already set `solicited`).
///
/// Reaching the cap ends the wait rather than dropping the bytes: what is
/// banked goes to the screen and the pane runs live from there. The cursor
/// lands where the output left it rather than where tmux reported it, as it
/// does for any pane whose cursor reply errors, which is much the better half
/// of the trade against a pane that shows nothing at all.
const BACKLOG_CAP: usize = 1 << 20;

/// What the body of a command's reply block is for, carried per command id.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    /// The reply only confirms the command went through.
    Ignore,
    Host,
    ListPanes,
    /// Every session on the server, for the surface to bank.
    ListSessions,
    /// The rename sweep that closes the attach burst.
    WindowNames,
    /// A pane's scrollback and screen.
    Capture(PaneId),
    /// That pane's cursor position.
    Cursor(PaneId),
    /// One window's panes, after a `%layout-change`.
    LayoutPanes(WindowId),
}

/// The per-pane bootstrap gate. Until the
/// capture comes back the pane's `%output` is dropped: the capture is about
/// to draw those same bytes. Between the capture and the cursor it is kept,
/// because it is genuinely new, and it goes in once the cursor is placed.
#[derive(Default)]
struct PaneGate {
    captured: bool,
    bootstrapped: bool,
    backlog: Vec<u8>,
}

/// What the surface must act on. Each variant names the `channels` transition
/// (or row write) it feeds; the wiring lives in `window::TerminalSurface`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayEvent {
    /// The tmux server's hostname resolved:
    /// `channels::Channels::host_changed`.
    HostChanged(String),
    /// The server's sessions as last listed, the server, and the one this
    /// gateway is attached to; which are worth a bank is the surface's call.
    SessionsSeen {
        attached: SessionId,
        socket: String,
        server_pid: u32,
        sessions: Vec<(SessionId, String)>,
    },
    /// A window needs a channel: `channels::Channels::open_tmux_window`,
    /// then [`Gateway::attach_window`] if the slot was granted.
    WindowAdded {
        window: WindowId,
        pane: PaneId,
        name: String,
    },
    /// `channels::Channels::set_title` on the window's row.
    WindowRenamed { window: WindowId, name: String },
    /// `channels::Channels::window_closed`.
    WindowClosed { window: WindowId },
    /// The window shows a different pane now; the row's tmux address follows.
    /// The screen redraw is this gateway's own (a fresh capture is already on
    /// its way), so the channel keeps its emulation and its scrollback.
    WindowPaneChanged { window: WindowId, pane: PaneId },
    /// Bytes for the channel row carrying this pane:
    /// `term::TmuxPane::feed`.
    Output { pane: PaneId, bytes: Vec<u8> },
    /// The attachment is over: `channels::Channels::collapse_bank`. With
    /// `lost_protocol` the envelope never closed and no `ST` is coming, so
    /// the surface must also force the gateway's parsers out of it
    /// (`term::Session::leave_control_mode`).
    Detached { lost_protocol: bool },
}

/// Ours and local: `/proc/<pid>` is a process of our uid and the socket path
/// stats to a socket; anything that will not answer is a no.
#[cfg(unix)]
pub fn server_is_local(socket: &str, pid: u32) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let uid = |path: String| std::fs::metadata(path).ok().map(|m| m.uid());
    let server = uid(format!("/proc/{pid}"));
    server.is_some()
        && server == uid("/proc/self".to_string())
        && std::fs::metadata(socket).is_ok_and(|m| m.file_type().is_socket())
}

/// No tmux runs on this platform, so no server can be ours and local; the
/// caller's refusal path is the whole answer.
#[cfg(not(unix))]
pub fn server_is_local(_socket: &str, _pid: u32) -> bool {
    false
}

/// The server's own binary, so both ends speak one dialect; `tmux` on PATH if `/proc` will not say.
pub fn tmux_binary(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "tmux".to_string())
}

/// One `tmux -CC` attachment's client half.
pub struct Gateway<W: Write> {
    codec: Codec,
    writer: W,
    /// Encoded command lines the transport has not taken yet, in the order
    /// they were sent. Whole lines go in; whatever prefix a write is allowed
    /// leaves; the remainder waits here for the next [`Gateway::flush`]. See
    /// the module doc's transport divergence for what the alternative costs.
    pending: Vec<u8>,
    intents: HashMap<CommandId, Intent>,

    /// Every window this client has announced, to its active pane: the diff
    /// base when `%window-add` forces a re-listing.
    windows: HashMap<WindowId, PaneId>,
    /// The name each window was last announced under: the rename sweep's
    /// diff base.
    names: HashMap<WindowId, String>,
    /// Attached panes by pane id: whose gate their `%output` runs.
    panes: HashMap<PaneId, PaneGate>,

    host: String,
    /// The server and the attached session, off the `Server` reply.
    socket: String,
    server_pid: u32,
    session: Option<SessionId>,
    /// A re-listing this advance owes; repeats of `%sessions-changed` cost one.
    relist_due: bool,
    attached: bool,
    /// The first `list-panes` reply has been consumed; window notifications
    /// mean something now.
    bootstrapped: bool,

    /// The size tmux has been told, the one last asked for, and when the
    /// asking settles.
    sent_size: Option<(u16, u16)>,
    wanted_size: Option<(u16, u16)>,
    resize_due: Option<Instant>,

    /// When the bootstrap went out, whether anything has answered it, and how
    /// many blocks arrived unsolicited meanwhile: the watchdog's three
    /// readings (see [`BOOTSTRAP_DEADLINE`] and [`Gateway::poll`]).
    opened: Instant,
    solicited: bool,
    unsolicited: usize,
    /// How many commands have been refused because [`PENDING_CAP`] was reached.
    /// See [`Gateway::sheds`].
    sheds: u64,
}

impl<W: Write> Gateway<W> {
    /// Raise the client and send the bootstrap at once: the server, the attached
    /// session's panes, the server's sessions. No timer; see the module doc.
    pub fn new(writer: W) -> Self {
        let mut gateway = Self {
            codec: Codec::new(),
            writer,
            pending: Vec::new(),
            intents: HashMap::new(),
            windows: HashMap::new(),
            names: HashMap::new(),
            panes: HashMap::new(),
            host: String::new(),
            socket: String::new(),
            server_pid: 0,
            session: None,
            relist_due: false,
            attached: true,
            bootstrapped: false,
            sent_size: None,
            wanted_size: None,
            resize_due: None,
            opened: Instant::now(),
            solicited: false,
            unsolicited: 0,
            sheds: 0,
        };
        gateway.command(&Command::Server, Intent::Host);
        gateway.command(&Command::ListPanes, Intent::ListPanes);
        gateway.command(&Command::ListSessions, Intent::ListSessions);
        gateway
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn attached(&self) -> bool {
        self.attached
    }

    /// How many encoded command bytes the transport has not taken yet.
    ///
    /// The instrument behind [`PENDING_CAP`]: a test measures the high-water
    /// mark through this, and it is the reading that says whether the peer on
    /// the other end of the control wire is draining at all.
    pub fn queued_bytes(&self) -> usize {
        self.pending.len()
    }

    /// How many commands this gateway has refused to send because the queue was
    /// at [`PENDING_CAP`].
    ///
    /// Most of what is refused is the user's own typing on its way to a pane
    /// (`send-keys`), so a shed here is the loss `term::Session::sheds` counts
    /// one wire further out. Monotonic, because the surface compares readings
    /// and puts a badge on the glass.
    pub fn sheds(&self) -> u64 {
        self.sheds
    }

    /// Encode, register the reply's intent, queue the line, and push at the
    /// transport. What the transport will not take stays queued for
    /// [`Self::flush`]; the command is on the codec's pairing queue either
    /// way, which is what makes the two orders one order.
    fn command(&mut self, command: &Command, intent: Intent) {
        if !self.attached {
            return;
        }
        // The shed, and it has to happen here rather than at the queue: once a
        // line is encoded the codec has it on its pairing queue and tmux is
        // owed an answer, so dropping the bytes alone would put every later
        // reply against the wrong question. What is shed is the newest and
        // never the queue's head, which is a line already half-written to a
        // transport that took a prefix of it.
        if self.pending.len() >= PENDING_CAP {
            self.sheds += 1;
            log::warn!(
                "tmux: {} byte(s) unsent and the transport is not draining; \
                 dropping a {command:?} rather than growing the queue",
                self.pending.len()
            );
            return;
        }
        let sent = self.codec.send(command);
        self.intents.insert(sent.id, intent);
        self.pending.extend_from_slice(sent.wire.as_bytes());
        self.flush();
    }

    /// Push the queue at the transport, keeping whatever it refuses.
    ///
    /// The surface calls this once per pump (through [`Self::poll`]), which is
    /// what drains a queue that grew while the PTY was full: nothing else wakes
    /// this type up. `WouldBlock` is backpressure and not death, the reader on
    /// the other side of this fd being tmux; every other error is the transport
    /// gone, which the gateway's own EOF path is already collapsing the bank
    /// for, so there is nothing to do but say what went down with it.
    pub fn flush(&mut self) {
        while !self.pending.is_empty() {
            match self.writer.write(&self.pending) {
                // A refusal dressed as a success. Treat it as backpressure
                // rather than spinning on it.
                Ok(0) => break,
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    std::io::ErrorKind::WouldBlock => break,
                    _ => {
                        log::warn!(
                            "tmux: transport gone with {} byte(s) unsent: {e}",
                            self.pending.len()
                        );
                        self.pending.clear();
                        break;
                    }
                },
            }
        }
    }

    // ---- the surface's asks ------------------------------------------

    /// Ask tmux for another window; it arrives later as `%window-add`.
    pub fn new_window(&mut self) {
        self.command(&Command::NewWindow, Intent::Ignore);
    }

    /// Ask tmux to kill a window; the channel goes when the close
    /// notification lands.
    pub fn kill_window(&mut self, window: &WindowId) {
        self.command(&Command::KillWindow(window.clone()), Intent::Ignore);
    }

    /// Ask tmux to let this client go; teardown follows on `%exit`.
    pub fn detach(&mut self) {
        self.command(&Command::DetachClient, Intent::Ignore);
    }

    /// Keystrokes for a pane, as hex `send-keys` lines (the chunking is
    /// [`Command::send_keys`]'s).
    pub fn send_keys(&mut self, pane: &PaneId, bytes: &[u8]) {
        for command in Command::send_keys(pane, bytes) {
            self.command(&command, Intent::Ignore);
        }
    }

    /// The client-size law: tell tmux the glass's grid. The *first* size goes
    /// out immediately, because tmux has to hear it before this client's
    /// windows are drawn at somebody else's, and later ones debounce behind
    /// [`RESIZE_DEBOUNCE`], flushed by [`Gateway::poll`].
    pub fn set_client_size(&mut self, columns: u16, rows: u16) {
        if columns == 0 || rows == 0 {
            return;
        }
        if self.wanted_size == Some((columns, rows)) {
            return;
        }
        self.wanted_size = Some((columns, rows));
        if self.sent_size.is_none() {
            self.flush_client_size();
        } else {
            // Restarted, not merely started: a drag arrives as dozens of
            // sizes and only the one it comes to rest on is worth a round
            // trip.
            self.resize_due = Some(Instant::now() + RESIZE_DEBOUNCE);
        }
    }

    /// Flush time-deferred work. The surface calls this once per pump, and
    /// `now` is the pump's own clock reading: this crate keeps deadlines,
    /// never timer objects.
    ///
    /// Three things are deferred here. The queued wire goes out (whatever the
    /// last flush left); a settled resize goes out behind it; and the
    /// bootstrap watchdog reads the attachment's pulse.
    ///
    /// The watchdog's shape is the flags gate failing: blocks keep arriving,
    /// every one of them marked unsolicited, so no reply ever resolves an
    /// intent: the server's name never lands, no listing ever becomes a
    /// channel, and the bank stands with its gateway and nothing behind it.
    /// That is worth a word at warn and an honest teardown, not a debug line
    /// and a dead bank. `lost_protocol`, because the peer's device string is
    /// still open and only the gateway's parsers can be told
    /// otherwise.
    pub fn poll(&mut self, now: Instant) -> Vec<GatewayEvent> {
        self.flush();
        if self.resize_due.is_some_and(|due| due <= now) {
            self.flush_client_size();
        }
        let mut out = Vec::new();
        let overdue = now.saturating_duration_since(self.opened) >= BOOTSTRAP_DEADLINE;
        if self.attached && !self.solicited && self.unsolicited > 0 && overdue {
            log::warn!(
                "tmux: the bootstrap went unanswered for {}s while {} block(s) arrived flagged \
                 unsolicited -- this peer never marks its answers to us solicited (the %begin \
                 flags gate), so no reply can ever be paired; detaching",
                BOOTSTRAP_DEADLINE.as_secs(),
                self.unsolicited
            );
            // Ask tmux to let the client go before dropping it, so the server
            // is not left holding a client nothing is driving. It goes out
            // ahead of the teardown because a detached gateway sends nothing.
            self.command(&Command::DetachClient, Intent::Ignore);
            self.teardown(true, &mut out);
        }
        out
    }

    fn flush_client_size(&mut self) {
        self.resize_due = None;
        let Some((columns, rows)) = self.wanted_size else {
            return;
        };
        if self.sent_size == Some((columns, rows)) {
            return;
        }
        self.sent_size = Some((columns, rows));
        self.command(&Command::ClientSize { columns, rows }, Intent::Ignore);
    }

    /// Route a window's active pane into a channel: register its gate and ask
    /// for its screen and cursor. The surface calls this after
    /// `open_tmux_window` granted the slot; a window refused a channel is
    /// never attached.
    pub fn attach_window(&mut self, window: &WindowId, pane: &PaneId) {
        // A re-attach replaces the wiring, never doubles it.
        self.panes.remove(pane);
        self.panes.insert(pane.clone(), PaneGate::default());
        self.windows.insert(window.clone(), pane.clone());
        // What the pane already holds, then where its cursor stands in it.
        // Both asked now so the pane's live output has a definite boundary to
        // be dropped before and kept after.
        self.command(&Command::capture_pane(pane), Intent::Capture(pane.clone()));
        self.command(
            &Command::CursorPosition(pane.clone()),
            Intent::Cursor(pane.clone()),
        );
    }

    // ---- the wire, coming back ---------------------------------------

    /// The envelope closed (`ST`) with no `%exit` before it: the gateway
    /// program died mid-protocol with its device string closed. The parsers
    /// are already out of the DCS, so nothing is lost; the attachment is
    /// simply over.
    pub fn control_mode_ended(&mut self) -> Vec<GatewayEvent> {
        let mut out = Vec::new();
        self.teardown(false, &mut out);
        out
    }

    /// Feed peeled DCS body bytes; act on what they complete; answer what the
    /// surface must do.
    pub fn advance(&mut self, bytes: &[u8]) -> Vec<GatewayEvent> {
        let mut out = Vec::new();
        for event in self.codec.feed(bytes) {
            if !self.attached {
                break;
            }
            match event {
                Event::Reply { id, block } => {
                    self.solicited = true; // the watchdog's pulse
                    let Some(intent) = self.intents.remove(&id) else {
                        log::warn!("tmux: a reply for a command never sent: {id:?}");
                        continue;
                    };
                    self.reply(intent, block, &mut out);
                }
                Event::Unsolicited { block } => {
                    // The attach burst: output of the commands that built the
                    // session, sent before this client asked anything
                    // (cpp:343-349). The flags gate already kept it off the
                    // pairing queue; nothing further to do with it but count
                    // it, because a wire that is *only* this is the shape
                    // [`Gateway::poll`]'s watchdog watches for.
                    self.unsolicited += 1;
                    log::debug!(
                        "tmux: unsolicited block {:?} of {} line(s)",
                        block.number,
                        block.body.len()
                    );
                }
                Event::GuardMismatch { began, closed } => {
                    log::warn!("tmux: guard mismatch, %begin {began:?} closed by {closed:?}");
                }
                Event::Notification(n) => self.notification(n, &mut out),
                Event::Foreign(line) => {
                    // Outside a block every line tmux sends begins with '%'.
                    // Anything else is not tmux: the client process was
                    // killed without closing its device string, and the
                    // shell behind it is talking to us (cpp:319-328).
                    log::warn!(
                        "tmux: protocol lost, foreign line: {:?}",
                        String::from_utf8_lossy(&line)
                    );
                    self.teardown(true, &mut out);
                }
            }
        }
        // One listing for however many `%sessions-changed` this advance
        // carried: tmux sends two for a single `new-session`.
        if std::mem::take(&mut self.relist_due) {
            self.command(&Command::ListSessions, Intent::ListSessions);
        }
        out
    }

    fn reply(&mut self, intent: Intent, block: tmux_cc::Block, out: &mut Vec<GatewayEvent>) {
        if block.error {
            log::warn!("tmux: command failed: {:?}", block.text().join(" "));
            // A bootstrap half-done leaves a pane deaf for good: an
            // unanswerable capture or cursor still opens the gate, on an
            // empty screen (cpp:357-363).
            if let Intent::Capture(pane) | Intent::Cursor(pane) = intent {
                // A capture that errored drew no screen. Its paired
                // `display-message` is still in flight, and tmux answers that
                // one perfectly well even when the capture failed -- so left
                // live, its reply walks the cursor up from a bottom row this
                // pane never wrote, against whatever the glass already held.
                // The gate has just opened on an empty screen; the cursor has
                // nowhere to be put on it.
                self.neutralise_cursor_intent(&pane);
                self.finish_pane_bootstrap(&pane, &[], out);
            }
            return;
        }
        match intent {
            Intent::Ignore => {}
            Intent::Host => {
                if let Some((socket, pid, session, host)) =
                    block.text().first().and_then(|l| tmux_cc::parse_server(l))
                {
                    self.socket = socket;
                    self.server_pid = pid;
                    self.session = Some(session);
                    self.host = host;
                    out.push(GatewayEvent::HostChanged(self.host.clone()));
                }
            }
            Intent::ListSessions => {
                // "<session id> <session name>"; the name is the remainder and
                // may hold spaces. Which of them this client is on is the
                // `Server` reply's, which lands first or not at all.
                let Some(attached) = self.session.clone() else {
                    return;
                };
                let sessions = block
                    .text()
                    .iter()
                    .filter_map(|line| {
                        let (id, name) = line.split_once(' ')?;
                        Some((SessionId::parse(id)?, name.to_string()))
                    })
                    .collect();
                out.push(GatewayEvent::SessionsSeen {
                    attached,
                    socket: self.socket.clone(),
                    server_pid: self.server_pid,
                    sessions,
                });
            }
            Intent::ListPanes => {
                // "<window id> <pane id> <pane active> <window name>"; the
                // name is the remainder and may hold spaces (cpp:376-394).
                // Names in a listing are format output, raw on the wire --
                // only notification names are vis(3)-escaped.
                for line in block.text() {
                    let fields: Vec<&str> = line.splitn(4, ' ').collect();
                    if fields.len() < 4 || fields[2] != "1" {
                        continue; // only a window's active pane is shown
                    }
                    let (Some(window), Some(pane)) =
                        (WindowId::parse(fields[0]), PaneId::parse(fields[1]))
                    else {
                        continue;
                    };
                    if self.windows.contains_key(&window) {
                        continue; // a %window-add re-listing: only the new window is news
                    }
                    let name = fields[3].to_string();
                    self.windows.insert(window.clone(), pane.clone());
                    self.names.insert(window.clone(), name.clone());
                    out.push(GatewayEvent::WindowAdded { window, pane, name });
                }
                if !self.bootstrapped {
                    self.bootstrapped = true;
                    // Renames shouted during the attach burst went unheard,
                    // because until this moment there was nothing to rename.
                    // Ask once for the names as they stand (cpp:395-401).
                    self.command(&Command::ListWindows, Intent::WindowNames);
                }
            }
            Intent::WindowNames => {
                for line in block.text() {
                    let Some((id, name)) = line.split_once(' ') else {
                        continue;
                    };
                    let Some(window) = WindowId::parse(id) else {
                        continue;
                    };
                    if !self.windows.contains_key(&window) {
                        continue;
                    }
                    if self.names.get(&window).map(String::as_str) == Some(name) {
                        continue;
                    }
                    self.names.insert(window.clone(), name.to_string());
                    out.push(GatewayEvent::WindowRenamed {
                        window,
                        name: name.to_string(),
                    });
                }
            }
            Intent::Capture(pane) => {
                let Some(gate) = self.panes.get_mut(&pane) else {
                    return;
                };
                // Home and erase first: on a fresh window there is nothing to
                // erase, and on a re-route the pane being left behind must
                // not stay under the new one's screen. Erasing forward keeps
                // the scrollback the channel already has (cpp:423-431).
                let mut screen: Vec<u8> = b"\x1b[H\x1b[J".to_vec();
                for (i, line) in block.body.iter().enumerate() {
                    if i > 0 {
                        screen.extend_from_slice(b"\r\n");
                    }
                    screen.extend_from_slice(line);
                }
                gate.captured = true; // live output from here on is news
                out.push(GatewayEvent::Output {
                    pane,
                    bytes: screen,
                });
            }
            Intent::Cursor(pane) => {
                // "<cursor_x> <cursor_y> <pane_height>". capture-pane always
                // ends on the pane's bottom row, so the cursor is walked up
                // from wherever that row landed instead of addressed
                // absolutely: a split window hands over a pane shorter than
                // the glass (cpp:433-458).
                let body = block.text();
                let fields: Vec<&str> = body
                    .first()
                    .map(|line| line.split_whitespace().collect())
                    .unwrap_or_default();
                let mut cursor = Vec::new();
                if fields.len() >= 3 {
                    if let (Ok(x), Ok(y), Ok(height)) = (
                        fields[0].parse::<i64>(),
                        fields[1].parse::<i64>(),
                        fields[2].parse::<i64>(),
                    ) {
                        let up = height - 1 - y;
                        if up > 0 {
                            cursor.extend_from_slice(format!("\x1b[{up}A").as_bytes());
                        }
                        cursor.extend_from_slice(format!("\x1b[{}G", x + 1).as_bytes());
                    }
                }
                self.finish_pane_bootstrap(&pane, &cursor, out);
            }
            Intent::LayoutPanes(window) => {
                // The window's panes as they stand; only which one is active
                // matters while a window shows one pane (cpp:460-472).
                let mut active = None;
                for line in block.text() {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    if fields.len() >= 2 && fields[1] == "1" {
                        active = PaneId::parse(fields[0]);
                    }
                }
                if let Some(pane) = active {
                    self.switch_active_pane(&window, pane, out);
                }
            }
        }
    }

    /// Retire a pane's pending [`Intent::Cursor`] without dropping the reply
    /// it belongs to.
    ///
    /// Retired and not removed: the command was sent, so tmux is going to
    /// answer it, and every reply has to find an intent waiting or the pairing
    /// logs a command it never sent. [`Intent::Ignore`] is exactly the
    /// "confirms the command went through" case, which is all that reply is
    /// still good for once its capture has failed.
    fn neutralise_cursor_intent(&mut self, pane: &PaneId) {
        let wanted = Intent::Cursor(pane.clone());
        for pending in self.intents.values_mut() {
            if *pending == wanted {
                *pending = Intent::Ignore;
            }
        }
    }

    /// The pane is caught up: what arrived between the capture and the cursor
    /// goes in first, in the order it came, and the cursor lands last because
    /// tmux reported it after those bytes had already moved it (cpp:220-234).
    fn finish_pane_bootstrap(&mut self, pane: &PaneId, cursor: &[u8], out: &mut Vec<GatewayEvent>) {
        let Some(gate) = self.panes.get_mut(pane) else {
            return;
        };
        gate.captured = true;
        gate.bootstrapped = true;
        let mut tail = std::mem::take(&mut gate.backlog);
        tail.extend_from_slice(cursor);
        if !tail.is_empty() {
            out.push(GatewayEvent::Output {
                pane: pane.clone(),
                bytes: tail,
            });
        }
    }

    /// A window is showing a different pane. The channel keeps its emulation,
    /// so its scrollback survives; the new pane's own capture draws over the
    /// screen (cpp:476-490).
    fn switch_active_pane(&mut self, window: &WindowId, pane: PaneId, out: &mut Vec<GatewayEvent>) {
        let previous = self.windows.get(window).cloned();
        if previous.as_ref() == Some(&pane) {
            return;
        }
        let was_attached = previous
            .as_ref()
            .is_some_and(|p| self.panes.remove(p).is_some());
        self.windows.insert(window.clone(), pane.clone());
        out.push(GatewayEvent::WindowPaneChanged {
            window: window.clone(),
            pane: pane.clone(),
        });
        if was_attached {
            self.attach_window(window, &pane);
        }
    }

    fn notification(&mut self, n: Notification, out: &mut Vec<GatewayEvent>) {
        match n {
            Notification::Output { pane, data }
            | Notification::ExtendedOutput { pane, data, .. } => {
                let Some(gate) = self.panes.get_mut(&pane) else {
                    return; // a pane not yet attached has nowhere to draw
                };
                if !gate.captured {
                    return; // the capture on its way holds these bytes already
                }
                if !gate.bootstrapped {
                    // Newer than the capture, older than the cursor.
                    gate.backlog.extend_from_slice(&data);
                    if gate.backlog.len() >= BACKLOG_CAP {
                        // The cursor is taking too long, or is never coming.
                        // See [`BACKLOG_CAP`]: the wait ends here and the pane
                        // shows what it has.
                        log::warn!(
                            "tmux: pane {pane:?} banked {} byte(s) waiting for its cursor; \
                             putting them on the screen and running live",
                            gate.backlog.len()
                        );
                        // And its cursor reply is retired on the way out, for
                        // the same reason an errored capture retires one: the
                        // walk-up it carries is measured from a capture that
                        // has since been drawn over by all these bytes, so
                        // honouring it would yank the cursor up an arbitrary
                        // number of rows. Live output has put the cursor where
                        // it belongs already.
                        self.neutralise_cursor_intent(&pane);
                        self.finish_pane_bootstrap(&pane, &[], out);
                    }
                    return;
                }
                out.push(GatewayEvent::Output { pane, bytes: data });
            }
            // What tmux says to the user rather than about the session:
            // `%message` is the control client's own message line, and
            // `%config-error` a line of `.tmux.conf` the server could not run.
            // A warning line is the surface for both, not a placeholder for a
            // richer one: neither is about a channel or a queue, so there is
            // nothing for a toast to stand in for. Placed above the bootstrap
            // gate deliberately -- a `.tmux.conf` error arrives during the
            // attach burst, before any listing has come back.
            Notification::Message { message } => log::warn!("tmux: %message {message}"),
            // A session came or went. Placed here for the same reason: the
            // attach burst carries these, and the listing they ask for is
            // owed once per advance however many of them arrive
            // ([`Gateway::advance`]).
            Notification::SessionsChanged => self.relist_due = true,
            Notification::ConfigError { message } => log::warn!("tmux: %config-error {message}"),
            // A name this codec knows, in a shape it does not
            // (`tmux_cc::Notification::Malformed`, distinct from `Unknown`:
            // the dialect moved rather than grew). Whatever that line was
            // going to change in these books did not happen, which is why it
            // is a warning rather than the unhandled-notification debug line
            // at the bottom of this match, and why it is above the bootstrap
            // gate, like the two lines before it, so a dialect that moved is
            // still audible during the attach burst.
            Notification::Malformed { name, rest } => log::warn!(
                "tmux: malformed {name}: {:?}",
                String::from_utf8_lossy(&rest)
            ),
            Notification::Exit { .. } => self.teardown(false, out),
            // During the attach burst it is the burst's end, and the bootstrap
            // has already gone out; afterwards it is `switch-client`, and this
            // client is on another session of the same server now.
            // `%client-session-changed` stays unhandled: re-listing on the
            // notification another client's arrival sends is a spawn loop.
            Notification::SessionChanged { session, .. } => {
                if self.bootstrapped {
                    self.session = Some(session);
                    self.relist_due = true;
                }
            }
            // Window notifications only make sense once the initial set is
            // known; before that the bootstrap listing is the one source of
            // truth.
            _ if !self.bootstrapped => {}
            Notification::WindowAdd { .. } => {
                // The notification names the window but not its pane; a fresh
                // listing does, and the known-window diff keeps the old
                // windows quiet.
                self.command(&Command::ListPanes, Intent::ListPanes);
            }
            Notification::WindowRenamed { window, name }
            | Notification::UnlinkedWindowRenamed { window, name } => {
                self.names.insert(window.clone(), name.clone());
                out.push(GatewayEvent::WindowRenamed { window, name });
            }
            Notification::WindowPaneChanged { window, pane } => {
                self.switch_active_pane(&window, pane, out);
            }
            Notification::LayoutChange { window, .. } => {
                // Which pane is active is not in the notification, and it may
                // have changed without a %window-pane-changed of its own (a
                // killed active pane), so the window is listed and compared
                // (cpp:560-570).
                if self.windows.contains_key(&window) {
                    self.command(
                        &Command::ListWindowPanes(window.clone()),
                        Intent::LayoutPanes(window),
                    );
                }
            }
            // tmux 3.5a says %unlinked-window-close for every closure, the
            // current window's included; the manual's %window-close is kept
            // handled for the future tmux that means it
            // (`tmux_cc::Notification::WindowClose`).
            Notification::WindowClose { window } | Notification::UnlinkedWindowClose { window } => {
                if let Some(pane) = self.windows.remove(&window) {
                    self.panes.remove(&pane);
                }
                self.names.remove(&window);
                out.push(GatewayEvent::WindowClosed { window });
            }
            other => log::debug!("tmux: unhandled notification {other:?}"),
        }
    }

    /// Give up the session. `lost_protocol` marks that the peer died without
    /// closing its device string, and the surface must tell the gateway
    /// channel's parsers no `ST` is coming.
    fn teardown(&mut self, lost_protocol: bool, out: &mut Vec<GatewayEvent>) {
        if !self.attached {
            return;
        }
        // One last push at the wire: a `detach-client` queued behind a full
        // PTY is the difference between tmux letting this client go and the
        // server holding a client nobody drives. Then the queue goes, because
        // after this nothing here may write that fd again.
        self.flush();
        self.pending.clear();
        self.attached = false;
        self.codec.clear_pending();
        self.intents.clear();
        self.windows.clear();
        self.names.clear();
        self.panes.clear();
        self.resize_due = None;
        out.push(GatewayEvent::Detached { lost_protocol });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Mutex, OnceLock};

    /// Every record's message, so a test can assert on what a warning said
    /// rather than on the fact that some code path was taken.
    struct CapturingLogger;

    static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    static LOGGER: CapturingLogger = CapturingLogger;

    fn messages() -> &'static Mutex<Vec<String>> {
        MESSAGES.get_or_init(|| Mutex::new(Vec::new()))
    }

    impl log::Log for CapturingLogger {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            messages()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("{}", record.args()));
        }
        fn flush(&self) {}
    }

    fn init_logger() {
        // Twice is an error, and every test in this binary shares the
        // process: the second caller wanted what the first already did.
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Warn);
    }

    /// A transport whose written lines a test can read back.
    #[derive(Clone, Default)]
    struct Wire(Rc<RefCell<Vec<u8>>>);

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A transport with a full buffer under it: it takes at most `budget`
    /// more bytes, ever, and refuses the rest with `WouldBlock`, which is
    /// what an `O_NONBLOCK` PTY master does when tmux has not read yet.
    #[derive(Clone, Default)]
    struct TightWire {
        written: Rc<RefCell<Vec<u8>>>,
        budget: Rc<std::cell::Cell<usize>>,
    }

    impl Write for TightWire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let take = buf.len().min(self.budget.get());
            if take == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "the master is full",
                ));
            }
            self.budget.set(self.budget.get() - take);
            self.written.borrow_mut().extend_from_slice(&buf[..take]);
            Ok(take)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TightWire {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.written.borrow()).into_owned()
        }
    }

    impl Wire {
        fn lines(&self) -> Vec<String> {
            String::from_utf8_lossy(&self.0.borrow())
                .lines()
                .map(str::to_string)
                .collect()
        }
        fn clear(&self) {
            self.0.borrow_mut().clear();
        }
    }

    fn gateway() -> (Gateway<Wire>, Wire) {
        let wire = Wire::default();
        let gateway = Gateway::new(wire.clone());
        (gateway, wire)
    }

    /// A solicited reply block for the n-th command sent (0-based), flags 1
    /// as tmux 3.5a marks answers to this client's stdin.
    fn reply(n: u64, body: &str) -> String {
        let mut s = format!("%begin 1 {} 1\r\n", 100 + n);
        for line in body.lines() {
            s.push_str(line);
            s.push_str("\r\n");
        }
        s.push_str(&format!("%end 1 {} 1\r\n", 100 + n));
        s
    }

    /// Walk a fresh gateway through the attach: server, one-window listing,
    /// one-session listing, rename sweep. Command numbers 0,1,2 are the
    /// constructor's; 3 is the sweep.
    fn attach<W: Write>(gateway: &mut Gateway<W>) -> Vec<GatewayEvent> {
        let mut events =
            gateway.advance(reply(0, "/tmp/tmux-1000/default 4242 $0 prime").as_bytes());
        events.extend(gateway.advance(reply(1, "@0 %0 1 bash").as_bytes()));
        events.extend(gateway.advance(reply(2, "$0 one").as_bytes()));
        events.extend(gateway.advance(reply(3, "@0 bash").as_bytes()));
        events
    }

    #[test]
    fn the_bootstrap_goes_out_at_construction_with_no_timer() {
        let (_gateway, wire) = gateway();
        assert_eq!(
            wire.lines(),
            vec![
                r##"display-message -p "#{socket_path} #{pid} #{session_id} #{host_short}""##,
                r##"list-panes -s -F "#{window_id} #{pane_id} #{pane_active} #{window_name}""##,
                r##"list-sessions -F "#{session_id} #{session_name}""##,
            ]
        );
    }

    /// The listing the surface banks from: every session the server holds,
    /// and which of them this client is attached to.
    #[test]
    fn a_session_listing_names_the_whole_server_and_the_session_this_client_is_on() {
        let (mut gateway, _wire) = gateway();
        gateway.advance(reply(0, "/tmp/tmux-1000/default 4242 $0 prime").as_bytes());
        gateway.advance(reply(1, "@0 %0 1 bash").as_bytes());
        let events = gateway.advance(reply(2, "$0 one\n$3 two three").as_bytes());
        assert_eq!(
            events,
            vec![GatewayEvent::SessionsSeen {
                attached: SessionId::parse("$0").unwrap(),
                socket: "/tmp/tmux-1000/default".into(),
                server_pid: 4242,
                sessions: vec![
                    (SessionId::parse("$0").unwrap(), "one".into()),
                    (SessionId::parse("$3").unwrap(), "two three".into()),
                ],
            }]
        );
    }

    /// `new-session` sends `%sessions-changed` twice on tmux 3.5a. Two
    /// notifications in one advance are one round trip.
    #[test]
    fn sessions_changed_relists_once_an_advance_however_often_it_arrives() {
        let (mut gateway, wire) = gateway();
        attach(&mut gateway);
        wire.clear();
        assert_eq!(
            gateway.advance(b"%sessions-changed\r\n%sessions-changed\r\n"),
            vec![]
        );
        assert_eq!(
            wire.lines(),
            vec![r##"list-sessions -F "#{session_id} #{session_name}""##]
        );
        // And the reply lands on the intent that was registered for it.
        let events = gateway.advance(reply(4, "$0 one\n$1 two").as_bytes());
        assert!(matches!(
            events.as_slice(),
            [GatewayEvent::SessionsSeen { sessions, .. }] if sessions.len() == 2
        ));
        // A second burst asks again: the coalescing is per advance, not once.
        wire.clear();
        gateway.advance(b"%sessions-changed\r\n");
        assert_eq!(wire.lines().len(), 1);
    }

    #[test]
    fn the_attach_burst_consumes_no_pending_command() {
        let (mut gateway, _wire) = gateway();
        // Two unsolicited blocks (flags 0), as the burst delivers them...
        let burst = "%begin 1 50 0\r\nnoise\r\n%end 1 50 0\r\n%begin 1 51 0\r\n%end 1 51 0\r\n";
        assert_eq!(gateway.advance(burst.as_bytes()), vec![]);
        // ...and the real replies still land on the right intents.
        let events = attach(&mut gateway);
        assert_eq!(events[0], GatewayEvent::HostChanged("prime".into()));
        assert!(matches!(events[1], GatewayEvent::WindowAdded { .. }));
    }

    #[test]
    fn the_listing_becomes_windows_and_the_sweep_corrects_a_late_rename() {
        let (mut gateway, wire) = gateway();
        let mut events =
            gateway.advance(reply(0, "/tmp/tmux-1000/default 4242 $0 prime").as_bytes());
        events.extend(gateway.advance(reply(1, "@0 %0 1 bash\n@1 %1 1 logs").as_bytes()));
        // Only active panes make windows; the sweep went out after the
        // first listing.
        assert_eq!(
            events,
            vec![
                GatewayEvent::HostChanged("prime".into()),
                GatewayEvent::WindowAdded {
                    window: WindowId::parse("@0").unwrap(),
                    pane: PaneId::parse("%0").unwrap(),
                    name: "bash".into(),
                },
                GatewayEvent::WindowAdded {
                    window: WindowId::parse("@1").unwrap(),
                    pane: PaneId::parse("%1").unwrap(),
                    name: "logs".into(),
                },
            ]
        );
        assert!(wire.lines().last().unwrap().starts_with("list-windows"));
        // The session listing (command 2) stands between the two: the sweep
        // was sent behind it and its reply comes back behind it too.
        gateway.advance(reply(2, "$0 one").as_bytes());
        // The sweep answers with one name moved on and one standing still.
        let events = gateway.advance(reply(3, "@0 vim\n@1 logs").as_bytes());
        assert_eq!(
            events,
            vec![GatewayEvent::WindowRenamed {
                window: WindowId::parse("@0").unwrap(),
                name: "vim".into(),
            }]
        );
    }

    #[test]
    fn the_pane_gate_drops_buffers_and_releases_in_the_references_order() {
        let (mut gateway, wire) = gateway();
        attach(&mut gateway);
        let window = WindowId::parse("@0").unwrap();
        let pane = PaneId::parse("%0").unwrap();
        wire.clear();
        gateway.attach_window(&window, &pane);
        assert_eq!(wire.lines().len(), 2, "capture and cursor, together");

        // Before the capture lands, %output is dropped: the capture on its
        // way holds those same bytes.
        assert_eq!(gateway.advance(b"%output %0 dropped\r\n"), vec![]);
        // The capture (command 4): home-and-erase, then the screen.
        let events = gateway.advance(reply(4, "line-one\nline-two").as_bytes());
        assert_eq!(
            events,
            vec![GatewayEvent::Output {
                pane: pane.clone(),
                bytes: b"\x1b[H\x1b[Jline-one\r\nline-two".to_vec(),
            }]
        );
        // Between capture and cursor, output backlogs.
        assert_eq!(gateway.advance(b"%output %0 kept\r\n"), vec![]);
        // The cursor (command 5) at x=3,y=1 of a 24-row pane: the backlog
        // flushes first, then the walk-up.
        let events = gateway.advance(reply(5, "3 1 24").as_bytes());
        assert_eq!(
            events,
            vec![GatewayEvent::Output {
                pane: pane.clone(),
                bytes: b"kept\x1b[22A\x1b[4G".to_vec(),
            }]
        );
        // Live from here on.
        let events = gateway.advance(b"%output %0 live\r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::Output {
                pane,
                bytes: b"live".to_vec(),
            }]
        );
    }

    #[test]
    fn a_window_add_relists_and_only_the_new_window_is_news() {
        let (mut gateway, wire) = gateway();
        attach(&mut gateway);
        wire.clear();
        assert_eq!(gateway.advance(b"%window-add @1\r\n"), vec![]);
        assert!(wire.lines()[0].starts_with("list-panes -s"));
        // The re-listing (command 4) carries both windows; only @1 lands.
        let events = gateway.advance(reply(4, "@0 %0 1 bash\n@1 %1 1 fresh").as_bytes());
        assert_eq!(
            events,
            vec![GatewayEvent::WindowAdded {
                window: WindowId::parse("@1").unwrap(),
                pane: PaneId::parse("%1").unwrap(),
                name: "fresh".into(),
            }]
        );
    }

    #[test]
    fn a_close_forgets_the_window_and_its_pane() {
        let (mut gateway, _wire) = gateway();
        attach(&mut gateway);
        let pane = PaneId::parse("%0").unwrap();
        gateway.attach_window(&WindowId::parse("@0").unwrap(), &pane);
        let events = gateway.advance(b"%unlinked-window-close @0\r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::WindowClosed {
                window: WindowId::parse("@0").unwrap(),
            }]
        );
        // The pane went with it: its output has nowhere to draw.
        assert_eq!(gateway.advance(b"%output %0 orphan\r\n"), vec![]);
    }

    #[test]
    fn exit_detaches_and_a_foreign_line_is_the_protocol_lost() {
        let (mut first, _wire) = gateway();
        attach(&mut first);
        let events = first.advance(b"%exit\r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::Detached {
                lost_protocol: false,
            }]
        );
        assert!(!first.attached());

        let (mut second, _wire) = gateway();
        attach(&mut second);
        let events = second.advance(b"bash-5.2$ \r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::Detached {
                lost_protocol: true,
            }]
        );
    }

    #[test]
    fn the_first_client_size_flushes_at_once_and_a_burst_settles_to_one() {
        let (mut gateway, wire) = gateway();
        wire.clear();
        gateway.set_client_size(120, 40);
        assert_eq!(wire.lines(), vec!["refresh-client -C 120x40"]);
        // A drag: many sizes, none sent until the debounce runs out.
        wire.clear();
        gateway.set_client_size(121, 40);
        gateway.set_client_size(122, 40);
        gateway.set_client_size(123, 41);
        gateway.poll(Instant::now());
        assert_eq!(wire.lines(), Vec::<String>::new());
        gateway.poll(Instant::now() + RESIZE_DEBOUNCE);
        assert_eq!(wire.lines(), vec!["refresh-client -C 123x41"]);
        // The size tmux already has is not resent.
        gateway.set_client_size(123, 41);
        gateway.poll(Instant::now() + RESIZE_DEBOUNCE);
        assert_eq!(wire.lines().len(), 1);
    }

    #[test]
    fn a_pane_switch_reroutes_and_recaptures_into_the_same_channel() {
        let (mut gateway, wire) = gateway();
        attach(&mut gateway);
        let window = WindowId::parse("@0").unwrap();
        gateway.attach_window(&window, &PaneId::parse("%0").unwrap());
        // Finish %0's bootstrap so the switch is from a live pane.
        gateway.advance(reply(4, "old").as_bytes());
        gateway.advance(reply(5, "0 0 24").as_bytes());
        wire.clear();

        let events = gateway.advance(b"%window-pane-changed @0 %5\r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::WindowPaneChanged {
                window: window.clone(),
                pane: PaneId::parse("%5").unwrap(),
            }]
        );
        // A fresh capture-and-cursor pair went out for the new pane...
        assert_eq!(wire.lines().len(), 2);
        assert!(wire.lines()[0].contains("-t %5"));
        // ...the old pane is deaf, and the new one gated until it lands.
        assert_eq!(gateway.advance(b"%output %0 stale\r\n"), vec![]);
        assert_eq!(gateway.advance(b"%output %5 early\r\n"), vec![]);
    }

    #[test]
    fn an_errored_capture_still_opens_the_gate() {
        let (mut gateway, _wire) = gateway();
        attach(&mut gateway);
        let pane = PaneId::parse("%0").unwrap();
        gateway.attach_window(&WindowId::parse("@0").unwrap(), &pane);
        // Both replies error (command 4, 5): the pane must not stay deaf.
        let err = "%begin 1 104 1\r\nno such pane\r\n%error 1 104 1\r\n";
        gateway.advance(err.as_bytes());
        let err = "%begin 1 105 1\r\nno such pane\r\n%error 1 105 1\r\n";
        gateway.advance(err.as_bytes());
        let events = gateway.advance(b"%output %0 alive\r\n");
        assert_eq!(
            events,
            vec![GatewayEvent::Output {
                pane,
                bytes: b"alive".to_vec(),
            }]
        );
    }

    /// The other half of the errored capture, and the dangerous one.
    ///
    /// `capture-pane` failing does not stop tmux answering the
    /// `display-message` that was sent with it: a pane can refuse to be
    /// captured and still report a cursor. Nothing drew a screen, so a cursor
    /// walk against it steps up through rows this pane never wrote -- over
    /// whatever the channel already had on the glass.
    #[test]
    fn an_errored_capture_takes_its_cursor_down_with_it() {
        let (mut gateway, _wire) = gateway();
        attach(&mut gateway);
        let pane = PaneId::parse("%0").unwrap();
        gateway.attach_window(&WindowId::parse("@0").unwrap(), &pane);

        // The capture (command 4) errors...
        let err = "%begin 1 104 1\r\nno such pane\r\n%error 1 104 1\r\n";
        assert_eq!(gateway.advance(err.as_bytes()), vec![]);
        // ...and the cursor (command 5) succeeds, from the bottom of a
        // 24-row pane: live, this is `ESC [ 23 A ESC [ 1 G`.
        assert_eq!(
            gateway.advance(reply(5, "0 0 24").as_bytes()),
            vec![],
            "the cursor walked a screen that was never drawn"
        );

        // The gate still opened, which is what the errored capture owes the
        // pane: its live output arrives rather than being dropped for good.
        assert_eq!(
            gateway.advance(b"%output %0 alive\r\n"),
            vec![GatewayEvent::Output {
                pane,
                bytes: b"alive".to_vec(),
            }]
        );
    }

    #[test]
    fn what_tmux_says_to_the_user_is_logged_and_changes_nothing_it_arrives_before() {
        let (mut gateway, wire) = gateway();
        // A `.tmux.conf` error arrives during the attach burst, before any
        // listing has come back. It must reach the log rather than the
        // bootstrap gate that swallows window news at this point, and it must
        // not consume a pending reply, or the server's name lands on the
        // listing's intent and every later reply is one behind.
        assert_eq!(
            gateway.advance(b"%config-error /etc/tmux.conf:3: unknown command\r\n"),
            vec![]
        );
        assert_eq!(
            gateway.advance(b"%message the server says hello\r\n"),
            vec![]
        );
        wire.clear();
        let events = attach(&mut gateway);
        assert_eq!(events[0], GatewayEvent::HostChanged("prime".into()));
        assert!(matches!(events[1], GatewayEvent::WindowAdded { .. }));
        assert!(gateway.attached(), "neither line is a teardown");
    }

    /// Backpressure is not data loss. A `write` on the gateway's
    /// master takes what fits and refuses the rest; a command whose tail is
    /// dropped there arrives at tmux joined to the next one, which is one
    /// command instead of two and a reply short for the rest of the
    /// attachment.
    #[test]
    fn a_transport_that_takes_a_prefix_loses_no_command_and_keeps_the_pairing() {
        let wire = TightWire::default();
        wire.budget.set(7);
        let mut gateway = Gateway::new(wire.clone());
        // The bootstrap pair went out into a master with room for seven
        // bytes: seven bytes of the first line are on the wire, and the rest
        // of it (with the whole second line behind it) is queued.
        assert_eq!(wire.text(), "display");

        // A paste-sized burst on top, into a wire that is still shut.
        let pane = PaneId::parse("%0").unwrap();
        let paste: Vec<u8> = (0..700u32).map(|i| b'a' + (i % 26) as u8).collect();
        gateway.send_keys(&pane, &paste);
        assert_eq!(wire.text(), "display", "nothing more can leave yet");

        // tmux reads; the next pump pushes what was held.
        wire.budget.set(usize::MAX);
        assert_eq!(gateway.poll(Instant::now()), vec![]);

        // Every line whole, in the order it was sent: the bootstrap pair,
        // then the paste's chunks.
        let text = wire.text();
        assert!(text.ends_with('\n'), "the wire ends on a line boundary");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            r##"display-message -p "#{socket_path} #{pid} #{session_id} #{host_short}""##
        );
        assert_eq!(
            lines[1],
            r##"list-panes -s -F "#{window_id} #{pane_id} #{pane_active} #{window_name}""##
        );
        assert_eq!(
            lines[2],
            r##"list-sessions -F "#{session_id} #{session_name}""##
        );
        // And the paste itself, reassembled from the hex `send-keys` lines:
        // every byte, once, in order.
        let mut delivered = Vec::new();
        for line in &lines[3..] {
            let hex = line
                .strip_prefix("send-keys -H -t %0 ")
                .unwrap_or_else(|| panic!("not a send-keys line: {line:?}"));
            for pair in hex.split(' ') {
                delivered.push(u8::from_str_radix(pair, 16).expect("a hex byte"));
            }
        }
        assert_eq!(delivered, paste, "the paste arrived whole and in order");

        // The pairing is the point: the bootstrap's two replies still land on
        // the two intents that were registered for them.
        let events = attach(&mut gateway);
        assert_eq!(events[0], GatewayEvent::HostChanged("prime".into()));
        assert!(matches!(events[1], GatewayEvent::WindowAdded { .. }));
    }

    /// The write side's watchdog, which it did not have. A tmux that has
    /// stopped reading the control PTY refuses every write, and `send_keys`
    /// encodes each byte as hex: unbounded, a user typing at a pane nobody is
    /// draining grows this process until the OOM killer arrives.
    ///
    /// The measurement is the point: the queue's high-water mark over a burst
    /// far larger than the cap, taken through `queued_bytes`.
    #[test]
    fn a_transport_that_never_drains_bounds_the_queue_and_keeps_the_pairing() {
        init_logger();
        let from = messages().lock().unwrap().len();

        // A wire that takes seven bytes and never another.
        let wire = TightWire::default();
        wire.budget.set(7);
        let mut gateway = Gateway::new(wire.clone());

        // Four times the cap of the user's typing, hex-encoded on the way in,
        // at a pane whose tmux is not reading.
        let pane = PaneId::parse("%0").unwrap();
        let chunk: Vec<u8> = (0..8192u32).map(|i| b'a' + (i % 26) as u8).collect();
        let mut high_water = 0;
        let mut offered = 0usize;
        while offered < 4 * PENDING_CAP {
            gateway.send_keys(&pane, &chunk);
            offered += chunk.len();
            high_water = high_water.max(gateway.queued_bytes());
        }

        // Bounded, and by the cap rather than by what was offered: the shed is
        // one whole command line past it at the very worst.
        assert!(
            high_water < PENDING_CAP + 4096,
            "the queue reached {high_water} byte(s) against a cap of {PENDING_CAP}"
        );
        assert!(
            high_water >= PENDING_CAP,
            "the cap must be the thing that bound it, not the offer: {high_water}"
        );
        let said = messages().lock().unwrap()[from..].join("\n");
        assert!(
            said.contains("not draining"),
            "the shed must say so out loud; it said: {said:?}"
        );
        // Out loud in a log is not out loud on the glass. The counter is what
        // the surface reads to put a badge up (`app::window::SHED_TMUX`), so
        // a shed that logged and did not count would be a silent one again.
        assert!(
            gateway.sheds() > 0,
            "the shed was logged but not counted, so nothing could show it"
        );

        // And the pairing survives the shed, which is what says the codec was
        // never told about a command the wire did not carry: the bootstrap's
        // two replies still land on the two intents registered for them.
        wire.budget.set(usize::MAX);
        assert_eq!(gateway.poll(Instant::now()), vec![]);
        let events = attach(&mut gateway);
        assert_eq!(events[0], GatewayEvent::HostChanged("prime".into()));
        assert!(matches!(events[1], GatewayEvent::WindowAdded { .. }));
    }

    /// The other unbounded queue: a pane banks its `%output` between its
    /// capture and its cursor, and a cursor reply that never arrives left it
    /// banking forever -- showing nothing, holding everything. At the cap the
    /// wait ends: what was banked goes to the screen and the pane runs live.
    #[test]
    fn a_pane_whose_cursor_never_arrives_stops_banking_and_shows_what_it_has() {
        init_logger();
        let from = messages().lock().unwrap().len();

        let (mut gateway, _wire) = gateway();
        attach(&mut gateway);
        let window = WindowId::parse("@0").unwrap();
        let pane = PaneId::parse("%0").unwrap();
        gateway.attach_window(&window, &pane);
        // The capture (command 4) comes back; the cursor never will.
        let events = gateway.advance(reply(4, "the screen").as_bytes());
        assert!(
            !events.is_empty(),
            "the capture reply draws the pane's screen"
        );

        // A pane running `yes`, four times the cap of it.
        let line = "x".repeat(1000);
        let mut delivered = 0usize;
        let mut out_events = Vec::new();
        let mut high_water = 0;
        while delivered < 4 * BACKLOG_CAP {
            out_events.extend(gateway.advance(format!("%output %0 {line}\r\n").as_bytes()));
            delivered += line.len();
            high_water = high_water.max(
                gateway
                    .panes
                    .get(&pane)
                    .map(|g| g.backlog.len())
                    .unwrap_or(0),
            );
        }

        // Bounded by the cap and not by the offer, which was four times it.
        // The reading is taken between `advance` calls, and the give-up
        // happens inside one, so the high-water mark sits one payload short of
        // the cap rather than on it.
        assert!(
            high_water < BACKLOG_CAP + line.len(),
            "the backlog reached {high_water} byte(s) against a cap of {BACKLOG_CAP}"
        );
        assert!(
            high_water + line.len() >= BACKLOG_CAP,
            "the cap must be the thing that bound it: {high_water}"
        );
        // The banked bytes were not dropped: they reached the screen, and
        // everything after them ran straight through.
        let bytes: usize = out_events
            .iter()
            .map(|e| match e {
                GatewayEvent::Output { bytes, .. } => bytes.len(),
                _ => 0,
            })
            .sum();
        assert!(
            bytes >= delivered - BACKLOG_CAP,
            "only {bytes} of {delivered} byte(s) reached the screen"
        );
        assert!(
            gateway.panes.get(&pane).is_some_and(|g| g.bootstrapped),
            "the pane runs live once the wait is over"
        );
        let said = messages().lock().unwrap()[from..].join("\n");
        assert!(
            said.contains("waiting for its cursor"),
            "the give-up must say so out loud; it said: {said:?}"
        );
    }

    /// The bootstrap watchdog. A peer that flags every block unsolicited
    /// (the answers to this client's own commands included) leaves the
    /// gateway waiting on replies that will never be marked as such: an
    /// gateway with no windows behind it, forever. The deadline says so out
    /// loud and gives the attachment up.
    #[test]
    fn a_bootstrap_no_flagged_reply_ever_answers_is_given_up_at_warn() {
        init_logger();
        let from = messages().lock().unwrap().len();

        let (mut gateway, wire) = gateway();
        wire.clear();
        // Blocks that carry exactly what the bootstrap asked for, every one
        // of them flagged 0.
        let burst = "%begin 1 50 0\r\n/tmp/tmux-1000/default 4242 $0 prime\r\n%end 1 50 0\r\n\
                     %begin 1 51 0\r\n@0 %0 1 bash\r\n%end 1 51 0\r\n";
        assert_eq!(gateway.advance(burst.as_bytes()), vec![]);

        // Inside the deadline this is only a slow attach.
        assert_eq!(gateway.poll(Instant::now()), vec![]);
        assert!(gateway.attached());

        // Past it, the attachment is over: `lost_protocol`, because the peer
        // never closed its device string and only the gateway's
        // parsers can be told that no `ST` is coming.
        let events = gateway.poll(Instant::now() + BOOTSTRAP_DEADLINE);
        assert_eq!(
            events,
            vec![GatewayEvent::Detached {
                lost_protocol: true
            }]
        );
        assert!(!gateway.attached());
        assert_eq!(
            wire.lines(),
            vec!["detach-client"],
            "tmux was asked to let the client go before it was dropped"
        );
        // Once. A torn-down gateway stays down and says nothing further.
        assert_eq!(
            gateway.poll(Instant::now() + BOOTSTRAP_DEADLINE * 2),
            vec![]
        );

        let said = messages().lock().unwrap()[from..].join("\n");
        assert!(
            said.contains("flags gate") && said.contains("unsolicited"),
            "the warning must name what went wrong; it said: {said:?}"
        );
    }

    /// The watchdog is a watchdog and not a deadline on the attachment: an
    /// attach that was answered is never given up, however long it then runs.
    #[test]
    fn a_bootstrap_that_was_answered_outlives_the_deadline() {
        let (mut gateway, _wire) = gateway();
        attach(&mut gateway);
        assert_eq!(
            gateway.poll(Instant::now() + BOOTSTRAP_DEADLINE * 10),
            vec![]
        );
        assert!(gateway.attached());
    }

    /// A wire that says nothing at all is not this watchdog's business
    /// either: the shape it names is blocks arriving with the flags wrong,
    /// and a silent peer is the gateway's own EOF path to notice.
    #[test]
    fn a_silent_wire_is_not_the_watchdogs_business() {
        let (mut gateway, _wire) = gateway();
        assert_eq!(
            gateway.poll(Instant::now() + BOOTSTRAP_DEADLINE * 10),
            vec![]
        );
        assert!(gateway.attached());
    }
}
