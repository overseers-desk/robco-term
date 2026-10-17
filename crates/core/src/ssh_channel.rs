//! The session variant an SSH channel feeds: remote bytes in, keystrokes
//! straight to the wire.
//!
//! Where [`super::session::Session`] owns a PTY and [`super::tmux_pane::TmuxPane`]
//! owns nothing but a screen, this variant owns a wire it can speak to.
//! That puts it on the PTY session's side of every seam the two differ on:
//! its bytes are its own (drained here, not routed by a gateway), its
//! writes name their destination (no queue-and-drain), its resize has a far
//! half (`window_change` where the PTY does `TIOCSWINSZ`), and its end is
//! session state (the wire's `Eof`, where a pane waits on a model
//! transition).
//!
//! The wire itself stays behind [`SshWire`], a trait with no SSH types in
//! it, so this crate compiles with no transport, no runtime and no crypto
//! under it: tests feed a `Vec`-backed fake, the app hands in the real
//! adapter, and the parser cannot tell them apart.
//!
//! The DCS tap is here from the start, not deferred: a `tmux -CC` typed on
//! a remote shell must be detectable exactly as on a local one, and the
//! tap at ground state is a byte-scan no-op.

use std::time::Instant;

use rio_vt::performer::handler::Processor;

use crate::dcs::{DcsParser, DcsTap};
use crate::session::{Pumped, Replies, ReplyListener, Term};
use crate::size::TermSize;

/// What the wire has for the grid.
#[derive(Debug)]
pub enum SshEvent {
    /// Remote bytes for the parser: the shell's own output.
    Data(Vec<u8>),
    /// Transport progress or failure text, arriving already rendered as
    /// bytes: how a notice looks on the glass is the adapter's decision,
    /// not this crate's. A separate variant from `Data` because a notice
    /// is not the connection having lived (see [`SshChannel::pump`]'s Eof
    /// law), and because nothing a transport says can open a DCS envelope.
    Notice(Vec<u8>),
    /// The remote command's exit status. Carried for a consumer that
    /// wants it; not printed, for parity with a local shell's exit.
    ExitStatus(u32),
    /// The channel is over: remote close, or the connection died.
    Eof,
}

/// The transport as this crate is allowed to see it.
pub trait SshWire: Send {
    /// The next event, if one is waiting. Never blocks.
    fn try_event(&mut self) -> Option<SshEvent>;
    /// Keystrokes, paste, reports: whole or not at all, the writer sheds
    /// and counts past its budget.
    fn send(&mut self, bytes: &[u8]);
    /// Tell the remote pty the glass changed shape.
    fn window_change(&mut self, cols: u16, rows: u16, pix_w: u16, pix_h: u16);
    /// Whole writes the wire refused for want of budget.
    fn sheds(&self) -> u64;
    /// An independently-owned writer onto the same channel, sharing the
    /// budget: the analogue of the dup'd fd `Session::control_mode_writer`
    /// hands a gateway. What makes two writers safe is the same discipline
    /// as there: the surface is single-threaded, and while control mode is
    /// active nothing writes this channel but the gateway
    /// ([`SshChannel::write`]'s swallow).
    fn writer(&self) -> Box<dyn std::io::Write + Send>;
}

/// A channel fed by an SSH connection rather than a PTY.
pub struct SshChannel<T: DcsTap> {
    term: Term,
    processor: Processor,
    dcs: DcsParser<T>,
    size: TermSize,
    wire: Box<dyn SshWire>,
    /// What the grid owes the remote side, waiting for the end of the pump
    /// that parsed the question. See [`crate::session::ReplyListener`].
    replies: Replies,
    /// A remote byte has arrived: the connection lived. What `Eof` means
    /// depends on it.
    lived: bool,
    eof: bool,
}

impl<T: DcsTap> SshChannel<T> {
    /// An empty screen of the glass's geometry, listening on the wire.
    pub fn new(
        size: TermSize,
        scrollback: usize,
        grapheme_clustering: bool,
        tap: T,
        wire: Box<dyn SshWire>,
    ) -> Self {
        let replies = Replies::default();
        Self {
            term: crate::session::new_term(
                size,
                scrollback,
                grapheme_clustering,
                ReplyListener::new(&replies),
            ),
            processor: Processor::default(),
            dcs: DcsParser::new(tap),
            size,
            wire,
            replies,
            lived: false,
            eof: false,
        }
    }

    /// Drain what the connection thread handed over and apply it.
    ///
    /// Every remote chunk goes to both consumers, tap then grid, exactly as
    /// the PTY pump does; the sync timeout is honoured here for the same
    /// reason it is there: owning the loop means nothing else will.
    ///
    /// The Eof law: a connection that lived (a remote byte arrived) ends
    /// its row through `Pumped::eof`, the path a died PTY walks. One that
    /// never lived does not -- the row stays, wearing the transport's
    /// refusal. A shell that fails to spawn never takes a slot; a
    /// connection that fails to establish already has one, and that slot is
    /// the only place its refusal is readable. Closing it is the user's,
    /// once they have read why it is dead.
    pub fn pump(&mut self) -> Pumped {
        let mut out = Pumped::default();
        if self.eof {
            out.eof = true;
            return out;
        }
        while let Some(event) = self.wire.try_event() {
            match event {
                SshEvent::Data(chunk) => {
                    self.lived = true;
                    out.bytes += chunk.len();
                    self.dcs.feed(&chunk);
                    self.processor.advance(&mut self.term, &chunk);
                }
                SshEvent::Notice(chunk) => {
                    out.bytes += chunk.len();
                    self.processor.advance(&mut self.term, &chunk);
                }
                SshEvent::ExitStatus(_) => {}
                SshEvent::Eof => {
                    if self.lived {
                        self.eof = true;
                        out.eof = true;
                    }
                    break;
                }
            }
        }
        // A question the remote program asked in this pump is answered in it,
        // through [`Self::write`] and so under the same control-mode swallow:
        // while the envelope is open the channel is the gateway's, and a
        // report generated then is dropped rather than spliced into its wire.
        let replies = std::mem::take(&mut *self.replies.lock().unwrap());
        if !replies.is_empty() {
            self.write(&replies);
        }
        if let Some(deadline) = self.processor.sync_timeout().sync_timeout() {
            if deadline <= Instant::now() {
                self.processor.stop_sync(&mut self.term);
            }
        }
        out
    }

    /// Put bytes on this channel's grid without them ever touching the wire:
    /// the path a question asked on the glass, and the echo of what is typed
    /// into it, take.
    ///
    /// Two things follow from local bytes being local. `lived` does not
    /// move -- a prompt is the connection asking, not the connection having
    /// lived, so a refusal that arrives after one still leaves the row under
    /// the Eof law and the user still gets to read why. And the DCS tap
    /// never sees them: control mode is a remote program's envelope, and
    /// nothing typed at this glass may open one. That is the same reasoning
    /// `SshEvent::Notice` runs on, and the same one line of body.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Send bytes to the remote side.
    ///
    /// The same swallow the PTY session has: while the tap says control
    /// mode is open, this wire belongs to a gateway and every byte typed,
    /// pasted or reported at it is swallowed rather than spliced.
    pub fn write(&mut self, bytes: &[u8]) {
        if self.dcs.tap().in_control_mode() {
            return;
        }
        self.wire.send(bytes);
    }

    /// Apply a new geometry to both halves: the grid first, so a redraw
    /// racing the remote response finds a consistent grid, then the far
    /// side through `window_change`, which is where its `SIGWINCH` comes
    /// from.
    pub fn resize(&mut self, size: TermSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.resize(size);
        let (width, height) = size.pixel_size();
        self.wire
            .window_change(size.cols() as u16, size.rows() as u16, width, height);
    }

    /// When the pending synchronized update expires, if one is pending.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Whole writes the wire refused. Real here, unlike a pane's zero:
    /// this session owns its writer and the writer has a budget.
    pub fn sheds(&self) -> u64 {
        self.wire.sheds()
    }

    pub fn size(&self) -> TermSize {
        self.size
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

    pub fn tap_mut(&mut self) -> &mut T {
        self.dcs.tap_mut()
    }

    /// The gateway's write half onto this channel. See [`SshWire::writer`].
    pub fn control_writer(&self) -> Box<dyn std::io::Write + Send> {
        self.wire.writer()
    }

    /// Force both parsers out of a DCS string that will never close, the
    /// counterpart of `Session::leave_control_mode` and for the same
    /// reason: at ground it is a no-op, so a healthy session pays nothing.
    pub fn leave_control_mode(&mut self) {
        const ST: &[u8] = b"\x1b\\";
        self.dcs.feed(ST);
        self.processor.advance(&mut self.term, ST);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcs::NoopTap;
    use crate::viewport_text;
    use rio_vt::crosswords::grid::Dimensions;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A wire whose far side is a script: events queue in, writes and
    /// resizes are recorded for the assertions.
    #[derive(Clone, Default)]
    struct FakeWire {
        events: Arc<Mutex<VecDeque<SshEvent>>>,
        sent: Arc<Mutex<Vec<u8>>>,
        resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    }

    impl SshWire for FakeWire {
        fn try_event(&mut self) -> Option<SshEvent> {
            self.events.lock().unwrap().pop_front()
        }
        fn send(&mut self, bytes: &[u8]) {
            self.sent.lock().unwrap().extend_from_slice(bytes);
        }
        fn window_change(&mut self, cols: u16, rows: u16, _pix_w: u16, _pix_h: u16) {
            self.resizes.lock().unwrap().push((cols, rows));
        }
        fn sheds(&self) -> u64 {
            0
        }
        fn writer(&self) -> Box<dyn std::io::Write + Send> {
            #[derive(Clone)]
            struct Onto(Arc<Mutex<Vec<u8>>>);
            impl std::io::Write for Onto {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0.lock().unwrap().extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            Box::new(Onto(self.sent.clone()))
        }
    }

    fn size() -> TermSize {
        TermSize::new(20, 5, 9, 18)
    }

    #[test]
    fn wire_bytes_land_on_the_grid_and_writes_reach_the_wire() {
        let wire = FakeWire::default();
        wire.events
            .lock()
            .unwrap()
            .push_back(SshEvent::Data(b"hello \x1b[1mworld\x1b[0m".to_vec()));
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));

        let pumped = s.pump();
        assert_eq!(pumped.bytes, 19);
        assert!(!pumped.eof);
        assert!(viewport_text(s.term())[0].starts_with("hello world"));

        s.write(b"ls\r");
        assert_eq!(*wire.sent.lock().unwrap(), b"ls\r");
    }

    /// The same answer a PTY session owes its child, owed here to a remote
    /// program: a query parsed in a pump is answered in that pump, out the
    /// wire the question came in on.
    #[test]
    fn a_query_from_the_remote_side_is_answered_on_the_wire() {
        let wire = FakeWire::default();
        wire.events
            .lock()
            .unwrap()
            .push_back(SshEvent::Data(b"\x1b[c\x1b[6n".to_vec()));
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));

        s.pump();
        assert_eq!(
            *wire.sent.lock().unwrap(),
            b"\x1b[?62;4;6;22;52c\x1b[1;1R".to_vec(),
            "the DA and the cursor report both left, in the order asked"
        );
    }

    #[test]
    fn a_resize_reflows_the_grid_and_tells_the_far_side() {
        let wire = FakeWire::default();
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));
        s.resize(TermSize::new(40, 10, 9, 18));
        assert_eq!(s.term().grid.columns(), 40);
        assert_eq!(*wire.resizes.lock().unwrap(), vec![(40, 10)]);
        // The same geometry again is not a second far-side call.
        s.resize(TermSize::new(40, 10, 9, 18));
        assert_eq!(wire.resizes.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_connection_that_never_lived_keeps_its_row_wearing_the_refusal() {
        let wire = FakeWire::default();
        {
            let mut q = wire.events.lock().unwrap();
            q.push_back(SshEvent::Notice(b"[ssh: refused]".to_vec()));
            q.push_back(SshEvent::Eof);
        }
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));
        let pumped = s.pump();
        assert!(
            !pumped.eof,
            "an unlived connection's end is not a row's end"
        );
        assert!(viewport_text(s.term())[0].contains("refused"));
        assert!(!s.pump().eof, "and stays not-ended: the user closes it");
    }

    #[test]
    fn fed_bytes_land_on_the_grid_without_reaching_the_wire_or_the_tap() {
        let wire = FakeWire::default();
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));
        s.feed(b"password: ");
        assert!(viewport_text(s.term())[0].starts_with("password:"));
        assert!(wire.sent.lock().unwrap().is_empty(), "a question is not a keystroke");
        // A control-mode envelope typed at the glass opens nothing: the tap
        // is fed by the wire alone.
        s.feed(b"\x1bP1000p%begin\x1b\\");
        assert_eq!(s.tap().hooks, 0);
        assert_eq!(s.tap().body_bytes, 0);
    }

    #[test]
    fn feeding_does_not_make_a_connection_lived() {
        let wire = FakeWire::default();
        wire.events.lock().unwrap().push_back(SshEvent::Eof);
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));
        s.feed(b"passphrase for id_ed25519: ");
        let pumped = s.pump();
        assert!(!pumped.eof, "a prompt is not the connection having lived");
        assert!(viewport_text(s.term())[0].contains("passphrase"));
    }

    #[test]
    fn eof_ends_the_session_and_stays_ended() {
        let wire = FakeWire::default();
        {
            let mut q = wire.events.lock().unwrap();
            q.push_back(SshEvent::Data(b"bye".to_vec()));
            q.push_back(SshEvent::ExitStatus(1));
            q.push_back(SshEvent::Eof);
        }
        let mut s = SshChannel::new(size(), 100, false, NoopTap::default(), Box::new(wire.clone()));
        let pumped = s.pump();
        assert!(pumped.eof);
        assert_eq!(pumped.bytes, 3);
        assert!(s.pump().eof, "a dead wire reports eof on every pump");
        assert_eq!(s.pump().bytes, 0);
    }
}
