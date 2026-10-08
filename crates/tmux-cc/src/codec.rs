//! The codec proper: commands out, events in, and the queue that joins them.

use std::collections::VecDeque;

use crate::command::Command;
use crate::escape::{unescape_octal, unvis_text};
use crate::event::{Block, Event, Notification};
use crate::ids::{PaneId, SessionId, WindowId};

/// Names the command a [`Event::Reply`] answers.
///
/// Opaque and per-codec. It is not tmux's own command number, which is a
/// server-wide counter shared with every other client and every internal
/// command queue item; the host wants a handle on *its* command, and this is
/// that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(pub u64);

/// A command encoded and enqueued: what to write, and what its reply will be
/// called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub id: CommandId,
    /// The line to write to the gateway, terminating newline included.
    pub wire: String,
}

/// A half-read block.
#[derive(Default)]
struct OpenBlock {
    number: Option<u64>,
    timestamp: Option<i64>,
    flags: Option<u64>,
    body: Vec<Vec<u8>>,
}

/// The tmux control-mode codec.
///
/// Holds three things and nothing else: the bytes of a line that has not
/// finished arriving, the block that has not finished closing, and the
/// commands that have not been answered. No timers, no session, no transport.
#[derive(Default)]
pub struct Codec {
    next_id: u64,
    /// Commands awaiting a reply block, oldest first.
    ///
    /// A FIFO because tmux answers a control client's commands in the order
    /// it received them, so the front of this queue owns the next block.
    pending: VecDeque<CommandId>,
    line: Vec<u8>,
    block: Option<OpenBlock>,
}

impl Codec {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode a command and enqueue it for pairing.
    ///
    /// The caller must actually write [`Sent::wire`], and in this order: the
    /// queue records the order the commands went out, and a line dropped
    /// between here and the transport puts every later reply one place wrong.
    pub fn send(&mut self, command: &Command) -> Sent {
        let id = CommandId(self.next_id);
        self.next_id += 1;
        self.pending.push_back(id);
        Sent {
            id,
            wire: format!("{}\n", command.to_wire()),
        }
    }

    /// How many commands are waiting for a reply.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Forget every unanswered command.
    ///
    /// For teardown, where the replies are never coming. Not called by the
    /// codec itself: a protocol that has gone wrong is the host's to declare
    /// over.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Whether a `%begin` is open. A block spans reads, so this can be true
    /// between calls.
    pub fn in_block(&self) -> bool {
        self.block.is_some()
    }

    /// Feed whatever arrived. Returns the events those bytes completed.
    ///
    /// Chunk boundaries mean nothing: a line, a block, or a `%output` payload
    /// may arrive in any number of pieces and is held until it is whole.
    ///
    /// Splitting on `\n` is safe on this protocol rather than merely
    /// conventional: 0x0a and 0x0d are inside the escaped set, so no `%output`
    /// payload can contain either (see [`crate::escape`]). Reply bodies come
    /// from tmux one screen line per line. The trailing `\r` of tmux's `\r\n`
    /// is stripped.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                // Popped rather than re-sliced: `take` already yields the
                // owned buffer the bytes accumulated in, and `strip_suffix`
                // borrows it, so the `to_vec` this replaces allocated and
                // copied a second whole line. Every `%output` line of every
                // pane's traffic passes through here.
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.line_done(line, &mut events);
            } else {
                self.line.push(byte);
            }
        }
        events
    }

    fn line_done(&mut self, line: Vec<u8>, events: &mut Vec<Event>) {
        // Inside a block every line is payload until that block's own guard: a
        // command's output may itself begin with '%' (pane ids do, and so does
        // the `%message` that `display-message` answers with, observed
        // inside its own reply block, which the manual says cannot happen).
        if self.block.is_some() {
            let is_end = line.starts_with(b"%end ");
            let is_error = line.starts_with(b"%error ");
            if is_end || is_error {
                self.close_block(&line, is_error, events);
            } else if let Some(block) = self.block.as_mut() {
                block.body.push(line);
            }
            return;
        }

        if line.starts_with(b"%begin ") {
            let (timestamp, number, flags) = guard_fields(&line);
            self.block = Some(OpenBlock {
                number,
                timestamp,
                flags,
                body: Vec::new(),
            });
            return;
        }

        if line.first() == Some(&b'%') {
            events.push(Event::Notification(parse_notification(&line)));
            return;
        }

        if line.is_empty() {
            return;
        }

        events.push(Event::Foreign(line));
    }

    fn close_block(&mut self, line: &[u8], error: bool, events: &mut Vec<Event>) {
        let open = self.block.take().unwrap_or_default();
        let (_, closed, _) = guard_fields(line);

        if closed != open.number || closed.is_none() {
            events.push(Event::GuardMismatch {
                began: open.number,
                closed,
            });
            return;
        }

        let block = Block {
            number: open.number,
            timestamp: open.timestamp,
            flags: open.flags,
            error,
            body: open.body,
        };

        // The pairing gate. A block tmux did not mark as answering one of this
        // client's commands never takes one off the queue, however many are
        // waiting: the attach burst is exactly that case, and a burst block
        // swallowing the first real command's slot is how every later reply
        // ends up attributed to the wrong question. See `Block::solicited` for
        // the evidence that the flag says this.
        // `pop_front` only inside the guard: popping to look at the queue is
        // how the burst would take the slot it must not take.
        let paired = if block.solicited() {
            self.pending.pop_front()
        } else {
            None
        };
        match paired {
            Some(id) => events.push(Event::Reply { id, block }),
            None => events.push(Event::Unsolicited { block }),
        }
    }
}

/// `%begin`/`%end`/`%error <timestamp> <number> <flags>`.
///
/// The number alone would pair a reply; all three are read here because the
/// flags field turned out to carry the solicited bit.
fn guard_fields(line: &[u8]) -> (Option<i64>, Option<u64>, Option<u64>) {
    let f = fields(line);
    let get = |i: usize| f.get(i).map(|b| String::from_utf8_lossy(b));
    (
        get(1).and_then(|s| s.parse().ok()),
        get(2).and_then(|s| s.parse().ok()),
        get(3).and_then(|s| s.parse().ok()),
    )
}

/// Split on spaces, dropping empty runs.
fn fields(line: &[u8]) -> Vec<&[u8]> {
    line.split(|&b| b == b' ')
        .filter(|f| !f.is_empty())
        .collect()
}

/// Everything after the first space, verbatim: interior spacing kept. `None`
/// when the line has no space at all.
fn rest_after_first_space(line: &[u8]) -> Option<&[u8]> {
    let at = line.iter().position(|&b| b == b' ')?;
    Some(&line[at + 1..])
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn parse_notification(line: &[u8]) -> Notification {
    let split = line.iter().position(|&b| b == b' ');
    let (name, rest) = match split {
        Some(at) => (&line[..at], &line[at + 1..]),
        None => (line, &line[line.len()..]),
    };
    let name = text(name);
    let args = fields(rest);

    // Every arm that can fail funnels through this, so a dialect that moved is
    // reported as such instead of being silently dropped.
    let malformed = || Notification::Malformed {
        name: name.clone(),
        rest: rest.to_vec(),
    };
    let window = |i: usize| args.get(i).and_then(|a| WindowId::parse(&text(a)));
    let pane = |i: usize| args.get(i).and_then(|a| PaneId::parse(&text(a)));
    let session = |i: usize| args.get(i).and_then(|a| SessionId::parse(&text(a)));
    // The remainder after `n` space-separated fields, verbatim: a name is the
    // rest of the line and may hold runs of spaces, which re-joining split
    // fields would collapse.
    let tail = |n: usize| -> Option<&[u8]> {
        let mut at = rest;
        for _ in 0..n {
            at = rest_after_first_space(at)?;
        }
        Some(at)
    };

    match name.as_str() {
        "%output" => match (pane(0), tail(1)) {
            (Some(pane), Some(payload)) => Notification::Output {
                pane,
                data: unescape_octal(payload),
            },
            _ => malformed(),
        },

        "%extended-output" => {
            // "<pane> <age> [more...] : <value>". The value is taken from the
            // first " : ": the fields before it never contain one, and the
            // value may contain any number.
            let Some(pane) = pane(0) else {
                return malformed();
            };
            let Some(at) = find(rest, b" : ") else {
                return malformed();
            };
            let head = fields(&rest[..at]);
            let Some(age) = head.get(1).and_then(|a| text(a).parse().ok()) else {
                return malformed();
            };
            Notification::ExtendedOutput {
                pane,
                age,
                extra: head[2..].iter().map(|f| text(f)).collect(),
                data: unescape_octal(&rest[at + 3..]),
            }
        }

        "%window-add" => match window(0) {
            Some(window) => Notification::WindowAdd { window },
            None => malformed(),
        },
        "%window-close" => match window(0) {
            Some(window) => Notification::WindowClose { window },
            None => malformed(),
        },
        "%unlinked-window-add" => match window(0) {
            Some(window) => Notification::UnlinkedWindowAdd { window },
            None => malformed(),
        },
        "%unlinked-window-close" => match window(0) {
            Some(window) => Notification::UnlinkedWindowClose { window },
            None => malformed(),
        },
        // A window name is vis-encoded, not raw: see `escape::unvis`.
        "%window-renamed" | "%unlinked-window-renamed" => match (window(0), tail(1)) {
            (Some(window), Some(rest)) => {
                let name_text = unvis_text(rest);
                if name == "%window-renamed" {
                    Notification::WindowRenamed {
                        window,
                        name: name_text,
                    }
                } else {
                    Notification::UnlinkedWindowRenamed {
                        window,
                        name: name_text,
                    }
                }
            }
            _ => malformed(),
        },
        "%window-pane-changed" => match (window(0), pane(1)) {
            (Some(window), Some(pane)) => Notification::WindowPaneChanged { window, pane },
            _ => malformed(),
        },

        "%layout-change" => {
            if args.len() < 3 {
                return malformed();
            }
            match window(0) {
                Some(window) => Notification::LayoutChange {
                    window,
                    layout: text(args[1]),
                    visible_layout: text(args[2]),
                    flags: args.get(3).map_or_else(String::new, |f| text(f)),
                },
                None => malformed(),
            }
        }

        "%session-changed" => match (session(0), tail(1)) {
            (Some(session), Some(rest)) => Notification::SessionChanged {
                session,
                name: unvis_text(rest),
            },
            _ => malformed(),
        },
        // The manual says `%session-renamed name`. tmux 3.5a sends the session
        // id first: `%session-renamed $1 two\\back`
        // (`tests/transcripts/03-rename.txt`). Observed bytes win.
        "%session-renamed" => match (session(0), tail(1)) {
            (Some(session), Some(rest)) => Notification::SessionRenamed {
                session,
                name: unvis_text(rest),
            },
            _ => malformed(),
        },
        "%session-window-changed" => match (session(0), window(1)) {
            (Some(session), Some(window)) => Notification::SessionWindowChanged { session, window },
            _ => malformed(),
        },
        "%sessions-changed" => Notification::SessionsChanged,

        "%client-detached" => match args.first() {
            Some(client) => Notification::ClientDetached {
                client: text(client),
            },
            None => malformed(),
        },
        "%client-session-changed" => match (args.first(), session(1), tail(2)) {
            (Some(client), Some(session), Some(rest)) => Notification::ClientSessionChanged {
                client: text(client),
                session,
                name: unvis_text(rest),
            },
            _ => malformed(),
        },

        "%config-error" => Notification::ConfigError {
            message: text(rest),
        },
        "%message" => Notification::Message {
            message: text(rest),
        },

        "%pause" => match pane(0) {
            Some(pane) => Notification::Pause { pane },
            None => malformed(),
        },
        "%continue" => match pane(0) {
            Some(pane) => Notification::Continue { pane },
            None => malformed(),
        },
        "%pane-mode-changed" => match pane(0) {
            Some(pane) => Notification::PaneModeChanged { pane },
            None => malformed(),
        },

        "%paste-buffer-changed" => match args.first() {
            Some(buffer) => Notification::PasteBufferChanged { name: text(buffer) },
            None => malformed(),
        },
        "%paste-buffer-deleted" => match args.first() {
            Some(buffer) => Notification::PasteBufferDeleted { name: text(buffer) },
            None => malformed(),
        },

        "%subscription-changed" => {
            let Some(at) = find(rest, b" : ") else {
                return malformed();
            };
            let head = fields(&rest[..at]);
            if head.len() < 5 {
                return malformed();
            }
            match (
                SessionId::parse(&text(head[1])),
                WindowId::parse(&text(head[2])),
                PaneId::parse(&text(head[4])),
            ) {
                (Some(session), Some(window), Some(pane)) => Notification::SubscriptionChanged {
                    name: text(head[0]),
                    session,
                    window,
                    window_index: text(head[3]),
                    pane,
                    extra: head[5..].iter().map(|f| text(f)).collect(),
                    value: rest[at + 3..].to_vec(),
                },
                _ => malformed(),
            }
        }

        "%exit" => Notification::Exit {
            reason: if rest.is_empty() {
                None
            } else {
                Some(text(rest))
            },
        },

        _ => Notification::Unknown {
            name,
            rest: rest.to_vec(),
        },
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PaneId;

    fn events(input: &[u8]) -> Vec<Event> {
        Codec::new().feed(input)
    }

    fn one(input: &[u8]) -> Event {
        let mut got = events(input);
        assert_eq!(got.len(), 1, "{got:?}");
        got.remove(0)
    }

    fn note(input: &[u8]) -> Notification {
        match one(input) {
            Event::Notification(n) => n,
            other => panic!("not a notification: {other:?}"),
        }
    }

    /// The framing does not copy the line it framed.
    ///
    /// `feed` accumulates a line into one buffer and hands it on at the
    /// newline; a reply body keeps that buffer verbatim, so the allocation the
    /// bytes arrived in is the allocation the block carries. Measured by
    /// identity rather than by a timer: with a `to_vec` in the middle the
    /// buffer handed on is a second allocation, and its address cannot be the
    /// first one's, because the first is still alive when it is made.
    #[test]
    fn a_body_line_reaches_its_block_in_the_buffer_it_arrived_in() {
        for tail in [&b"\n"[..], &b"\r\n"[..]] {
            let mut codec = Codec::new();
            codec.feed(b"%begin 1 1 1\r\n");
            // Enough of the line in before the address is taken that pushing
            // the rest cannot reallocate under the reading.
            codec.feed(b"a line of body text");
            codec.feed(&tail[..tail.len() - 1]);
            let arrived = codec.line.as_ptr();
            assert!(!codec.line.is_empty(), "the buffer is the one accumulating");

            let mut events = codec.feed(b"\n%end 1 1 1\r\n");
            let Event::Unsolicited { block } = events.remove(0) else {
                panic!("a block");
            };
            assert_eq!(block.body.len(), 1);
            assert_eq!(block.body[0], b"a line of body text");
            assert_eq!(
                block.body[0].as_ptr(),
                arrived,
                "the line was copied on its way to the block"
            );
        }
    }

    #[test]
    fn output_arrives_unescaped_and_keeps_its_spacing() {
        assert_eq!(
            note(b"%output %0 two  spaces\\015\\012\r\n"),
            Notification::Output {
                pane: PaneId::parse("%0").unwrap(),
                data: b"two  spaces\r\n".to_vec(),
            }
        );
    }

    #[test]
    fn an_empty_output_payload_is_still_an_output() {
        assert_eq!(
            note(b"%output %0 \r\n"),
            Notification::Output {
                pane: PaneId::parse("%0").unwrap(),
                data: Vec::new(),
            }
        );
    }

    #[test]
    fn extended_output_splits_at_the_first_colon_and_keeps_later_ones() {
        assert_eq!(
            note(b"%extended-output %2 17 spare : a : b\r\n"),
            Notification::ExtendedOutput {
                pane: PaneId::parse("%2").unwrap(),
                age: 17,
                extra: vec!["spare".to_string()],
                data: b"a : b".to_vec(),
            }
        );
    }

    #[test]
    fn a_window_name_is_the_rest_of_the_line() {
        assert_eq!(
            note(b"%window-renamed @1 two  spaces here \r\n"),
            Notification::WindowRenamed {
                window: WindowId::parse("@1").unwrap(),
                name: "two  spaces here ".to_string(),
            }
        );
    }

    #[test]
    fn exit_carries_a_reason_only_when_there_is_one() {
        assert_eq!(note(b"%exit\r\n"), Notification::Exit { reason: None });
        assert_eq!(
            note(b"%exit server exited\r\n"),
            Notification::Exit {
                reason: Some("server exited".to_string()),
            }
        );
    }

    #[test]
    fn an_unknown_notification_keeps_its_bytes() {
        assert_eq!(
            note(b"%some-future-thing @1 stuff\r\n"),
            Notification::Unknown {
                name: "%some-future-thing".to_string(),
                rest: b"@1 stuff".to_vec(),
            }
        );
    }

    #[test]
    fn a_known_name_with_the_wrong_shape_is_malformed_not_unknown() {
        assert_eq!(
            note(b"%window-add %1\r\n"),
            Notification::Malformed {
                name: "%window-add".to_string(),
                rest: b"%1".to_vec(),
            }
        );
        assert_eq!(
            note(b"%layout-change @1 only-one\r\n"),
            Notification::Malformed {
                name: "%layout-change".to_string(),
                rest: b"@1 only-one".to_vec(),
            }
        );
    }

    #[test]
    fn a_layout_change_for_a_window_not_current_has_empty_flags() {
        let Notification::LayoutChange { window, flags, .. } = note(b"%layout-change @1 a b \r\n")
        else {
            panic!("not a layout change");
        };
        assert!(window.to_string() == "@1" && flags.is_empty());
    }

    #[test]
    fn a_reply_pairs_with_the_command_that_asked() {
        let mut codec = Codec::new();
        let first = codec.send(&Command::Server);
        let second = codec.send(&Command::ListPanes);
        assert_eq!(
            first.wire,
            "display-message -p \"#{socket_path} #{pid} #{session_id} #{host_short}\"\n"
        );
        assert_eq!(codec.pending(), 2);

        let got = codec.feed(
            b"%begin 100 7 1\r\nhostname\r\n%end 100 7 1\r\n\
              %begin 101 9 1\r\n@0 %0 1 bash\r\n%end 101 9 1\r\n",
        );
        assert_eq!(got.len(), 2);
        match &got[0] {
            Event::Reply { id, block } => {
                assert_eq!(*id, first.id);
                assert_eq!(block.text(), vec!["hostname"]);
                assert!(!block.error);
            }
            other => panic!("{other:?}"),
        }
        match &got[1] {
            Event::Reply { id, block } => {
                assert_eq!(*id, second.id);
                assert_eq!(block.text(), vec!["@0 %0 1 bash"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(codec.pending(), 0);
    }

    #[test]
    fn an_error_block_still_pairs_and_says_so() {
        let mut codec = Codec::new();
        let sent = codec.send(&Command::NewWindow);
        let got = codec.feed(b"%begin 1 2 1\r\nparse error: no\r\n%error 1 2 1\r\n");
        match &got[..] {
            [Event::Reply { id, block }] => {
                assert_eq!(*id, sent.id);
                assert!(block.error);
                assert_eq!(block.text(), vec!["parse error: no"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_attach_burst_never_takes_a_pending_slot() {
        // The whole point of the flags gate: a flags-0 block arriving while a
        // command is outstanding must leave the queue alone.
        let mut codec = Codec::new();
        let sent = codec.send(&Command::Server);
        let got = codec.feed(
            b"%begin 1 276 0\r\nburst\r\n%end 1 276 0\r\n\
              %begin 2 281 1\r\nmine\r\n%end 2 281 1\r\n",
        );
        match &got[..] {
            [Event::Unsolicited { block }, Event::Reply { id, block: mine }] => {
                assert_eq!(block.text(), vec!["burst"]);
                assert!(!block.solicited());
                assert_eq!(*id, sent.id);
                assert_eq!(mine.text(), vec!["mine"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_solicited_block_with_nothing_pending_is_unsolicited() {
        match &events(b"%begin 1 2 1\r\n%end 1 2 1\r\n")[..] {
            [Event::Unsolicited { .. }] => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_percent_line_inside_a_block_is_body_not_notification() {
        let mut codec = Codec::new();
        let sent = codec.send(&Command::Server);
        let got = codec.feed(b"%begin 1 2 1\r\n%message hello\r\n%0 %1\r\n%end 1 2 1\r\n");
        match &got[..] {
            [Event::Reply { id, block }] => {
                assert_eq!(*id, sent.id);
                assert_eq!(block.text(), vec!["%message hello", "%0 %1"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_mismatched_guard_drops_the_body_and_says_so() {
        let mut codec = Codec::new();
        codec.send(&Command::Server);
        let got = codec.feed(b"%begin 1 2 1\r\nbody\r\n%end 1 3 1\r\n");
        assert_eq!(
            got,
            vec![Event::GuardMismatch {
                began: Some(2),
                closed: Some(3),
            }]
        );
        // The command is still outstanding: nothing answered it.
        assert_eq!(codec.pending(), 1);
    }

    #[test]
    fn a_block_survives_arbitrary_chunk_boundaries() {
        let stream: &[u8] = b"%begin 1 2 1\r\nbody line\r\n%end 1 2 1\r\n%output %0 hi\r\n";
        for split in 1..stream.len() {
            let mut codec = Codec::new();
            codec.send(&Command::Server);
            let mut got = codec.feed(&stream[..split]);
            // Split anywhere inside the block and the block is still open
            // between the two calls: the state that spans reads is real.
            let after_begin = b"%begin 1 2 1\r\n".len();
            let after_end = stream.len() - b"%output %0 hi\r\n".len();
            let inside = (after_begin..after_end).contains(&split);
            assert_eq!(codec.in_block(), inside, "split at {split}");
            got.extend(codec.feed(&stream[split..]));
            assert!(!codec.in_block());
            assert_eq!(got.len(), 2, "split at {split}: {got:?}");
            assert!(matches!(got[0], Event::Reply { .. }), "split at {split}");
            assert!(matches!(
                got[1],
                Event::Notification(Notification::Output { .. })
            ));
        }
    }

    #[test]
    fn a_foreign_line_is_reported_not_swallowed() {
        assert_eq!(
            one(b"a@host:~$ \r\n"),
            Event::Foreign(b"a@host:~$ ".to_vec())
        );
        // An empty line is neither foreign nor an event.
        assert!(events(b"\r\n").is_empty());
    }

    #[test]
    fn a_line_without_its_newline_is_not_an_event_yet() {
        let mut codec = Codec::new();
        assert!(codec.feed(b"%output %0 partial").is_empty());
        assert_eq!(
            codec.feed(b"\r\n"),
            vec![Event::Notification(Notification::Output {
                pane: PaneId::parse("%0").unwrap(),
                data: b"partial".to_vec(),
            })]
        );
    }
}
