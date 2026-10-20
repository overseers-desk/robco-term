//! The DCS tap: the seam a DCS consumer stands on, and the parser that
//! feeds it.
//!
//! The application owns the read loop itself rather than using rio-vt's
//! `Machine`. rio-vt's `Handler` trait has no hook/put/unhook, and
//! the `Performer` that does see DCS is private and discards anything it
//! does not recognise, so a device-control stream is invisible from
//! the `Crosswords` side. The public `Perform` trait *does* see it. The
//! price is that we cannot use rio-vt's `Machine`: the read loop is ours,
//! and every byte it reads enters [`DcsParser::feed`], which is the only
//! thing that feeds the grid.
//!
//! One feeder and not two. rio-vt's DCS string state leaves a block on
//! `0x18`, `0x1A`, `0x1B` and `0x9C`, so a grid parser fed the same bytes
//! drops out of the envelope at the first `ESC [ 31 m` a `capture-pane -e`
//! reply carries and prints the rest of tmux's protocol as text on the
//! glass. So this parser splits the stream: what stands outside the
//! envelope it hands back for the grid, the introducer and the body it
//! keeps, and a grid that never saw `ESC P 1000 p` has no DCS state for
//! anything in the body to break.
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
    /// Held as well as fed: until the final byte fires `hook` these bytes
    /// may belong to the grid or to an envelope, and there is no
    /// un-sending them, so they wait in `held` until the tap has answered.
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
    /// Introducer bytes whose destination is not yet known (`ScanState::Intro`).
    held: Vec<u8>,
}

/// The longest introducer the hold waits out. tmux's is seven bytes, so one
/// past this cannot be the envelope and is let go to the grid.
const MAX_HELD: usize = 16;

impl<T: DcsTap> DcsParser<T> {
    pub fn new(tap: T) -> Self {
        Self {
            parser: Parser::default(),
            tap,
            state: ScanState::Scan,
            held: Vec::new(),
        }
    }

    /// Hand bytes to the VT parser, which dispatches any DCS in them to
    /// the tap.
    fn vt(&mut self, bytes: &[u8]) {
        let mut perform = DcsOnly { tap: &mut self.tap };
        self.parser.advance(&mut perform, bytes);
    }

    /// Feed the bytes as they came off the wire; `grid` receives, appended
    /// in order, the ones the grid may see. Everything from `ESC P 1000 p`
    /// to its `ST` stays here, terminator included; a foreign DCS reaches
    /// the grid whole, since the grid's own parser is what consumes it.
    pub fn feed(&mut self, bytes: &[u8], grid: &mut Vec<u8>) {
        let mut i = 0;
        while i < bytes.len() {
            i += match self.state {
                ScanState::Scan => self.scan(&bytes[i..], grid),
                ScanState::Intro { after_esc } => self.intro(bytes[i], after_esc, grid),
                ScanState::Envelope { esc } => self.envelope(bytes[i], esc),
            };
        }
    }

    /// Return to ground from an envelope the wire will never close: the
    /// control client died, so no `%exit` is coming and
    /// [`DcsTap::may_close`] would hold the scanner inside for good. The VT
    /// parser is handed the `ESC \` it is waiting for, so the tap hears its
    /// `unhook` through the usual path; nothing reaches the grid, which
    /// never entered the envelope. At ground it is a no-op.
    pub fn reset(&mut self) {
        self.vt(b"\x1b\\");
        self.held.clear();
        self.state = ScanState::Scan;
    }

    /// Bulk-feed up to the next `ESC`, then arm the introducer matcher.
    fn scan(&mut self, rest: &[u8], grid: &mut Vec<u8>) -> usize {
        let n = rest.iter().position(|&b| b == 0x1B).unwrap_or(rest.len());
        if n > 0 {
            self.vt(&rest[..n]);
            grid.extend_from_slice(&rest[..n]);
        }
        if n == rest.len() {
            return n;
        }
        self.vt(&rest[n..n + 1]);
        self.held.push(0x1B);
        self.state = ScanState::Intro { after_esc: true };
        n + 1
    }

    /// Track the DCS introducer grammar (`ESC P`, parameter and
    /// intermediate bytes, then a final in `0x40..=0x7E`) alongside the
    /// parser, so the byte that fires `hook` can be recognised as it is
    /// fed rather than after the body is gone. The held bytes go to the
    /// grid when the block proves foreign and nowhere when the tap claims it.
    fn intro(&mut self, byte: u8, after_esc: bool, grid: &mut Vec<u8>) -> usize {
        self.vt(&[byte]);
        self.held.push(byte);
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
                0x20..=0x3F if self.held.len() < MAX_HELD => ScanState::Intro { after_esc: false },
                // The final byte. `hook` has just fired, so the tap can
                // say whether the block it opened is its control mode.
                0x40..=0x7E if self.tap.in_control_mode() => ScanState::Envelope { esc: false },
                _ => ScanState::Scan,
            }
        };
        match self.state {
            ScanState::Scan => grid.append(&mut self.held),
            ScanState::Envelope { .. } => self.held.clear(),
            ScanState::Intro { .. } => {}
        }
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
    /// content rather than only on counts. `claims` says whether it takes a
    /// `1000p` block as its control mode (the shipped tap's answer) or
    /// sees every block through rio-vt's own DCS string state; in control
    /// mode it lets the envelope close once an `%exit` line has ended.
    #[derive(Default)]
    struct Recording {
        claims: bool,
        active: bool,
        opened: Option<(Vec<u16>, char)>,
        body: Vec<u8>,
        closed: bool,
    }

    impl DcsTap for Recording {
        fn hook(&mut self, params: &[u16], action: char) {
            self.opened = Some((params.to_vec(), action));
            self.active = self.claims && params == [1000] && action == 'p';
        }
        fn put(&mut self, byte: u8) {
            self.body.push(byte);
        }
        fn unhook(&mut self) {
            self.closed = true;
            self.active = false;
        }
        fn in_control_mode(&self) -> bool {
            self.active
        }
        fn may_close(&self) -> bool {
            self.body.ends_with(b"%exit\r\n")
        }
    }

    const DCS: &[u8] = b"\x1bP1000p%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\x1b\\AFTER-DCS";

    /// Feed `wire` in `chunk`-sized reads; answer what the grid was handed.
    fn fed<T: DcsTap>(p: &mut DcsParser<T>, wire: &[u8], chunk: usize) -> Vec<u8> {
        let mut grid = Vec::new();
        for part in wire.chunks(chunk) {
            p.feed(part, &mut grid);
        }
        grid
    }

    #[test]
    fn a_foreign_dcs_block_reaches_the_tap_and_the_grid_whole() {
        let mut p = DcsParser::new(Recording::default());
        assert_eq!(fed(&mut p, DCS, DCS.len()), DCS);
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
        fed(&mut p, a, a.len());
        assert!(p.tap().opened.is_some());
        assert!(!p.tap().closed, "block closed early at a read boundary");
        fed(&mut p, b, b.len());
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
        let text = b"plain text\r\n\x1b[31mred\x1b[0m";
        assert_eq!(fed(&mut p, text, text.len()), text);
        let t = p.tap();
        assert!(t.opened.is_none());
        assert!(t.body.is_empty());
    }

    /// The bug this parser exists to keep off the glass: a `capture-pane -e`
    /// reply carries `ESC [ 31 m` and a raw `0x9C`, each of which ends
    /// rio-vt's DCS string state, so a grid fed the envelope prints the rest
    /// of the protocol. Fed only what stands outside, the grid sees the
    /// text before and after and nothing between, at every read boundary,
    /// including one inside `ESC P 1000 p` and one after a lone `ESC`.
    #[test]
    fn the_grid_gets_what_stands_outside_the_envelope_at_every_read_boundary() {
        let body: &[u8] = b"%begin 1 1 1\r\n\x1b[31mred\x1b[0m\x9c tail\r\n%end 1 1 1\r\n%exit\r\n";
        let wire = [b"before\x1bP1000p", body, b"\x1b\\after"].concat();
        for chunk in [1usize, 3, 7, 64, wire.len()] {
            let mut p = DcsParser::new(Recording {
                claims: true,
                ..Recording::default()
            });
            assert_eq!(
                fed(&mut p, &wire, chunk),
                b"beforeafter",
                "chunked by {chunk}"
            );
            assert_eq!(p.tap().body, body, "chunked by {chunk}");
            assert!(
                p.tap().closed,
                "chunked by {chunk}: the envelope never closed"
            );
        }
    }

    #[test]
    fn an_introducer_too_long_to_be_the_envelope_is_let_go() {
        let mut p = DcsParser::new(Recording::default());
        let wire = [&b"\x1bP"[..], &[b';'; MAX_HELD][..], b"q\x1b\\x"].concat();
        assert_eq!(fed(&mut p, &wire, wire.len()), wire);
        assert!(p.held.is_empty());
    }

    #[test]
    fn a_reset_leaves_an_envelope_the_wire_never_closed() {
        let mut p = DcsParser::new(Recording {
            claims: true,
            ..Recording::default()
        });
        assert!(fed(&mut p, b"\x1bP1000p%begin", 64).is_empty());
        p.reset();
        assert!(p.tap().closed, "the tap never heard the block close");
        assert_eq!(fed(&mut p, b"after", 64), b"after");
    }
}
