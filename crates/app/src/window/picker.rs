//! The destination picker's lifecycle: raising it on a free home slot,
//! painting its page, reading its keys and taking it down again.
//!
//! The page itself is [`crate::picker`], which is pure state over keystrokes
//! and a painter that turns it into bytes. What is here is everything that
//! needs the window: a channel to paint into, a connection to dial, and the
//! config file to write a chosen default to.
//!
//! Fields touched: `picker`, the page while it stands; `channels`, whose
//! free home slot carries it; `viewport` and `session_config`, which size
//! and configure that carrier; and `settings`, the only handle in the
//! program that writes `ssh.default`.

use std::time::Instant;
use config::toml::Scalar;
use term::{ChannelSession, TmuxPane};

use crate::channels::Close;
use crate::ssh::SshRequest;

use super::{spawn, TerminalSurface};

impl TerminalSurface {
    /// Put a shed write queue on the glass.
    ///
    /// Both write queues are capped (`term::session::INPUT_CAP`,
    /// `crate::tmux::PENDING_CAP`), and what a cap does when it bites is throw
    /// away what the user just typed, until now traced only by a `log::warn!`
    /// nobody is watching. So the counters are read once per pump and a rise
    /// raises the badge, two words wide because the plate is twice the text.
    /// The two wires get two words because the remedy differs: an unread tty is
    /// a shell this program spawned, a full gateway queue is the tmux server.
    /// Both at once is one badge -- the pty one, the wire nearer the user's
    /// hand -- because they are one event, "your typing is being thrown away",
    /// and the log line beside it carries the byte counts.
    /// Raise the destination picker on a free home slot: a bare screen
    /// painted with the configured servers, taking the air. Idempotent
    /// while one stands: the chord again only brings it back on air.
    pub fn open_picker(&mut self) {
        if let Some(slot) = self.picker.as_ref().map(|picker| picker.slot) {
            if self.channels.slot_title(0, slot).is_some() {
                self.channels.select_channel(0, slot);
                self.channel_changed();
                return;
            }
            self.picker = None;
        }
        let size = self.viewport.term_size();
        let scrollback = self.session_config.scrollback;
        let clustering = self.grapheme_clustering();
        let slot = self.channels.first_free(0);
        let opened = self.channels.open_channel(0, slot, || {
            Some(ChannelSession::TmuxPane(TmuxPane::new(
                size, scrollback, clustering,
            )))
        });
        if !opened {
            log::warn!("no free home slot for the destination picker");
            self.notice.raise("no free slot for the picker", Instant::now());
            return;
        }
        self.picker = Some(crate::picker::Picker::new(slot));
        self.paint_picker();
        self.channel_changed();
    }

    /// Repaint the picker's page from the live config and the page's own
    /// state, whole: raise, resize, and every key it took, because a
    /// keystroke moves more of the page than the column it landed in.
    pub(super) fn paint_picker(&mut self) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let slot = picker.slot;
        let bytes = crate::picker::paint(&self.live_config().ssh.hosts, picker);
        let size = self.viewport.term_size();
        if let Some(row) = self
            .channels
            .rows_mut()
            .find(|r| r.bank == 0 && r.channel == slot)
        {
            // The page rides a pane's screen, but no tmux sizes it: the
            // window does, at the glass's size.
            if let Some(screen) = row.session.tmux_pane_mut() {
                screen.resize(size);
                screen.feed(&bytes);
            }
        }
    }

    /// The picker's keyboard: only while its page is the channel on the
    /// air. Answers whether the key was the picker's.
    ///
    /// Every verdict that opens something writes the default first, when the
    /// checkbox is ticked, and dials after. The write is the user's standing
    /// answer to "where do sessions start"; the connection is one session.
    /// A dial that fails, or a window that is switched off a second later,
    /// must not be the reason the answer was lost, so the answer goes to the
    /// file while there is nothing left that can go wrong with it.
    pub(super) fn picker_key(&mut self, logical: &winit::keyboard::Key) -> bool {
        // Out of the field for the duration: the verdicts below reach for
        // `&mut self`, and the page is put back only if it still stands.
        let Some(mut picker) = self.picker.take() else {
            return false;
        };
        let slot = picker.slot;
        if self.channels.on_air() != (0, slot) {
            // The page is standing but off the air: the keyboard is the
            // visible channel's. A dead picker row (closed by hand) is
            // forgotten here, the one place that would otherwise trust it.
            if self.channels.slot_title(0, slot).is_some() {
                self.picker = Some(picker);
            }
            return false;
        }
        let hosts = self.live_config().ssh.hosts;
        let verdict = picker.key(logical, &hosts);
        let make_default = picker.make_default();
        match verdict {
            crate::picker::Verdict::Localhost => {
                if make_default {
                    // Localhost's spelling in the file is an empty default,
                    // and it names no row.
                    self.set_default_connection("");
                }
                // The replacement stands before the page goes, so closing
                // the page is never closing the last channel.
                let (config, size) = (self.session_now(), self.viewport.term_size());
                let mut refused = false;
                self.channels.open_first_free(|| {
                    spawn(&config, size).or_else(|| {
                        refused = true;
                        None
                    })
                });
                if refused {
                    self.notice.raise(super::NO_SHELL, Instant::now());
                }
                self.retire_picker(slot);
                true
            }
            crate::picker::Verdict::Host(index) => {
                let Some(row) = hosts.get(index) else {
                    self.picker = Some(picker);
                    return true;
                };
                let user = if row.user.is_empty() {
                    crate::ssh::invoking_user()
                } else {
                    Some(row.user.clone())
                };
                let Some(user) = user else {
                    log::warn!(
                        "[[ssh.host]] {:?} names no user and {} is unset",
                        row.host,
                        crate::ssh::USER_VAR
                    );
                    self.notice.raise(format!("no user for {}", row.host), Instant::now());
                    self.picker = Some(picker);
                    return true;
                };
                if make_default {
                    // The row is already in the file; the default is the
                    // only thing that has to move.
                    self.set_default_connection(&row.host);
                }
                let req = SshRequest {
                    user,
                    host: row.host.clone(),
                    port: row.port,
                    keys: crate::ssh::key_path(&row.key),
                    // What the row left blank is what `~/.ssh/config` is
                    // allowed to fill, the reading `crate::ssh` states.
                    unsaid: crate::ssh::Unsaid {
                        user: row.user.is_empty(),
                        port: row.port == 22,
                    },
                    notice: None,
                };
                self.connect_ssh(&req);
                self.retire_picker(slot);
                true
            }
            crate::picker::Verdict::Cancel => {
                self.retire_picker(slot);
                true
            }
            crate::picker::Verdict::Ignored => {
                // The page stands and has very likely changed: a character
                // typed, the box ticked, an error raised or cleared.
                self.picker = Some(picker);
                self.paint_picker();
                true
            }
        }
    }

    /// Make a destination the file already names the default connection, an
    /// empty `host` being localhost's own spelling for it.
    ///
    /// The picker's checkbox is the only thing that calls this, and the only
    /// thing in the program that sets `ssh.default`: one surface, so there is
    /// one answer to where sessions start and one place it was given.
    fn set_default_connection(&mut self, host: &str) {
        let Some(settings) = self.settings.as_ref() else {
            // No handle: `--default-settings`, or a headless surface that
            // never attached one. The connection is dialled and nothing is
            // persisted, which is what "never touch the user's real config"
            // means. The seam drag's shape, for the seam drag's reason.
            log::debug!("the picker chose {host:?} as the default with no config to write");
            return;
        };
        if let Err(e) = settings.write_key("ssh.default", Scalar::String(host.to_string())) {
            log::error!("could not write ssh.default = {host:?}: {e}");
            self.notice.raise("could not save the default", Instant::now());
        }
    }

    /// Take the picker's page down. Its `Close` verdict is honoured like
    /// any channel's, so an Esc on the last channel anywhere switches the
    /// appliance off, the law `close_channel` already applies.
    fn retire_picker(&mut self, slot: u32) {
        self.picker = None;
        if self.channels.close_channel(0, slot) == Close::CloseWindow {
            self.eof = true;
        }
        self.channel_changed();
    }
}
