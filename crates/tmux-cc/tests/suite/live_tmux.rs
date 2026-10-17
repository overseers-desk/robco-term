//! The encoder against a tmux that is actually running.
//!
//! The transcript tests prove the decoder reads what tmux said. They cannot
//! prove the encoder says anything tmux understands, because a fixture is a
//! recording of commands that already worked. These tests close that loop: the
//! codec's own bytes go to a live server, and the answer comes back through
//! the codec.
//!
//! They need `/usr/bin/tmux` and a few seconds. If it is not there they skip
//! rather than fail: a machine without tmux is a machine that cannot run
//! this, not a codec that is wrong.

use crate::support;

use support::{Server, TMUX};
use tmux_cc::{Command, Event, Notification, PaneId, WindowId};

fn have_tmux() -> bool {
    let ok = std::path::Path::new(TMUX).exists();
    if !ok {
        eprintln!("skipping: no {TMUX}");
    }
    ok
}

/// Collect the notifications a gateway has heard so far.
fn notifications(events: Vec<Event>) -> Vec<Notification> {
    events
        .into_iter()
        .filter_map(|e| match e {
            Event::Notification(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn a_new_window_asked_for_through_the_codec_arrives_and_parses() {
    if !have_tmux() {
        return;
    }
    let server = Server::start("live-new-window", "one", "/bin/cat");
    let mut gateway = server.attach("one");
    gateway.settle(600);

    gateway.send(&Command::NewWindow);
    assert!(
        gateway.wait_for(b"%window-add", 5000),
        "tmux never announced the window: {}",
        String::from_utf8_lossy(gateway.transcript())
    );
    gateway.settle(300);

    let notes = notifications(gateway.replay());
    let added: Vec<String> = notes
        .iter()
        .filter_map(|n| match n {
            Notification::WindowAdd { window } => Some(window.as_str().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(added, vec!["@1"]);

    // And tmux agrees, from outside the protocol.
    let windows = server.run(&["list-windows", "-F", "#{window_id}"]);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["@0", "@1"]);
}

#[test]
fn the_whole_command_set_is_a_dialect_tmux_speaks() {
    if !have_tmux() {
        return;
    }
    let server = Server::start("live-commands", "one", "/bin/cat");
    let mut gateway = server.attach("one");
    gateway.settle(600);

    // Every variant the encoder can produce, once. A command tmux does not
    // understand comes back as an `%error` block, so "no reply is an error"
    // is the assertion that the quoting and the flags are right.
    let pane = PaneId::parse("%0").unwrap();
    let mut commands = vec![
        Command::Server,
        Command::ListPanes,
        Command::ListWindows,
        Command::ListWindowPanes(WindowId::parse("@0").unwrap()),
        Command::capture_pane(&pane),
        Command::CursorPosition(pane.clone()),
        Command::ClientSize {
            columns: 100,
            rows: 30,
        },
        Command::NewWindow,
    ];
    commands.extend(Command::send_keys(&pane, b"hello\r"));
    commands.push(Command::KillWindow(WindowId::parse("@1").unwrap()));
    commands.push(Command::DetachClient);

    for command in &commands {
        gateway.send(command);
        gateway.settle(250);
    }
    gateway.settle(800);

    let events = gateway.replay();
    let blocks: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Reply { id, block } => Some((*id, block)),
            _ => None,
        })
        .collect();
    assert_eq!(
        blocks.len(),
        commands.len(),
        "not every command was answered"
    );
    for (at, ((id, block), command)) in blocks.iter().zip(&commands).enumerate() {
        assert!(
            !block.error,
            "tmux refused `{}`: {:?}",
            command.to_wire(),
            block.text()
        );
        // The reply that came back k-th is the one the k-th command asked
        // for: the FIFO law, against a live server rather than a fixture.
        assert_eq!(*id, tmux_cc::CommandId(at as u64));
    }
}

#[test]
fn the_bootstrap_replies_are_the_shapes_the_reference_reads() {
    if !have_tmux() {
        return;
    }
    let server = Server::start("live-bootstrap", "one", "/bin/cat");
    server.run(&["new-window", "-t", "one", "-d", "/bin/cat"]);
    let mut gateway = server.attach("one");
    gateway.settle(600);

    gateway.send(&Command::Server);
    gateway.send(&Command::ListPanes);
    gateway.send(&Command::ListWindows);
    gateway.settle(1000);

    let blocks: Vec<_> = gateway
        .replay()
        .into_iter()
        .filter_map(|e| match e {
            Event::Reply { block, .. } => Some(block),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 3);

    // The identity line: one line, four fields, and the socket is this
    // server's own.
    let identity = blocks[0].text();
    assert_eq!(identity.len(), 1);
    let (socket, pid, session, host) =
        tmux_cc::parse_server(&identity[0]).expect("the server line");
    assert_eq!(socket, server.socket().display().to_string());
    assert!(pid > 0);
    assert_eq!(session.as_str(), "$0");
    assert!(!host.is_empty());

    // list-panes: "<window> <pane> <active> <width> <height> <name>", one
    // line per pane.
    let panes = blocks[1].text();
    assert_eq!(panes.len(), 2);
    for line in &panes {
        let fields: Vec<&str> = line.splitn(6, ' ').collect();
        assert_eq!(fields.len(), 6, "{line:?}");
        assert!(WindowId::parse(fields[0]).is_some(), "{line:?}");
        assert!(PaneId::parse(fields[1]).is_some(), "{line:?}");
        assert_eq!(fields[2], "1", "only active panes here: {line:?}");
        assert!(fields[3].parse::<u16>().is_ok_and(|w| w > 0), "{line:?}");
        assert!(fields[4].parse::<u16>().is_ok_and(|h| h > 0), "{line:?}");
    }

    // list-windows: "<window> <name>".
    let windows = blocks[2].text();
    assert_eq!(windows.len(), 2);
    for line in &windows {
        let (id, name) = line.split_once(' ').expect("window id and name");
        assert!(WindowId::parse(id).is_some(), "{line:?}");
        assert!(!name.is_empty());
    }
}

#[test]
fn keys_sent_as_hex_reach_the_pane_and_come_back_as_output() {
    if !have_tmux() {
        return;
    }
    // The bytes chosen are exactly the ones quoting would have mangled: a
    // double quote, a backslash, a hash, a dollar. `send-keys -H` is why none
    // of them need quoting, and `%output` is where they reappear.
    let server = Server::start("live-keys", "one", "/bin/cat");
    let mut gateway = server.attach("one");
    gateway.settle(600);

    let pane = PaneId::parse("%0").unwrap();
    let awkward: &[u8] = b"a\"b\\c#d$e\r";
    for command in Command::send_keys(&pane, awkward) {
        gateway.send(&command);
    }
    assert!(
        gateway.wait_for(b"%output", 5000),
        "the pane never wrote anything back"
    );
    gateway.settle(400);

    let echoed: Vec<Vec<u8>> = notifications(gateway.replay())
        .into_iter()
        .filter_map(|n| match n {
            Notification::Output { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    let joined: Vec<u8> = echoed.concat();
    assert!(
        support::find(&joined, b"a\"b\\c#d$e").is_some(),
        "the awkward bytes did not survive: {:?}",
        String::from_utf8_lossy(&joined)
    );
}

#[test]
fn a_killed_client_leaves_the_envelope_open_and_the_session_standing() {
    if !have_tmux() {
        return;
    }
    // Issue #4's first half, live: the client dies without an `%exit` and
    // without an `ST`, and the server does not care.
    let server = Server::start("live-kill", "one", "/bin/cat");
    let mut gateway = server.attach("one");
    gateway.settle(600);
    gateway.send(&Command::NewWindow);
    assert!(gateway.wait_for(b"%window-add", 5000));
    gateway.kill_client();

    assert!(
        !support::envelope_closed(gateway.transcript()),
        "a SIGKILLed client somehow closed its device string"
    );
    let notes = notifications(gateway.replay());
    assert!(!notes.iter().any(|n| matches!(n, Notification::Exit { .. })));

    // The session outlived it, with both windows, and a fresh attach is told
    // about neither: it has to ask.
    let mut second = server.attach("one");
    second.settle(700);
    second.send(&Command::ListPanes);
    second.settle(600);
    let events = second.replay();
    assert!(
        !notifications(events.clone())
            .iter()
            .any(|n| matches!(n, Notification::WindowAdd { .. })),
        "the re-attach was told about a window it should have had to ask for"
    );
    let listing = events
        .iter()
        .find_map(|e| match e {
            Event::Reply { block, .. } => Some(block.text()),
            _ => None,
        })
        .expect("a reply to list-panes");
    assert_eq!(listing.len(), 2, "{listing:?}");
}
