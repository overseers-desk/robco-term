//! The SSH bank flow end to end through the surface the binary runs:
//! `TerminalSurface::headless` dialling the in-process far side that
//! `ssh_link::test_server` serves, asserted at what the user reads on the
//! glass, the shape `tmux_flow` set. No sshd runs and none is needed.
//!
//! The environment (SSH_AUTH_SOCK) is set once here and both tests want it
//! set, so ordering between them does not matter.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use app::ssh::{KnownHosts, SshRequest};
use app::window::TerminalSurface;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use ssh_link::russh::keys::{ssh_key, Algorithm, PrivateKey};
use ssh_link::test_server::{mint, serve_agent, serve_echo, serve_with, AuthPlan};
use term::{viewport_text, CellSize, SessionConfig, Viewport};

/// One far side for the whole suite: an echo server whose host key the
/// fixtures record, and an agent holding the one identity it authorizes.
struct FarSide {
    /// Kept alive: the servers run on its threads.
    _rt: tokio::runtime::Runtime,
    port: u16,
    host_key_line: String,
    seen: std::sync::Arc<std::sync::Mutex<ssh_link::test_server::Seen>>,
    _dir: tempfile::TempDir,
}

fn far_side() -> &'static FarSide {
    static FAR: OnceLock<FarSide> = OnceLock::new();
    FAR.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("scratch dir");
        let identity = mint(Algorithm::Ed25519);
        let host_key: PrivateKey = mint(Algorithm::Ed25519);
        let host_public = host_key.public_key().to_openssh().expect("openssh form");
        let (port, seen) =
            rt.block_on(serve_echo(vec![host_key], identity.public_key().clone()));
        let sock = rt.block_on(serve_agent(dir.path(), &identity));
        std::env::set_var("SSH_AUTH_SOCK", &sock);
        FarSide {
            _rt: rt,
            port,
            host_key_line: format!("[127.0.0.1]:{port} {host_public}"),
            seen,
            _dir: dir,
        }
    })
}

/// A second far side, wanting a password and offering nothing else: what
/// proves the prompted half of authentication end to end on the glass.
struct PasswordSide {
    _rt: tokio::runtime::Runtime,
    port: u16,
    host_key_line: String,
}

fn password_side() -> &'static PasswordSide {
    static SIDE: OnceLock<PasswordSide> = OnceLock::new();
    SIDE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let host_key: PrivateKey = mint(Algorithm::Ed25519);
        let host_public = host_key.public_key().to_openssh().expect("openssh form");
        let (port, _seen) = rt.block_on(serve_with(
            vec![host_key],
            AuthPlan::Password {
                user: "overseer".into(),
                password: "tumblers".into(),
                refuse_first: 0,
            },
        ));
        PasswordSide { _rt: rt, port, host_key_line: format!("[127.0.0.1]:{port} {host_public}") }
    })
}

fn shell() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".into()),
        ..SessionConfig::default()
    }
}

fn surface() -> TerminalSurface {
    let viewport = Viewport::new(720, 490, 1.0, CellSize::new(9.0, 18.0));
    TerminalSurface::headless(&shell(), viewport)
}

fn request(port: u16) -> SshRequest {
    SshRequest::parse(&format!("overseer@127.0.0.1:{port}")).expect("the spelling")
}

/// The invoking user's name, which is what a destination with no user in
/// it falls back on -- and what `~/.ssh/config` gets to outrank.
fn invoking_user_is(name: &str) {
    std::env::set_var(app::ssh::USER_VAR, name);
}

/// A destination spelled as an operator would spell it, put through the
/// same reading the connect path uses, against a `~/.ssh/config` written
/// for the occasion.
///
/// The file is named rather than found: the connect path asks the user's
/// own home for it, and a suite that moved `HOME` under the process to be
/// asked about would be reaching into every other test in the binary.
fn dial(spec: &str, config: &str) -> SshRequest {
    let dir = tempfile::tempdir().expect("scratch dir");
    let path = dir.path().join("config");
    std::fs::write(&path, config).expect("fixture");
    let mut req = SshRequest::parse(spec).expect("the spelling");
    req.take_counsel(app::ssh_config::read(&path, &req.host));
    std::mem::forget(dir);
    req
}

fn known_hosts(lines: &[&str]) -> PathBuf {
    let dir = tempfile::tempdir().expect("scratch dir");
    let path = dir.path().join("known_hosts");
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("fixture");
    // Leak the dir so the file outlives this helper; a test process's
    // scratch is the OS's to sweep.
    std::mem::forget(dir);
    path
}

/// Pump until the glass satisfies `pred`; a timeout says what stood there.
fn pump_until(surface: &mut TerminalSurface, what: &str, pred: impl Fn(&TerminalSurface) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        surface.pump();
        if pred(surface) {
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    panic!("timed out waiting for {what}\nglass: {:?}", surface.viewport_text());
}

/// The glass with the grid's own hard wrap taken back out. A notice long
/// enough to fold is still one sentence, and a test looking for a phrase
/// in it should not have to know where the fold fell.
fn glass_unwrapped(surface: &TerminalSurface) -> String {
    surface
        .channels()
        .current()
        .map(|row| viewport_text(row.session.term()).join(""))
        .unwrap_or_default()
}

fn glass_contains(surface: &TerminalSurface, word: &str) -> bool {
    surface
        .channels()
        .current()
        .map(|row| viewport_text(row.session.term()).join("\n").contains(word))
        .unwrap_or(false)
}

#[test]
fn an_ssh_bank_lives_types_and_dies_on_the_glass() {
    let far = far_side();
    let mut s = surface();
    let home = (s.channels().current_bank(), s.channels().current_channel());
    assert_eq!(home, (0, 1), "the set came up on the home shell");

    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    // The ask takes the air: a new bank, its first channel current.
    let bank = s.channels().current_bank();
    assert_ne!(bank, 0, "the connection stands as its own bank");
    assert_eq!(s.channels().current_channel(), 1);
    assert_eq!(s.channels().current().unwrap().title, "overseer@127.0.0.1");

    // The far side's greeting reaches the glass through the ordinary pump.
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));

    // Typing goes out the wire and the echo comes home.
    s.write(b"wasteland");
    pump_until(&mut s, "the echo of what was typed", |s| glass_contains(s, "wasteland"));

    // Another of what you are looking at: `new_channel` on this bank is a
    // second multiplexed channel of the same connection, on slot 2, and
    // its close leaves the first standing.
    s.new_channel();
    assert_eq!(
        (s.channels().current_bank(), s.channels().current_channel()),
        (bank, 2),
        "rows: {:?}",
        s.channels().rows().iter().map(|r| (r.bank, r.channel)).collect::<Vec<_>>()
    );
    pump_until(&mut s, "the second channel's greeting", |s| glass_contains(s, "ready"));
    s.write(b"twin");
    pump_until(&mut s, "the second channel's echo", |s| glass_contains(s, "twin"));
    s.close_channel();
    assert!(s.channels().rows().iter().any(|r| (r.bank, r.channel) == (bank, 1)));

    // The remote end exits: the row dies the way a shell's does, the bank
    // goes with it, and the air falls home.
    s.write(b"\x04");
    pump_until(&mut s, "the bank to be swept", |s| {
        s.channels().rows().iter().all(|r| r.bank == 0)
    });
    assert_eq!(s.channels().current_bank(), 0);
}

#[test]
fn a_tmux_opener_echoed_over_ssh_raises_a_bank_the_channel_carries() {
    let far = far_side();
    let mut s = surface();
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    let ssh_bank = s.channels().current_bank();
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));

    // The far side is an echo, so typing tmux's control-mode opener and the
    // `%session-changed` that says it attached brings the same bytes back as
    // remote output, exactly what a remote tmux -CC prints; the tap on the
    // SSH channel must detect it, and a tmux bank rise with the channel
    // standing on its SSH slot as the carrier.
    s.write(b"\x1bP1000p%session-changed $0 one\r\n");
    pump_until(&mut s, "a tmux bank to take the air", |s| {
        s.channels().manager_of(s.channels().current_bank()).is_some_and(|m| m.is_tmux())
    });
    assert_eq!(s.channels().current_channel(), 1);
    assert!(s.channels().is_carrier(ssh_bank, 1));

    // The gateway's bootstrap went out over the SSH wire: the far side
    // received the client-size and bootstrap commands as channel data.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        {
            let seen = far.seen.lock().unwrap();
            let text = String::from_utf8_lossy(&seen.received).into_owned();
            if text.contains("refresh-client") || text.contains("display-message") {
                break;
            }
        }
        assert!(Instant::now() < deadline, "no bootstrap reached the far side");
        s.pump();
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[test]
fn a_link_lost_under_a_carrier_ends_its_attachment_and_the_carrier_says_so() {
    let far = far_side();
    let mut s = surface();
    // Paging wants a bank drawn.
    s.set_cabinet(chassis::Cabinet::from_config(&config::Config::default(), 720.0, 490.0));
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    let ssh_bank = s.channels().current_bank();
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));

    // tmux's opener alone: the bank rises, and its gateway holds the
    // bootstrap until a `%session-changed`, so the echo has nothing of the
    // gateway's to send back as a protocol of its own.
    s.write(b"\x1bP1000p");
    pump_until(&mut s, "a tmux bank to take the air", |s| {
        s.channels().manager_of(s.channels().current_bank()).is_some_and(|m| m.is_tmux())
    });

    // The link drops under it, cut from a second channel of the same
    // connection since the carrier takes no key.
    assert!(s.step_bank(-1));
    assert_eq!(s.channels().on_air(), (ssh_bank, 1), "the carrier, on the air");
    s.new_channel();
    assert_eq!(s.channels().on_air(), (ssh_bank, 2));
    pump_until(&mut s, "the second channel's greeting", |s| glass_contains(s, "ready"));
    s.write(b"\x1c");
    pump_until(&mut s, "the tmux bank to go with the link", |s| {
        s.channels().banks().iter().all(|b| !b.manager.is_tmux())
    });
    assert_eq!(
        s.channels().slot_title(ssh_bank, 1).as_deref(),
        Some("tmux -CC ended: connection lost")
    );
    assert_eq!(
        s.channels().rows().iter().filter(|r| r.bank == ssh_bank).count(),
        2,
        "both channels stand"
    );
}

#[test]
fn a_disconnect_asked_for_takes_the_rows_with_it() {
    let far = far_side();
    let mut s = surface();
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
    assert!(s.detach_bank());
    pump_until(&mut s, "the bank to be swept", |s| {
        s.channels().rows().iter().all(|r| r.bank == 0)
    });
    assert_eq!(s.channels().banks().len(), 1);
}

#[test]
fn a_dropped_link_leaves_its_rows_saying_so_until_each_is_closed() {
    let far = far_side();
    let mut s = surface();
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    let bank = s.channels().current_bank();
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
    s.new_channel();
    pump_until(&mut s, "the second channel's greeting", |s| glass_contains(s, "ready"));
    s.write(b"lastword");
    pump_until(&mut s, "the echo", |s| glass_contains(s, "lastword"));

    // The far side drops the connection with no channel closed, as a dead
    // network does once the keepalive gives up.
    s.write(b"\x1c");
    pump_until(&mut s, "the loss on the glass", |s| {
        glass_unwrapped(s).contains("connection lost")
    });
    for _ in 0..20 {
        s.pump();
    }
    assert!(glass_contains(&s, "lastword"), "the screen as the link left it");
    let rows = |s: &TerminalSurface| s.channels().rows().iter().filter(|r| r.bank == bank).count();
    assert_eq!(rows(&s), 2, "both channels stand");

    s.close_channel();
    assert_eq!(rows(&s), 1, "closing one is the user's");
    assert_eq!(s.channels().on_air(), (bank, 1));
    assert!(glass_unwrapped(&s).contains("connection lost"));
}

#[test]
fn a_refused_host_key_is_readable_until_the_user_closes_it() {
    let far = far_side();
    let mut s = surface();

    // An empty known_hosts: unknown host. The question is asked and the
    // user types `no`.
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[])])),
    );
    let bank = s.channels().current_bank();
    assert_ne!(bank, 0);

    pump_until(&mut s, "the authenticity question", |s| {
        glass_contains(s, "authenticity")
    });
    type_line(&mut s, "no");

    // The refusal lands on the channel's own glass and the row stays: a
    // connection that never lived keeps its slot, because that slot is the
    // only place its refusal is readable.
    pump_until(&mut s, "the refusal on the glass", |s| {
        glass_contains(s, "was not accepted")
    });
    for _ in 0..5 {
        s.pump();
    }
    assert_eq!(s.channels().current_bank(), bank, "the dead glass is still readable");

    // The user, having read it, closes the channel: the bank goes.
    s.close_channel();
    assert!(s.channels().rows().iter().all(|r| r.bank == 0));
}

#[test]
fn a_first_key_accepted_on_the_glass_is_recorded_and_the_connection_goes_on() {
    let far = far_side();
    let mut s = surface();

    // Nothing recorded for this host, and a file the policy may write to:
    // exactly the state a user meets the first time they dial a box.
    let file = known_hosts(&[]);
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![file.clone()])),
    );

    // The question is on the glass, with the evidence above it, and it is
    // asked on the channel the connection stands on -- not in a dialog, not
    // in the settings window, not on stderr.
    pump_until(&mut s, "the authenticity question", |s| {
        glass_contains(s, "authenticity")
    });
    assert!(glass_contains(&s, "SHA256:"), "the fingerprint is the evidence");
    assert!(glass_contains(&s, "Type yes to accept"));

    // Typed through the keyboard the terminal already had, and echoed,
    // because a trust decision is not a secret.
    type_line(&mut s, "yes");
    assert!(glass_contains(&s, "yes"), "a yes/no answer is shown as it is typed");

    // Past trust and into the shell: the connection carried on where it
    // was blocked.
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));

    // And the key is in the file, so the next connection asks nothing.
    let recorded = std::fs::read_to_string(&file).expect("the fixture");
    assert!(
        recorded.contains(&format!("[127.0.0.1]:{}", far.port)),
        "nothing was recorded: {recorded:?}"
    );
}

/// A password asked for by the server, typed on the glass, and gone.
///
/// The last assertion is the invariant's teeth: the whole point of doing
/// this on the terminal's own grid rather than in a native dialog is that
/// the grid is a thing the user can read, so what is on it has to be
/// exactly what they may see. A password is not.
#[test]
fn a_password_typed_on_the_glass_reaches_the_wire_and_never_the_glass() {
    let far = password_side();
    let mut s = surface();
    s.connect_ssh_with(
        &request(far.port),
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );

    // Wrong first, so the retry line and a second prompt are on the glass
    // too, and both secrets have to be absent from it at the end.
    pump_until(&mut s, "the password question", |s| glass_contains(s, "password"));
    type_line(&mut s, "sarsaparilla");
    pump_until(&mut s, "the retry line", |s| glass_contains(s, "permission denied"));
    type_line(&mut s, "tumblers");
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));

    let glass = s.viewport_text().join("\n");
    assert!(
        !glass.contains("tumblers"),
        "the password is on the glass: {glass:?}"
    );
    assert!(
        !glass.contains("sarsaparilla"),
        "the wrong password is on the glass: {glass:?}"
    );
    // Not even a count of it: an asterisk row would be a length.
    assert!(!glass.contains("***"), "{glass:?}");
    assert!(glass.contains("authenticated as overseer (password)"), "{glass:?}");
}

/// The chord, as the keyboard hands it to the surface.
fn press(surface: &mut TerminalSurface, key: Key, mods: ModifiersState) {
    surface.key_input(&key, None, mods);
}

/// An answer typed on the glass, character by character through
/// `key_input`: the same path a hand takes.
fn type_text(surface: &mut TerminalSurface, answer: &str) {
    for c in answer.chars() {
        let text = c.to_string();
        surface.key_input(
            &Key::Character(text.as_str().into()),
            Some(&text),
            ModifiersState::empty(),
        );
    }
}

/// The same, committed with Enter: what answering a prompt looks like.
fn type_line(surface: &mut TerminalSurface, answer: &str) {
    type_text(surface, answer);
    surface.key_input(&Key::Named(NamedKey::Enter), None, ModifiersState::empty());
}

#[test]
fn the_picker_offers_the_configured_rows_and_the_default_stays_put() {
    let far = far_side();
    let mut s = surface();
    let mut cfg = config::Config::default();
    cfg.ssh.hosts.push(config::SshHost {
        host: "127.0.0.1".into(),
        user: "overseer".into(),
        port: far.port,
        key: String::new(),
    });
    s.set_config(cfg);

    // Shift+Alt+T: the page takes a free slot and the air.
    let chord = ModifiersState::ALT | ModifiersState::SHIFT;
    press(&mut s, Key::Character("T".into()), chord);
    let slot = s.channels().current_channel();
    assert_eq!(s.channels().current_bank(), 0);
    assert_ne!(slot, 1, "the page takes a free slot, not the shell's");
    assert!(glass_contains(&s, "SELECT DESTINATION"));
    assert!(glass_contains(&s, "overseer@127.0.0.1"));

    // Esc: the page goes, nothing opened, nothing dialled.
    press(&mut s, Key::Named(NamedKey::Escape), ModifiersState::empty());
    assert!(s.channels().rows().iter().all(|r| r.channel != slot));
    assert!(s.channels().rows().iter().all(|r| r.bank == 0));

    // Again, choose the configured server: a connection bank stands and
    // the page is gone. (The trust verdict is the real policy's and this
    // host is unknown to it; the connection path under a trusted key is
    // the earlier tests'.)
    press(&mut s, Key::Character("T".into()), chord);
    press(&mut s, Key::Character("2".into()), ModifiersState::empty());
    let bank = s.channels().current_bank();
    assert_ne!(bank, 0, "the chosen server stands as its own bank");
    assert_eq!(s.channels().current().unwrap().title, "overseer@127.0.0.1");
    assert!(s.channels().rows().iter().all(|r| r.bank != 0 || r.channel == 1));

    // Again, choose localhost: a second local shell, no connection.
    press(&mut s, Key::Character("T".into()), chord);
    press(&mut s, Key::Character("1".into()), ModifiersState::empty());
    assert_eq!(s.channels().current_bank(), 0);
    let home_rows = s.channels().rows().iter().filter(|r| r.bank == 0).count();
    assert_eq!(home_rows, 2, "the shell that was asked for, beside the first");
    assert!(!glass_contains(&s, "SELECT DESTINATION"));
}

/// A third far side, authorizing one key and nothing else: what proves an
/// `IdentityFile` the config file named reaches the named-key stage of
/// authentication, where a `[[ssh.host]]` row's own `key` goes.
struct KeySide {
    _rt: tokio::runtime::Runtime,
    port: u16,
    host_key_line: String,
    /// The private key, on disk, for an `IdentityFile` line to point at.
    key_file: PathBuf,
}

fn key_side() -> &'static KeySide {
    static SIDE: OnceLock<KeySide> = OnceLock::new();
    SIDE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("scratch dir");
        let identity = mint(Algorithm::Ed25519);
        let host_key: PrivateKey = mint(Algorithm::Ed25519);
        let host_public = host_key.public_key().to_openssh().expect("openssh form");
        let (port, _seen) = rt.block_on(serve_with(
            vec![host_key],
            AuthPlan::Key(identity.public_key().clone()),
        ));
        let key_file = dir.path().join("id_named");
        std::fs::write(
            &key_file,
            identity.to_openssh(ssh_key::LineEnding::LF).expect("openssh form").as_bytes(),
        )
        .expect("fixture");
        // The file outlives this helper; a test process's scratch is the
        // OS's to sweep.
        std::mem::forget(dir);
        KeySide {
            _rt: rt,
            port,
            host_key_line: format!("[127.0.0.1]:{port} {host_public}"),
            key_file,
        }
    })
}

/// The file's counsel, taken: an alias that is a name for nothing on this
/// machine reaches a server on loopback, under an account the operator
/// never spelled, on a port they never spelled either.
#[test]
fn a_config_alias_puts_the_connection_where_the_file_says_it_is() {
    let far = far_side();
    invoking_user_is("resident");
    let req = dial(
        "vault",
        &format!(
            "Host vault\n  HostName 127.0.0.1\n  Port {}\n  User overseer\n  \
             ServerAliveInterval 60\n",
            far.port
        ),
    );
    assert_eq!(req.host, "127.0.0.1", "HostName is where the destination is");
    assert_eq!(req.port, far.port);
    assert_eq!(req.user, "overseer", "the file outranks the invoking user's name");

    let mut s = surface();
    s.connect_ssh_with(
        &req,
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    assert_eq!(s.channels().current().unwrap().title, "overseer@127.0.0.1");
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
    // A file that was followed is a file nothing was said about, and the
    // tuning directive beside the counsel decided nothing, so it cost no
    // word either.
    let glass = glass_unwrapped(&s);
    assert!(!glass.contains("cannot honour"), "{glass:?}");
}

/// The other half of the precedence: a user the operator spelled is the
/// user that reaches the wire, whatever the file would rather.
#[test]
fn a_user_the_operator_spelled_outranks_the_files() {
    let far = password_side();
    let req = dial(
        &format!("overseer@vault:{}", far.port),
        "Host vault\n  HostName 127.0.0.1\n  User intruder\n  Port 24\n",
    );
    assert_eq!(req.host, "127.0.0.1", "the alias still translates");
    assert_eq!(req.user, "overseer");
    assert_eq!(req.port, far.port, "a port that was spelled is the port");

    let mut s = surface();
    s.connect_ssh_with(
        &req,
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    // The far side accepts that password for that account and no other,
    // so the greeting is the assertion: it authenticated as overseer.
    pump_until(&mut s, "the password question", |s| glass_contains(s, "password"));
    type_line(&mut s, "tumblers");
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
    assert!(glass_contains(&s, "authenticated as overseer (password)"));
}

/// `IdentityFile` lands where a `[[ssh.host]]` row's `key` lands: the
/// named-key stage, tried ahead of the agent.
#[test]
fn an_identity_file_the_file_names_authenticates_the_connection() {
    let far = key_side();
    invoking_user_is("resident");
    let req = dial(
        &format!("127.0.0.1:{}", far.port),
        &format!("Host 127.0.0.1\n  IdentityFile {}\n", far.key_file.display()),
    );
    assert_eq!(req.keys, vec![far.key_file.clone()]);

    let mut s = surface();
    s.connect_ssh_with(
        &req,
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    pump_until(&mut s, "the key the file named to be accepted", |s| {
        glass_contains(s, "authenticated as resident")
    });
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
}

/// The loud refusal, end to end: a block carrying `ProxyJump` yields
/// nothing at all -- not even the `HostName` beside it -- the reason is on
/// the channel's own glass, and the connection goes where the destination
/// was spelled.
#[test]
fn a_directive_this_build_cannot_honour_is_refused_out_loud_and_the_dial_goes_as_spelled() {
    let far = far_side();
    let req = dial(
        &format!("overseer@127.0.0.1:{}", far.port),
        "Host 127.0.0.1\n  HostName 10.255.255.1\n  Port 24\n  ProxyJump gate\n",
    );
    assert_eq!(req.host, "127.0.0.1", "the HostName beside the refusal is not taken either");
    assert_eq!(req.port, far.port);

    let mut s = surface();
    s.connect_ssh_with(
        &req,
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    pump_until(&mut s, "the refusal on the glass", |s| glass_contains(s, "ProxyJump"));
    let notice = glass_unwrapped(&s);
    assert!(notice.contains("cannot honour"), "{notice:?}");
    assert!(notice.contains("as spelled"), "{notice:?}");
    // And having refused the counsel, it connects: the destination as
    // spelled is a real server, and 10.255.255.1 is not.
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
}

/// The other invariant this feature has to keep: a terminal whose user has
/// no `~/.ssh/config` is a terminal that never had one read.
#[test]
fn no_config_file_moves_nothing_and_says_nothing() {
    let far = far_side();
    let spelled = request(far.port);
    let mut consulted = spelled.clone();
    let dir = tempfile::tempdir().expect("scratch dir");
    consulted.take_counsel(app::ssh_config::read(&dir.path().join("config"), "127.0.0.1"));
    assert_eq!(consulted, spelled, "a file that is not there moves no field of a request");

    let mut s = surface();
    s.connect_ssh_with(
        &consulted,
        Box::new(KnownHosts::over(vec![known_hosts(&[&far.host_key_line])])),
    );
    pump_until(&mut s, "the remote shell's greeting", |s| glass_contains(s, "ready"));
    let glass = glass_unwrapped(&s);
    assert!(!glass.contains("config"), "{glass:?}");
    assert!(!glass.contains("cannot honour"), "{glass:?}");
}

/// `Ctrl+Shift+T` on a local bank asks for another of what a window opens
/// on, and where the `[ssh]` table names a default that is a connection.
/// The trust verdict is the real policy's and this host is unknown to it;
/// what is asserted is where the channel went, which the dial decides
/// before any key is presented.
#[test]
fn a_new_channel_on_a_local_bank_follows_the_configured_default() {
    let far = far_side();
    let mut s = surface();
    assert_eq!(s.channels().current_bank(), 0);

    // No default: the second channel is a second shell at home, which is
    // the terminal an empty `[ssh]` table leaves standing.
    s.new_channel();
    assert_eq!(s.channels().current_bank(), 0);
    assert_eq!(s.channels().rows().iter().filter(|r| r.bank == 0).count(), 2);

    let mut cfg = config::Config::default();
    cfg.ssh.hosts.push(config::SshHost {
        host: "127.0.0.1".into(),
        user: "overseer".into(),
        port: far.port,
        key: String::new(),
    });
    cfg.ssh.default = "127.0.0.1".into();
    s.set_config(cfg);

    s.new_channel();
    let bank = s.channels().current_bank();
    assert_ne!(bank, 0, "the default stands as its own bank");
    assert_eq!(s.channels().current().unwrap().title, "overseer@127.0.0.1");
    assert_eq!(s.channels().current_channel(), 1);
    assert_eq!(
        s.channels().rows().iter().filter(|r| r.bank == 0).count(),
        2,
        "the home bank kept the shells it had and gained none"
    );
}
