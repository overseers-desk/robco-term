//! A terminal session: a PTY, the VT state, and the read loop joining them.
//!
//! The read loop is hand-rolled rather than rio-vt's `Machine`, because
//! `Machine` owns the bytes and hands them only to
//! `Crosswords`, which cannot see DCS. Owning the loop is the price of
//! the tmux tap, and it buys something else worth having: the loop is
//! synchronous and driven by whoever calls [`Session::pump`], so the
//! whole session runs headless in a test with no threads and no window.
//!
//! Neither side of the pty ever blocks this loop, so `pump` drains what
//! is there and returns; it never waits on the child. On Unix that is the
//! master fd opened `O_NONBLOCK` by `teletypewriter`; on Windows it is
//! the ConPTY pipes' buffering threads, whose reads answer zero when
//! nothing has arrived and whose writes take what their ring has room
//! for.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::Crosswords;
use rio_vt::event::{EventListener, RioEvent, WindowId};
use rio_vt::performer::handler::Processor;
#[cfg(unix)]
use rio_vt::teletypewriter::create_pty_with_spawn;
#[cfg(windows)]
use rio_vt::teletypewriter::{create_pty, ChildEvent, EventedPty};
use rio_vt::teletypewriter::{ProcessReadWrite, Pty, WinsizeBuilder};

use crate::dcs::{DcsParser, DcsTap};
use crate::size::TermSize;

/// The VT state machine and grid in one type, as rio-vt models it.
pub type Term = Crosswords<ReplyListener>;

/// A fresh grid of `size`, the one place a channel's grid is built.
///
/// Local, SSH and tmux channels face the same glass, so they take the same
/// grid: the same cursor, the same window id, and the same answer to how
/// wide a character is. `grapheme_clustering` is DEC private mode 2027, and
/// it decides whether a grapheme cluster occupies one slot or each of its
/// code points occupies its own. Off, the layout is `wcwidth`'s, which is
/// what a program padding a table counted on when it wrote the row. An
/// application that wants clusters asks for them with `CSI ? 2027 h`, and
/// a reset returns to whatever is passed here.
pub(crate) fn new_term(
    size: TermSize,
    scrollback: usize,
    grapheme_clustering: bool,
    listener: ReplyListener,
) -> Term {
    let mut term = Crosswords::new(
        size,
        CursorShape::Block,
        listener,
        WindowId::from(0u64),
        0,
        scrollback,
    );
    term.set_grapheme_clustering(grapheme_clustering);
    term
}

/// Where a grid parks the answers it owes, between the parse that made them
/// and the pump that sends them. See [`ReplyListener`].
pub type Replies = Arc<Mutex<Vec<u8>>>;

/// The ear on a grid that has something to say back.
///
/// `Crosswords` writes nothing itself. A query it can answer -- primary and
/// secondary DA, the cursor position report, XTVERSION, the keyboard and mode
/// reports, colour requests, xtgettcap -- becomes a `RioEvent::PtyWrite`
/// handed to its listener, and rio-vt's own `VoidListener` takes the trait's
/// default for that, which is nothing. A grid wearing it is a terminal that
/// answers no question it is asked: vim's DA probe times out, a script that
/// positions itself by CPR reads a reply that never comes.
///
/// `EventListener::send_event` takes `&self`, so the ear has to be interior-
/// mutable and shared: the listener the grid owns and the [`Replies`] its
/// session drains are two handles on one queue. A queue rather than a channel
/// because the drain is a single reader on the same thread as the parse, and
/// bytes are what the far end wants -- concatenating them is the whole job.
///
/// Only `PtyWrite` is caught. The closure-carrying events (`ClipboardLoad`,
/// `ColorRequest`, `TextAreaSizeRequest`) go unanswered, because nothing here
/// yet knows what to answer them with; the esctest harness answers the size
/// one for itself, being the only place that knows a cell's pixels.
/// `ClipboardLoad` is OSC 52 asking to read what the local user has copied,
/// which most terminals leave off by default.
#[derive(Clone, Default)]
pub struct ReplyListener {
    /// `None` on a grid whose answers have nowhere to go. See
    /// [`ReplyListener::detached`].
    queue: Option<Replies>,
}

impl ReplyListener {
    /// An ear on the given queue: what the grid answers lands there.
    pub fn new(replies: &Replies) -> Self {
        Self {
            queue: Some(replies.clone()),
        }
    }

    /// An ear that hears and drops, for a grid whose answers cannot be sent
    /// on this path. The behaviour rio-vt's `VoidListener` has, kept as a
    /// state of this type so that [`Term`] stays one name across every
    /// screen the app holds.
    pub fn detached() -> Self {
        Self { queue: None }
    }
}

impl EventListener for ReplyListener {
    fn send_event(&self, event: RioEvent, _id: WindowId) {
        if let (Some(queue), RioEvent::PtyWrite(_, text)) = (&self.queue, event) {
            queue.lock().unwrap().extend_from_slice(text.as_bytes());
        }
    }
}

/// Read buffer size. Matches the order of magnitude a PTY hands over per
/// wakeup; larger buffers stop helping once the kernel's own buffer is
/// the limit.
const READ_BUF: usize = 65_536;

/// How much unwritten input may wait for a child that is not reading before
/// this session starts refusing more.
///
/// The queue is there to hold the tail of one write the master would not take
/// whole (see [`Session::write`]), and that is all it is there for. A child
/// that has stopped reading its tty -- stopped, wedged, or simply busy for a
/// long time -- makes every flush a no-op, and every keystroke, paste and
/// terminal report aimed at it then accumulates with nothing to drain it.
///
/// The shed is on the *newest* write and never on the queue, and a write is
/// always taken whole or not at all: splitting one would put a hole in the
/// middle of a paste, which is the exact failure the queue was built to
/// prevent. So the ceiling is this cap plus one write, and one write is
/// bounded by the paste the user actually made.
pub const INPUT_CAP: usize = 4 << 20;

/// How a session is started.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Program to run. `None` means the user's login shell, which is the
    /// production default; tests name `/bin/sh` for determinism.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: Option<String>,
    /// Applied on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// Scrollback lines retained above the screen.
    pub scrollback: usize,
    /// Whether the grid measures by grapheme cluster (mode 2027) rather
    /// than by code point. See [`new_term`].
    pub grapheme_clustering: bool,
    /// Bytes a second to take the child's output at, or `None` to take it
    /// as fast as the pty gives it up. See [`Session::set_rate`], which is
    /// how a rate changed under a running session reaches it.
    pub rate: Option<u32>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            working_directory: None,
            env: vec![("TERM".to_string(), "xterm-256color".to_string())],
            scrollback: 10_000,
            grapheme_clustering: false,
            rate: None,
        }
    }
}

/// What one `pump` did, so a caller can decide whether to redraw.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pumped {
    /// Bytes read from the child this call.
    pub bytes: usize,
    /// The child's end closed: the session is over.
    pub eof: bool,
}

impl Pumped {
}

/// A live terminal session.
pub struct Session<T: DcsTap> {
    term: Term,
    pty: Pty,
    processor: Processor,
    dcs: DcsParser<T>,
    size: TermSize,
    buf: Box<[u8; READ_BUF]>,
    /// Bytes on their way to the child that the master would not take yet.
    /// See [`Session::write`].
    input: Vec<u8>,
    /// The grid's end of [`Session::send_replies`]: the other handle on the
    /// queue this session's [`ReplyListener`] fills.
    replies: Replies,
    /// How many writes have been refused because the queue was full.
    /// See [`Session::sheds`].
    sheds: u64,
    /// The rate the child's output is taken at, or `None` for as fast as
    /// it is written. See [`Session::set_rate`].
    rate: Option<u32>,
    /// Bytes this session may still take, earned at `rate` since
    /// [`Session::credited`]. Fractional because a slow line earns less
    /// than a byte between pumps: 300 baud against the host's 125 Hz poll
    /// is a quarter of one.
    credit: f64,
    /// When `credit` was last brought up to date.
    credited: Instant,
    eof: bool,
}

impl<T: DcsTap> Session<T> {
    /// Spawn the child and build the session around it.
    pub fn spawn(config: &SessionConfig, size: TermSize, tap: T) -> std::io::Result<Self> {
        #[cfg(unix)]
        let pty = {
            let (px_w, px_h) = size.pixel_size();
            create_pty_with_spawn(
                config.program.as_deref(),
                config.args.clone(),
                &config.working_directory,
                Some(config.env.clone()),
                size.cols() as u16,
                size.rows() as u16,
                px_w,
                px_h,
            )?
        };
        // ConPTY's geometry is cells alone; a `program` of `None` is the
        // platform's default console host, the analogue of the login shell.
        #[cfg(windows)]
        let pty = create_pty(
            config.program.as_deref(),
            config.args.clone(),
            &config.working_directory,
            Some(config.env.clone()),
            size.cols() as u16,
            size.rows() as u16,
        )?;

        let replies = Replies::default();
        let term = new_term(
            size,
            config.scrollback,
            config.grapheme_clustering,
            ReplyListener::new(&replies),
        );

        Ok(Self {
            term,
            pty,
            processor: Processor::default(),
            dcs: DcsParser::new(tap),
            size,
            buf: Box::new([0u8; READ_BUF]),
            input: Vec::new(),
            replies,
            sheds: 0,
            rate: config.rate,
            credit: 0.0,
            credited: Instant::now(),
            eof: false,
        })
    }

    /// Drain whatever the child has written and apply it.
    ///
    /// Every chunk goes to two consumers: the DCS tap and the grid. See
    /// `dcs.rs` for why feeding both is correct rather than double work.
    ///
    /// A pump that parsed a question also sends its answer
    /// ([`Session::send_replies`]): the child asks and hears back within the
    /// one call, which is what a program blocking on its own DA probe needs.
    pub fn pump(&mut self) -> Pumped {
        let mut out = Pumped::default();
        if self.eof {
            out.eof = true;
            return out;
        }

        // The other direction first: whatever a paste left queued when the
        // tty's buffer filled ([`Session::write`]). A pump is the only thing
        // that comes back to it.
        self.drop_input_in_control_mode();
        if let Err(e) = self.flush_input() {
            log::warn!("could not write to the pty: {e}");
        }

        // What this pump may take. `None` is the whole buffer and no
        // arithmetic: the unconfigured path, and the one a gateway keeps
        // whatever the rate says. An open control-mode envelope carries
        // tmux's protocol rather than a picture of a shell, and a capture
        // of a thousand lines is not something to meter out at reading
        // speed.
        let mut budget = self.budget();
        if self.dcs.tap().in_control_mode() {
            budget = None;
        }

        loop {
            let want = match budget {
                Some(0) => break,
                Some(bytes) => bytes.min(READ_BUF),
                None => READ_BUF,
            };
            match self.pty.reader().read(&mut self.buf[..want]) {
                // A zero-length read: on Unix no slave fd is open, which
                // is usually the child having closed its end, but not
                // always; on Windows the buffering pipe answers zero
                // whenever nothing has arrived, the normal idle read.
                // `child_gone` is what tells the cases apart on both.
                Ok(0) => {
                    if self.child_gone() {
                        self.eof = true;
                        out.eof = true;
                    }
                    break;
                }
                Ok(n) => {
                    out.bytes += n;
                    if let Some(bytes) = budget.as_mut() {
                        *bytes -= n;
                        self.credit -= n as f64;
                    }
                    // Copy out so the two consumers can each take
                    // `&mut self`-adjacent borrows without fighting the
                    // borrow checker over `self.buf`.
                    let chunk: Vec<u8> = self.buf[..n].to_vec();
                    self.dcs.feed(&chunk);
                    self.processor.advance(&mut self.term, &chunk);
                    // A short read means the buffer was not filled, so
                    // there is nothing queued behind it.
                    if n < want {
                        break;
                    }
                }
                Err(e) => {
                    match e.kind() {
                        // Nothing pending: the normal exit from this loop.
                        std::io::ErrorKind::WouldBlock => {}
                        std::io::ErrorKind::Interrupted => continue,
                        // Everything else is effectively EIO: no slave
                        // fd is open. Only the child can tell us whether
                        // that is fatal.
                        _ => {
                            if self.child_gone() {
                                self.eof = true;
                                out.eof = true;
                            }
                        }
                    }
                    break;
                }
            }
        }

        // Whatever the grid was asked in the loop above, answered now, before
        // the queue is examined for a control mode that may have opened in
        // that same loop.
        self.send_replies();

        // And again on the way out, because the envelope may have opened in
        // the loop just above: this is the one call that stands between the
        // control mode starting and the gateway's first write, which the host
        // makes later in this same pump.
        self.drop_input_in_control_mode();

        self.expire_sync();
        out
    }

    /// Take the child's output at `rate` bytes a second, or as fast as it
    /// is written when `None`.
    ///
    /// The throttle is the read and never a queue. A pump takes what the
    /// rate has earned and leaves the rest where it was, so the tty buffer
    /// fills and the child blocks in `write` exactly as it would behind a
    /// slow line. Nothing is held here that the grid has not already been
    /// given, which is what keeps a copy of the screen, a search of the
    /// scrollback and an interrupt meaning what they mean at full speed.
    ///
    /// How far the child runs ahead of the picture is the buffer between
    /// them rather than this rate. On Unix that is the tty's own few
    /// kilobytes. On Windows the reader `teletypewriter` runs keeps a 64 KiB
    /// ring of its own ahead of this loop, half a minute of a 19200 line, so
    /// a child there writes that much before the rate reaches it at all.
    /// The glass fills at the rate on both.
    ///
    /// The line starts empty on every change: a rate set mid-session earns
    /// from the moment it is set rather than from what the last one banked.
    pub fn set_rate(&mut self, rate: Option<u32>) {
        if self.rate == rate {
            return;
        }
        self.rate = rate;
        self.credit = 0.0;
        self.credited = Instant::now();
    }

    /// What this pump may take, `None` being no limit.
    ///
    /// A pause banks no more than one second's worth, so a channel that sat
    /// off the screen for a minute comes back at its rate instead of
    /// emptying its tty buffer into the grid in one frame.
    fn budget(&mut self) -> Option<usize> {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.credited).as_secs_f64();
        self.credited = now;
        let rate = f64::from(self.rate?);
        self.credit = (self.credit + elapsed * rate).min(rate);
        Some(self.credit as usize)
    }

    /// Send the child what the grid owes it.
    ///
    /// [`ReplyListener`] parked the bytes; this is where they become a write.
    /// They go out through [`Session::write`] rather than straight at the
    /// master, and every rule that path has applies to them: they queue behind
    /// a paste the tty would not take whole, and they shed at [`INPUT_CAP`]
    /// when the child has stopped reading -- a child that is not reading is
    /// not reading its answers either.
    ///
    /// That path's control-mode swallow applies too, and it is the one worth
    /// spelling out: while the envelope is open this fd belongs to the
    /// gateway, so a report the grid generated in this same pump is dropped
    /// rather than spliced into the gateway's wire. The querying program is a
    /// shell that is no longer there.
    fn send_replies(&mut self) {
        let replies = std::mem::take(&mut *self.replies.lock().unwrap());
        if replies.is_empty() {
            return;
        }
        if let Err(e) = self.write(&replies) {
            log::warn!("could not answer the child's query: {e}");
        }
    }

    /// Give up any input still queued for a shell that has become a tmux
    /// control-mode wire.
    ///
    /// The gateway writes that fd through its own `dup` while this queue
    /// writes it here, and the module doc above [`Session::control_mode_writer`]
    /// rests on nothing but the gateway writing it while control mode is
    /// active. A tail left over from before the envelope opened -- the rest of
    /// a paste the tty's buffer refused, which is exactly the case this queue
    /// exists for -- would otherwise flush underneath the gateway and splice
    /// the user's half-line into the middle of a command line.
    ///
    /// Dropped rather than held: a gateway swallows every byte typed, pasted
    /// or reported at it (`app::window::TerminalSurface::write`), and these
    /// are bytes aimed at a shell that is not there any more. Holding them
    /// would only deliver them at a detach, to a shell that never asked.
    fn drop_input_in_control_mode(&mut self) {
        if self.input.is_empty() || !self.dcs.tap().in_control_mode() {
            return;
        }
        log::warn!(
            "tmux control mode opened with {} byte(s) still queued for the shell; \
             the gateway swallows them rather than splicing them into its wire",
            self.input.len()
        );
        self.input.clear();
    }

    /// Has the child actually exited?
    ///
    /// Reading a PTY master reports EIO (and sometimes a zero-length
    /// read) whenever no process holds the slave open. That is the right
    /// signal for "the shell exited", but it is *also* true for a moment
    /// at startup: `openpty` returns, the parent closes its copy of the
    /// slave fd, and until the forked child reaches `exec` and opens it,
    /// nobody holds it. A first `pump` landing in that window would
    /// otherwise mark the session finished before the shell ever ran,
    /// a race that only shows up on a loaded machine, where the child
    /// takes longer to get going.
    ///
    /// `waitpid(WNOHANG)` is the discriminator: still running means the
    /// error was transient and the next pump will find the child.
    #[cfg(unix)]
    fn child_gone(&mut self) -> bool {
        match self.pty.waitpid() {
            // Alive: whatever we just saw was the startup window.
            Ok(None) => false,
            Ok(Some(_)) => true,
            // We cannot ask, so we cannot claim it is still running.
            Err(_) => true,
        }
    }

    /// The same question under ConPTY, which has no `waitpid`: the pty's
    /// watcher thread posts the exit as an event. Consuming it here is
    /// sound because the first `true` marks the session finished for good.
    #[cfg(windows)]
    fn child_gone(&mut self) -> bool {
        matches!(self.pty.next_child_event(), Some(ChildEvent::Exited(_)))
    }

    /// Flush a synchronized update whose deadline has passed.
    ///
    /// A `BSU` (`ESC P = 1 s`) tells the terminal to hold rendering until
    /// `ESU`. If the application dies mid-update the screen would freeze
    /// forever, so the sequence carries a timeout. `Processor` only
    /// enforces it lazily, on the next `advance`; an embedder that stops
    /// receiving bytes never reaches that call. Honouring it here, on
    /// every pump, is the price of owning the read loop: nothing else calls
    /// `sync_timeout()` for us.
    fn expire_sync(&mut self) {
        if let Some(deadline) = self.processor.sync_timeout().sync_timeout() {
            if deadline <= Instant::now() {
                self.processor.stop_sync(&mut self.term);
            }
        }
    }

    /// When the pending synchronized update expires, if one is pending.
    ///
    /// The app's event loop uses this as a wakeup deadline: with no PTY
    /// traffic it would otherwise sleep past the timeout and leave the
    /// screen held.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Send bytes to the child (keystrokes, paste, responses).
    ///
    /// Queued, then pushed as far as the master will take them. The master is
    /// `O_NONBLOCK` (see this module's opening note), so a write bigger than
    /// the free space in the tty's input buffer takes a prefix and refuses the
    /// rest, and a paste is exactly that write. `write_all` would report the
    /// refusal as an error *after* the prefix had gone, and the tail would be
    /// gone with it: a paste arriving at the shell with a hole in the middle.
    /// So the tail waits here and [`Session::pump`] pushes it again, on every
    /// pump, until the child has read enough to make room.
    ///
    /// A queue past [`INPUT_CAP`] means the child has stopped reading, and
    /// this refuses rather than grows: see that constant for which end sheds
    /// and why a write is taken whole or not at all. A refusal is not an
    /// error -- nothing went wrong on the wire, and there is nothing for the
    /// caller to retry -- so it is logged and swallowed, the same shape the
    /// gateway's own shed has.
    ///
    /// The error this can still answer is a real one (the master is gone)
    /// and the queue is dropped with it, because the child that would have
    /// read it is what just died.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        // While control mode is active this fd is the gateway's, and a gateway
        // swallows every byte typed, pasted *or reported* at it. The surface
        // swallows what it knows about (`app::window::TerminalSurface::write`,
        // gated on the row already being a gateway); this is the same rule at
        // the one place that can see the envelope rather than the row, which
        // is what also catches a report the grid generates from inside the
        // pump. See [`Self::drop_input_in_control_mode`].
        if self.dcs.tap().in_control_mode() {
            return Ok(());
        }
        if self.input.len() >= INPUT_CAP {
            self.sheds += 1;
            log::warn!(
                "the child is not reading its tty and {} byte(s) are queued for it; \
                 dropping {} more rather than growing the queue",
                self.input.len(),
                bytes.len()
            );
            return Ok(());
        }
        self.input.extend_from_slice(bytes);
        self.flush_input()
    }

    /// How many bytes are queued for a child that has not taken them yet.
    ///
    /// The instrument behind [`INPUT_CAP`]: a test measures the high-water
    /// mark through this, and it is the reading a host would want if it ever
    /// grows a way to tell the user their shell has stopped listening.
    pub fn queued_input(&self) -> usize {
        self.input.len()
    }

    /// How many writes this session has thrown away because the queue was at
    /// [`INPUT_CAP`].
    ///
    /// The log line at the shed says what was lost, and a log is not on the
    /// glass: this counter is how a host tells the user instead. It only ever
    /// grows, so a host that remembers the last reading learns "something was
    /// dropped since you last looked" by comparing, without this type having to
    /// know what a host would do about it (`app::window` raises a badge).
    pub fn sheds(&self) -> u64 {
        self.sheds
    }

    /// Push the input queue at the master, keeping what it refuses.
    fn flush_input(&mut self) -> std::io::Result<()> {
        while !self.input.is_empty() {
            match self.pty.writer().write(&self.input) {
                // Nothing taken and nothing wrong: backpressure, wearing a
                // success. Stop rather than spin.
                Ok(0) => break,
                Ok(n) => {
                    self.input.drain(..n);
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    // The buffer is full; the child is what empties it.
                    std::io::ErrorKind::WouldBlock => break,
                    _ => {
                        self.input.clear();
                        return Err(e);
                    }
                },
            }
        }
        Ok(())
    }

    /// A second, independent handle onto the PTY master's write side.
    ///
    /// The tmux gateway owns its transport (its commands must go out the
    /// moment it decides to send them, not on the next time the host
    /// happens to hold this session), while this session keeps owning the
    /// read side and the loop. A `dup` of the master fd gives both their
    /// own handle onto one file description.
    ///
    /// Two handles onto one fd is only safe because of what the host does
    /// with them, and not because of anything the fd guarantees. This is a
    /// tty, so there is no `PIPE_BUF` atomicity to lean on: a write here can
    /// take a prefix and refuse the rest, which is why the gateway queues
    /// (`app::tmux::Gateway::flush`) rather than assuming a whole line left.
    /// What keeps the wire clean is that the surface is single-threaded (one
    /// pump, one writer running at a time, never two mid-line) and that
    /// while control mode is active nothing writes this fd but the gateway:
    /// the channel holding this session is the gateway, and a gateway
    /// swallows every byte typed, pasted or reported at it
    /// (`app::window::TerminalSurface::write`).
    #[cfg(unix)]
    pub fn control_mode_writer(&mut self) -> std::io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.pty.writer().try_clone()?))
    }

    /// On Windows no local tmux exists to raise a control mode with, so
    /// there is no second writer to hand out; a remote `tmux -CC` rides an
    /// SSH channel and gets its writer there. Refusing loudly beats
    /// wiring a gateway to a wire that cannot exist.
    #[cfg(windows)]
    pub fn control_mode_writer(&mut self) -> std::io::Result<Box<dyn Write + Send>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no local tmux exists on this platform to drive a control mode",
        ))
    }

    /// Force both parsers out of a DCS string that will never close.
    ///
    /// When the control client is killed without closing its device
    /// string, no `ST` is ever coming,
    /// and until one arrives the grid's parser swallows everything the
    /// shell prints as DCS body. Feeding a synthetic `ST` to both
    /// consumers returns them to ground; at ground it is a no-op, so
    /// calling it on a healthy session costs nothing.
    pub fn leave_control_mode(&mut self) {
        const ST: &[u8] = b"\x1b\\";
        self.dcs.feed(ST);
        self.processor.advance(&mut self.term, ST);
    }

    /// Apply a new geometry to both halves.
    ///
    /// Both, and in this order. The grid is reflowed first so a redraw
    /// racing the child's response finds a consistent grid; the child is
    /// then told through `TIOCSWINSZ`, which is the only way it learns
    /// its size (and what makes it send `SIGWINCH` to its foreground
    /// job). Skipping either half is the classic half-resized terminal.
    pub fn resize(&mut self, size: TermSize) -> std::io::Result<()> {
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        self.term.resize(size);
        let (width, height) = size.pixel_size();
        self.pty.set_winsize(WinsizeBuilder {
            rows: size.rows() as u16,
            cols: size.cols() as u16,
            width,
            height,
        })
    }

    /// The geometry last applied.
    pub fn size(&self) -> TermSize {
        self.size
    }

    /// Columns as the *grid itself* reports them.
    ///
    /// Deliberately read from `Crosswords` rather than from the cached
    /// `size`, so a caller can tell whether a resize actually reached
    /// the grid instead of only being recorded here.
    pub fn grid_cols(&self) -> usize {
        self.term.grid.columns()
    }

    /// Screen rows as the grid reports them. See [`Self::grid_cols`].
    pub fn grid_rows(&self) -> usize {
        self.term.grid.screen_lines()
    }

    /// Lines retained above the screen.
    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }

    /// Put bytes on this session's grid without them ever reaching the
    /// child: what the terminal itself says on the glass, as against what
    /// the program below said.
    ///
    /// The DCS tap is not fed, for the reason [`crate::SshChannel::feed`]
    /// gives at more length: control mode is a remote program's envelope,
    /// and nothing written at this glass may open one.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    pub fn term(&self) -> &Term {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term {
        &mut self.term
    }

    pub fn tap(&self) -> &T {
        self.dcs.tap()
    }

    /// The tap, mutably. The shipped tap ([`crate::tmux_cc::ControlModeTap`])
    /// hands its edges and its peeled body over through draining `take_`
    /// methods, so reaching it through `&self` is not enough.
    pub fn tap_mut(&mut self) -> &mut T {
        self.dcs.tap_mut()
    }

    /// True once the child's end has closed.
    pub fn is_finished(&self) -> bool {
        self.eof
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the cell after `[<warning sign><variation selector>` falls,
    /// which is the whole of what mode 2027 decides for a channel.
    fn column_of_x(grapheme_clustering: bool) -> usize {
        let mut term = new_term(
            TermSize::new(20, 5, 9, 18),
            100,
            grapheme_clustering,
            ReplyListener::detached(),
        );
        let mut processor = Processor::default();
        processor.advance(&mut term, "[\u{26a0}\u{fe0f} X]".as_bytes());
        crate::viewport_text(&term)[0]
            .chars()
            .position(|c| c == 'X')
            .expect("the row holds an X")
    }

    /// A program padding a table counts columns with `wcwidth`, where a
    /// variation selector adds nothing. The grid it writes into agrees,
    /// or every column after the emoji sits one place right of where the
    /// program put it.
    #[test]
    fn a_variation_selector_takes_no_column_of_its_own() {
        assert_eq!(column_of_x(false), 3);
    }

    /// The other half of the same switch: asked for clusters, the grid
    /// gives the pair one two-column slot, as ghostty and kitty do.
    #[test]
    fn grapheme_clustering_widens_the_emoji_it_is_asked_for() {
        assert_eq!(column_of_x(true), 4);
    }

    /// The listener alone, without a PTY under it: what it catches, what it
    /// lets past, and that a detached one is the no-op it claims to be.
    /// `transcript.rs` proves the other half, that a caught reply reaches a
    /// real child.
    #[test]
    fn the_listener_catches_pty_writes_and_nothing_else() {
        let replies = Replies::default();
        let listener = ReplyListener::new(&replies);
        let id = WindowId::from(0u64);

        listener.send_event(RioEvent::PtyWrite(0, "\x1b[?62c".to_string()), id);
        // Not a reply the far end is waiting on: a title is the host's to
        // draw, and this listener is not the host.
        listener.send_event(RioEvent::Title("robco".to_string()), id);
        listener.send_event(RioEvent::PtyWrite(0, "\x1b[1;1R".to_string()), id);

        assert_eq!(*replies.lock().unwrap(), b"\x1b[?62c\x1b[1;1R".to_vec());
    }

    #[test]
    fn a_detached_listener_drops_what_it_hears() {
        let replies = Replies::default();
        let listener = ReplyListener::detached();
        listener.send_event(
            RioEvent::PtyWrite(0, "\x1b[?62c".to_string()),
            WindowId::from(0u64),
        );
        assert!(replies.lock().unwrap().is_empty());
    }
}
