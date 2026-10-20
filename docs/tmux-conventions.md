# tmux conventions

robco-term's `tmux -CC` client is compared here with the two other clients of tmux control mode: iTerm2, where the integration began, and WezTerm. A behaviour both of them share is treated as a convention. For each convention this page says whether robco-term follows it, departs from it on purpose, or departs from it with no reason recorded anywhere in the project. Behaviours only iTerm2 has come after.

The peers were read at iTerm2 commit `ec4517e6` (2026-10-17) and WezTerm commit `e2a98e1b` (2026-10-10), and their wire behaviour was checked against tmux 3.5a. Paths are relative to each project's root. Statements about older tmux releases come from tmux's source at the release tag.

| Convention | robco-term |
|---|---|
| Write nothing to tmux until `%session-changed` | Follows |
| Capture as much history at attach as a scrollback holds | Follows |
| Learn the server's dialect before sending commands that depend on it | Departs on purpose |
| Show every pane of a window | Departs on purpose |
| Say in the carrier pane how to leave control mode | Departs on purpose |

## Write nothing until `%session-changed`

tmux enters control mode before it knows whether there is a session to attach. With no server running, `tmux -CC attach` sends the opening `ESC P1000p`, an error block reading `no sessions`, and `%exit`, and is gone within milliseconds. Anything a client writes after the opening then reaches the shell that ran tmux, which runs each line as a command.

- iTerm2 enables writes in its `%session-changed` handler (`sources/tmux/TmuxGateway.m`): "Don't write until we get session-changed. This way we know it's not a one-and-done command (e.g., tmux -CC list-windows)." The start of control mode in `sources/PTYSession/PTYSession.m` records the cause, iTerm2 issue 4429, "where we used to send a command immediately and tmux would terminate immediately and we would spam the user's command line."
- WezTerm starts in `WaitForInitialGuard` and queues its first command on `SessionChanged` (`mux/src/tmux.rs`).
- robco-term holds its bootstrap and the client size until `%session-changed` arrives (`Gateway` in `crates/app/src/tmux.rs`). A run that ends without one, such as a failed attach or `tmux -CC ls`, receives nothing from it.

## Capture as much history as a scrollback holds

When a window is attached, its pane's earlier output is fetched with `capture-pane`.

- iTerm2 asks for the larger of the window's height and the profile's scrollback length (`maxHistory` in `sources/tmux/TmuxController.m`).
- WezTerm asks for tmux's own `#{history_limit}` for the window (`ListAllWindows` and `CapturePane` in `mux/src/tmux_commands.rs`).
- robco-term asks for as many lines as the channel's own scrollback holds (`Gateway::attach_window` in `crates/app/src/tmux.rs`). tmux returns no more than it holds, so the deep request costs nothing where a pane's history is short.

## Learn the server's dialect first

tmux's command syntax has changed across releases. The commands robco-term sends are the ones tmux 3.3 and later accept.

- iTerm2 asks for `#{version}` and gates individual features on it: `send-keys -H` from tmux 3.0a (`serverSupportsLiteralByteInjection` in `sources/tmux/TmuxGateway.m`) and per-window sizes (`canPinWindowSizes` in `sources/tmux/TmuxController.m`), among others.
- WezTerm sends `list-commands` first and picks the `refresh-client -C` syntax from the usage text it returns (`ListCommands` and `Resize` in `mux/src/tmux_commands.rs`). It uses what it learns for sizing only, and sends `send-keys -H` to every server.
- robco-term asks neither and checks no version: it supports tmux 3.3 and later. Older servers are out of scope because tmux accepts `refresh-client -C @window:WxH` from 3.3, so on 3.2 and older every per-window size robco-term sends fails and is logged as a failed command. `send-keys -H` exists from 3.0a, so on older servers nothing typed in robco-term reaches a pane, as in WezTerm. Hyperlinks in a tmux window need tmux 3.4, the first release with OSC 8 support.

## Show every pane of a window

- iTerm2 draws a window's panes as native split panes, built from tmux's layout (`sources/tmux/TmuxWindowOpener.m`, `sources/tmux/TmuxLayoutParser.m`).
- WezTerm builds split panes from each pane's position and size (`ListAllPanes` in `mux/src/tmux_commands.rs`).
- robco-term has no split view anywhere. By design, a window gets one channel, which shows one pane at a time, the window's active pane (`Intent::ListPanes` and `Intent::LayoutPanes` in `crates/app/src/tmux.rs`). `Ctrl`+`Shift`+`O` steps the window to its next pane with `select-pane -t @N.+`, and the channel follows. tmux's own prefix chords do nothing there, because typed keys reach the pane through `send-keys`, which bypasses tmux's key bindings: on tmux 3.5a, `send-keys -H` of `C-b o` leaves the active pane where it was.

## Say how to leave control mode

- iTerm2 prints `** tmux mode started **` and a command menu into the session that carries control mode, where `Esc` detaches (`sources/PTYSession/PTYSession.m`).
- WezTerm prints `[This pane is running tmux control mode. Press q to detach]` into the pane that carries it, where `q` detaches (`mux/src/localpane.rs`).
- robco-term writes nothing of its own into the carrier channel. `INVARIANTS.md` rules it out: "Nothing the appliance draws of its own accord may enter the session: it takes no keystroke, writes no cell the terminal owns". The attachment's bank has a title row instead, and pressing it raises a card reading `detach: Alt+Shift+D` (`Cmd` for `Alt` on macOS); `Alt+Shift+D` then detaches. The three clients use three different keys, so which key detaches is not a convention.

## Only iTerm2

These behaviours are iTerm2's alone; WezTerm, like robco-term, has none of them.

**A pane's terminal state is restored at attach.** For every pane, iTerm2 fetches the alternate screen as well as the normal one, the pane's modes (cursor keys, keypad, mouse reporting, insert, wrap), its scroll region, tab stops and cursor visibility, and the output tmux holds but has not yet parsed (`appendRequestsForWindowPane` in `sources/tmux/TmuxWindowOpener.m`, the field list in `sources/tmux/TmuxStateParser.m`). robco-term restores the screen and the cursor position. A program that was full-screen when the window was attached, such as an editor or a pager, therefore leaves its screen behind when it exits, and a program that turned on mouse reporting before the attach receives no clicks until it turns it on again. These two consequences follow from the code and have not been observed.

**tmux is held open after `%exit`.** iTerm2 sets tmux's `wait-exit` client flag, and after `%exit` writes the empty line that releases it (`ping` in `sources/tmux/TmuxController.m`, the `%exit` branch in `sources/tmux/TmuxGateway.m`, both for tmux 3.2 and later). On tmux 3.5a, a client that has set the flag gets no closing `ESC \` after `%exit`, and every line it writes is absorbed until an empty one ends the client. robco-term does not set the flag. A line it writes between tmux's exit and its own reading of `%exit`, such as a keystroke or a resize, reaches the shell, which runs it as a command. WezTerm does not set the flag either; it writes two newlines after every `%exit`, which with no flag set reach the shell as two empty command lines.

**tmux's reason for leaving is shown.** iTerm2 prints `tmux failed with error: “…”` when the attach itself fails (`sources/PTYSession/PTYSession.m`), and `tmux exited unexpectedly.` with the line when `%exit` carries a reason (`sources/tmux/TmuxGateway.m`). robco-term shows neither on screen: a failed attach leaves the shell's prompt and nothing else. It logs both at warn: the message a failed attach sends, such as `no sessions`, and the reason `%exit` carries, such as `server exited unexpectedly`.

**Panes that fall behind are paused.** With its pause mode enabled in settings, iTerm2 sets tmux's `pause-after` flag and resumes paused panes itself (`enablePauseModeIfPossible` in `sources/tmux/TmuxController.m`).

## Where all three agree

- Replies are paired with commands by the order the commands were sent; robco-term also checks the `%begin` flags field, which marks a reply to this client's own command, and tmux has set that field since 2.9 at least.
- Sizes go to tmux as `refresh-client -C`.
- Typing at the pane that carries control mode reaches nothing.
- `%config-error` and `%message` stay off the screen: iTerm2 logs them as unrecognised lines, WezTerm logs configuration errors, robco-term logs both as warnings.
