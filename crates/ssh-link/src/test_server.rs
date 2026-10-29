//! An SSH far side that lives inside the test process: a russh server on
//! loopback and an agent on a Unix socket. Behind the `test-server`
//! feature, for this crate's suite and the app's; nothing of it reaches a
//! shipped binary.
//!
//! No sshd runs and none is needed: russh's server side compiles
//! unconditionally, which is what lets trust and auth be proven on every
//! machine the workspace builds on.

use std::sync::{Arc, Mutex};

use russh::keys::ssh_key;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Response, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};

/// What the far side saw, for assertions that need it.
#[derive(Default)]
pub struct Seen {
    pub resizes: Vec<(u32, u32)>,
    /// Every byte the client sent, in order.
    pub received: Vec<u8>,
}

/// How the far side decides who gets in. One plan per server, because a
/// real sshd's `PasswordAuthentication`/`PubkeyAuthentication` is a
/// configuration and not a per-request whim, and because a client that is
/// meant to skip the methods a server does not offer can only be caught
/// doing otherwise by a server that offers exactly one.
pub enum AuthPlan {
    /// One authorized public key.
    Key(ssh_key::PublicKey),
    /// One account and its password. `refuse_first` rejects that many
    /// attempts before it starts checking, which is how a test sees the
    /// retry line without needing two different passwords.
    Password { user: String, password: String, refuse_first: usize },
    /// A two-prompt challenge: an employee number that echoes and a
    /// passphrase that does not, answered in that order.
    KeyboardInteractive { answers: Vec<String> },
}

impl AuthPlan {
    /// What the server advertises, which is what the client's method
    /// skipping is judged against. `none` is in every set because the
    /// client's opening probe is a `none` request.
    fn methods(&self) -> MethodSet {
        let kinds: &[MethodKind] = match self {
            AuthPlan::Key(_) => &[MethodKind::None, MethodKind::PublicKey],
            AuthPlan::Password { .. } => &[MethodKind::None, MethodKind::Password],
            AuthPlan::KeyboardInteractive { .. } => {
                &[MethodKind::None, MethodKind::KeyboardInteractive]
            }
        };
        MethodSet::from(kinds)
    }

    /// A rejection that leaves the same methods on offer, which is what a
    /// real sshd does: a wrong password does not stop it wanting one.
    fn refuse(&self) -> Auth {
        Auth::Reject { proceed_with_methods: Some(self.methods()), partial_success: false }
    }
}

/// The test shell: authenticates by its [`AuthPlan`], replies to pty and
/// shell requests, prints `ready`, echoes everything, and treats a lone
/// 0x04 as "exit with status 7".
pub struct Echo {
    plan: Arc<AuthPlan>,
    /// Password attempts refused so far, against the plan's `refuse_first`.
    refused: usize,
    seen: Arc<Mutex<Seen>>,
}

impl server::Handler for Echo {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        // The key's material alone: a comment is metadata, and a key that
        // travelled through a commented file (a ppk's `Comment:` header)
        // is still the key the plan authorised.
        match &*self.plan {
            AuthPlan::Key(authorized) if key.key_data() == authorized.key_data() => {
                Ok(Auth::Accept)
            }
            plan => Ok(plan.refuse()),
        }
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let AuthPlan::Password { user: account, password: secret, refuse_first } = &*self.plan
        else {
            return Ok(self.plan.refuse());
        };
        if self.refused < *refuse_first {
            self.refused += 1;
            return Ok(self.plan.refuse());
        }
        if user == account && password == secret {
            Ok(Auth::Accept)
        } else {
            Ok(self.plan.refuse())
        }
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let AuthPlan::KeyboardInteractive { answers } = &*self.plan else {
            return Ok(self.plan.refuse());
        };
        let Some(response) = response else {
            return Ok(Auth::Partial {
                name: "Vault-Tec Overseer Terminal".into(),
                instructions: "Two questions before the door opens.".into(),
                prompts: vec![
                    ("Employee number: ".into(), true),
                    ("Passphrase: ".into(), false),
                ]
                .into(),
            });
        };
        let given: Vec<String> = response
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .collect();
        if given == *answers {
            Ok(Auth::Accept)
        } else {
            Ok(self.plan.refuse())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        // The channel pumps itself; dropping it here would close it.
        std::mem::forget(channel);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, &b"ready\r\n"[..])?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen.lock().unwrap().received.extend_from_slice(data);
        // Ctrl+\ drops the connection under every channel, closing none:
        // the shape of a link that died.
        if data == b"\x1c" {
            return Err(russh::Error::HUP);
        }
        if data == b"\x04" {
            session.exit_status_request(channel, 7)?;
            session.eof(channel)?;
            session.close(channel)?;
        } else {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen.lock().unwrap().resizes.push((col_width, row_height));
        Ok(())
    }
}

/// A freshly-minted key of the given algorithm.
pub fn mint(algorithm: Algorithm) -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), algorithm).unwrap()
}

/// Serve [`Echo`] on loopback with the given host keys, forever, under
/// the given plan. Answers the port and the far side's log. Must be
/// called on a tokio runtime.
pub async fn serve_with(
    host_keys: Vec<PrivateKey>,
    plan: AuthPlan,
) -> (u16, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let plan = Arc::new(plan);
    let config = Arc::new(server::Config {
        keys: host_keys,
        methods: plan.methods(),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let far = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let handler = Echo { plan: plan.clone(), refused: 0, seen: far.clone() };
            let config = config.clone();
            tokio::spawn(async move {
                let _ = server::run_stream(config, stream, handler).await;
            });
        }
    });
    (port, seen)
}

/// The same under the key plan, which is what most of the suite wants.
pub async fn serve_echo(
    host_keys: Vec<PrivateKey>,
    authorized: ssh_key::PublicKey,
) -> (u16, Arc<Mutex<Seen>>) {
    serve_with(host_keys, AuthPlan::Key(authorized)).await
}

/// An agent on a Unix socket in `dir`, holding one identity. Answers the
/// socket path, ready to become `SSH_AUTH_SOCK`. Must be called on a tokio
/// runtime.
#[cfg(unix)]
pub async fn serve_agent(dir: &std::path::Path, identity: &PrivateKey) -> std::path::PathBuf {
    let sock = dir.join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(russh::keys::agent::server::serve(
        tokio_stream::wrappers::UnixListenerStream::new(listener),
        (),
    ));
    let mut client = russh::keys::agent::client::AgentClient::connect_uds(&sock)
        .await
        .unwrap();
    client.add_identity(identity, &[]).await.unwrap();
    sock
}
