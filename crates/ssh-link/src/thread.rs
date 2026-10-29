//! The connection's own thread: a current-thread runtime driving russh,
//! feeding the loop-side endpoints in `channel`. The comments here and in
//! `channel` call this thread's [`run`] loop the supervisor: it connects,
//! authenticates, opens channels, and outlives every channel task it
//! spawns.
//!
//! One thread per connection, named after the host the way the
//! single-instance listener is named for its job. The thread ends when the
//! connection does, whichever side ended it; every channel hears `Eof`
//! first, so no row outlives its wire silently.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::PublicKeyOrCertificate;
use russh::client::AuthResult;
use russh::{ChannelMsg, MethodKind, MethodSet};
use tokio::sync::mpsc;

use crate::channel::{ChannelCmd, WireEvent, EVENT_QUEUE};
use crate::{Answer, Asker, ChannelHandle, HostPolicy, SshTarget};

/// How many times a secret may be got wrong -- a key file's passphrase, a
/// password -- before that method is given up on and the next one tried.
/// `ssh`'s own `NumberOfPasswordPrompts`, and the same number for the same
/// reason: enough for a typo, few enough that a wrong guess does not
/// become an interrogation.
const PASSPHRASE_TRIES: usize = 3;

/// The end of an authentication attempt. Three answers rather than two,
/// because withdrawing a question is not the same as failing to satisfy
/// it: a user who pressed `Esc` at a passphrase prompt has said they do
/// not want to connect, and falling through to the next method would
/// answer that by asking them something else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Authenticated {
    Yes,
    No,
    Cancelled,
}

/// Commands from the loop side to the connection itself.
pub(crate) enum LinkCmd {
    /// Another channel on this connection: shell it, then drive its wire.
    Open { wire: ChannelWire, size: (u16, u16, u16, u16) },
}

/// The supervisor's grip on one channel's loop-side endpoints.
pub(crate) struct ChannelWire {
    pub events: mpsc::Sender<WireEvent>,
    pub cmd: mpsc::UnboundedReceiver<ChannelCmd>,
    pub queued: Arc<std::sync::atomic::AtomicUsize>,
}

pub(crate) fn endpoints() -> (ChannelHandle, ChannelWire) {
    let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sheds = Arc::new(std::sync::atomic::AtomicU64::new(0));
    (
        ChannelHandle { events: event_rx, cmd: cmd_tx, queued: queued.clone(), sheds },
        ChannelWire { events: event_tx, cmd: cmd_rx, queued },
    )
}

/// Spawn the connection thread. The wire is the first (and in stage 1 only)
/// channel's; its `Eof` is how the loop side learns the connection is over.
pub(crate) fn spawn(
    target: SshTarget,
    policy: Box<dyn HostPolicy>,
    asker: Asker,
    wire: ChannelWire,
    link_cmd: mpsc::UnboundedReceiver<LinkCmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("ssh-{}", target.host))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    // No runtime, no wire pump: the blocking sender is safe
                    // here because nothing async exists yet.
                    let _ = wire.events.blocking_send(WireEvent::Notice(format!(
                        "runtime failed to start: {e}"
                    )));
                    let _ = wire.events.blocking_send(WireEvent::Eof);
                    return;
                }
            };
            rt.block_on(run(target, policy, asker, wire, link_cmd));
        })
}

/// What `check_server_key` leaves behind for the error path: the policy's
/// refusal text, which russh's own error does not carry.
struct Verdicts {
    refusal: Mutex<Option<String>>,
}

struct Handler {
    policy: Box<dyn HostPolicy>,
    host: String,
    port: u16,
    verdicts: Arc<Verdicts>,
    /// The policy's line to the glass. Held here because this is where
    /// russh calls the policy from, and the policy has no other way to
    /// reach a human from inside a handshake.
    ask: Asker,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        match self.policy.verify(&self.host, self.port, key, &self.ask) {
            Ok(()) => Ok(true),
            Err(text) => {
                *self.verdicts.refusal.lock().unwrap() = Some(text);
                Ok(false)
            }
        }
    }
}

async fn notice(wire: &ChannelWire, text: impl Into<String>) {
    let _ = wire.events.send(WireEvent::Notice(text.into())).await;
}

/// How often the client asks a silent server whether it is still there.
///
/// A shell left sitting at its prompt sends nothing, and a NAT or a stateful
/// firewall between here and the server forgets an idle mapping in minutes.
/// Neither end notices: the connection is not closed, it is unreachable, and
/// the first the user hears of it is that their next keystroke goes nowhere
/// and stays nowhere. russh runs the probe loop itself once this is set, so
/// setting it is the whole of the fix.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// The connection's configuration, split out from [`run`] so that what it
/// promises is testable without a socket: the keepalive, and the host-key
/// order the policy recorded.
///
/// `keepalive_max` is left at russh's default of three. Three unanswered
/// probes a minute apart is about three minutes to call a peer dead, the same
/// order as OpenSSH's own `ServerAliveInterval`/`ServerAliveCountMax` and long
/// enough that a laptop's lid or a train's tunnel is not mistaken for a
/// disconnection.
///
/// The recorded algorithms lead the preference list, or a host recorded under
/// one algorithm reads as unknown when the server also holds a preferred one.
pub(crate) fn client_config(order: Option<Vec<russh::keys::Algorithm>>) -> client::Config {
    let mut config = client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        ..Default::default()
    };
    if let Some(order) = order {
        if !order.is_empty() {
            let mut key: Vec<_> = order;
            for alg in config.preferred.key.iter() {
                if !key.contains(alg) {
                    key.push(alg.clone());
                }
            }
            config.preferred.key = key.into();
        }
    }
    config
}

async fn run(
    target: SshTarget,
    mut policy: Box<dyn HostPolicy>,
    asker: Asker,
    wire: ChannelWire,
    mut link_cmd: mpsc::UnboundedReceiver<LinkCmd>,
) {
    let verdicts = Arc::new(Verdicts { refusal: Mutex::new(None) });

    notice(&wire, format!("connecting to {}:{}", target.host, target.port)).await;

    let config = client_config(policy.key_order(&target.host, target.port));

    let handler = Handler {
        policy,
        host: target.host.clone(),
        port: target.port,
        verdicts: verdicts.clone(),
        ask: asker.clone(),
    };

    let mut handle = match client::connect(
        Arc::new(config),
        (target.host.as_str(), target.port),
        handler,
    )
    .await
    {
        Ok(handle) => handle,
        Err(e) => {
            // The policy's own words beat russh's, when it spoke.
            let text = match verdicts.refusal.lock().unwrap().take() {
                Some(refusal) => refusal,
                None => format!("connection failed: {e}"),
            };
            notice(&wire, text).await;
            let _ = wire.events.send(WireEvent::Eof).await;
            return;
        }
    };

    match authenticate(&mut handle, &target, &wire, &asker).await {
        Authenticated::Yes => {}
        Authenticated::Cancelled => {
            // The user withdrew a question rather than failing one. Saying
            // so is what distinguishes the row they meant to close from the
            // row that could not get in.
            notice(&wire, "the question was cancelled").await;
            let _ = wire.events.send(WireEvent::Eof).await;
            return;
        }
        Authenticated::No => {
            let _ = wire.events.send(WireEvent::Eof).await;
            return;
        }
    }

    // The first channel, then any the loop side asks for: opened here (the
    // handle is the supervisor's alone), each then driven on a task of its
    // own -- this runtime is one thread, so a task is concurrency, not
    // parallelism. The supervisor ends when the loop side drops the Link,
    // and the disconnect then ends every channel task through its Eof.
    let hanging_up = Arc::new(AtomicBool::new(false));
    raise(&handle, &target.term, target.size, wire, &hanging_up).await;
    while let Some(LinkCmd::Open { wire, size }) = link_cmd.recv().await {
        raise(&handle, &target.term, size, wire, &hanging_up).await;
    }
    hanging_up.store(true, Ordering::Relaxed);
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
}

/// Open a session channel with a pty and shell on it, and put its pump on
/// a task. A refusal at any step lands on the wire and ends with `Eof`, so
/// the row asking hears why.
async fn raise(
    handle: &client::Handle<Handler>,
    term: &str,
    size: (u16, u16, u16, u16),
    wire: ChannelWire,
    hanging_up: &Arc<AtomicBool>,
) {
    let channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            notice(&wire, format!("channel refused: {e}")).await;
            let _ = wire.events.send(WireEvent::Eof).await;
            return;
        }
    };
    let (cols, rows, pix_w, pix_h) = size;
    let pty = channel
        .request_pty(
            true,
            term,
            u32::from(cols),
            u32::from(rows),
            u32::from(pix_w),
            u32::from(pix_h),
            &[],
        )
        .await;
    let shell = match pty {
        Ok(()) => channel.request_shell(true).await,
        Err(e) => Err(e),
    };
    if let Err(e) = shell {
        notice(&wire, format!("shell refused: {e}")).await;
        let _ = wire.events.send(WireEvent::Eof).await;
        return;
    }
    tokio::spawn(drive(channel, wire, hanging_up.clone()));
}

/// Pump one raised channel against its wire until either side ends it, or
/// the connection under it goes. That last end is the only one nobody chose,
/// so it is the one said on the wire before the `Eof`, unless `hanging_up`
/// shows it was the loop side's own disconnect.
async fn drive(channel: russh::Channel<client::Msg>, mut wire: ChannelWire, hanging_up: Arc<AtomicBool>) {
    let mut channel = channel;
    let mut lost = false;
    loop {
        tokio::select! {
            cmd = wire.cmd.recv() => match cmd {
                Some(ChannelCmd::Data(bytes)) => {
                    let len = bytes.len();
                    let sent = channel.data(&bytes[..]).await;
                    wire.queued.fetch_sub(len, Ordering::Relaxed);
                    if sent.is_err() {
                        lost = true;
                        break;
                    }
                }
                Some(ChannelCmd::WindowChange { cols, rows, pix_w, pix_h }) => {
                    let _ = channel
                        .window_change(
                            u32::from(cols),
                            u32::from(rows),
                            u32::from(pix_w),
                            u32::from(pix_h),
                        )
                        .await;
                }
                // Close, or the loop side dropped the handle: this channel
                // is done, and the connection's own end is the Link's.
                Some(ChannelCmd::Close) | None => break,
            },
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    if wire.events.send(WireEvent::Data(data.to_vec())).await.is_err() {
                        break;
                    }
                }
                // A pty merges stderr into the stream server-side; extended
                // data still arrives for a session without one, and the
                // glass is where it belongs either way.
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if wire.events.send(WireEvent::Data(data.to_vec())).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    let _ = wire.events.send(WireEvent::ExitStatus(exit_status)).await;
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => break,
                None => {
                    lost = true;
                    break;
                }
                Some(_) => {}
            },
        }
    }

    if lost && !hanging_up.load(Ordering::Relaxed) {
        notice(&wire, "connection lost").await;
    }
    let _ = channel.eof().await;
    let _ = wire.events.send(WireEvent::Eof).await;
}

/// Publickey auth, in intent order: the keys the configuration names, in
/// the order it names them, then whatever agent is running, then (only
/// when no key was named at all) the default key files `ssh` itself would
/// try. Each stage's failure puts its reason on the wire, and the summary
/// that closes a lost cause names what the server offers.
///
/// A cancellation anywhere in the sequence ends it where it stands. The
/// method after the one the user just declined would ask them something
/// else, and "no" to a question about connecting is an answer about
/// connecting, not about that question.
async fn authenticate(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    ask: &Asker,
) -> Authenticated {
    // Not an auth attempt that could succeed: it is the probe whose reply
    // says what the server would take. That list is kept as a value and
    // refreshed from every rejection, because it is what decides which of
    // the methods below are tried at all. A server that wants a key will
    // not be satisfied by a password, and asking for one would be this
    // program inventing a question nobody asked.
    let mut offered = match handle.authenticate_none(target.user.clone()).await {
        Ok(AuthResult::Success) => return Authenticated::Yes,
        Ok(AuthResult::Failure { remaining_methods, .. }) => remaining_methods,
        Err(e) => {
            notice(wire, format!("authentication failed: {e}")).await;
            return Authenticated::No;
        }
    };
    if offered.contains(&MethodKind::PublicKey) {
        for path in &target.key_files {
            match try_key_file(handle, target, wire, ask, &mut offered, path).await {
                Authenticated::No => {}
                end => return end,
            }
        }
        match agent_auth(handle, target, wire, &mut offered).await {
            Authenticated::No => {}
            end => return end,
        }
        if target.key_files.is_empty() {
            for path in default_key_files() {
                match try_key_file(handle, target, wire, ask, &mut offered, &path).await {
                    Authenticated::No => {}
                    end => return end,
                }
            }
        }
    }
    // Then the prompted methods, in `ssh`'s own PreferredAuthentications
    // order: keyboard-interactive before password, because the server
    // composes the questions in the first and gets only a password in the
    // second, so the first is the one that can carry a second factor.
    if offered.contains(&MethodKind::KeyboardInteractive) {
        match challenge(handle, target, wire, ask, &mut offered).await {
            Authenticated::No => {}
            end => return end,
        }
    }
    if offered.contains(&MethodKind::Password) {
        match password(handle, target, wire, ask, &mut offered).await {
            Authenticated::No => {}
            end => return end,
        }
    }
    notice(wire, format!("authentication failed; the server offers {offered:?}")).await;
    Authenticated::No
}

/// The server's own challenge: it composes the questions, this asks them
/// in the order given, and the echo flag on each is what says whether the
/// answer may appear on the glass.
///
/// One round is the common case (a password prompt by another name) and
/// several are legal: a one-time code after a password, a PIN after a
/// touch. The loop is the protocol's rather than a retry -- the server
/// decides how many rounds there are, and ends them by succeeding or
/// failing.
async fn challenge(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    ask: &Asker,
    offered: &mut MethodSet,
) -> Authenticated {
    use russh::client::KeyboardInteractiveAuthResponse as Step;
    let mut step = match handle
        .authenticate_keyboard_interactive_start(target.user.clone(), None)
        .await
    {
        Ok(step) => step,
        Err(e) => {
            notice(wire, format!("the server's challenge failed: {e}")).await;
            return Authenticated::No;
        }
    };
    loop {
        match step {
            Step::Success => {
                notice(
                    wire,
                    format!("authenticated as {} (keyboard-interactive)", target.user),
                )
                .await;
                return Authenticated::Yes;
            }
            Step::Failure { remaining_methods, .. } => {
                *offered = remaining_methods;
                notice(wire, "the server did not accept the challenge").await;
                return Authenticated::No;
            }
            Step::InfoRequest { name, instructions, prompts } => {
                // The server's framing, when it sent any, goes on the wire
                // before the questions it frames: the ordering the desk
                // depends on, applied to somebody else's words.
                for line in [name.trim(), instructions.trim()] {
                    if !line.is_empty() {
                        notice(wire, line).await;
                    }
                }
                let mut answers = Vec::with_capacity(prompts.len());
                for prompt in prompts {
                    // The server says whether this one may be seen. A
                    // one-time code or an employee number is ordinary text;
                    // anything it wants hidden is a secret here too.
                    let kind = if prompt.echo { Answer::Text } else { Answer::Secret };
                    let asking = ask.clone();
                    let question = prompt.prompt.clone();
                    let answer = tokio::task::spawn_blocking(move || asking.ask(question, kind))
                        .await
                        .ok()
                        .flatten();
                    match answer {
                        Some(text) => answers.push(text),
                        None => return Authenticated::Cancelled,
                    }
                }
                step = match handle.authenticate_keyboard_interactive_respond(answers).await {
                    Ok(next) => next,
                    Err(e) => {
                        notice(wire, format!("the server's challenge failed: {e}")).await;
                        return Authenticated::No;
                    }
                };
            }
        }
    }
}

/// The last method, and the plainest: the account's password, asked for
/// under the name the account is spelled with so it is clear which door
/// the answer opens.
async fn password(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    ask: &Asker,
    offered: &mut MethodSet,
) -> Authenticated {
    let question = format!("{}@{}'s password: ", target.user, target.host);
    for attempt in 0..PASSPHRASE_TRIES {
        if attempt > 0 {
            // `ssh`'s own words, on the wire before the question they are
            // about, so the user reads why they are being asked again.
            notice(wire, "permission denied, please try again").await;
        }
        let asking = ask.clone();
        let prompt = question.clone();
        let answer = tokio::task::spawn_blocking(move || asking.ask(prompt, Answer::Secret))
            .await
            .ok()
            .flatten();
        let Some(secret) = answer else {
            return Authenticated::Cancelled;
        };
        match handle.authenticate_password(target.user.clone(), secret).await {
            Ok(AuthResult::Success) => {
                notice(wire, format!("authenticated as {} (password)", target.user)).await;
                return Authenticated::Yes;
            }
            Ok(AuthResult::Failure { remaining_methods, .. }) => *offered = remaining_methods,
            Err(e) => {
                notice(wire, format!("password authentication failed: {e}")).await;
                return Authenticated::No;
            }
        }
    }
    Authenticated::No
}

/// The key files `ssh` itself tries when none is named, kept to the ones
/// that exist. One list on every platform: OpenSSH for Windows reads the
/// same `.ssh` under the profile directory.
fn default_key_files() -> Vec<std::path::PathBuf> {
    let Some(home) = std::env::home_dir() else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .filter(|path| path.exists())
        .collect()
}

/// Whether `path` is a PuTTY key file, and if so whether it is encrypted:
/// `Some(true)` wants a passphrase, `Some(false)` does not, `None` is some
/// other format.
///
/// russh reads ppk like any other key format, but its error type cannot say
/// "this one is encrypted" for ppk the way `KeyIsEncrypted` says it for the
/// rest (ssh-key keeps the ppk error variants private, and a wrong
/// passphrase surfaces as a MAC mismatch besides). The file's own opening
/// headers answer the question the error cannot: `PuTTY-User-Key-File-N:`
/// on the first line, `Encryption: aes256-cbc` or `none` on the second.
/// Only those two lines are read; trailing `\r` is tolerated for a file
/// that has crossed from Windows.
fn ppk_encrypted(path: &std::path::Path) -> Option<bool> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let mut lines = std::io::BufReader::new(file).lines();
    let version = lines.next()?.ok()?;
    version
        .trim_end_matches('\r')
        .strip_prefix("PuTTY-User-Key-File-")?
        .split(':')
        .next()?
        .parse::<u8>()
        .ok()?;
    let encryption = lines.next()?.ok()?;
    let cipher = encryption.trim_end_matches('\r').strip_prefix("Encryption:")?;
    Some(cipher.trim() != "none")
}

/// One key file against the server, asking for its passphrase if it has
/// one. russh reads OpenSSH, PKCS#8, PKCS#1/SEC1 and PuTTY ppk (v2 and
/// v3) alike; a DSA ppk parses but cannot sign, because the build leaves
/// the legacy `dsa` algorithm out, so signing fails locally with an
/// unsupported-algorithm error before anything reaches the server.
///
/// The ask runs on a blocking task rather than on this one. A human types
/// at a human's pace, and this runtime is a single thread that is also
/// driving the russh session: parking it on the answer would stop reading
/// packets for as long as someone is looking for a sticky note.
async fn try_key_file(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    ask: &Asker,
    offered: &mut MethodSet,
    path: &std::path::Path,
) -> Authenticated {
    use russh::keys::{load_secret_key, Error as KeysError, PrivateKeyWithHashAlg};
    let ppk_wants_passphrase = ppk_encrypted(path) == Some(true);
    let mut passphrase: Option<String> = None;
    let mut asked = 0usize;
    let key = loop {
        let attempt = load_secret_key(path, passphrase.as_deref());
        let Err(e) = attempt else {
            break attempt.expect("the Ok arm");
        };
        // Two shapes of one situation: the key wants a passphrase and has
        // not been given one (`KeyIsEncrypted`, or an encrypted ppk, whose
        // errors never carry that variant), or it was given one that did
        // not fit. Any other error with no passphrase in play is an
        // unreadable file.
        let unlocked = !matches!(e, KeysError::KeyIsEncrypted)
            && !(ppk_wants_passphrase && passphrase.is_none());
        if unlocked && passphrase.is_none() {
            notice(wire, format!("could not read {}: {e}", path.display())).await;
            return Authenticated::No;
        }
        if unlocked {
            // The wire before the desk: this is the line that explains the
            // question about to be asked, so it goes out first.
            notice(wire, format!("that passphrase did not open {}", path.display())).await;
        }
        if asked >= PASSPHRASE_TRIES {
            notice(wire, format!("{} stays locked", path.display())).await;
            return Authenticated::No;
        }
        asked += 1;
        let asking = ask.clone();
        let prompt = format!("passphrase for {}: ", path.display());
        let answer = tokio::task::spawn_blocking(move || asking.ask(prompt, Answer::Secret))
            .await
            .ok()
            .flatten();
        match answer {
            Some(text) => passphrase = Some(text),
            None => return Authenticated::Cancelled,
        }
    };
    let hash_alg = if key.algorithm().is_rsa() {
        match handle.best_supported_rsa_hash().await {
            Ok(Some(alg)) => alg,
            _ => None,
        }
    } else {
        None
    };
    match handle
        .authenticate_publickey(
            target.user.clone(),
            PrivateKeyWithHashAlg::new(std::sync::Arc::new(key), hash_alg),
        )
        .await
    {
        Ok(AuthResult::Success) => {
            notice(
                wire,
                format!("authenticated as {} ({})", target.user, path.display()),
            )
            .await;
            Authenticated::Yes
        }
        Ok(AuthResult::Failure { remaining_methods, .. }) => {
            *offered = remaining_methods;
            notice(wire, format!("the server did not accept {}", path.display())).await;
            Authenticated::No
        }
        Err(e) => {
            notice(
                wire,
                format!("authentication with {} failed: {e}", path.display()),
            )
            .await;
            Authenticated::No
        }
    }
}

/// This platform's agent: whatever `SSH_AUTH_SOCK` names. The agent path
/// asks nobody anything -- signing is the agent's business, and any
/// unlocking it wants it does at its own prompt -- so `Cancelled` is not
/// among its answers.
#[cfg(unix)]
async fn agent_auth(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    offered: &mut MethodSet,
) -> Authenticated {
    let agent = match AgentClient::connect_env().await {
        Ok(agent) => agent,
        Err(_) => {
            notice(wire, "no agent is reachable (SSH_AUTH_SOCK is unset or dead)").await;
            return Authenticated::No;
        }
    };
    sign_with(handle, target, wire, offered, agent).await
}

/// This platform's agents: the OpenSSH Authentication Agent service on its
/// fixed pipe name, then Pageant. Tried in that order because the service
/// ships with Windows and Pageant is the add-on.
#[cfg(windows)]
async fn agent_auth(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    offered: &mut MethodSet,
) -> Authenticated {
    const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    if let Ok(agent) = AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
        return sign_with(handle, target, wire, offered, agent).await;
    }
    match AgentClient::connect_pageant().await {
        Ok(agent) => sign_with(handle, target, wire, offered, agent).await,
        Err(_) => {
            notice(
                wire,
                "no agent answered: neither the OpenSSH Authentication Agent service nor Pageant is running",
            )
            .await;
            Authenticated::No
        }
    }
}

/// The identities loop, blind to which stream the agent answers on.
async fn sign_with<S>(
    handle: &mut client::Handle<Handler>,
    target: &SshTarget,
    wire: &ChannelWire,
    offered: &mut MethodSet,
    mut agent: AgentClient<S>,
) -> Authenticated
where
    S: russh::keys::agent::client::AgentStream + Send + Unpin,
{
    let identities = match agent.request_identities().await {
        Ok(ids) => ids,
        Err(e) => {
            notice(wire, format!("the agent refused to list identities: {e}")).await;
            return Authenticated::No;
        }
    };
    if identities.is_empty() {
        notice(wire, "the agent holds no identities").await;
        return Authenticated::No;
    }

    for identity in identities {
        let AgentIdentity::PublicKey { key, comment } = identity else {
            // A certificate identity waits on the trust design that #14
            // parks with the interface; skipping it is the legible choice.
            continue;
        };
        let hash_alg = if key.algorithm().is_rsa() {
            match handle.best_supported_rsa_hash().await {
                Ok(Some(alg)) => alg,
                _ => None,
            }
        } else {
            None
        };
        match handle
            .authenticate_publickey_with(target.user.clone(), key, hash_alg, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => {
                notice(wire, format!("authenticated as {} ({comment})", target.user)).await;
                return Authenticated::Yes;
            }
            Ok(AuthResult::Failure { remaining_methods, .. }) => {
                *offered = remaining_methods;
                continue;
            }
            Err(e) => {
                notice(wire, format!("agent signing failed: {e}")).await;
                return Authenticated::No;
            }
        }
    }

    notice(wire, "no identity in the agent was accepted").await;
    Authenticated::No
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::Algorithm;

    /// The keepalive, pinned. An idle shell behind a NAT is the case: with no
    /// interval russh never probes, the mapping is forgotten, and the row is
    /// dead without anything having said so.
    #[test]
    fn a_connection_probes_a_silent_server_and_gives_it_three_chances() {
        let config = client_config(None);
        assert_eq!(
            config.keepalive_interval,
            Some(Duration::from_secs(60)),
            "a silent connection must still say something once a minute"
        );
        assert_eq!(
            config.keepalive_max, 3,
            "three missed probes is about three minutes to call a peer dead"
        );
    }

    /// The order the policy recorded leads, and nothing russh would have
    /// offered is dropped to make room for it: a server holding both a
    /// recorded key and a default-preferred one must present the recorded one,
    /// while a server holding neither must still be able to negotiate.
    #[test]
    fn a_recorded_key_order_leads_the_preference_list() {
        let recorded = Algorithm::Rsa { hash: None };
        let keys: Vec<_> = client_config(Some(vec![recorded.clone()]))
            .preferred
            .key
            .iter()
            .cloned()
            .collect();

        assert_eq!(keys.first(), Some(&recorded), "the recorded key must lead");
        assert_eq!(
            keys.iter().filter(|a| **a == recorded).count(),
            1,
            "leading is a move, not a second entry"
        );
        for offered in client_config(None).preferred.key.iter() {
            assert!(
                keys.contains(offered),
                "{offered:?} was dropped from the preference list"
            );
        }
    }

    /// An empty recording is not a recording: russh's own order stands.
    #[test]
    fn no_recording_leaves_russhs_own_order_alone() {
        let default: Vec<_> = client_config(None).preferred.key.iter().cloned().collect();
        let empty: Vec<_> = client_config(Some(Vec::new()))
            .preferred
            .key
            .iter()
            .cloned()
            .collect();
        assert_eq!(default, empty);
    }

    /// A ppk file names its format on the first line and its cipher on the
    /// second; those two lines are the whole of what the peek trusts.
    #[test]
    fn an_encrypted_ppk_header_wants_a_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.ppk");
        std::fs::write(
            &path,
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\nother lines\n",
        )
        .unwrap();
        assert_eq!(ppk_encrypted(&path), Some(true));
    }

    #[test]
    fn an_unencrypted_ppk_header_wants_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.ppk");
        std::fs::write(
            &path,
            "PuTTY-User-Key-File-2: ssh-rsa\nEncryption: none\nother lines\n",
        )
        .unwrap();
        assert_eq!(ppk_encrypted(&path), Some(false));
    }

    /// A file crossed from Windows carries a trailing \r on every line; the
    /// peek must not let that hide the version digit or the cipher name.
    #[test]
    fn a_crlf_header_is_still_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.ppk");
        std::fs::write(
            &path,
            "PuTTY-User-Key-File-3: ssh-rsa\r\nEncryption: aes256-cbc\r\nother lines\r\n",
        )
        .unwrap();
        assert_eq!(ppk_encrypted(&path), Some(true));
    }

    /// An OpenSSH key's own first line must not read as a ppk header: the
    /// two formats share nothing but both being text.
    #[test]
    fn an_openssh_key_is_not_mistaken_for_a_ppk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_ed25519");
        std::fs::write(&path, "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n").unwrap();
        assert_eq!(ppk_encrypted(&path), None);
    }

    #[test]
    fn a_missing_file_sniffs_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist");
        assert_eq!(ppk_encrypted(&path), None);
    }
}
