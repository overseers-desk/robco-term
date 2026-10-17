//! The DCS tap: the seam a DCS consumer stands on, and the parser that
//! feeds it.
//!
//! The application owns the read loop itself rather than using rio-vt's
//! `Machine`. rio-vt's `Handler` trait has no hook/put/unhook, and
//! the `Performer` that does see DCS is private and discards anything it
//! does not recognise, so a device-control stream is invisible from
//! the `Crosswords` side. The public `Perform` trait *does* see it. The
//! price is that we cannot use rio-vt's `Machine`: the read loop is ours,
//! and it feeds every byte to two consumers, this tap and
//! `Processor::advance`.
//!
//! Feeding both is safe rather than duplicative. The VT parser consumes a
//! DCS block whole and hands `Handler` nothing from its body, so the body
//! never reaches the grid; text after `ESC \` does. The tap sees the body,
//! the grid sees the rest, and neither needs the other to have run.
//!
//! Nothing here knows what a block means. [`NoopTap`] counts blocks and is
//! the test double; the shipped tap is [`ControlModeTap`](crate::tmux_cc::ControlModeTap),
//! which recognises the tmux control-mode envelope and lives in
//! [`tmux_cc`](crate::tmux_cc) with the rest of the protocol's knowledge.

use rio_vt::performer::parser::{Params, Parser, Perform};

/// What the read loop hands a DCS consumer.
///
/// Deliberately narrower than `Perform`: a tap has no business seeing
/// ordinary printable bytes or CSI sequences, which belong to the grid.
/// A tap implements this rather than `Perform`, so a protocol layer never
/// has to care that a VT parser is underneath.
pub trait DcsTap {
    /// A DCS block opened. `params`/`action` identify it: tmux control
    /// mode is params `[1000]` with action `'p'`.
    fn hook(&mut self, params: &[u16], action: char);

    /// One byte of the block's body.
    fn put(&mut self, byte: u8);

    /// The block closed (`ESC \` or `ST`).
    fn unhook(&mut self);

    /// Whether the tap stands inside a control-mode envelope right now.
    ///
    /// Two things read this answer, and both need the same one:
    ///
    /// * [`DcsParser`], deciding whether the body belongs to the tap
    ///   verbatim rather than to rio-vt's DCS string state, which eats
    ///   bytes a control-mode body carries as payload (see
    ///   [`tmux_cc`](crate::tmux_cc)).
    /// * [`Session`](crate::Session), deciding whether it may still push
    ///   its own queue at the PTY. Control mode shares the wire with a
    ///   second writer (`app::tmux::Gateway`, through
    ///   [`Session::control_mode_writer`](crate::Session::control_mode_writer))
    ///   and the protocol on it is line-oriented, so a queued paste tail
    ///   flushed underneath the gateway splices the user's half-line into
    ///   the middle of a command. See [`Session::pump`](crate::Session::pump).
    ///
    /// A tap that has no control mode leaves this false and gets rio-vt's
    /// standard DCS handling throughout.
    fn in_control_mode(&self) -> bool {
        false
    }

    /// Whether an `ESC \` arriving now ends the control-mode envelope.
    fn may_close(&self) -> bool {
        true
    }
}

/// The no-op tap: sees everything, remembers nothing.
///
/// It counts, and only so the transcript test can prove the bytes
/// arrive. This is the test double; the shipped tap is
/// [`ControlModeTap`](crate::tmux_cc::ControlModeTap).
#[derive(Default, Debug)]
pub struct NoopTap {
    pub hooks: usize,
    pub body_bytes: usize,
    pub unhooks: usize,
}

impl DcsTap for NoopTap {
    fn hook(&mut self, _params: &[u16], _action: char) {
        self.hooks += 1;
    }
    fn put(&mut self, _byte: u8) {
        self.body_bytes += 1;
    }
    fn unhook(&mut self) {
        self.unhooks += 1;
    }
}

/// Adapts a [`DcsTap`] to rio-vt's `Perform`, discarding everything that
/// is not DCS.
///
/// The empty methods are the point of the type: `Perform` is a wide
/// trait covering the whole VT vocabulary, and the grid is already
/// handling all of it through `Processor`. Only the three DCS callbacks
/// forward.
struct DcsOnly<'a, T: DcsTap> {
    tap: &'a mut T,
}

impl<T: DcsTap> Perform for DcsOnly<'_, T> {
    fn hook(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        self.tap.hook(&flat, action);
    }

    fn put(&mut self, byte: u8) {
        self.tap.put(byte);
    }

    fn unhook(&mut self) {
        self.tap.unhook();
    }
}

/// Where the parser's byte scanner stands, which is not always where its
/// VT parser stands.
///
/// The scanner exists because a control-mode body is *not* a VT DCS
/// string. rio-vt implements the string state ECMA-48 specifies, which
/// drops `0x7F`, drops `0x80..=0xFF`, and treats a raw `0x9C` as the
/// terminator; a tmux control-mode envelope carries all three as ordinary
/// payload (see [`tmux_cc`](crate::tmux_cc) for what emits them). Under the
/// standard state machine the high half of such a body vanishes, a stray
/// `0x9C` closes the envelope early, and the first colour sequence inside
/// it unhooks the block.
///
/// So a block the tap answers [`DcsTap::in_control_mode`] for bypasses the
/// VT parser and reaches the tap verbatim, terminated by an `ESC \` only
/// where the tap answers [`DcsTap::may_close`]: tmux closes after `%exit`.
/// Every other DCS keeps rio-vt's standard handling: a tap that wants none
/// of the body discards it anyway, and a sixel that ends on an 8-bit `ST`
/// must still be able to end.
#[derive(Debug)]
enum ScanState {
    /// Outside any envelope. Bytes go to the VT parser in bulk, up to
    /// the next `ESC` -- the only byte that can open a DCS here, since
    /// this parser dispatches a bare C1 `0x90` as `execute` rather than
    /// as an introducer.
    Scan,
    /// Inside a DCS introducer. Fed one byte at a time so that the
    /// takeover can happen on exactly the byte the hook fires on, before
    /// the parser has swallowed any of the body. Persists across `feed`
    /// calls, because a read boundary can fall inside `ESC P 1000 p`.
    Intro { after_esc: bool },
    /// Inside a control-mode envelope, where the VT parser is not
    /// involved. `esc` holds a pending `ESC` whose next byte decides
    /// whether it was the terminator or payload.
    Envelope { esc: bool },
}

/// The tap's own VT parser, running alongside the grid's.
///
/// Its own parser and not a shared one: `Processor` owns its parser
/// privately, and a DCS block's state spans reads, so the tap needs a
/// state machine that persists across `feed` calls the same way.
pub struct DcsParser<T: DcsTap> {
    parser: Parser,
    tap: T,
    state: ScanState,
}

impl<T: DcsTap> DcsParser<T> {
    pub fn new(tap: T) -> Self {
        Self {
            parser: Parser::default(),
            tap,
            state: ScanState::Scan,
        }
    }

    /// Hand bytes to the VT parser, which dispatches any DCS in them to
    /// the tap.
    fn vt(&mut self, bytes: &[u8]) {
        let mut perform = DcsOnly { tap: &mut self.tap };
        self.parser.advance(&mut perform, bytes);
    }

    /// Feed the same bytes the grid is getting.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            i += match self.state {
                ScanState::Scan => self.scan(&bytes[i..]),
                ScanState::Intro { after_esc } => self.intro(bytes[i], after_esc),
                ScanState::Envelope { esc } => self.envelope(bytes[i], esc),
            };
        }
    }

    /// Bulk-feed up to the next `ESC`, then arm the introducer matcher.
    fn scan(&mut self, rest: &[u8]) -> usize {
        let n = rest.iter().position(|&b| b == 0x1B).unwrap_or(rest.len());
        if n > 0 {
            self.vt(&rest[..n]);
        }
        if n == rest.len() {
            return n;
        }
        self.vt(&rest[n..n + 1]);
        self.state = ScanState::Intro { after_esc: true };
        n + 1
    }

    /// Track the DCS introducer grammar (`ESC P`, parameter and
    /// intermediate bytes, then a final in `0x40..=0x7E`) alongside the
    /// parser, so the byte that fires `hook` can be recognised as it is
    /// fed rather than after the body is gone.
    fn intro(&mut self, byte: u8, after_esc: bool) -> usize {
        self.vt(&[byte]);
        self.state = if after_esc {
            match byte {
                b'P' => ScanState::Intro { after_esc: false },
                // `ESC ESC`: the second one is the introducer's.
                0x1B => ScanState::Intro { after_esc: true },
                _ => ScanState::Scan,
            }
        } else {
            match byte {
                // Parameters and intermediates: still in the introducer.
                0x20..=0x3F => ScanState::Intro { after_esc: false },
                // The final byte. `hook` has just fired, so the tap can
                // say whether the block it opened is its control mode.
                0x40..=0x7E if self.tap.in_control_mode() => ScanState::Envelope { esc: false },
                _ => ScanState::Scan,
            }
        };
        1
    }

    /// One byte of a control-mode body; `ESC \` ends it where [`DcsTap::may_close`] holds.
    fn envelope(&mut self, byte: u8, esc: bool) -> usize {
        if esc {
            if byte == b'\\' && self.tap.may_close() {
                // Let the VT parser close the block it opened, so it
                // returns to ground and the tap hears its `unhook`
                // through the same path every other DCS uses.
                self.vt(b"\x1b\\");
                self.state = ScanState::Scan;
                return 1;
            }
            // Not the terminator, so the `ESC` was payload: a colour
            // sequence or hyperlink in a `capture-pane -e` reply.
            self.tap.put(0x1B);
        }
        if byte == 0x1B {
            self.state = ScanState::Envelope { esc: true };
        } else {
            self.tap.put(byte);
            self.state = ScanState::Envelope { esc: false };
        }
        1
    }

    pub fn tap(&self) -> &T {
        &self.tap
    }

    pub fn tap_mut(&mut self) -> &mut T {
        &mut self.tap
    }
}

impl<T: DcsTap + Default> Default for DcsParser<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap that reconstructs the body, so the test can assert on
    /// content rather than only on counts. It claims no control mode, so
    /// it sees every block through rio-vt's own DCS string state.
    #[derive(Default)]
    struct Recording {
        opened: Option<(Vec<u16>, char)>,
        body: Vec<u8>,
        closed: bool,
    }

    impl DcsTap for Recording {
        fn hook(&mut self, params: &[u16], action: char) {
            self.opened = Some((params.to_vec(), action));
        }
        fn put(&mut self, byte: u8) {
            self.body.push(byte);
        }
        fn unhook(&mut self) {
            self.closed = true;
        }
    }

    const DCS: &[u8] = b"\x1bP1000p%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\x1b\\AFTER-DCS";

    #[test]
    fn a_dcs_block_reaches_the_tap_whole() {
        let mut p = DcsParser::new(Recording::default());
        p.feed(DCS);
        let t = p.tap();
        assert_eq!(t.opened, Some((vec![1000], 'p')));
        assert!(t.closed);
        assert_eq!(
            String::from_utf8_lossy(&t.body),
            "%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1"
        );
    }

    #[test]
    fn a_block_split_across_reads_is_still_one_block() {
        // The read loop hands over whatever the PTY gave it, which has
        // no relationship to escape-sequence boundaries. Splitting mid
        // body must not close or reopen the block.
        let mut p = DcsParser::new(Recording::default());
        let (a, b) = DCS.split_at(20);
        p.feed(a);
        assert!(p.tap().opened.is_some());
        assert!(!p.tap().closed, "block closed early at a read boundary");
        p.feed(b);
        let t = p.tap();
        assert!(t.closed);
        assert_eq!(
            String::from_utf8_lossy(&t.body),
            "%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1"
        );
    }

    #[test]
    fn ordinary_text_never_reaches_the_tap() {
        let mut p = DcsParser::new(Recording::default());
        p.feed(b"plain text\r\n\x1b[31mred\x1b[0m");
        let t = p.tap();
        assert!(t.opened.is_none());
        assert!(t.body.is_empty());
    }
}
