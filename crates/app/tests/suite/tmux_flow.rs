//! The tmux -CC bank flow against a live tmux, end to end through the surface
//! the binary runs: `scripts/eval/tmux-flow.sh`'s phases, minus the pixels.
//!
//! The shell script drives the appliance's own UI with xdotool and judges
//! the glass; what
//! it is *really* asserting (the attach, per-window channels, output
//! routing, the bank's lifetime, detach and the wire's death) lives below the
//! pixels, in the model these tests reach directly. `TerminalSurface::headless`
//! is the same surface the window runs, pump and all, so the whole chain is
//! under test: a real `tmux -CC` client on the channel's own PTY, the DCS tap
//! peeling the envelope, the codec, the gateway, and `app::channels`.
//!
//! Phases 8 to 11 are the interaction layer's, and they are the same shape one
//! level up: what the user does with the attachment (the carrier refusing the
//! air, a special key on its way to a pane, a pane channel's close) asserted
//! against what tmux and the model then hold.
//!
//! Phase 7 is the killed-client re-attach reproduction: kill the whole appliance (not
//! a detach) over a session carrying several windows, re-attach, and every
//! pre-existing window must get a channel.
//!
//! Needs `/usr/bin/tmux` (3.5a is what the codec was measured against) and a
//! few seconds; skips without it, like `tmux-cc`'s live tests.

use std::path::{Path, PathBuf};
use std::process::Command as OsCommand;
use std::time::{Duration, Instant};

use app::shell::Surface;
use app::window::TerminalSurface;
use term::rio_vt::crosswords::grid::Dimensions;
use term::{viewport_text, CellSize, SessionConfig, Viewport};
use winit::dpi::PhysicalSize;
use winit::keyboard::{Key, ModifiersState, NamedKey};

const TMUX: &str = "/usr/bin/tmux";

fn have_tmux() -> bool {
    let ok = Path::new(TMUX).exists();
    if !ok {
        eprintln!("skipping: no {TMUX}");
    }
    ok
}

/// A private tmux server, its socket under a scratch, killed on drop.
struct Server {
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Server {
    /// One detached session named `one`, every pane running `/bin/cat`: it
    /// echoes, it never prints a prompt, and automatic renaming is off so
    /// every name in an assertion is one somebody asked for.
    fn start() -> Server {
        let dir = tempfile::tempdir().expect("scratch dir");
        let server = Server {
            socket: dir.path().join("socket"),
            _dir: dir,
        };
        server.run(&[
            "new-session",
            "-d",
            "-s",
            "one",
            "-x",
            "80",
            "-y",
            "24",
            "/bin/cat",
        ]);
        server.run(&["set-option", "-g", "default-command", "/bin/cat"]);
        server.run(&["set-option", "-g", "automatic-rename", "off"]);
        server
    }

    /// The other hand: make things happen on the server that the client must
    /// then hear about.
    fn run(&self, args: &[&str]) -> String {
        let out = OsCommand::new(TMUX)
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn run_bytes(&self, args: &[&[u8]]) -> String {
        use std::os::unix::ffi::OsStrExt;
        let out = OsCommand::new(TMUX)
            .arg("-S")
            .arg(&self.socket)
            .args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a)))
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn windows(&self) -> Vec<String> {
        self.run(&["list-windows", "-t", "one", "-F", "#{window_id}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Every window of every session on the server.
    fn all_windows(&self) -> Vec<String> {
        self.run(&["list-windows", "-a", "-F", "#{window_id}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn clients(&self) -> usize {
        self.run(&["list-clients", "-F", "#{client_pid}"])
            .lines()
            .count()
    }

    /// Put `bytes` into a pane's `cat`, which echoes them onto the pane's own
    /// screen. `-H` because the interesting bytes are escapes and `send-keys`
    /// would otherwise read them as key names.
    fn send_raw(&self, target: &str, bytes: &[u8]) {
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut args = vec!["send-keys", "-H", "-t", target];
        args.extend(hex.iter().map(String::as_str));
        self.run(&args);
    }

    /// What tmux's own emulator holds for a pane, with its attributes turned
    /// back into escape sequences: the very thing the gateway's
    /// `capture-pane -peqJ` reply carries on attach.
    fn captured(&self, target: &str) -> String {
        self.run(&["capture-pane", "-p", "-e", "-t", target])
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = OsCommand::new(TMUX)
            .arg("-S")
            .arg(&self.socket)
            .arg("kill-server")
            .output();
    }
}

/// The channel's shell: an interactive bash the test types the attach into,
/// exactly the flow script's posture. `--norc` so the developer's prompt
/// hooks stay out of the transcript.
fn shell() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/bash".to_string()),
        args: vec!["--norc".to_string(), "--noprofile".to_string()],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 500,
        grapheme_clustering: false,
        rate: None,
    }
}

fn surface() -> TerminalSurface {
    // A tmux client refuses to attach from inside another tmux; the test
    // must not inherit a developer session's TMUX into the shell.
    std::env::remove_var("TMUX");
    let viewport = Viewport::new(720, 490, 1.0, CellSize::new(9.0, 18.0));
    TerminalSurface::headless(&shell(), viewport)
}

/// Type the attach into the channel on the air.
fn type_attach(surface: &mut TerminalSurface, server: &Server) {
    let line = format!("{TMUX} -S {} -CC attach -t one\r", server.socket.display());
    surface.write(line.as_bytes());
}

/// The surface's channel rows, flattened for a failure message.
fn rows_of(surface: &TerminalSurface) -> String {
    surface
        .channels()
        .rows()
        .iter()
        .map(|r| {
            format!(
                "(bank {} ch {} tmux {:?} title {:?})",
                r.bank, r.channel, r.tmux, r.title
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pump until the model satisfies `pred`; a timeout is a failure that says
/// what the model held instead.
fn pump_until(surface: &mut TerminalSurface, what: &str, pred: impl Fn(&TerminalSurface) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        surface.pump();
        if pred(surface) {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    panic!(
        "timed out waiting for {what}\nrows: {}\nglass: {:?}",
        rows_of(surface),
        surface.viewport_text()
    );
}

/// Attach and wait until the bank stands with a channel per existing window.
/// Answers the tmux bank's id.
fn attach(surface: &mut TerminalSurface, server: &Server) -> u32 {
    let windows = server.windows().len();
    attach_expecting(surface, server, 2, windows)
}

/// The same, on a server holding more than the one session: `banks` counts
/// home's and every attachment's, and `windows` every window that must end up
/// with a channel of its own.
fn attach_expecting(
    surface: &mut TerminalSurface,
    server: &Server,
    banks: usize,
    windows: usize,
) -> u32 {
    type_attach(surface, server);
    pump_until(surface, "the attach to raise its banks and channels", |s| {
        s.channels().banks().len() == banks
            && s.channels()
                .rows()
                .iter()
                .filter(|r| r.tmux.is_some())
                .count()
                == windows
    });
    surface
        .channels()
        .banks()
        .iter()
        .find(|b| b.manager.is_tmux())
        .expect("a tmux bank")
        .id
}

/// One key press, as the window's own `key_pressed` would reduce it.
///
/// `text` is the text the platform produced with **every** modifier applied --
/// winit's `text_with_all_modifiers`, which the window reads through
/// `app::window::key_text`. So `Ctrl+C` arrives here as `Some("\u{3}")`, the
/// way X11 hands it over, and not as the bare `"c"` of winit's plain `text`
/// field.
fn press(surface: &mut TerminalSurface, key: Key, text: Option<&str>, mods: ModifiersState) {
    surface.key_input(&key, text, mods);
}

/// The short hostname tmux reports, which is what a tmux bank is named for.
fn host() -> String {
    String::from_utf8_lossy(
        &OsCommand::new("hostname")
            .arg("-s")
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string()
}

/// `Alt+Shift+D`, the chord modifier with Shift and D.
fn detach_key(surface: &mut TerminalSurface) {
    press(surface, Key::Character("D".into()), Some("D"), CHORD | ModifiersState::SHIFT);
}

/// The carrier's own glass holds none of tmux's protocol, the envelope having
/// gone to the tap and nowhere else.
fn assert_carrier_clean(surface: &TerminalSurface) {
    let row = surface.channels().rows().iter().find(|r| (r.bank, r.channel) == (0, 1));
    let text = viewport_text(row.expect("the carrier row").session.term()).join("\n");
    for leak in ["%begin", "%output", "\x1b"] {
        assert!(!text.contains(leak), "{leak:?} on the carrier's glass:\n{text}");
    }
}

/// The tmux window the channel on the air stands on.
fn window_on_air(surface: &TerminalSurface) -> &str {
    let row = surface.channels().current().expect("a channel on the air");
    let (window, _) = row.tmux.as_ref().expect("a tmux row");
    window.as_str()
}

/// The row a window id landed on.
fn row_of<'a>(
    surface: &'a TerminalSurface,
    bank: u32,
    window: &str,
) -> Option<&'a app::channels::Row<app::window::AppSession>> {
    surface
        .channels()
        .rows()
        .iter()
        .find(|r| r.bank == bank && r.tmux.as_ref().is_some_and(|(w, _)| w.as_str() == window))
}

#[test]
fn phase_1_a_fresh_attach_populates_a_channel_for_every_window_from_slot_one() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "logs"]);
    assert_eq!(server.windows().len(), 2, "the session starts with two");

    // Colour the window before anybody attaches to it.
    //
    // `capture-pane -peqJ` reports a pane's attributes as the escape
    // sequences that produce them, so the reply that draws this window's
    // screen on attach is full of raw `ESC [ ... m` -- inside the control
    // mode DCS envelope, where a VT parser reads the first `ESC` as the end
    // of the string. Under that reading the attachment dies here, before a
    // single channel stands, which is why this sits in phase 1 rather than
    // in a phase of its own. An OSC 8 hyperlink's `ESC \` terminator, also
    // in such a capture, is what a VT parser reads as the envelope's close.
    let windows = server.windows();
    server.send_raw(&windows[0], b"\x1b[31mRED-PROMPT\x1b[0m $ \r\n");
    let link: &[u8] = b"\x1b]8;;http://x.y\x1b\\LINK\x1b]8;;\x1b\\\r\n";
    server.send_raw(&windows[0], link);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !server.captured(&windows[0]).contains("LINK") {
        assert!(
            Instant::now() < deadline,
            "the pane never took the coloured text"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        server.captured(&windows[0]).contains('\x1b'),
        "capture-pane -e reported no escape sequences; the scenario is vacuous"
    );
    assert!(
        server.captured(&windows[0]).contains("\x1b\\"),
        "the capture holds no hyperlink terminator; the scenario is vacuous"
    );

    let mut surface = surface();
    let bank = attach(&mut surface, &server);

    // The coloured screen came across the envelope and onto the channel's own
    // glass: every byte of that capture survived the DCS tap.
    pump_until(&mut surface, "the coloured capture to land", |s| {
        row_of(s, bank, &windows[0])
            .map(|r| {
                viewport_text(r.session.term())
                    .join("\n")
                    .contains("RED-PROMPT")
            })
            .unwrap_or(false)
    });

    // Every pre-existing window has a channel, from slot 1, and nothing else
    // stands on the bank: the carrier stays home.
    assert!(surface.channels().is_carrier(0, 1));
    let on_bank = surface.channels().rows().iter().filter(|r| r.bank == bank);
    assert_eq!(on_bank.count(), server.windows().len(), "{}", rows_of(&surface));
    for (slot, window) in server.windows().iter().enumerate() {
        let row = row_of(&surface, bank, window).unwrap_or_else(|| {
            panic!(
                "window {window} got no channel; rows: {}",
                rows_of(&surface)
            )
        });
        assert_eq!(row.channel as usize, slot + 1, "windows fill from slot 1");
    }

    assert_carrier_clean(&surface);

    // The air crossed with the attach and the first window landed under it;
    // the ones tmux volunteers after it line up behind.
    assert_eq!(surface.channels().on_air(), (bank, 1));

    // The bank is named for the host the tmux server reported.
    let host = host();
    pump_until(&mut surface, "the tmux hostname to resolve", |s| {
        s.channels().bank_identity(bank) == Some(format!("tmux -CC # @{host}"))
    });
    assert_eq!(surface.bank_strips().band, Some(format!("tmux -CC # @{host}")));

    // The client-size law, read off its real effect: tmux draws the
    // session's windows at the size the client published. (The session was
    // created 80x24; the published grid differs, so a window standing at it
    // proves the `refresh-client -C` arrived. `#{client_height}` itself
    // reads empty for a control client on tmux 3.5a, so the client's row is
    // no instrument.)
    let size = Viewport::new(720, 490, 1.0, CellSize::new(9.0, 18.0)).term_size();
    let want = format!("{}x{}", size.cols(), size.rows());
    assert_ne!(want, "80x24", "the probe must differ from the seed size");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = server.run(&["list-windows", "-F", "#{window_width}x#{window_height}"]);
        if got.lines().all(|l| l.trim() == want) && !got.trim().is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tmux never drew the windows at the glass's grid: wanted {want}, windows say {got:?}"
        );
        surface.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn phase_2_new_windows_arrive_volunteered_stand_back_and_asked_for_take_the_air() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    assert_eq!(surface.channels().on_air(), (bank, 1));

    // A window somebody else opens lines up without taking the air.
    server.run(&["new-window", "-t", "one", "-n", "fresh"]);
    pump_until(&mut surface, "the volunteered window's channel", |s| {
        s.channels().slot_title(bank, 2) == Some("fresh")
    });
    assert_eq!(surface.channels().current_channel(), 1, "the air held");

    // A window this set asks for takes it outright (Ctrl+Shift+T's path).
    surface.new_channel();
    pump_until(&mut surface, "the asked-for window to take the air", |s| {
        s.channels().current_channel() == 3
    });
    assert_eq!(server.windows().len(), 3);
}

#[test]
fn phase_3_a_rename_lands_unescaped_in_the_bank() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let window = server.windows()[0].clone();

    // A name carrying a backslash arrives vis(3)-escaped on the wire; the
    // codec decodes it, so the bank shows the name itself, not the escaped
    // `a\\b` a verbatim read would show.
    server.run_bytes(&[b"rename-window", b"-t", window.as_bytes(), b"a\\b"]);
    pump_until(&mut surface, "the unescaped rename", |s| {
        s.channels().slot_title(bank, 1) == Some(r"a\b")
    });
}

#[test]
fn phase_4_output_flows_to_the_channel_of_its_pane_and_keystrokes_divert_back() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "logs"]);
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let windows = server.windows();

    // Words typed into each pane from the server side must surface on that
    // window's channel and no other.
    server.run(&["send-keys", "-t", &windows[0], "word-zero", "Enter"]);
    server.run(&["send-keys", "-t", &windows[1], "word-one", "Enter"]);
    pump_until(&mut surface, "both panes' output to route", |s| {
        let sees = |window: &str, word: &str| {
            row_of(s, bank, window)
                .map(|r| viewport_text(r.session.term()).join("\n").contains(word))
                .unwrap_or(false)
        };
        sees(&windows[0], "word-zero") && sees(&windows[1], "word-one")
    });
    let crossed = |s: &TerminalSurface, window: &str, word: &str| {
        row_of(s, bank, window)
            .map(|r| viewport_text(r.session.term()).join("\n").contains(word))
            .unwrap_or(false)
    };
    assert!(
        !crossed(&surface, &windows[0], "word-one"),
        "window {}'s output leaked into {}",
        windows[1],
        windows[0]
    );
    assert!(!crossed(&surface, &windows[1], "word-zero"));

    // The write side: keystrokes on the pane channel on the air become
    // send-keys, land in the pane's cat, and echo back as %output.
    assert_eq!(surface.channels().current_channel(), 1);
    surface.write(b"typed-into-tmux\r");
    pump_until(&mut surface, "the diverted keystrokes to echo back", |s| {
        viewport_text(s.channels().current().unwrap().session.term())
            .join("\n")
            .contains("typed-into-tmux")
    });
    assert!(
        !crossed(&surface, &windows[1], "typed-into-tmux"),
        "keystrokes for one pane reached another"
    );
}

#[test]
fn phase_5_a_killed_window_loses_its_channel_and_the_bank_stands() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "logs"]);
    server.run(&["new-window", "-t", "one", "-n", "spare"]);
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let doomed = server.windows()[1].clone();

    server.run(&["kill-window", "-t", &doomed]);
    pump_until(&mut surface, "the killed window's channel to go", |s| {
        row_of(s, bank, &doomed).is_none()
    });
    // The bank and its other channels stand; the air never left slot 1.
    assert_eq!(surface.channels().banks().len(), 2);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(
        surface
            .channels()
            .rows()
            .iter()
            .filter(|r| r.bank == bank)
            .count(),
        2,
        "two survivors: {}",
        rows_of(&surface)
    );
}

#[test]
fn phase_6_alt_shift_d_detaches_and_the_carrier_is_live_again() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    assert_eq!(surface.channels().on_air().0, bank, "the tmux bank on view");
    assert_carrier_clean(&surface);
    assert!(surface.detach_bank());
    pump_until(&mut surface, "the bank to collapse", |s| {
        s.channels().banks().len() == 1
    });
    assert_eq!(surface.channels().on_air(), (0, 1), "the air on the carrier");
    assert!(!surface.channels().is_carrier(0, 1));
    assert_carrier_clean(&surface);
    assert_eq!(server.clients(), 0, "tmux let the client go");
    assert!(
        !server.windows().is_empty(),
        "tmux kept the session and its windows"
    );
}

/// The killed-client re-attach phase, its required reproduction: kill the whole
/// appliance (a crash, not a detach; the tmux server and its windows
/// survive), then attach from a fresh appliance. Every window the session
/// already holds must get a channel, the same as if it had been opened from
/// this client. The failure mode this guards against is a listener race that
/// drops previously open windows from the bank on reattach; that race needs
/// an asynchronous step between attach and listener registration, which this
/// architecture's synchronous attach does not have, so this test is the
/// proof that the re-attach keeps every window on the bank.
#[test]
fn phase_7_issue_4_reattach_after_appliance_death_restores_every_window() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut first = surface();
    let bank = attach(&mut first, &server);
    server.run(&["new-window", "-t", "one", "-n", "second"]);
    server.run(&["new-window", "-t", "one", "-n", "third"]);
    pump_until(&mut first, "three windows on the first appliance", |s| {
        s.channels()
            .rows()
            .iter()
            .filter(|r| r.bank == bank)
            .count()
            == 3
    });

    // The kill. Dropping the surface closes the PTY master: SIGHUP takes the
    // shell and the tmux client with it, mid-protocol, no detach-client and
    // no %exit ever sent.
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.clients() > 0 {
        assert!(
            Instant::now() < deadline,
            "the killed appliance's client never left the server's books"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let windows = server.windows();
    assert_eq!(windows.len(), 3, "the session survived with its windows");

    // The re-attach, from a cold appliance.
    let mut second = surface();
    let bank = attach(&mut second, &server);
    for window in &windows {
        assert!(
            row_of(&second, bank, window).is_some(),
            "window {window} got no channel on re-attach; rows: {}",
            rows_of(&second)
        );
    }
    // And they are the right channels: names as the server holds them.
    pump_until(&mut second, "the restored titles", |s| {
        s.channels().slot_title(bank, 2) == Some("second")
            && s.channels().slot_title(bank, 3) == Some("third")
    });
    assert_eq!(second.channels().on_air(), (bank, 1));
}

// ---- the interaction phases ----------------------------------------------

/// The carrier's pty is the protocol's wire, so it never takes the air: a
/// press on its strip leaves the air where it was, and the codec, which pairs
/// every reply to a command it sent, is still in step afterwards. The way out
/// is `Alt+Shift+D`.
#[test]
fn phase_8_the_carrier_refuses_the_air_and_alt_shift_d_detaches() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "logs"]);
    let mut surface = surface_with_bank();
    let bank = attach(&mut surface, &server);
    press(&mut surface, Key::Named(NamedKey::PageUp), None, CHORD);
    let home = surface.channels().on_air();
    surface.press_strip(1);
    assert_eq!(surface.channels().on_air(), home, "the carrier took no air");

    // A window asked for walks the whole pairing chain (new-window, the
    // %window-add re-listing, the capture and the cursor), and the answer has
    // to arrive on the right channel with the right name.
    press(&mut surface, Key::Named(NamedKey::PageDown), None, CHORD);
    surface.new_channel();
    pump_until(&mut surface, "a window asked for", |s| {
        s.channels().rows().iter().filter(|r| r.bank == bank).count() == 3
    });
    let fresh = server.windows()[2].clone();
    pump_until(&mut surface, "the fresh window's title", |s| {
        row_of(s, bank, &fresh).is_some_and(|r| !r.title.is_empty())
    });

    detach_key(&mut surface);
    pump_until(&mut surface, "the key to detach the bank", |s| {
        s.channels().banks().len() == 1
    });
    assert_eq!(surface.channels().on_air(), (0, 1), "the carrier, live again");
    assert_eq!(server.clients(), 0, "tmux let the client go");
    assert!(!server.windows().is_empty(), "and kept the session");
}

/// Keystroke diversion, completely: a keytab key and a control key both reach
/// the far pane as the bytes they encode to.
///
/// Keystroke diversion translates nothing: `send_keys` sends
/// `send-keys -H -t <pane> <hex byte>...`, one hex pair per byte, chunked at
/// 256, so there is no key-name table and no key that cannot be sent. The
/// instrument is the pane itself:
/// `cat -v` under `stty raw -echo` prints what it reads, `^[[A` for the arrow
/// and `^C` for the interrupt, and it prints it because the byte arrived, not
/// because a terminal interpreted it.
#[test]
fn phase_10_a_keytab_key_and_a_control_key_reach_the_tmux_pane_as_bytes() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    // Raw so the line discipline neither echoes nor eats the interrupt: what
    // the pane shows is what `send-keys` delivered.
    server.run(&[
        "new-window",
        "-t",
        "one",
        "-n",
        "keys",
        "stty raw -echo; exec cat -v",
    ]);
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let window = server.windows()[1].clone();

    // The air onto that window's channel (windows fill from slot 1).
    surface.cycle_channel(1);
    assert_eq!(surface.channels().current_channel(), 2);
    assert_eq!(
        window_on_air(&surface),
        window,
        "the air is on the raw pane's channel"
    );

    let shows = |s: &TerminalSurface, what: &str| {
        row_of(s, bank, &window)
            .map(|r| viewport_text(r.session.term()).join("\n").contains(what))
            .unwrap_or(false)
    };

    // A keytab key: `key Up -AnyMod -AppCuKeys : "\E[A"`.
    press(
        &mut surface,
        Key::Named(NamedKey::ArrowUp),
        None,
        ModifiersState::empty(),
    );
    pump_until(&mut surface, "the arrow to reach the pane", |s| {
        shows(s, "^[[A")
    });

    // A control key, which the keytab does not bind: it rides the event's
    // text with every modifier applied.
    press(
        &mut surface,
        Key::Character("c".into()),
        Some("\u{3}"),
        ModifiersState::CONTROL,
    );
    pump_until(&mut surface, "the interrupt to reach the pane", |s| {
        shows(s, "^C")
    });

    // And neither key crossed into the other window's channel.
    let other = server.windows()[0].clone();
    let crossed = row_of(&surface, bank, &other)
        .map(|r| viewport_text(r.session.term()).join("\n"))
        .unwrap_or_default();
    assert!(!crossed.contains("^[[A") && !crossed.contains("^C"));
}

/// Closing a pane channel: the close asks tmux to kill the window and asks
/// the user nothing (there is no confirmation anywhere in the appliance),
/// and the row stays until `%window-close` comes back. The row is the
/// picture of what tmux holds, so it goes when tmux says so and not when the
/// user asks.
#[test]
fn phase_11_a_pane_close_asks_tmux_without_confirming_and_the_row_waits_for_the_answer() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "logs"]);
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let doomed = server.windows()[0].clone();

    assert_eq!(surface.channels().current_channel(), 1);
    surface.close_channel();
    // Nothing was asked and nothing was removed: the close is a request on the
    // wire, and the row is still standing the instant after it.
    assert!(
        row_of(&surface, bank, &doomed).is_some(),
        "the row went before tmux answered; rows: {}",
        rows_of(&surface)
    );

    pump_until(&mut surface, "tmux's own close to take the row", |s| {
        row_of(s, bank, &doomed).is_none()
    });
    assert_eq!(server.windows().len(), 1, "tmux killed the window");
    assert_eq!(surface.channels().banks().len(), 2, "the bank stands");
}

/// A paste at scale onto a pane channel, which is the flow that finds every
/// place the transport is assumed rather than checked.
///
/// Tens of kilobytes leave as hex `send-keys` lines, three wire bytes per
/// pasted byte, at 256 bytes a line: a hundred-odd command lines arriving at
/// an `O_NONBLOCK` PTY master faster than tmux reads it. A `write_all` there
/// takes a prefix and reports the refusal, and the tail of that line is gone:
/// tmux reads the stump joined to the next command, runs one command where two
/// were sent, and answers one block where two were expected, after which
/// every reply lands on the intent of the command before it, for good.
///
/// So both halves are asserted. The pane's own record of what it received is a
/// file it writes everything to, compared byte for byte with what was pasted;
/// and the pairing is asked to prove itself afterwards, by a command whose
/// reply has to come back on the right intent for a window to appear at all.
#[test]
fn phase_12_a_paste_at_scale_reaches_the_pane_whole_and_the_pairing_holds() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    // The instrument: a pane with no line discipline in the way, writing
    // every byte it is handed to a file. `stty raw -echo` is what makes the
    // file a faithful record: no echo doubling it, no CR/LF translation
    // changing its length.
    let sink = server.socket.with_file_name("paste-sink");
    server.run(&[
        "new-window",
        "-t",
        "one",
        "-n",
        "sink",
        &format!("stty raw -echo; exec cat > {}", sink.display()),
    ]);
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let window = server.windows()[1].clone();

    // The air onto the sink's channel; a paste goes to the channel on the air.
    surface.cycle_channel(1);
    assert_eq!(
        window_on_air(&surface),
        window,
        "the air is on the sink pane's channel"
    );

    // Thirty-two kilobytes in one write, which is what a paste is. No
    // newlines: nothing in it can be delivered early by a line discipline, so
    // the whole thing has to survive the queue to arrive at all.
    const PASTE: usize = 32 * 1024;
    let paste: Vec<u8> = (0..PASTE).map(|i| b'a' + (i % 26) as u8).collect();
    surface.write(&paste);

    let want = paste.clone();
    let path = sink.clone();
    pump_until(
        &mut surface,
        "the paste to reach the pane whole",
        move |_| {
            std::fs::read(&path)
                .map(|got| got.len() >= want.len())
                .unwrap_or(false)
        },
    );
    let got = std::fs::read(&sink).expect("the pane's record");
    assert_eq!(
        got.len(),
        paste.len(),
        "the pane received a different count"
    );
    assert_eq!(got, paste, "the pane received different bytes");

    // And the pairing: a window asked for after all that traffic still has to
    // come back as a channel with a name, which walks the whole chain --
    // new-window, the %window-add re-listing, the capture and the cursor --
    // with every reply landing on the intent that was registered for it.
    surface.new_channel();
    pump_until(&mut surface, "a window asked for after the paste", |s| {
        s.channels().rows().iter().filter(|r| r.bank == bank).count() == 3
    });
    let fresh = server.windows()[2].clone();
    pump_until(&mut surface, "the fresh window's title", |s| {
        row_of(s, bank, &fresh).is_some_and(|r| !r.title.is_empty())
    });
}

/// A tmux window's output asks for the redraw that shows it.
///
/// A pane row's own `pump` reads nothing and cannot: its bytes arrive off
/// the gateway's wire, which the surface drains after the loop that counts what
/// the channel on the air produced. Counted only there, `pump` answered zero
/// for a tmux window however much it printed, so `Tick.redraw` was left
/// entirely to the effects clock -- the picture arriving on the next effects
/// frame instead of the next frame, and not at all on a window whose effects
/// are not running.
#[test]
fn phase_13_a_tmux_panes_output_is_counted_as_the_redraw_it_asks_for() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface();
    let bank = attach(&mut surface, &server);
    let windows = server.windows();

    // The air is on the attachment's first window, which is the pane about to
    // print.
    assert_eq!(surface.channels().current_channel(), 1);
    assert!(row_of(&surface, bank, &windows[0]).is_some());

    // Drain what the attach itself is still delivering, so what is counted
    // below can only be the output asked for here.
    for _ in 0..20 {
        surface.pump();
        std::thread::sleep(Duration::from_millis(15));
    }

    server.run(&["send-keys", "-t", &windows[0], "printf REDRAW-ME", "Enter"]);

    let mut counted = 0usize;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        counted += surface.pump();
        let arrived = viewport_text(surface.channels().current().unwrap().session.term())
            .join("\n")
            .contains("REDRAW-ME");
        if arrived {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pane's output never reached the channel on the air"
        );
        std::thread::sleep(Duration::from_millis(15));
    }

    assert!(
        counted > 0,
        "the output reached the glass without one pump reporting a byte, so \
         nothing but the effects clock would have drawn it"
    );
}

// ---- the sessions of a local server --------------------------------------

/// Every tmux session on a local server gets a bank of its own: the one the
/// user typed the attach into, and one the terminal raises for every other
/// session it finds on that server, each with its own `tmux -CC` client.
///
/// The client the terminal starts goes to the socket the app learned from the
/// server's own reply, which is this test's private server and never a
/// developer's default one -- and `server.clients()` is what says so, since a
/// client that went elsewhere would leave this server holding one.
#[test]
fn phase_14_a_second_session_on_the_server_gets_a_bank_and_a_client_of_its_own() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-session", "-d", "-s", "two", "/bin/cat"]);
    let mut surface = surface();
    let typed = attach_expecting(&mut surface, &server, 3, 2);

    let found = surface
        .channels()
        .banks()
        .iter()
        .filter(|b| b.manager.is_tmux())
        .map(|b| b.id)
        .find(|id| *id != typed)
        .unwrap_or_else(|| panic!("no bank for the session found; rows: {}", rows_of(&surface)));

    for bank in [typed, found] {
        let rows = surface.channels().rows().iter().filter(|r| r.bank == bank);
        let windows: Vec<u32> = rows.map(|r| r.tmux.as_ref().map_or(0, |_| r.channel)).collect();
        assert_eq!(windows, vec![1], "one window row, from slot 1");
    }

    // Both banks are named for the server they stand on, which only a client
    // whose own bootstrap was answered can name.
    let host = host();
    pump_until(&mut surface, "both banks to name their server", |s| {
        let title = Some(format!("tmux -CC # @{host}"));
        s.channels().bank_identity(typed) == title && s.channels().bank_identity(found) == title
    });
    assert_eq!(
        server.clients(),
        2,
        "the client the terminal started went to another server"
    );
    // Home holds the carrier alone, the found bank standing on no slot of it.
    assert!(surface.channels().is_carrier(0, 1));
    assert_eq!(surface.channels().first_free(0), 2);
}

/// A session killed on the server takes its bank with it and leaves home
/// exactly as it was: nothing comes home from a bank that never stood on a
/// home slot.
#[test]
fn phase_15_a_killed_session_takes_its_bank_and_home_keeps_its_slots() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-session", "-d", "-s", "two", "/bin/cat"]);
    let mut surface = surface();
    let typed = attach_expecting(&mut surface, &server, 3, 2);
    let free = surface.channels().first_free(0);
    let rows = surface
        .channels()
        .rows()
        .iter()
        .filter(|r| r.bank == 0)
        .count();

    server.run(&["kill-session", "-t", "two"]);
    pump_until(&mut surface, "the killed session's bank to go", |s| {
        s.channels().banks().len() == 2
    });
    assert_eq!(
        surface.channels().first_free(0),
        free,
        "home was written to"
    );
    assert_eq!(
        surface
            .channels()
            .rows()
            .iter()
            .filter(|r| r.bank == 0)
            .count(),
        rows
    );
    assert_eq!(
        surface.channels().current_bank(),
        typed,
        "the air never left the attachment the user was on"
    );
    assert_eq!(server.clients(), 1, "tmux let the dead session's client go");
}

/// Phase 7 doubled: a cold appliance over a server holding two sessions and
/// several windows restores a bank per session and a channel per window.
#[test]
fn phase_16_a_cold_reattach_restores_every_session_and_every_window() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-session", "-d", "-s", "two", "/bin/cat"]);
    server.run(&["new-window", "-t", "one", "-n", "second"]);
    server.run(&["new-window", "-t", "two", "-n", "other"]);
    let windows = server.all_windows();
    assert_eq!(windows.len(), 4, "two sessions of two windows each");

    let mut first = surface();
    attach_expecting(&mut first, &server, 3, 4);

    // The kill: dropping the surface closes every PTY master, and SIGHUP takes
    // both tmux clients with it, mid-protocol, no detach and no %exit.
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.clients() > 0 {
        assert!(
            Instant::now() < deadline,
            "the killed appliance's clients never left the server's books"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(server.all_windows(), windows, "the server kept everything");

    let mut second = surface();
    attach_expecting(&mut second, &server, 3, 4);
    for window in &windows {
        assert!(
            second
                .channels()
                .rows()
                .iter()
                .any(|r| r.tmux.as_ref().is_some_and(|(w, _)| w.as_str() == window)),
            "window {window} got no channel on re-attach; rows: {}",
            rows_of(&second)
        );
    }
    // And the names came back with them, both sessions'.
    pump_until(&mut second, "the restored titles", |s| {
        let titles: Vec<&str> = s
            .channels()
            .rows()
            .iter()
            .map(|r| r.title.as_str())
            .collect();
        titles.contains(&"second") && titles.contains(&"other")
    });
    assert_eq!(server.clients(), 2);
}

/// The chord modifier, forked per platform as the window forks it.
#[cfg(target_os = "macos")]
const CHORD: ModifiersState = ModifiersState::SUPER;
#[cfg(not(target_os = "macos"))]
const CHORD: ModifiersState = ModifiersState::ALT;

/// The flow's surface with a cabinet on it: the pager keys turn nothing
/// without a bank drawn, and 730px of window is eleven rows, so each bank's
/// stretch here is one page and a step is a crossing.
fn surface_with_bank() -> TerminalSurface {
    std::env::remove_var("TMUX");
    let viewport = Viewport::new(720, 730, 1.0, CellSize::new(9.0, 18.0));
    let mut surface = TerminalSurface::headless(&shell(), viewport);
    surface.set_cabinet(chassis::Cabinet::from_config(
        &config::Config::default(),
        720.0,
        730.0,
    ));
    surface
}

/// `Alt`+`PageUp` / `PageDown` crossing from one bank's stretch into another
/// is the band switch it looks like: the bank landed on comes back showing the
/// channel it was left on, and the one left behind keeps its own against the
/// return. Stepping between one bank's own screenfuls stays view-only, which
/// `channel_bank` pins; only a crossing moves the air, and only a second bank
/// can show it.
#[test]
fn phase_17_paging_across_banks_brings_each_one_back_to_the_channel_it_was_left_on() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["new-window", "-t", "one", "-n", "second"]);
    let mut surface = surface_with_bank();

    // A second home channel, because the one the attach is typed into
    // carries the wire and takes no air.
    surface.new_channel();
    assert_eq!(surface.channels().on_air(), (0, 2));
    let bank = attach(&mut surface, &server);
    assert_eq!(
        surface.bank_strips().page_count,
        2,
        "one page per bank, so a step is a crossing"
    );
    assert_eq!(surface.channels().on_air(), (bank, 1), "the air crossed");

    // Leave the attachment standing on its second window. Its band is a
    // label naming the key that detaches.
    surface.press_strip(2);
    assert_eq!(surface.channels().on_air(), (bank, 2));
    let shell = chassis::shell_metrics(&config::Config::default());
    let band = chassis::furniture::strip_rect(surface.cabinet().unwrap().geometry(), &shell, 0);
    let at = winit::dpi::PhysicalPosition::new(band.x + 4.0, band.y + 4.0);
    surface.mouse_pressed(winit::event::MouseButton::Left, at, ModifiersState::empty());
    assert_eq!(surface.notice().text(), app::window::BAND_DETACH);
    assert_eq!(surface.channels().on_air(), (bank, 2));

    // Page home. Home was left on slot 2, which carries the wire, so the air
    // falls to the first row home still has.
    press(&mut surface, Key::Named(NamedKey::PageUp), None, CHORD);
    assert_eq!(surface.channels().on_air(), (0, 1));
    assert_eq!(surface.notice().text(), app::window::BAND_DETACH, "home has no card");

    // Home takes a channel of its own, and that is where it is left.
    surface.new_channel();
    assert_eq!(surface.channels().on_air(), (0, 3));

    // Back to the attachment: the window it was left on, not its first.
    press(&mut surface, Key::Named(NamedKey::PageDown), None, CHORD);
    assert_eq!(surface.channels().on_air(), (bank, 2));
    let name = surface.channels().bank_identity(bank).unwrap();
    assert_eq!(surface.notice().text(), name, "crossing in names the bank");
    let strips = surface.bank_strips();
    assert!(
        strips.current_row.is_some(),
        "the bank came back showing the channel it put on the air"
    );

    // ...and home the same way, and the pumps the event loop runs between
    // keystrokes leave it there.
    press(&mut surface, Key::Named(NamedKey::PageUp), None, CHORD);
    assert_eq!(surface.channels().on_air(), (0, 3));
    for _ in 0..8 {
        surface.pump();
    }
    assert_eq!(surface.channels().on_air(), (0, 3));
}

/// The first-run path: the only channel there is types `tmux -CC` and carries
/// the wire. Paging home finds nothing but the carrier, so the air lands on the
/// slot beside it and tunes to no signal; the new-channel key there takes 02;
/// and the detach lands on the carrier, live again.
#[test]
fn phase_18_paging_home_off_a_first_channels_attach_tunes_the_glass_to_no_signal() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface_with_bank();
    attach(&mut surface, &server);
    press(&mut surface, Key::Named(NamedKey::PageUp), None, CHORD);
    assert_eq!(surface.channels().on_air(), (0, 2), "home takes the air");
    assert!(surface.channels().current().is_none(), "no row on it");
    assert_eq!(surface.no_signal(), Some(app::window::NO_SIGNAL));

    surface.new_channel();
    assert_eq!(surface.channels().on_air(), (0, 2));
    assert!(surface.no_signal().is_none(), "a lit row ends the message");

    press(&mut surface, Key::Named(NamedKey::PageDown), None, CHORD);
    detach_key(&mut surface);
    pump_until(&mut surface, "the bank to collapse", |s| {
        s.channels().banks().len() == 1
    });
    assert_eq!(surface.channels().on_air(), (0, 1), "the carrier");
}

/// The active pane of every window of session `one`, at the size tmux gave
/// it.
fn pane_sizes(server: &Server) -> Vec<(String, usize, usize)> {
    let format = "#{window_id} #{pane_active} #{pane_width} #{pane_height}";
    server
        .run(&["list-panes", "-s", "-t", "one", "-F", format])
        .lines()
        .filter_map(|line| match line.split(' ').collect::<Vec<_>>()[..] {
            [window, "1", width, height] => Some((
                window.to_string(),
                width.parse().ok()?,
                height.parse().ok()?,
            )),
            _ => None,
        })
        .collect()
}

/// The grid each of those windows' channels stands at.
fn channel_sizes(
    surface: &TerminalSurface,
    bank: u32,
    panes: &[(String, usize, usize)],
) -> Vec<(String, usize, usize)> {
    panes
        .iter()
        .filter_map(|(window, _, _)| {
            let grid = &row_of(surface, bank, window)?.session.term().grid;
            Some((window.clone(), grid.columns(), grid.screen_lines()))
        })
        .collect()
}

/// The plain client's screen as tmux holds it.
fn plain_client(server: &Server) -> (usize, usize) {
    let format = "#{client_control_mode} #{client_width} #{client_height}";
    server
        .run(&["list-clients", "-F", format])
        .lines()
        .find_map(|line| match line.split(' ').collect::<Vec<_>>()[..] {
            ["0", width, height] => Some((width.parse().ok()?, height.parse().ok()?)),
            _ => None,
        })
        .expect("a plain client")
}

type Sizes = Vec<(String, usize, usize)>;

/// Pump both surfaces until every channel stands at its pane's size and
/// `until` holds of what tmux reports, or the deadline passes; returns tmux's
/// pane sizes and the channels' sizes for the assertions to compare.
fn settled(
    surface: &mut TerminalSurface,
    other: &mut TerminalSurface,
    server: &Server,
    bank: u32,
    until: impl Fn(&TerminalSurface, &Sizes) -> bool,
) -> (Sizes, Sizes) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        surface.pump();
        other.pump();
        let panes = pane_sizes(server);
        let channels = channel_sizes(surface, bank, &panes);
        if (panes == channels && until(surface, &panes)) || Instant::now() > deadline {
            return (panes, channels);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// The carrier's row: a PTY channel, so its grid is the glass's.
fn glass(surface: &TerminalSurface) -> (usize, usize) {
    let channels = surface.channels();
    let row = channels.rows().iter().find(|r| channels.is_carrier(r.bank, r.channel));
    let grid = &row.expect("the carrier row").session.term().grid;
    (grid.columns(), grid.screen_lines())
}

/// A plain `tmux attach` on the same session, typed in a channel of its own
/// as the owner runs one. Under tmux's `window-size latest` a window shared
/// by two clients follows the one last active on it, and this client's
/// `send-keys` is no activity; its per-window size caps every
/// window at the glass all the same, and sizes a window tmux finds no client
/// to follow for. A window shown to a smaller client still shrinks to it.
/// Every channel is drawn at the size tmux gave its pane.
#[test]
fn phase_19_with_a_second_client_on_the_session_every_channel_stands_at_its_panes_size() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    let mut surface = surface();
    let bank = attach(&mut surface, &server);

    // No status line, so a window the plain client sizes is its whole screen.
    server.run(&["set-option", "-g", "status", "off"]);
    let plain = Viewport::new(1530, 738, 1.0, CellSize::new(9.0, 18.0));
    let mut other = TerminalSurface::headless(&shell(), plain);
    let line = format!("{TMUX} -S {} attach -t one\r", server.socket.display());
    other.write(line.as_bytes());
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.clients() < 2 && Instant::now() < deadline {
        surface.pump();
        other.pump();
        std::thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(server.clients(), 2, "the plain client attached");

    // A key through the plain client makes it the window's latest client,
    // larger than the glass; the window stays at the glass.
    let at_glass = |s: &TerminalSurface, panes: &Sizes| {
        panes.iter().all(|(_, w, h)| (*w, *h) == glass(s))
    };
    other.write(b"x");
    let (panes, channels) = settled(&mut surface, &mut other, &server, bank, at_glass);
    assert_eq!(channels, panes);
    assert!(at_glass(&surface, &panes), "{panes:?}");

    // A window this client asks for, then a glass that grows: tmux finds no
    // latest client for the new window, and the per-window size sizes it.
    surface.new_channel();
    pump_until(&mut surface, "the asked-for window to take the air", |s| {
        s.channels().current_channel() == 2
    });
    surface.resized(PhysicalSize::new(900, 580));
    let (panes, channels) = settled(&mut surface, &mut other, &server, bank, at_glass);
    assert_eq!(channels, panes);
    assert!(at_glass(&surface, &panes), "{panes:?}");

    // The plain client shrinks below the glass: the window it shows follows
    // it down, and so does that window's channel.
    other.resized(PhysicalSize::new(540, 310));
    let shown = |s: &TerminalSurface, panes: &Sizes| {
        let on_air = window_on_air(s);
        let pane = panes.iter().find(|(w, _, _)| w == on_air);
        pane.map(|(_, w, h)| (*w, *h))
    };
    let (panes, channels) = settled(&mut surface, &mut other, &server, bank, |s, panes| {
        shown(s, panes) == Some(plain_client(&server))
    });
    assert_eq!(channels, panes);
    assert_eq!(
        shown(&surface, &panes),
        Some(plain_client(&server)),
        "the shared window follows the smaller client"
    );
}

/// A `tmux -CC attach` with no server to attach to: tmux exits without
/// `%session-changed`, so the gateway writes nothing for the shell to run.
#[test]
fn phase_20_an_attach_that_never_happens_hands_the_shell_nothing() {
    if !have_tmux() {
        return;
    }
    let dir = tempfile::tempdir().expect("scratch dir");
    let mut surface = surface();
    pump_until(&mut surface, "the prompt", |s| {
        !s.viewport_text()[0].is_empty()
    });
    let prompt = surface.viewport_text()[0].trim_end().to_string();
    let socket = dir.path().join("socket");
    surface.write(format!("{TMUX} -S {} -CC attach\r", socket.display()).as_bytes());
    // Half a second is ample for a stray line to be echoed and run.
    let typed = Instant::now();
    pump_until(&mut surface, "the prompt again", |s| {
        let back = s.viewport_text()[1..]
            .iter()
            .any(|l| l.trim_end() == prompt);
        back && typed.elapsed() > Duration::from_millis(500)
    });
    // Under the typed line: the prompt, and nothing the shell ran.
    let glass = surface.viewport_text();
    let after: Vec<&str> = glass[1..3].iter().map(|l| l.trim_end()).collect();
    assert_eq!(after, [prompt.as_str(), ""], "{glass:?}");
}

/// A split window's channel shows its active pane, and `Ctrl+Shift+O` steps
/// tmux to the window's next pane, as `C-b o` would: tmux's active pane
/// moves, and the channel follows it.
#[test]
fn phase_21_the_pane_chord_steps_a_split_window_to_its_next_pane() {
    if !have_tmux() {
        return;
    }
    let server = Server::start();
    server.run(&["split-window", "-t", "one"]);
    let active = || {
        let pane = server.run(&["display-message", "-p", "-t", "one", "#{pane_id}"]);
        pane.trim().to_string()
    };
    let before = active();
    let mut surface = surface();
    attach(&mut surface, &server);
    let shown = |s: &TerminalSurface| {
        let (_, pane) = s.channels().current()?.tmux.clone()?;
        Some(pane.to_string())
    };
    // The window's channel is on the air; were it not, the chord would
    // decline and the wait below would time out naming every row.
    let (o, ctrl_shift) = (
        Key::Character("O".into()),
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    );
    press(&mut surface, o, Some("\u{f}"), ctrl_shift);
    pump_until(&mut surface, "the channel to follow tmux", |s| {
        shown(s).is_some_and(|pane| pane != before)
    });
    assert_eq!(shown(&surface), Some(active()));
}
