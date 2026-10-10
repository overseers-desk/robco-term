//! The session variant a tmux pane feeds: `%output` in, `send-keys` out.
//!
//! [`super::session::Session`] is PTY-only: its read loop, its EOF law and
//! its resize all assume a child on a master fd. A tmux pane has none of
//! that: its bytes arrive as `%output` payloads peeled off the gateway's
//! control stream, and its keystrokes leave as `send-keys` commands on that
//! same stream. The variant lives here, beside the PTY session, and
//! [`ChannelSession`] is the one type a channel slot holds.
//!
//! What a [`TmuxPane`] does not have:
//!
//! * **A DCS tap.** The envelope lives on the gateway's own PTY; a pane's
//!   payload is already inside it. A `tmux -CC` run *inside a tmux pane*
//!   is tmux's own nesting problem, refused by tmux itself.
//! * **A transport.** Keystrokes buffer in the session and the host drains
//!   them to the gateway ([`TmuxPane::take_input`]); the session cannot
//!   name the pane it is, because the pane id is the channel row's
//!   (`app::channels::Row::tmux`) and panes move under a window
//!   (`%window-pane-changed`) without the session noticing.
//! * **An EOF.** A pane ends when tmux says `%window-close`, a model
//!   transition, not a session state.
//! * **An answer to a query.** The PTY session and the SSH channel each hand
//!   their grid a live [`crate::session::ReplyListener`], so a DA or CPR asked
//!   of them is answered on the pump that parsed it. A pane's grid gets the
//!   detached one: its answers would have to leave as `send-keys` on the
//!   gateway's control stream, a different mechanism through a queue this type
//!   does not own, and the program asking is anyway talking to tmux's own
//!   terminal rather than to this one.

use std::time::Instant;

use rio_vt::performer::handler::Processor;

use crate::dcs::DcsTap;
use crate::session::{Pumped, ReplyListener, Session, Term};
use crate::size::TermSize;
use crate::ssh_channel::SshChannel;

/// A channel fed by a tmux pane rather than a PTY.
pub struct TmuxPane {
    term: Term,
    processor: Processor,
    size: TermSize,
    /// Keystrokes waiting to become `send-keys`. See [`Self::take_input`].
    input: Vec<u8>,
}

impl TmuxPane {
    /// An empty screen of the glass's geometry.
    ///
    /// The geometry is the glass's and not the pane's, deliberately: every
    /// channel is the same rectangle of glass, and tmux is told that size
    /// once per client (`refresh-client -C`, the client-size law in
    /// `app::channels`' module doc), so pane and glass agree except for
    /// split windows, which the channel model does not draw.
    pub fn new(size: TermSize, scrollback: usize, grapheme_clustering: bool) -> Self {
        Self {
            term: crate::session::new_term(
                size,
                scrollback,
                grapheme_clustering,
                ReplyListener::detached(),
            ),
            processor: Processor::default(),
            size,
            input: Vec::new(),
        }
    }

    /// Apply bytes the gateway routed here: a `%output` payload, or the
    /// capture-and-cursor bootstrap the gateway synthesises on attach.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Queue keystrokes for the pane. The write side of the diversion: the
    /// host drains this to `Gateway::send_keys` on its next pump.
    pub fn write(&mut self, bytes: &[u8]) {
        self.input.extend_from_slice(bytes);
    }

    /// Drain what [`Self::write`] queued.
    pub fn take_input(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.input)
    }

    /// Reflow the grid. There is no `TIOCSWINSZ` half: the program behind
    /// this screen sits on tmux's PTY, and tmux resizes it from the client
    /// size the host publishes.
    pub fn resize(&mut self, size: TermSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.resize(size);
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
}

/// What a channel slot holds: a PTY, a tmux pane, or an SSH channel.
///
/// The methods are the surface `app::window` already drove when every slot
/// was a [`Session`]; each match arm says what the operation means on the
/// side that has no native one. The SSH variant sits with the PTY on every
/// arm the pane differs on: its bytes, its writer, its resize's far half
/// and its end are all its own (`crate::ssh_channel`'s module doc walks
/// the four).
pub enum ChannelSession<T: DcsTap> {
    /// A PTY-backed session: a shell this program spawned, or the gateway
    /// channel whose PTY carries the control stream.
    Pty(Session<T>),
    /// A tmux pane's screen, fed by the gateway.
    TmuxPane(TmuxPane),
    /// A remote shell on an SSH channel this program owns.
    Ssh(SshChannel<T>),
}

impl<T: DcsTap> ChannelSession<T> {
    /// Drain the slot's own source. A tmux pane has none; its bytes arrive
    /// through [`TmuxPane::feed`] on the gateway's pump, so its arm is idle.
    /// An SSH channel's source is the connection thread's queue.
    pub fn pump(&mut self) -> Pumped {
        match self {
            ChannelSession::Pty(s) => s.pump(),
            ChannelSession::TmuxPane(_) => Pumped::default(),
            ChannelSession::Ssh(s) => s.pump(),
        }
    }

    /// Take this slot's output at `rate` bytes a second, where the slot
    /// has a source of its own to take it from.
    ///
    /// Only the PTY has one. A pane's bytes and an SSH channel's have
    /// already been read by the time they arrive here, a gateway's parse
    /// and a connection thread's queue respectively, so slowing either
    /// arm would hold read bytes back from the grid rather than leave
    /// them unread at the source. That is the display emulation this is
    /// not: see [`Session::set_rate`].
    pub fn set_rate(&mut self, rate: Option<u32>) {
        match self {
            ChannelSession::Pty(s) => s.set_rate(rate),
            ChannelSession::TmuxPane(_) | ChannelSession::Ssh(_) => {}
        }
    }

    /// Bytes the terminal puts on this slot's grid itself, reaching no
    /// child, no wire and no tmux server: a question asked on the glass,
    /// the find line, and the echo of what is typed into either.
    pub fn feed(&mut self, bytes: &[u8]) {
        match self {
            ChannelSession::Pty(s) => s.feed(bytes),
            ChannelSession::TmuxPane(s) => s.feed(bytes),
            ChannelSession::Ssh(s) => s.feed(bytes),
        }
    }

    /// Bytes from the keyboard (or a paste, or a mouse report). A PTY's go
    /// to the child; a pane's queue for `send-keys`.
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            ChannelSession::Pty(s) => s.write(bytes),
            ChannelSession::TmuxPane(s) => {
                s.write(bytes);
                Ok(())
            }
            ChannelSession::Ssh(s) => {
                s.write(bytes);
                Ok(())
            }
        }
    }

    pub fn resize(&mut self, size: TermSize) -> std::io::Result<()> {
        match self {
            ChannelSession::Pty(s) => s.resize(size),
            ChannelSession::TmuxPane(s) => {
                s.resize(size);
                Ok(())
            }
            ChannelSession::Ssh(s) => {
                s.resize(size);
                Ok(())
            }
        }
    }

    pub fn term(&self) -> &Term {
        match self {
            ChannelSession::Pty(s) => s.term(),
            ChannelSession::TmuxPane(s) => s.term(),
            ChannelSession::Ssh(s) => s.term(),
        }
    }

    pub fn term_mut(&mut self) -> &mut Term {
        match self {
            ChannelSession::Pty(s) => s.term_mut(),
            ChannelSession::TmuxPane(s) => s.term_mut(),
            ChannelSession::Ssh(s) => s.term_mut(),
        }
    }

    /// When the pending synchronized update expires. A PTY session and an
    /// SSH channel each hold one, because each owns its parser; a pane's
    /// BSU/ESU pass through tmux, which runs its own timeout server-side.
    pub fn sync_deadline(&self) -> Option<Instant> {
        match self {
            ChannelSession::Pty(s) => s.sync_deadline(),
            ChannelSession::TmuxPane(_) => None,
            ChannelSession::Ssh(s) => s.sync_deadline(),
        }
    }

    /// How many writes this slot has thrown away for want of queue
    /// ([`Session::sheds`]).
    ///
    /// Zero on a tmux pane, and not because nobody counted: a pane's
    /// input queue is drained whole by the host on every pump
    /// ([`TmuxPane::take_input`]), so it holds one pump's typing and has no
    /// cap to shed against. What can shed on that path is the gateway's own
    /// command queue, one level up, and the gateway counts that itself.
    pub fn sheds(&self) -> u64 {
        match self {
            ChannelSession::Pty(s) => s.sheds(),
            ChannelSession::TmuxPane(_) => 0,
            ChannelSession::Ssh(s) => s.sheds(),
        }
    }

    pub fn tmux_pane_mut(&mut self) -> Option<&mut TmuxPane> {
        match self {
            ChannelSession::TmuxPane(s) => Some(s),
            _ => None,
        }
    }

    /// The DCS tap, on the variants that carry one: a pane has none, its
    /// payload being already inside the gateway's envelope.
    pub fn tap_mut(&mut self) -> Option<&mut T> {
        match self {
            ChannelSession::Pty(s) => Some(s.tap_mut()),
            ChannelSession::TmuxPane(_) => None,
            ChannelSession::Ssh(s) => Some(s.tap_mut()),
        }
    }

    /// A gateway's write half onto this slot's wire: the dup'd fd on a PTY,
    /// the budget-sharing writer on an SSH channel, nothing on a pane.
    pub fn control_writer(&mut self) -> Option<std::io::Result<Box<dyn std::io::Write + Send>>> {
        match self {
            ChannelSession::Pty(s) => Some(s.control_mode_writer()),
            ChannelSession::TmuxPane(_) => None,
            ChannelSession::Ssh(s) => Some(Ok(s.control_writer())),
        }
    }

    /// Force the slot's parsers out of an unclosable DCS string, where the
    /// slot has parsers of its own to force.
    pub fn leave_control_mode(&mut self) {
        match self {
            ChannelSession::Pty(s) => s.leave_control_mode(),
            ChannelSession::TmuxPane(_) => {}
            ChannelSession::Ssh(s) => s.leave_control_mode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport_text;
    use rio_vt::crosswords::grid::Dimensions;

    fn size() -> TermSize {
        TermSize::new(20, 5, 9, 18)
    }

    #[test]
    fn fed_bytes_land_on_the_grid_and_input_queues_until_drained() {
        let mut s = TmuxPane::new(size(), 100, false);
        s.feed(b"hello \x1b[1mworld\x1b[0m");
        assert!(viewport_text(s.term())[0].starts_with("hello world"));

        s.write(b"ls");
        s.write(b"\r");
        assert_eq!(s.take_input(), b"ls\r");
        assert!(s.take_input().is_empty(), "a drain drains");
    }

    #[test]
    fn a_resize_reflows_the_grid_alone() {
        let mut s = TmuxPane::new(size(), 100, false);
        s.feed(b"abc");
        s.resize(TermSize::new(40, 10, 9, 18));
        assert_eq!(s.term().grid.columns(), 40);
        assert!(viewport_text(s.term())[0].starts_with("abc"));
    }
}
