//! tmux control mode on the wire: the `DCS 1000 p` envelope and the tap
//! that peels it. Named for the protocol, as the `tmux-cc` crate is: that
//! crate parses the lines this module peels out of the envelope.
//!
//! This is the whole of what `term` knows about tmux. Everything below it
//! ([`dcs`](crate::dcs)) sees a DCS block and a tap; everything above it
//! (`app::tmux`, `app::channels`) decides what an attachment means. In
//! between sits [`ControlModeTap`]: it recognises the envelope, buffers the
//! peeled body, and reports the two edges the surface acts on -- the envelope
//! opening (detection) and its closing (`ST`). It decides nothing.
//!
//! The envelope's body is not a VT DCS string, which is why
//! [`DcsParser`](crate::dcs::DcsParser) hands it over verbatim while
//! [`ControlModeTap::in_control_mode`] is true. tmux escapes only
//! `0x00..=0x1F` and the backslash inside `%output`, as octal, so `0x7F`,
//! the whole `0x80..=0xFF` run and a raw `0x9C` among them ride the
//! envelope as payload; a `capture-pane -e` reply carries raw `ESC [ ... m`
//! by design. Under ECMA-48's string state those would be dropped, dropped,
//! and read as the terminator. tmux closes the envelope only after an `%exit`
//! line, so an `ESC \` before one is payload, the hyperlink terminator inside
//! a `capture-pane -e` reply among it. A pane that itself shows a line
//! beginning `%exit` followed by a hyperlink terminator still closes it early.

use crate::dcs::DcsTap;

/// tmux control mode's DCS params and action: `ESC P 1000 p`.
const TMUX_PARAMS: &[u16] = &[1000];
const TMUX_ACTION: char = 'p';
const EXIT: &[u8] = b"%exit";

/// The shipped tap: recognises `DCS 1000 p`, tmux's control-mode
/// envelope, and peels its body for the gateway.
///
/// Three things cross this seam, each drained by its own `take_`:
///
/// * **Detection** ([`ControlModeTap::take_detected`]): the envelope
///   opened. The host turns the channel this session sits on into
///   an attachment (`app::channels::Channels::attach`) and raises a
///   gateway over this tap's body stream.
/// * **The body** ([`ControlModeTap::take_body`]): the bytes between
///   `ESC P 1000 p` and `ST`, exactly what the gateway's codec eats.
///   Buffered here rather than handed to a callback so the tap stays
///   free of any `tmux-cc` type: `term` knows there is an envelope, not
///   what is in it.
/// * **The close** ([`ControlModeTap::take_ended`]): `ST` arrived, after
///   the `%exit` line tmux ends every attachment with, so the codec has
///   already seen the detach; the host collapses the page on this edge
///   only if that line never reached it.
///
/// Any DCS that is not `1000p` is ignored whole: its body neither
/// buffers nor flips the flags, so a sixel or a `DECRQSS` reply cannot
/// impersonate an attachment.
#[derive(Default, Debug)]
pub struct ControlModeTap {
    /// Inside a `1000p` envelope right now.
    active: bool,
    detected: bool,
    ended: bool,
    body: Vec<u8>,
    /// How much of `EXIT` the current body line has matched; `None` once not.
    line: Option<usize>,
    /// The last complete body line began with `EXIT`.
    exited: bool,
}

impl ControlModeTap {
    /// The envelope opened since the last call. One edge per envelope.
    pub fn take_detected(&mut self) -> bool {
        std::mem::take(&mut self.detected)
    }

    /// The envelope closed (`ST`) since the last call.
    pub fn take_ended(&mut self) -> bool {
        std::mem::take(&mut self.ended)
    }

    /// Drain the peeled body bytes that arrived since the last call.
    pub fn take_body(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }
}

impl DcsTap for ControlModeTap {
    fn hook(&mut self, params: &[u16], action: char) {
        self.active = params == TMUX_PARAMS && action == TMUX_ACTION;
        if self.active {
            self.detected = true;
            // A new envelope is a new conversation. Whatever a dead one left
            // undrained (a tail nobody pumped before its gateway went, its
            // close edge) must not reach the next gateway as its own.
            self.body.clear();
            self.ended = false;
            self.line = Some(0);
            self.exited = false;
        }
    }

    fn put(&mut self, byte: u8) {
        if !self.active {
            return;
        }
        self.body.push(byte);
        match (byte, self.line) {
            (b'\n', line) => {
                self.exited = line == Some(EXIT.len());
                self.line = Some(0);
            }
            (_, Some(n)) if n < EXIT.len() => self.line = (EXIT[n] == byte).then_some(n + 1),
            _ => {}
        }
    }

    fn unhook(&mut self) {
        if self.active {
            self.active = false;
            self.ended = true;
        }
    }

    fn in_control_mode(&self) -> bool {
        self.active
    }

    fn may_close(&self) -> bool {
        self.exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcs::DcsParser;

    const TMUX_DCS: &[u8] =
        b"\x1bP1000p%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\r\n%exit\r\n\x1b\\AFTER-DCS";

    #[test]
    fn the_control_tap_detects_peels_and_reports_the_close() {
        let mut p = DcsParser::new(ControlModeTap::default());
        p.feed(TMUX_DCS);
        let tap = p.tap_mut();
        assert!(tap.take_detected());
        assert!(!tap.take_detected(), "one edge per envelope");
        assert_eq!(
            String::from_utf8_lossy(&tap.take_body()),
            "%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\r\n%exit\r\n"
        );
        assert!(tap.take_body().is_empty(), "a drain drains");
        assert!(tap.take_ended());
        assert!(!tap.in_control_mode());
    }

    #[test]
    fn a_foreign_dcs_is_no_attachment() {
        // A DECRQSS reply: DCS 1 $ r ... ST. Wrong params, wrong action.
        let mut p = DcsParser::new(ControlModeTap::default());
        p.feed(b"\x1bP1$r0m\x1b\\");
        let tap = p.tap_mut();
        assert!(!tap.take_detected());
        assert!(tap.take_body().is_empty());
        assert!(!tap.take_ended(), "a foreign close is not tmux's close");
    }

    /// A recorded tmux 3.5a attach whose panes print every byte there is,
    /// this crate's own copy of the transcript `tmux-cc` reads.
    ///
    /// tmux escapes `0x00..=0x1F` and the backslash as octal inside
    /// `%output` and nothing else, so `0x7F` and the whole `0x80..=0xFF`
    /// run -- `0x9C` among them -- ride the envelope raw. Under rio-vt's
    /// DCS string state those are, in order, dropped, dropped, and read
    /// as the terminator.
    const OCTAL_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/04-output-octal.txt");

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Feed `wire` at several chunk sizes, returning the body each time.
    ///
    /// The chunk sizes matter as much as the bytes: the read loop hands
    /// over whatever the PTY gave it, so a boundary can fall between an
    /// `ESC` and the byte that decides whether it was a terminator.
    fn peel_at_every_boundary(wire: &[u8]) -> Vec<Vec<u8>> {
        [1usize, 3, 7, 64, wire.len()]
            .into_iter()
            .map(|chunk| {
                let mut p = DcsParser::new(ControlModeTap::default());
                let mut body = Vec::new();
                for part in wire.chunks(chunk) {
                    p.feed(part);
                    body.extend(p.tap_mut().take_body());
                }
                assert!(
                    p.tap_mut().take_ended(),
                    "chunked by {chunk}: the envelope never closed"
                );
                body
            })
            .collect()
    }

    #[test]
    fn every_byte_of_a_recorded_envelope_reaches_the_tap_unchanged() {
        // The canonical peel: from after `ESC P 1000 p` to the `ESC \`,
        // which is how `tmux-cc`'s transcript support reads the same
        // fixture and how tmux emits it.
        let start = find(OCTAL_FIXTURE, b"\x1bP1000p").expect("an envelope") + 7;
        let end = start + find(&OCTAL_FIXTURE[start..], b"\x1b\\").expect("an ST");
        let expected = &OCTAL_FIXTURE[start..end];

        // Not a vacuous assertion: these are the three bytes the VT DCS
        // state would have eaten, and they are really in the fixture.
        assert!(expected.contains(&0x7F), "no raw 0x7F in the fixture");
        assert!(expected.contains(&0x9C), "no raw 0x9C in the fixture");
        assert!(expected.contains(&0xFF), "no raw 0xFF in the fixture");

        for (body, chunk) in peel_at_every_boundary(OCTAL_FIXTURE).into_iter().zip([
            1usize,
            3,
            7,
            64,
            OCTAL_FIXTURE.len(),
        ]) {
            assert_eq!(
                body.len(),
                expected.len(),
                "chunked by {chunk}: {} body bytes for {} on the wire",
                body.len(),
                expected.len()
            );
            assert_eq!(body, expected, "chunked by {chunk}: a byte changed");
        }
    }

    #[test]
    fn a_capture_reply_keeps_the_escape_sequences_it_is_made_of() {
        // `capture-pane -e` answers with the page's own escape sequences,
        // so the first `ESC [ 31 m` is payload. `ESC \` ends the
        // envelope only after `%exit`, and a coloured prompt is full of the other kind.
        let body: &[u8] =
            b"%begin 1 1 1\r\n\x1b[31mred\x1b[0m $ \x1b[1;32mprompt\x1b[m\r\n%end 1 1 1\r\n%exit\r\n";
        let mut wire = Vec::from(&b"\x1bP1000p"[..]);
        wire.extend_from_slice(body);
        wire.extend_from_slice(b"\x1b\\");

        for (peeled, chunk) in
            peel_at_every_boundary(&wire)
                .into_iter()
                .zip([1usize, 3, 7, 64, wire.len()])
        {
            assert_eq!(peeled, body, "chunked by {chunk}: the reply was cut");
        }

        // And the tap is back on the ground afterwards, ready for the
        // text that follows the envelope.
        let mut p = DcsParser::new(ControlModeTap::default());
        p.feed(&wire);
        assert!(!p.tap().in_control_mode());
    }

    #[test]
    fn a_foreign_dcs_still_ends_the_way_its_own_standard_says() {
        // The takeover is the tmux envelope's alone. A sixel that closes
        // on an 8-bit `ST` must still close, or the tap would swallow
        // every attachment that came after it.
        let mut p = DcsParser::new(ControlModeTap::default());
        p.feed(b"\x1bPq#0;2;0;0;0\x9c");
        assert!(!p.tap().in_control_mode());

        // A real attachment right behind it is still seen.
        p.feed(TMUX_DCS);
        let tap = p.tap_mut();
        assert!(tap.take_detected(), "the sixel ate the attachment");
        assert_eq!(
            String::from_utf8_lossy(&tap.take_body()),
            "%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\r\n%exit\r\n"
        );
    }

    #[test]
    fn the_body_survives_read_boundaries_and_drains_between_them() {
        let mut p = DcsParser::new(ControlModeTap::default());
        let (a, b) = TMUX_DCS.split_at(20);
        p.feed(a);
        assert!(p.tap_mut().take_detected());
        let first = p.tap_mut().take_body();
        p.feed(b);
        let second = p.tap_mut().take_body();
        let whole = [first, second].concat();
        assert_eq!(
            String::from_utf8_lossy(&whole),
            "%begin 1 1 1\r\n%output %1 hi\r\n%end 1 1 1\r\n%exit\r\n"
        );
        assert!(p.tap_mut().take_ended());
    }

    #[test]
    fn a_hyperlink_terminator_before_exit_is_payload() {
        let reply: &[u8] =
            b"%begin 1 2 1\r\n\x1b]8;;http://x.y\x1b\\link\x1b]8;;\x1b\\\r\n%end 1 2 1\r\n";
        let mut wire = Vec::from(&b"\x1bP1000p"[..]);
        wire.extend_from_slice(reply);
        for chunk in [1usize, 3, 7, 64, wire.len()] {
            let mut p = DcsParser::new(ControlModeTap::default());
            let mut body = Vec::new();
            for part in wire.chunks(chunk) {
                p.feed(part);
                body.extend(p.tap_mut().take_body());
                assert!(
                    !p.tap_mut().take_ended(),
                    "chunked by {chunk}: closed early"
                );
            }
            assert!(
                p.tap().in_control_mode(),
                "chunked by {chunk}: not still open"
            );
            assert_eq!(body, reply, "chunked by {chunk}: the reply was cut");
        }
    }

    #[test]
    fn exit_with_a_reason_closes_the_envelope() {
        let mut p = DcsParser::new(ControlModeTap::default());
        p.feed(b"\x1bP1000p%exit server exited unexpectedly\r\n\x1b\\");
        assert!(p.tap_mut().take_ended());
        assert!(!p.tap().in_control_mode());
    }
}
