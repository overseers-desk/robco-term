//! What comes back: reply blocks and notifications, typed.
//!
//! The whole vocabulary of `man tmux`'s CONTROL MODE section is here, not only
//! the notifications this appliance acts on, because a name the codec does not
//! know is a name the host cannot see. Everything unrecognised still arrives,
//! as [`Notification::Unknown`], with its bytes intact.

use crate::codec::CommandId;
use crate::ids::{PaneId, SessionId, WindowId};

/// One thing the codec finished reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A block that answers a command this codec sent.
    Reply { id: CommandId, block: Block },

    /// A block that answers nobody.
    ///
    /// tmux emits these during the attach burst: the output of the commands
    /// that built the session, any number of them, before this client has
    /// asked anything. They must not consume a pending command, or every
    /// later reply is one behind. See [`Block::solicited`] for how they are
    /// told apart.
    Unsolicited { block: Block },

    /// A `%begin` closed by an `%end`/`%error` carrying a different number.
    /// The body is dropped: it cannot be attributed.
    GuardMismatch {
        began: Option<u64>,
        closed: Option<u64>,
    },

    /// A notification, outside any block.
    Notification(Notification),

    /// A line that is neither inside a block nor starts with `%`.
    ///
    /// tmux never sends one. A plausible cause is the protocol having been
    /// lost: the client process was killed without closing its device
    /// string, and the shell behind it is talking now. Deciding that is the
    /// host's; the codec only reports the line.
    Foreign(Vec<u8>),
}

/// A `%begin` ... `%end`/`%error` block, whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The command number in the guard lines. tmux's own counter, server-wide:
    /// it neither starts at one nor advances by one between a client's
    /// commands (276, 281, 284 in a recorded attach), so it identifies a block
    /// but does not order one client's blocks. Ordering is the queue's job.
    pub number: Option<u64>,
    /// Seconds since the epoch, tmux's clock.
    pub timestamp: Option<i64>,
    /// The guard's third field.
    pub flags: Option<u64>,
    /// Closed by `%error` rather than `%end`.
    pub error: bool,
    /// The lines between the guards, raw. A reply body is not escaped the way
    /// a `%output` payload is: `capture-pane -e` returns escape sequences as
    /// themselves, so a body line is bytes.
    pub body: Vec<Vec<u8>>,
}

impl Block {
    /// Whether tmux marked this block as the answer to a command the client
    /// sent.
    ///
    /// **Observed, not documented.** `man tmux` says of the guards' third
    /// field only "flags (currently not used)". tmux 3.5a uses it: every block
    /// of the attach burst carries `0` and every block answering a command
    /// from this client's stdin carries `1`
    /// (`tests/transcripts/01-fresh-session.txt` and every other recording).
    /// The codec uses it as the gate on its pairing queue, which is how it
    /// needs no bootstrap delay: without this flag, a client cannot tell the
    /// attach burst from a real reply and has no choice but to wait 300 ms or
    /// for `%session-changed` before daring to send its first command.
    pub fn solicited(&self) -> bool {
        self.flags.is_some_and(|flags| flags & 1 != 0)
    }

    /// The body as text, lossily. For the bodies that are text (a host name,
    /// a pane listing, a cursor position), which is all of them except a
    /// capture.
    pub fn text(&self) -> Vec<String> {
        self.body
            .iter()
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect()
    }
}

/// A control-mode notification.
///
/// Text fields are decoded lossily, so a name carrying invalid UTF-8 arrives
/// as replacement characters rather than as an error. Payload fields, where
/// the bytes are the point, stay `Vec<u8>`.
///
/// Two escapings live on this wire and they are not each other. A `%output`
/// payload is octal-escaped over the C0 set and the backslash; a **window or
/// session name** is C-escaped by `vis(3)`, so its backslash arrives doubled
/// and its control bytes as `\t`, `\a` and the rest. Names go through
/// [`crate::escape::unvis`]; buffer names and `%message` text were measured
/// not to be encoded at all and are left alone. Each variant says which it
/// gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notification {
    /// `%output pane-id value`: a pane wrote something. `data` is already
    /// through [`crate::escape::unescape_octal`].
    Output { pane: PaneId, data: Vec<u8> },

    /// `%extended-output pane-id age ... : value`: the form `%output` takes
    /// once the `pause-after` client flag is set (`refresh-client -f
    /// pause-after=N`). `age` is the milliseconds tmux held the output.
    /// `extra` is the arguments between the age and the `:`, which the manual
    /// says are for future use and should be ignored; they are kept rather
    /// than dropped so that "ignored" stays the host's decision.
    ExtendedOutput {
        pane: PaneId,
        age: u64,
        extra: Vec<String>,
        data: Vec<u8>,
    },

    /// `%window-add window-id`: a window was linked to this session. It does
    /// not name the window's pane, which is why a caller has to follow it
    /// with a fresh `list-panes`.
    WindowAdd { window: WindowId },

    /// `%window-close window-id`.
    ///
    /// **Never observed from tmux 3.5a.** Killing a window produced
    /// `%unlinked-window-close` in every recording, for the current window and
    /// for a background one alike (`tests/transcripts/05-kill-window.txt`):
    /// by the time the notification is written the window is already off the
    /// session's list, so tmux calls it unlinked. Treating the two names as
    /// one event is what a client working against tmux 3.5a has to do. Kept
    /// as its own variant because the manual defines it and a future tmux may
    /// mean it.
    WindowClose { window: WindowId },

    /// `%window-renamed window-id name`. The name is the rest of the line,
    /// interior spacing included, decoded through
    /// [`crate::escape::unvis`]: names are C-escaped, unlike a `%output`
    /// payload and unlike anything the manual mentions.
    WindowRenamed { window: WindowId, name: String },

    /// `%window-pane-changed window-id pane-id`: the window's active pane
    /// moved, and tmux names the new one outright.
    WindowPaneChanged { window: WindowId, pane: PaneId },

    /// `%unlinked-window-add window-id`: a window created in some other
    /// session.
    UnlinkedWindowAdd { window: WindowId },

    /// `%unlinked-window-close window-id`. See [`Notification::WindowClose`]:
    /// in practice this is how every window closure arrives.
    UnlinkedWindowClose { window: WindowId },

    /// `%unlinked-window-renamed window-id name`.
    UnlinkedWindowRenamed { window: WindowId, name: String },

    /// `%layout-change window-id layout visible-layout flags`; flags is empty for a window that is not current.
    LayoutChange {
        window: WindowId,
        layout: String,
        visible_layout: String,
        flags: String,
    },

    /// `%session-changed session-id name`: this client is now on that
    /// session. It is also the end of the attach burst.
    SessionChanged { session: SessionId, name: String },

    /// `%session-renamed session-id name`.
    ///
    /// The manual gives this one argument, `name`. tmux 3.5a sends two, the
    /// session id first. Observed bytes win, so the id is here.
    SessionRenamed { session: SessionId, name: String },

    /// `%session-window-changed session-id window-id`.
    SessionWindowChanged {
        session: SessionId,
        window: WindowId,
    },

    /// `%sessions-changed`: a session was created or destroyed.
    SessionsChanged,

    /// `%client-detached client`.
    ClientDetached { client: String },

    /// `%client-session-changed client session-id name`.
    ClientSessionChanged {
        client: String,
        session: SessionId,
        name: String,
    },

    /// `%config-error error`.
    ConfigError { message: String },

    /// `%message message`: sent by `display-message`.
    ///
    /// Observed **inside** a reply block, which the manual says cannot happen
    /// ("A notification will never occur inside an output block"); see
    /// [`Block`]'s note. Reaching the host as a notification therefore means
    /// it arrived outside one. Raw, not `vis`-encoded: measured.
    Message { message: String },

    /// `%pause pane-id`: output paused under the `pause-after` flag.
    Pause { pane: PaneId },

    /// `%continue pane-id`: resumed.
    Continue { pane: PaneId },

    /// `%pane-mode-changed pane-id`.
    PaneModeChanged { pane: PaneId },

    /// `%paste-buffer-changed name`. Raw, not `vis`-encoded: measured, a
    /// buffer named `buf\back` arrives with one backslash.
    PasteBufferChanged { name: String },

    /// `%paste-buffer-deleted name`. Raw, as above.
    PasteBufferDeleted { name: String },

    /// `%subscription-changed name session-id window-id window-index pane-id
    /// ... : value`: a format subscribed to with `refresh-client -B` moved.
    SubscriptionChanged {
        name: String,
        session: SessionId,
        window: WindowId,
        window_index: String,
        pane: PaneId,
        extra: Vec<String>,
        value: Vec<u8>,
    },

    /// `%exit [reason]`: the client is going away.
    Exit { reason: Option<String> },

    /// A `%name` the codec has no variant for. Nothing is dropped.
    Unknown { name: String, rest: Vec<u8> },

    /// A `%name` the codec knows, whose arguments did not fit: too few
    /// fields, or an id under the wrong sigil. Distinct from
    /// [`Notification::Unknown`] because it means the dialect moved, not that
    /// it grew.
    Malformed { name: String, rest: Vec<u8> },
}
