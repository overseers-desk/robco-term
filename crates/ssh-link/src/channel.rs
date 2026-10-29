//! The sync-facing half of one SSH channel: what the event loop holds.
//!
//! The loop side never blocks and never sees the runtime. Bytes arrive on a
//! bounded queue drained by `try_event` on the surface's pump; keystrokes
//! leave through an unbounded sender gated by a byte budget, because the
//! bound that matters is bytes queued, not messages queued (a paste is one
//! message however large).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

/// What one channel's remote side did, delivered in wire order.
///
/// `Notice` carries connection progress and failure text for the user's
/// glass; a failure notice precedes the `Eof` the supervisor sends after it,
/// so an epitaph always lands before the row dies.
#[derive(Debug)]
pub enum WireEvent {
    /// Payload bytes from the remote pty.
    Data(Vec<u8>),
    /// The remote command's exit status. Carried, not printed: a local
    /// shell's status is not printed either.
    ExitStatus(u32),
    /// Connection progress and failure text, one line, no trailing newline.
    Notice(String),
    /// The channel is over: remote close, or the connection under it died.
    Eof,
}

/// Commands from the loop side to this channel's task.
#[derive(Debug)]
pub(crate) enum ChannelCmd {
    Data(Vec<u8>),
    WindowChange { cols: u16, rows: u16, pix_w: u16, pix_h: u16 },
    Close,
}

/// How many `WireEvent`s may wait unread before the reader task suspends.
///
/// The suspension is the flow control: an unread queue stops the reader,
/// russh stops extending the SSH window, and TCP pushes back to the server.
/// At russh's ~32KiB data packets this is on the order of 2MiB in flight,
/// the same magnitude as the gateway's pending cap.
pub(crate) const EVENT_QUEUE: usize = 64;

/// Outbound bytes a channel will hold unflushed before shedding, and the
/// same whole-write-or-nothing law: a partial paste is worse than none.
/// Mirrors the PTY session's input cap.
pub(crate) const INPUT_CAP: usize = 4 << 20;

/// One channel's endpoints, held (boxed behind `term`'s wire trait) inside
/// the session variant.
pub struct ChannelHandle {
    pub(crate) events: mpsc::Receiver<WireEvent>,
    pub(crate) cmd: mpsc::UnboundedSender<ChannelCmd>,
    /// Outbound bytes accepted but not yet on the wire. The supervisor
    /// subtracts as `data` calls complete.
    pub(crate) queued: Arc<AtomicUsize>,
    /// Whole writes refused for want of budget.
    pub(crate) sheds: Arc<AtomicU64>,
}

impl ChannelHandle {
    /// The next event, if one is waiting. Called from the surface's pump.
    /// A sender that vanished without its `Eof` (a task cancelled by the
    /// runtime going down) reads as `Eof`, so no row waits on a wire that is
    /// gone.
    pub fn try_event(&mut self) -> Option<WireEvent> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(WireEvent::Eof),
        }
    }

    /// Queue keystrokes for the wire, whole or not at all.
    pub fn send(&mut self, bytes: &[u8]) {
        let held = self.queued.load(Ordering::Relaxed);
        if held.saturating_add(bytes.len()) > INPUT_CAP {
            self.sheds.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
        // A closed peer means the connection died; the death reaches the
        // grid as this channel's `connection lost` and Eof, so a refused send
        // needs no second report here.
        let _ = self.cmd.send(ChannelCmd::Data(bytes.to_vec()));
    }

    /// Tell the remote pty the glass changed shape.
    pub fn window_change(&mut self, cols: u16, rows: u16, pix_w: u16, pix_h: u16) {
        let _ = self
            .cmd
            .send(ChannelCmd::WindowChange { cols, rows, pix_w, pix_h });
    }

    pub fn sheds(&self) -> u64 {
        self.sheds.load(Ordering::Relaxed)
    }

    /// An independently-owned writer onto the same channel, sharing the
    /// byte budget and the shed counter. What a tmux gateway writes with
    /// once the channel's shell has become a control-mode wire.
    pub fn writer(&self) -> InputWriter {
        InputWriter {
            cmd: self.cmd.clone(),
            queued: self.queued.clone(),
            sheds: self.sheds.clone(),
        }
    }
}

/// See [`ChannelHandle::writer`]. Whole-write-or-nothing under the shared
/// budget; a refusal is reported as `Ok` with the bytes counted shed, the
/// shape `Session::write` gives its own refusals, because there is nothing
/// on the wire for the caller to retry.
pub struct InputWriter {
    cmd: mpsc::UnboundedSender<ChannelCmd>,
    queued: Arc<AtomicUsize>,
    sheds: Arc<AtomicU64>,
}

impl std::io::Write for InputWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let held = self.queued.load(Ordering::Relaxed);
        if held.saturating_add(bytes.len()) > INPUT_CAP {
            self.sheds.fetch_add(1, Ordering::Relaxed);
            return Ok(bytes.len());
        }
        self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
        if self.cmd.send(ChannelCmd::Data(bytes.to_vec())).is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the connection is over",
            ));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for ChannelHandle {
    fn drop(&mut self) {
        let _ = self.cmd.send(ChannelCmd::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::endpoints;

    #[test]
    fn a_write_over_budget_sheds_whole_and_is_counted() {
        let (mut handle, wire) = endpoints();
        handle.send(&vec![b'x'; INPUT_CAP - 1]);
        assert_eq!(handle.sheds(), 0);
        // One byte of budget left: a two-byte write sheds whole, and the
        // budgeted byte is still spendable after it.
        handle.send(b"ab");
        assert_eq!(handle.sheds(), 1);
        handle.send(b"c");
        assert_eq!(handle.sheds(), 1);
        drop(wire);
    }
}
