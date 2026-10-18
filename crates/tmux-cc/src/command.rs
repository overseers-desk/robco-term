//! The commands a control client sends, and the quoting they need.
//!
//! # The set
//!
//! One variant per command, and no way to say anything else: a control client
//! is not a shell, and the closed set is what lets every reply be paired with
//! a known question.
//!
//! `resize-window` is deliberately absent: it sizes a window for every client,
//! while `refresh-client -C` speaks for this client alone, for its whole
//! screen or for one window (the client-size law in `app::channels`).
//!
//! # Quoting, measured against tmux 3.5a's own lexer
//!
//! A command line written to a control client's stdin is lexed by tmux, not by
//! a shell, and the rules were read off the server rather than the manual
//! (`tests/transcripts/09-quoting.txt`):
//!
//! * `#` **starts a comment**, anywhere on the line. `display-message -p
//!   #{host_short}` is not an error: the argument silently vanishes and tmux
//!   prints its default status line instead. This is why every format string
//!   is quoted, and why an unquoted one fails so quietly.
//! * Inside double quotes, `\"` is a quote, `\\` is a backslash, and
//!   `${VAR}` **is expanded** from tmux's environment. `#{...}` is untouched by
//!   the lexer and expanded later by the command itself.
//! * Single quotes suppress `$` expansion and backslash escapes, but **not**
//!   `#{...}`: format expansion happens in the command, after lexing.
//!   `'#{host_short}'` still expands.
//! * `;` inside quotes is literal; bare, it separates commands.
//!
//! So [`quote_format`] protects the quote, the backslash and the dollar and
//! leaves `#` alone, which is the whole point of quoting a format. Free user
//! text needs no quoting at all: the one command that carries it, `send-keys`,
//! carries it as hex ([`escape::hex_arguments`](crate::escape::hex_arguments)).

use crate::escape::hex_arguments;
use crate::ids::{PaneId, WindowId};

/// One command per chunk keeps a large paste from building one huge line.
pub const SEND_KEYS_CHUNK: usize = 256;

/// One line of the control protocol, going out.
///
/// Every variant is exactly one wire line and therefore exactly one reply
/// block. That one-to-one is what the codec's pairing queue rests on, so a
/// command that would need two lines (a paste larger than
/// [`SEND_KEYS_CHUNK`]) is built as several variants by
/// [`Command::send_keys`] rather than as one variant that expands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// The server in one reply line: its socket, its pid, the session this
    /// client is attached to, and the host name the appliance titles the
    /// gateway with.
    Server,

    /// Every session on the server, one line each: id and name.
    ListSessions,

    /// The pane listing shared by the bootstrap and the `%window-add`
    /// re-query: one line per pane, across every window of the session, with
    /// the size tmux gave the pane.
    ListPanes,

    /// The window listing behind the rename sweep.
    ListWindows,

    /// One window's panes, which is active and how big each is: the
    /// `%layout-change` re-query.
    ListWindowPanes(WindowId),

    /// A pane's screen and up to `history` lines of scrollback above it.
    /// `-p` to stdout, `-e` with escape sequences, `-q` quiet, `-J` joining
    /// wrapped lines.
    CapturePane { pane: PaneId, history: usize },

    /// Where that pane's cursor stands, and how tall the pane is. The height
    /// is asked for because a capture ends on the pane's bottom row, so the
    /// cursor is walked up from there rather than addressed absolutely.
    CursorPosition(PaneId),

    /// Keystrokes into a pane as hex bytes. Build with
    /// [`Command::send_keys`], which chunks.
    SendKeys { pane: PaneId, bytes: Vec<u8> },

    /// Another window in this session. The window itself arrives later, as
    /// `%window-add`.
    NewWindow,

    /// Close a window. Its channel goes when the close notification lands,
    /// not here.
    KillWindow(WindowId),

    /// Ask tmux to let this client go. Teardown follows on `%exit`.
    DetachClient,

    /// The client-size law: tell tmux how big this client's screen is. tmux
    /// sizes the windows this client creates from it.
    ClientSize { columns: u16, rows: u16 },

    /// This client's size for one window. tmux caps the window at it,
    /// whichever client its `window-size` rule follows.
    WindowSize {
        window: WindowId,
        columns: u16,
        rows: u16,
    },
}

impl Command {
    /// Split a keystroke or paste into as many `send-keys` lines as it takes.
    ///
    /// Empty input is no commands, not one empty command: `send-keys -H -t %1`
    /// with no bytes is a valid tmux command that does nothing, and a reply
    /// block for it would be a pending entry bought for nothing.
    pub fn send_keys(pane: &PaneId, bytes: &[u8]) -> Vec<Command> {
        bytes
            .chunks(SEND_KEYS_CHUNK)
            .map(|chunk| Command::SendKeys {
                pane: pane.clone(),
                bytes: chunk.to_vec(),
            })
            .collect()
    }

    /// The command as tmux reads it, without its terminating newline.
    pub fn to_wire(&self) -> String {
        match self {
            Command::Server => format!(
                "display-message -p {}",
                quote_format("#{socket_path} #{pid} #{session_id} #{host_short}")
            ),
            Command::ListSessions => format!(
                "list-sessions -F {}",
                quote_format("#{session_id} #{session_name}")
            ),
            Command::ListPanes => format!(
                "list-panes -s -F {}",
                quote_format(
                    "#{window_id} #{pane_id} #{pane_active} #{pane_width} #{pane_height} #{window_name}"
                )
            ),
            Command::ListWindows => format!(
                "list-windows -F {}",
                quote_format("#{window_id} #{window_name}")
            ),
            Command::ListWindowPanes(window) => format!(
                "list-panes -t {} -F {}",
                window,
                quote_format("#{pane_id} #{pane_active} #{pane_width} #{pane_height}")
            ),
            Command::CapturePane { pane, history } => {
                // `-S -N` counts backwards from the top of the screen, so the
                // sign is part of the number and not a flag.
                format!("capture-pane -peqJ -t {pane} -S -{history}")
            }
            Command::CursorPosition(pane) => format!(
                "display-message -p -t {} {}",
                pane,
                quote_format("#{cursor_x} #{cursor_y} #{pane_height}")
            ),
            Command::SendKeys { pane, bytes } => {
                format!("send-keys -H -t {}{}", pane, hex_arguments(bytes))
            }
            Command::NewWindow => "new-window".to_string(),
            Command::KillWindow(window) => format!("kill-window -t {window}"),
            Command::DetachClient => "detach-client".to_string(),
            Command::ClientSize { columns, rows } => {
                format!("refresh-client -C {columns}x{rows}")
            }
            Command::WindowSize {
                window,
                columns,
                rows,
            } => format!("refresh-client -C {window}:{columns}x{rows}"),
        }
    }
}

/// Wrap a tmux format string so the lexer hands it over whole.
///
/// Double quotes, because a format has to keep its `#{...}` live and double
/// quotes are what tmux's lexer passes `#` through unread. The quote, the
/// backslash and the dollar are escaped, in that order of concern: the first
/// two would end or corrupt the argument, and the third would be substituted
/// from the server's environment (`${VAR}` expands inside double quotes --
/// measured, not documented).
pub fn quote_format(format: &str) -> String {
    let mut out = String::with_capacity(format.len() + 2);
    out.push('"');
    for ch in format.chars() {
        if matches!(ch, '"' | '\\' | '$') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(n: &str) -> PaneId {
        PaneId::parse(n).unwrap()
    }

    #[test]
    fn the_bootstrap_pair_is_the_references_own_text() {
        assert_eq!(
            Command::Server.to_wire(),
            r##"display-message -p "#{socket_path} #{pid} #{session_id} #{host_short}""##
        );
        assert_eq!(
            Command::ListPanes.to_wire(),
            r##"list-panes -s -F "#{window_id} #{pane_id} #{pane_active} #{pane_width} #{pane_height} #{window_name}""##
        );
        assert_eq!(
            Command::ListSessions.to_wire(),
            r##"list-sessions -F "#{session_id} #{session_name}""##
        );
    }

    #[test]
    fn the_rename_sweep_and_the_layout_requery() {
        assert_eq!(
            Command::ListWindows.to_wire(),
            r##"list-windows -F "#{window_id} #{window_name}""##
        );
        assert_eq!(
            Command::ListWindowPanes(WindowId::parse("@2").unwrap()).to_wire(),
            r##"list-panes -t @2 -F "#{pane_id} #{pane_active} #{pane_width} #{pane_height}""##
        );
    }

    #[test]
    fn the_attach_pair_carries_the_references_flags_and_the_callers_depth() {
        assert_eq!(
            Command::CapturePane {
                pane: pane("%5"),
                history: 10_000
            }
            .to_wire(),
            "capture-pane -peqJ -t %5 -S -10000"
        );
        assert_eq!(
            Command::CursorPosition(pane("%5")).to_wire(),
            r##"display-message -p -t %5 "#{cursor_x} #{cursor_y} #{pane_height}""##
        );
    }

    #[test]
    fn the_short_commands() {
        assert_eq!(Command::NewWindow.to_wire(), "new-window");
        assert_eq!(Command::DetachClient.to_wire(), "detach-client");
        assert_eq!(
            Command::KillWindow(WindowId::parse("@7").unwrap()).to_wire(),
            "kill-window -t @7"
        );
        assert_eq!(
            Command::ClientSize {
                columns: 100,
                rows: 30
            }
            .to_wire(),
            "refresh-client -C 100x30"
        );
        assert_eq!(
            Command::WindowSize {
                window: WindowId::parse("@3").unwrap(),
                columns: 100,
                rows: 30
            }
            .to_wire(),
            "refresh-client -C @3:100x30"
        );
    }

    #[test]
    fn send_keys_is_hex_and_needs_no_quoting() {
        let cmds = Command::send_keys(&pane("%1"), b"a\"\\#\n");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].to_wire(), "send-keys -H -t %1 61 22 5c 23 0a");
    }

    #[test]
    fn a_paste_is_chunked_into_whole_commands() {
        let paste = vec![b'x'; SEND_KEYS_CHUNK * 2 + 1];
        let cmds = Command::send_keys(&pane("%1"), &paste);
        assert_eq!(cmds.len(), 3);
        // Every line is one command, so every line is one reply block.
        let bytes: usize = cmds
            .iter()
            .map(|c| match c {
                Command::SendKeys { bytes, .. } => bytes.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(bytes, paste.len());
        assert_eq!(cmds[2].to_wire(), "send-keys -H -t %1 78");
    }

    #[test]
    fn nothing_to_send_is_no_command_at_all() {
        assert!(Command::send_keys(&pane("%1"), b"").is_empty());
    }

    #[test]
    fn quoting_protects_the_three_that_bite_and_spares_the_hash() {
        assert_eq!(quote_format("#{a} #{b}"), r##""#{a} #{b}""##);
        assert_eq!(quote_format(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_format(r"a\b"), r#""a\\b""#);
        assert_eq!(quote_format("a${V}b"), r#""a\${V}b""#);
    }
}
