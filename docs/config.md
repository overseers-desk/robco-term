# Configuring RobCo Terminal

The terminal reads one TOML file, watches it, and reloads the moment it is
saved. Everything below is about that file. Two hands write it: the
settings window that right-clicking the glass opens (`robco-settings`,
shipped beside the terminal; see [`settings-gui.md`](settings-gui.md)), and
whatever you write text in, because editing the file directly is just as
supported: change a number, save, and watch the glass change while the
editor is still open.

This document is for the person doing either. If instead you are writing a
program of your own that edits the file on a user's behalf (a dotfiles
script, another settings GUI, a linter), the rules your writer has to obey
are in [`config-format.md`](config-format.md).

## Where the file is

| Platform | Path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/robco-term/config.toml`, or `~/.config/robco-term/config.toml` |
| macOS | `~/Library/Application Support/robco-term/config.toml` |
| Windows | `%APPDATA%\robco-term\config.toml` |

Nothing creates it for you at install time, and a fresh install does not have
one. That is not a broken state: a missing file means every setting takes its
default, which is the appliance you see on first launch. Create the file when
you have something to say. The terminal makes the directory itself so the
watch has something to watch, so you can create the file inside it at any
point in a running session and the next save will land.

Two other locations, for orientation. Under `robco-term` in your cache
directory the terminal writes the shader preset it generates at startup;
every byte of it comes from constants inside the binary, so deleting it costs
one regeneration and loses nothing. Under `robco-term` in your data
directory it writes crash backtraces. Neither is configuration and neither
needs backing up.

## How an edit reaches the glass

Save the file. That is the whole procedure.

The terminal watches the file's *directory*, so it survives the
write-to-temp-then-rename that careful editors do, and it reloads on its own.
You do not need to restart it or signal it. If you would rather force a
reload anyway (from a script, say, after generating the file), send the
process `SIGUSR1` on Linux and macOS.

If the file you saved does not parse, the terminal keeps the settings it
already had and logs the parse error. It does not fall back to defaults and
it does not go blank, so a stray bracket costs you the edit, not the session.
Fix it and save again.

Most edits are a uniform pushed into a running shader, which is why they look
instantaneous. A handful rebuild the filter chain instead, because they
change the shape of the pipeline rather than a number inside it: the rebuild
is quick, but it clears whatever phosphor burn-in had accumulated, so the
ghost of your last screenful starts over. Those keys are:

`general.window_scaling`, `general.font_scaling`, `general.bloom_quality`,
`general.burn_in_quality`, `general.chassis_shown`,
`general.led_characters`, `screen.font_name`, `screen.font_source`,
`screen.font_width`, `screen.line_spacing`, `screen.margin`,
`screen.frame_size`, `chassis.shell`, `chassis.channel_indicator`,
`chassis.channel_display`, `chassis.frame_size`, `chassis.bank_font_name`.

Nothing else about them is special. If you are not chasing a burn-in trail
across an edit, you will not notice the difference.

## The file is a diff, and `name` is what it is a diff against

Every setting has a built-in default, so a key you do not write takes it. A
useful config file is three or four lines long, not two hundred.

The subtlety worth reading twice: `[screen]` and `[chassis]` each have a
`name` key, and it is not a label. **It selects which built-in preset the
rest of that table is measured against.**

```toml
[screen]
name = "Deep Blue"
bloom = 0.9
```

That is the whole Deep Blue preset with one value moved, not the default
screen wearing Deep Blue's name. Change `name` to `"Vintage"` and every
screen key you did *not* pin now resolves to Vintage's value instead of Deep
Blue's; the two lines you did pin stay where you put them. A `name` matching
no built-in preset is fine and means what a name always means for a look you
saved yourself: the shipped default is the base, and your keys sit on top.

The two axes resolve independently. Any screen can sit in any chassis, which
is the point of splitting them.

Built-in screen names: `Default Amber`, `Monochrome Green`, `Deep Blue`,
`Commodore 64`, `Commodore PET`, `Apple ][`, `Atari 400`, `IBM VGA 8x16`,
`IBM 3278 Reborn`, `Neon Cyan`, `Ghost Terminal`, `Plasma`, `Boring`,
`E-Ink`.

Built-in chassis names: `Annunciator`, `Slide Rule`, `Switchboard`.

## Profiles

A **profile** is a whole appliance kept under a name of your own: a screen
and the chassis it stands in, and nothing else. General settings are yours,
not a profile's, so putting on a different look never re-fits your LED bank
or changes your window scaling.

A profile is a file: `config.<name>.toml`, sitting beside `config.toml` in
the same directory. It holds `[screen]` and `[chassis]` and no `[general]`.
Because it is read through the same loader as the main file, the `name` rule
above applies inside it too. A saved profile is a preset base plus your
overrides, exactly like the main file.

Start under one with `--profile`:

```console
$ robco-term --profile workshop
```

The name resolves in this order:

1. **A saved profile**: `config.workshop.toml` beside your config file. Both
   axes come from it; your general settings and everything else keep coming
   from `config.toml`.
2. **A built-in screen preset**: `robco-term --profile "Deep Blue"` puts
   that screen behind the glass and leaves the cabinet standing.
3. **Neither**: the terminal refuses to start and prints the built-in names
   it does know.

Refusing is deliberate: it is the answer that cannot quietly hand you the
wrong look under the right name.

The named look is applied on *every* load, not just at startup, so a live
edit to `config.toml` while running under `--profile` will not silently take
the profile back off. General keys are untouched by the overlay, which is why
editing one in `config.toml` still reaches a run launched under `--profile`.

`--default-settings` ignores your files entirely and starts from the built-in
defaults. Under it, `--profile` can only name a built-in screen. A saved
profile is user config, and that is what the flag is refusing to read.

## The keys

Defaults below are the shipped ones: `[general]`'s own declarations,
`[screen]`'s from the `Default Amber` preset, `[chassis]`'s from
`Annunciator`. Everything typed as a fraction runs `0.0` to `1.0` and is a
slider by nature, and the interesting values are usually not the ends.

### `[general]`

The knobs that belong to you rather than to a look. These survive every
profile switch.

| Key | Default | What it does |
|---|---|---|
| `effects_frame_skip` | `3` | How many frames the effects clock holds a value before jumping. The CRT animates at 60/skip Hz, so the shipped `3` is 20 Hz. Lower it for a faster-moving picture at more GPU cost. This is the cadence while somebody is at the glass; a window that has lost the keyboard, or has gone five minutes without a keystroke, click or scroll, holds its picture and animates at nothing at all until you come back. Output still arrives and still draws, and a critter waits for you rather than crossing an empty screen. |
| `window_scaling` | `1.0` | Scales the whole appliance: glass, chassis and all. |
| `font_scaling` | `1.0` | Scales the type, and so the number of rows and columns the window holds. |
| `show_terminal_size` | `true` | Whether the size badge appears in the well while you drag the window. |
| `bloom_quality` | `0.5` | Sizes the bloom framebuffer and sets the blur radius. Costs GPU; buys a smoother glow. |
| `burn_in_quality` | `0.5` | Sizes the burn-in accumulator. |
| `led_characters` | `12` | How many characters wide the bank's channel strips are. Dragging the seam between the bank and the screen well writes this key for you. On a window too small to hold both this and the terminal's floor, the strips draw narrower than this; the setting itself is not touched. |
| `chassis_shown` | `true` | Whether the cabinet is drawn around the tube at all. With it off, the tube stands bare in its own moulding, and the `[screen]` table's frame keys govern rather than `[chassis]`'s (see below). |
| `grapheme_clustering` | `false` | Whether the grid measures text by grapheme cluster, DEC private mode 2027. Off means one column per code point, the `wcwidth` layout whiptail, tmux and shell line editors assume, so their tables and boxes line up. On means an emoji written with a variation selector, a joiner or a skin tone takes one two-column slot, as ghostty and kitty do by default. A program can ask for cluster widths at runtime with `CSI ? 2027 h` whatever this key says; the key sets what it finds and what a reset returns to. |
| `selection_model` | `"konsole"` | Which house's selection model the pointer follows. `konsole` points at a cell and grows a range of cells. `rio` points at the seam between two cells, so a drag begun on the right half of a character leaves that character out; it brings rio's own word separators with it, and a double click on a bracket takes everything up to its partner. Read at the start of each gesture, so an edit reaches the next drag. |
| `show_menubar` | `false` | Present in the schema; nothing in this build reads it. The bar macOS draws for every application is the platform's own and answers to no key here. |
| `use_custom_command` | `false` | As above. Use `--program` or `-e` to run something other than your shell. |
| `custom_command` | `""` | As above. |

### `[screen]`

Everything behind the glass: the phosphor, the type, the geometry, and the
effects that age them.

| Key | Default | What it does |
|---|---|---|
| `name` | `"Default Amber"` | Selects the preset base. See above: this is not a label. |
| `background_color` | `"#000000"` | The unlit tube. |
| `font_color` | `"#ff8100"` | The phosphor. |
| `brightness` | `0.5` | Overall picture brightness. |
| `contrast` | `0.8` | Picture contrast. |
| `ambient_light` | `0.3` | Room light falling on the glass, which is also what lifts the frame out of black. |
| `window_opacity` | `1.0` | Translucency of the whole window. |
| `saturation_color` | `0.2` | How far the phosphor colour pulls the picture toward itself. |
| `chroma_color` | `0.2` | Colour bleed. |
| `flickering` | `0.1` | Brightness flicker. |
| `horizontal_sync` | `0.1` | Horizontal sync wobble. |
| `static_noise` | `0.1` | Snow. |
| `jitter` | `0.2` | Per-frame positional jitter. |
| `rgb_shift` | `0.0` | Colour-channel separation. |
| `glowing_line` | `0.2` | The travelling scan line. |
| `burn_in` | `0.3` | How long a lit pixel's ghost survives after it goes dark. `0.0` turns the accumulator off. |
| `bloom` | `0.6` | Glow intensity. Its *quality* is `general.bloom_quality`. |
| `screen_curvature` | `0.2` | How far the tube bulges. Also what your clicks are mapped back through, so selection follows the curve. |
| `rasterization` | `"no_rasterization"` | Which scanline/pixel grid is laid over the type. One of `no_rasterization`, `scanline_rasterization`, `pixel_rasterization`, `subpixel_rasterization`, `modern_rasterization`. |
| `font_name` | `"TERMINESS_SCALED"` | The glyph face, by catalogue key (see below). A key naming nothing falls back to the shipped default rather than refusing to draw. |
| `font_source` | `"bundled_fonts"` | Present in the schema; nothing in this build reads it. |
| `font_width` | `1.0` | Cell width as a multiple of the face's own. Bitmap faces want pixel-exact ratios. |
| `line_spacing` | `0.1` | Extra height per row. |
| `margin` | `0.3` | Inset between the type and the bezel. |
| `monospace_trigger` | `'[\|\x{2500}-\x{257F}]\|-{3,}\|={3,}\|\S.*\S {4,}'` | A regular expression over a row's text, trailing blanks trimmed. A row it matches is set in the configured face; any other row is set in the prose face, when the run has one (`--prose-font`). The shipped run of spaces must come after text: indentation and list markers line up in prose by themselves, and only a run after words of varying width needs the ruler. Write it as a TOML literal string (single quotes) so the backslashes stay; the file is UTF-8, so `[│┼─]` is as good as a code-point range. Live. |
| `frame_size` | `0.1` | The tube's own moulding. Governs only when `general.chassis_shown` is off. |
| `screen_radius` | `0.1` | Corner radius of that moulding, `4` to `120` pixels across the range. Governs only when `general.chassis_shown` is off. |
| `frame_color` | `"#cfcfcf"` | As above: the bare tube's moulding. |
| `frame_shininess` | `0.3` | As above. |
| `blinking_cursor` | `false` | Carried for schema parity. The cursor does not blink in this build regardless of the value. |

The four frame keys exist in both `[screen]` and `[chassis]` because a tube
has its own moulding and a cabinet has another. Whichever is showing is the
one that governs: with `general.chassis_shown = true` the `[chassis]` values
win, and with it off the `[screen]` values do.

Bundled `font_name` keys: `TERMINESS_SCALED`, `BIGBLUE_TERMINAL_SCALED`,
`EXCELSIOR_SCALED`, `GREYBEARD_SCALED`, `COMMODORE_PET_SCALED`,
`GOHU_11_SCALED`, `COZETTE_SCALED`, `UNSCII_8_SCALED`,
`UNSCII_8_THIN_SCALED`, `UNIFONT`, `APPLE_II_SCALED`,
`ATARI_400_SCALED`, `COMMODORE_64_SCALED`, `IBM_EGA_8x8`, `IBM_VGA_8x16`,
`TERMINESS`, `HACK`, `FIRA_CODE`, `IOSEVKA`, `JETBRAINS_MONO`, `IBM_3278`,
`SOURCE_CODE_PRO`, `DEPARTURE_MONO_SCALED`, `OPENDYSLEXIC`. The `_SCALED`
ones, with `IBM_EGA_8x8` and `IBM_VGA_8x16`, are the low-resolution faces,
drawn from their embedded bitmap strikes at integer scale; the rest are
outline faces.

### `[chassis]`

The cabinet the tube is mounted in, and the way its bank marks which channel
is on air.

| Key | Default | What it does |
|---|---|---|
| `name` | `"Annunciator"` | Selects the preset base. |
| `shell` | `"annunciator"` | Which kit paints the body: `annunciator`, `slide-rule`, or `switchboard`. Note the hyphen. |
| `channel_indicator` | `"glow"` | How the bank marks the live channel: `glow`, `pointer`, or `switch`. |
| `channel_display` | `"led"` | What the bank's channel windows are made of: `led` or `tape`. |
| `frame_size` | `0.45` | The cabinet's bezel around the glass. |
| `screen_radius` | `0.44` | Corner radius of that bezel, `4` to `120` pixels across the range. |
| `frame_color` | `"#001735"` | Bezel colour. |
| `frame_shininess` | `0.3` | How hard the bezel's highlight is. |
| `bank_font_name` | `"COZETTE_SCALED"` | The face this cabinet letters its channel bank in. Each cabinet carries its own: the lamp cabinets letter their strips in Cozette, and `Switchboard` stamps its tape in Departure Mono. A name matching no bundled face falls back to the kit's own. Bundled faces are listed under `[screen]`'s `font_name`. |

The shells differ in more than colour: each has its own furniture, and
the presets pair each with the indicator and display style it was built
around. Mixing them works. A `slide-rule` shell with `tape` windows is a
legitimate config, just not a combination anything shipped.

### `[ssh]`

Where a new session starts: on a local shell, or on one of the
pre-configured servers below. The settings window lists these as its
SSH tab's radios, localhost first, the checked radio being the default;
the picker on the glass (`docs/ssh.md`) sets the same `default` at
connect time, tick by tick. The terminal reads the table at launch,
so a change applies to the next session started, and a channel under
`tmux -CC` control is never affected (its windows come from tmux, not from
spawning).
`docs/ssh.md` carries the connection contract these rows feed.

| Key | Default | What it does |
|---|---|---|
| `default` | `""` | The `host` of the `[[ssh.host]]` row new sessions start on. Empty means localhost, today's behaviour unchanged; a value matching no row is logged at launch and behaves as empty. Written by hand, or by the picker's checkbox (`docs/ssh.md`), which is the only thing in the program that sets it. A typed destination ticked there appends its `[[ssh.host]]` row and moves this key in the same edit, so a default and the row it names cannot arrive apart. |
| `host` | `[]` | The server rows, written as `[[ssh.host]]` tables. Each carries `host` (the destination), `user` (empty leaves the account to `~/.ssh/config`, then to the invoking user's name), `port` (22 unless said otherwise, and 22 is a gap the config file may fill), and `key` (a key file path, tried ahead of the agent, `~/` meaning home). What a row leaves unsaid is what `~/.ssh/config` is allowed to fill, and what it says outranks that file: `docs/ssh.md` carries the whole precedence. |

```toml
[ssh]
default = "vault.example.com"

[[ssh.host]]
host = "vault.example.com"
user = "overseer"
port = 22
key = ""
```

### `[bindings]`

The terminal's own chords, as `docs/controls.md` lists them, and what a
hand that wants them elsewhere writes. Every chord in that document is a
shipped row; a row here with the same `key` and `with` replaces it, a row
on a new trigger adds one, and the later of two rows on one trigger stands.
An empty table, or none, is the shipped set exactly. What bytes a key sends
to the program is not here: that is the keytab, and a row's `esc` reaches
the same wire.

| Key | Default | What it does |
|---|---|---|
| `keys` | `[]` | The rows, an array of inline tables. Each carries `key`, `with`, and one of `action` or `esc`. `key` is a single character, matched to the key the layout produces and without regard to case (`"b"` and `"B"` are one binding, and on Dvorak it is the key that types a b), or a named key: `pageup`, `pagedown`, `home`, `end`, `insert`, `delete`, `enter`, `tab`, `escape`, `space`, `backspace`, `left`, `right`, `up`, `down`, `f1` to `f24`, and `digit` for any of the ten. `with` is the modifiers held, joined by `\|`: `ctrl`, `shift`, `alt`, `super`, and `chord` for the chord modifier, which is `alt` here and `super` on macOS, the one spelling that lands on the same finger everywhere; `control`, `option`, `cmd` name the same keys. Empty is no modifier. `action` names what the chord does: `new_channel`, `close_channel`, `next_channel`, `prev_channel`, `move_channel_left`, `move_channel_right`, `page_bank_up`, `page_bank_down`, `select_channel`, `store_channel`, `open_picker`, `copy`, `paste`, `open_find`, `fold_bank`, `new_window`, `close_window`; or one of the two unbinds, `pass`, which lets the key through to the program as if the terminal never had it, and `swallow`, which takes the key and sends nothing. `esc` instead writes an escape sequence to the program: the bytes after the leading `ESC`, so `"[15~"` is F5, spelled as TOML spells it (`\u001b` for a further escape byte; TOML has no `\x1b`). A row naming an unknown action, a key or modifier that does not parse, or both `action` and `esc` or neither, is logged and left out, the chord staying at its shipped meaning; a row with a field this table does not have is a parse error, and the file keeps its last good reading. |

```toml
[bindings]
keys = [
  { key = "b", with = "ctrl | shift", action = "fold_bank" },
  { key = "c", with = "ctrl | shift", action = "pass" },
  { key = "f5", esc = "[15~" },
]
```

### `[critters]`

Every so often something drawn walks across the glass and leaves. A whale
spouts, three ducks paddle past, a locomotive runs through. It is drawn over
the terminal's picture rather than into it, so text scrolls behind one, a
keystroke does not chase it away, and a selection copied across one gives
back what the session wrote. Every piece is off any cell it touches within
about a second, which is why the wide ones cross faster than the small ones,
and why there is no key here for how fast they go.

One critter per interval, counted from midnight. Shipped, it arrives at the
start of its interval, so at `minutes = 15` you get one at :00, :15, :30 and
:45 by the clock in your room, and catching one tells you the time. Under
`timing = "random"` it arrives at an unpredictable point inside the same
interval instead, so it still comes four times an hour but never when you
expect it.

An interval you were not there for brings nothing. A window counts as
unattended the moment it loses the keyboard, so an arrival held over would be
one waiting at the moment you came back, greeting every return.

| Key | Default | What it does |
|---|---|---|
| `enabled` | `true` | Whether anything crosses the glass at all. Off is the same silence as a build with none of this in it. |
| `timing` | `"clock"` | `clock` puts the arrival at the start of each interval, on the wall-clock mark; `random` puts it at an unpredictable point inside the same interval. |
| `minutes` | `15.0` | The minutes in one interval. The settings window's slider runs 1 to 120; the file takes any number, and anything under a second is read as a second. |
| `dolphins` | `true` | A pair leaping and going back in, asciiquarium's. |
| `ducks` | `true` | Three abreast, quacking down the line, asciiquarium's. |
| `swan` | `true` | asciiquarium's, and the only one that needs no animating to look alive. |
| `whale` | `true` | asciiquarium's, which swims a while and then blows. |
| `ship` | `true` | A three-master, asciiquarium's. |
| `monster` | `true` | The sea monster, its humps travelling down its back, asciiquarium's. |
| `pacman` | `true` | Chomping, with the ghost behind him, from jbanana's `anims`. He is being chased, so he only ever goes left. |
| `locomotive` | `true` | The D51 from `sl`, rods turning once a column as `sl` turns them. It has run right to left since 1993 and still does. |

Turning one off makes the others correspondingly more likely rather than
leaving a gap in the schedule where it used to be.

### `[serial]`

A terminal on a serial line took its characters at the speed of the wire, and
a screen arrived a line at a time rather than all at once. This table puts the
terminal back on one. It ships off, and off it costs nothing: the glass fills
as fast as the machine can fill it.

What is slowed is the reading and never the drawing. The terminal takes what
the rate has earned and leaves the rest in the tty's buffer, so the program
blocks once that fills, exactly as it would have. Everything liveable about
this follows from that. `Ctrl`+`C` stops the output because it stops the
program writing it. A copy of the screen holds what the screen holds. A search
of the scrollback reads everything that arrived. Nothing is queued anywhere
inside the terminal waiting to be drawn.

How far a program runs ahead of the picture is the buffer between them rather
than the rate: a Unix tty holds a few kilobytes, while on Windows the
terminal's own reader keeps as much as 64 KiB, which at 19200 is half a minute
of screen. The picture arrives at the rate on both.

It reaches a local shell and nothing else. A channel on SSH, and a tmux window
arriving through control mode, have had their bytes read off a transport
before they reach a grid, so metering them there would hold back bytes already
in hand instead of leaving them unread at the far end. The gateway channel a
`tmux -CC` opens is exempt for the same reason: its own PTY carries tmux's
protocol rather than a picture of a shell.

| Key | Default | What it does |
|---|---|---|
| `enabled` | `false` | Whether output is metered at all. Off is a build with none of this in it. |
| `baud` | `19200` | Bits a second. At eight data bits, no parity and one stop bit, a character is ten bits, so the terminal reads a tenth of this many bytes a second and 19200 fills an 80 by 25 screen in about a second. The settings window offers the standard rates from 300 to 115200; the file takes any number. |

## A worked example

```toml
# A quieter amber, in the wooden cabinet.
[general]
font_scaling = 1.2
effects_frame_skip = 2

[screen]
name = "Default Amber"
flickering = 0.0
static_noise = 0.0
burn_in = 0.15

[chassis]
name = "Slide Rule"
```

Four keys moved, one screen preset and one chassis preset named. Everything
else comes from those two presets. That means every colour, every geometry,
and every effect not listed. That is what "the file is a diff" buys you: this file
still means the same thing after a release that retunes Default Amber, and it
still means the same thing if you copy it to another machine.
