//! Opening an SSH connection, and opening another channel on one already
//! standing.
//!
//! Everything a connection needs from the window is here: the grid to size
//! the remote pty to, the `TERM` the session config advertises, and the bank
//! the model raises for it. The policy is [`crate::ssh`]'s and the transport
//! is `ssh_link`'s; neither is restated.
//!
//! Fields touched: `channels`, which raises the bank and holds the rows;
//! `banks`, whose record takes the `Link` (dropping it is the disconnect)
//! and the `AskDesk` the connection asks its questions over; `viewport` and
//! `session_config`, which size and name the remote pty.

use ssh_link::{Link, SshTarget};
use term::{ChannelSession, ControlModeTap, SshChannel};

use crate::channels::BankId;
use crate::ssh::{KnownHosts, SshRequest, WireAdapter};

use super::TerminalSurface;

impl TerminalSurface {
    /// Open an SSH connection as a new bank, its first channel on the air.
    /// The trust policy is the program's own (`crate::ssh::KnownHosts`).
    ///
    /// This is where `~/.ssh/config` is asked about the destination, and
    /// the only place: every road to a connection -- the command line, the
    /// configured default, the picker's own row -- arrives here, and a
    /// file read once on the way to the wire cannot disagree with itself.
    /// What the file could not be honoured over comes back as the
    /// request's own notice and is said on the channel's glass below.
    pub fn connect_ssh(&mut self, req: &SshRequest) {
        let mut req = req.clone();
        req.consult_ssh_config();
        self.connect_ssh_with(&req, Box::new(KnownHosts::new()));
    }

    /// Another channel on an SSH bank's connection, from its own link, on
    /// `slot`.
    pub(super) fn open_ssh_channel(&mut self, bank: BankId, slot: u32) {
        let Some(link) = self
            .banks
            .get(&bank)
            .and_then(|runtime| runtime.link.as_ref())
        else {
            log::warn!("bank {bank} asked for an ssh channel with no link standing");
            return;
        };
        let size = self.viewport.term_size();
        let (pix_w, pix_h) = size.pixel_size();
        let handle = match link.open_channel((size.cols() as u16, size.rows() as u16, pix_w, pix_h))
        {
            Ok(handle) => handle,
            Err(over) => {
                log::warn!("bank {bank}: {over}");
                return;
            }
        };
        let scrollback = self.session_config.scrollback;
        let clustering = self.grapheme_clustering();
        let wire = WireAdapter::new(handle);
        self.channels.open_ssh_channel(bank, slot, move || {
            Some(ChannelSession::Ssh(SshChannel::new(
                size,
                scrollback,
                clustering,
                ControlModeTap::default(),
                Box::new(wire),
            )))
        });
    }

    /// The same, under a caller's trust policy: what a test with fixture
    /// files drives.
    pub fn connect_ssh_with(&mut self, req: &SshRequest, policy: Box<dyn ssh_link::HostPolicy>) {
        let size = self.viewport.term_size();
        // The remote pty faces the same glass the local ones do, so it
        // advertises the TERM the session config gives them.
        let term_name = self
            .session_config
            .env
            .iter()
            .find(|(key, _)| key == "TERM")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| "xterm-256color".to_string());
        let (pix_w, pix_h) = size.pixel_size();
        let target = SshTarget {
            user: req.user.clone(),
            host: req.host.clone(),
            port: req.port,
            term: term_name,
            size: (size.cols() as u16, size.rows() as u16, pix_w, pix_h),
            key_files: req.keys.clone(),
        };
        // The connection's line to the person at the glass. It is built
        // here, before the thread starts, so no question can be asked
        // before there is a desk to receive it.
        let (asker, desk) = ssh_link::ask::desk();
        // A counsel refused is the first thing said about this connection,
        // and it goes by the desk rather than the wire because it was
        // decided before there was a wire. The glass cannot tell the two
        // carriers apart, which is the point of `notice_bytes`.
        if let Some(notice) = &req.notice {
            asker.say(notice.clone());
        }
        let (link, handle) = match Link::connect(target, policy, asker) {
            Ok(pair) => pair,
            Err(e) => {
                log::error!("could not start the ssh thread for {}: {e}", req.host);
                return;
            }
        };
        let scrollback = self.session_config.scrollback;
        let clustering = self.grapheme_clustering();
        let wire = WireAdapter::new(handle);
        let bank = self
            .channels
            .open_ssh_bank(&req.user, &req.host, req.port, move || {
                Some(ChannelSession::Ssh(SshChannel::new(
                    size,
                    scrollback,
                    clustering,
                    ControlModeTap::default(),
                    Box::new(wire),
                )))
            });
        if let Some(bank) = bank {
            let runtime = self.banks.entry(bank).or_default();
            runtime.link = Some(link);
            runtime.desk = Some(desk);
        }
    }
}
