# The settings window

Right-clicking the glass opens `robco-settings`, a separate program shipped
in every official package beside `robco-term`. On macOS the same window is
also under the application menu, at **Settings…** or `Cmd`+`,`, where a Mac
looks for it. It is the graphical face of
[the config file](config.md): a tab per table -- General, Screen, Chassis,
SSH, Critters, Serial -- a preset picker at the head of each look axis,
sliders and pickers for every key the build reads, a marker on each value the
file pins away from its preset, and a per-row reset that unpins one.

The **SSH** tab is a list rather than a form, `[[ssh.host]]` being a table
per server. The checked radio is where a new session starts: localhost at
the top, then one row per server with its host, user, port and key beside
it, `+` to add a row and `✕` to take one away. Checking a row writes its
host into `ssh.default`; renaming the checked row rewrites `ssh.default` in
the same edit, because that key names a row by its host string and would
otherwise be left pointing at a name nothing answers to. A field left at
its default (an empty user, port 22) is not written out, the same
diff-against-defaults rule the other tabs follow, while the host is written
whatever it holds, being what the row is identified by. The terminal reads
the table when it launches, so a change here reaches the next session
started rather than the ones already running.

The **Critters** tab is where the appliance's own company is settled:
whether anything crosses the glass, when, and which of the eight pieces are
still in the cast. On the shipped timing a critter crosses on each quarter
hour, so catching one tells you the time; the other setting keeps the same
interval and puts the arrival at an unpredictable point inside it.
[`[critters]`](config.md#critters) names each piece.

The **Serial** tab puts the terminal on a slow line: a switch, and the rate
beside it as the standard baud figures a wire ran at. It is the reading that
slows and not the drawing, so what the tab changes is how fast a local shell's
output is taken, and a program blocks when the buffer between them fills, as
it would have. A shell reached over SSH, and a tmux window under `tmux -CC`,
are not slowed; the tab says so in a note under its rows, that being where
somebody turning the switch on is looking, and [`[serial]`](config.md#serial)
gives the reason. The rate is a picker over an ordinary integer key, so a
figure the list does not offer can still be written by hand and the window
will show it.

The **Font size** row sits on the Screen tab's Type group, directly under
Font, as a number typed in pixels. It writes a `[general]` key, so the preset
dialog leaves it alone. Its readout says what the chosen face draws, `24 px
(12 × 2)` for Terminess, and a note under the group states the rule.

## How it behaves

Every change is applied the moment it is made. There is no Apply button
because there is nothing to apply: each change is one minimal edit to
`config.toml`, written under the contract in
[`config-format.md`](config-format.md), and the running terminal's own file
watch carries it to the glass. The terminal is the preview.

**OK** closes the window and keeps what you see. **Cancel** writes back the
exact bytes the file held when the window opened (or removes the file it
created), and the glass follows. An edit made by another hand while the
window is open, your editor included, is picked up when the window regains
focus.

Switching presets while some keys are pinned raises the one dialog the app
has: **Switch** writes the new `name` alone and drops the table's pinned
keys, so the whole preset shows; **Keep my look** writes the `name` and pins
whatever the new base would move, so nothing visible changes. This is the
switch-or-rename decision `config-format.md` requires a preset picker to
make deliberately, put to the person who owns the answer.

Comments, formatting and keys the app does not know stay byte-identical
through every edit; the tests in `settings/tests/` hold it to that.

A failure the window cannot continue past is written to a log as well as
shown and printed, because the shipped Windows image has no console to
print to and a message box needs a Tk that may be the thing that failed.
The log is `$XDG_STATE_HOME/robco-term/settings.log`, or
`%LOCALAPPDATA%\robco-term\settings.log` on Windows, or the same pair of
names under the temporary directory where neither variable is set; a window
embedded in the terminal writes to the log the terminal names instead, so
the two accounts of one run stay together. Every uncaught background error
goes there too. A launch that shows nothing has left a reason behind.

## How it relates to the terminal

The two are separate processes with the config file between them; there is
no other channel. The terminal spawns the app on right-click, or on the
macOS menu's Settings item, which names the focused window and reaches the
same spawn (a sibling binary first, then `$PATH`); a second is declined
while one runs.
The app asks the terminal for the schema when it opens: `robco-term
--dump-settings` prints the defaults, the presets with every field
resolved, the enum value lists and the bundled font catalogue, with
each face's native pixel size and class beside its name, and
`settings/lib/model.tcl` reads them from that. The Rust source is
therefore the only statement of those values, and the window cannot show
a stale one. The price is that the window needs the terminal binary
present to open, and says so by name when it is not: the binary is the
one the spawning terminal names in `ROBCO_SETTINGS_TERMINAL`, else the
sibling executable under the name the platform gives it
(`robco-term.exe` on Windows, `robco-term` everywhere else), else one on
`$PATH`, and where the window is embedded in the terminal's own
executable the terminal is this process and needs no finding. The
machine's installed faces are a second ask over `robco-term
--list-renderable-fonts`, made when the user opts into system fonts
rather than at open, because that answer costs a walk of the platform's
font directories.

## Why Tcl/Tk and not a second Rust binary

Tk is the toolkit, and it stays. Writing this window as another binary in
the workspace would put the choice of widget set back on the table, and the
sets a Rust program reaches for are Qt and GTK. Neither belongs in this
repository: `docs/arch.md` states why the terminal links no toolkit, and a
sibling binary that linked one would bring it back through the packaging,
the build hosts and the licence surface even though the terminal itself
stayed clean. Tk draws the platform's stock widgets, ships as one
self-contained executable per platform, and asks nothing of the terminal's
own dependency graph.

The cost is a second language, and the price it charges is real: the
window's own value model, its build scripts and its interpreter packaging
are lines this repository maintains. The schema is no longer among them.

## Building and shipping it

The app is Tcl/Tk 9, under `settings/` in this repository: `lib/` is the
schema-bearing value model and the vendored tomledit module (the
byte-preserving TOML editor), `ui/` the window, `tests/` the tcltest
suites (`tclsh9.0 settings/tests/all.tcl`). tomledit's home is the
teatotal module shelf, <https://github.com/teatotal/teatotal>, where its
own test suite, man page and updates live; the copy in `lib/` is vendored
byte-identical from there. During development the entry script
`settings/robco-settings` runs directly against a system Tcl/Tk 9.

Releases ship it as one self-contained executable per platform, built by
`settings/zipfs/build-selfcontained.sh` (Unix) or `.ps1` (Windows): a
static Tcl 9 + Tk with the scripts folded in by `zipfs mkimg`, no installed
Tcl required, named `robco-settings-<version>-<os>-<arch>` under `dist/`.
Its version has one home, the `ROBCO_SETTINGS_VERSION` line in the entry
script. `cargo run -p xtask -- dist --settings-binary <that file>` (and
`deb` likewise) stages it beside `robco-term`; both refuse to package
without it, because no official package omits the settings app. A
self-compiled terminal without it loses only the right-click: the press
logs one warning and the file remains yours to edit. On macOS the pair
ships inside `RobCo Terminal.app` on a disk image
(`packaging/macos/bundle.sh`); on Windows the settings archive rides
inside the terminal exe itself. `.github/workflows/ci.yml` proves the
Windows and macOS builds on every push, Linux being proven on the
development machines before each push; `release.yml` builds the shipped
artifacts on tag.
