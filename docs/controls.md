# Controls

Every key and pointer gesture the terminal takes for itself. Everything not
listed here goes to the program on the other end, which is why the list is
worth knowing: a key that is bound here is a key your editor never sees, and
a click that is taken here is a click it never receives.

The chord modifier is `Alt`, and `Cmd` on macOS.

Every other key held with `Alt` reaches the program as an escape followed by
the key, which is what a shell reads as Meta: `Alt`+`.` fills in the last
argument, `Alt`+`Backspace` rubs out the word behind the cursor. On macOS the
`Option` key composes characters instead, as it does in Terminal and iTerm2,
so the same bindings are typed there by pressing `Esc` and then the key.

## Channels

| Key | What it does |
|---|---|
| `Alt`+digit | Bring that channel to the screen |
| `Alt`+`Shift`+digit | Move the channel on screen to that slot |
| `Ctrl`+`Shift`+`T` | New channel |
| `Shift`+`Alt`+`T` | Pick where one new channel goes: a page of the configured SSH servers and localhost takes a free slot, a digit connects, `0` takes a destination you type, `Tab` ticks "make this the default connection", `Esc` steps back and then cancels |
| `Ctrl`+`Shift`+`W` | Close this channel |
| `Ctrl`+`PageUp` / `PageDown` | Previous / next channel |
| `Ctrl`+`Shift`+`Left` / `Right` | Move the channel on screen one slot |
| `Alt`+`PageUp` / `PageDown` | Page the bank |
| `Ctrl`+`Shift`+`B` | Fold the bank away, or bring it back |

The digit `0` on its own means channel 10. Digits are typed one at a time,
and the chord fires the moment no longer slot number could still match what
you have typed. Let go of the chord modifier to fire it early. So on a bank
of nine, `Alt` then `3` switches immediately, while on a bank of thirty it
waits to see whether a second digit is coming. The digit chords name the
numerals engraved on the bank while one is drawn, and the channel itself
where none is. Only the pager keys need the bank on show, having nothing to
step without it.

Paging over another screenful of the same bank leaves the channel on screen
alone, so a bank of more slots than the window fits can be read through
without interrupting anything. Paging onto another bank's slots switches to
that bank, on the channel it was last showing: it is the band switch it looks
like, and a band comes back to the station it was left on. A bank whose last
channel has since exited comes back on the first slot it still holds, and one
holding no channel at all comes back to no signal, where `Ctrl`+`Shift`+`T`
starts the next one.

`Ctrl`+`Shift`+`Left` / `Right` swap the channel on screen with its
neighbour, and take an empty slot outright. The first slot and the last are
walls: a step past either leaves the bank as it stands.

## Windows

| Key | What it does |
|---|---|
| `Ctrl`+`Shift`+`N` | New window |
| `Ctrl`+`Shift`+`Q` | Close this window |
| `F11` | Fullscreen, with or without modifiers |

`F11` ignores its modifiers, so the same key works for a hand used to
Konsole's `Ctrl`+`Shift`+`F11` and one used to a bare `F11`.

## Copying and pasting

| Key | What it does |
|---|---|
| `Ctrl`+`Shift`+`C` | Copy the selection to the clipboard |
| `Ctrl`+`Shift`+`V` | Paste the clipboard |
| Double-click | Select the word under the pointer |
| Triple-click | Select the whole line under the pointer, wrapping and all |

A mark stays with the channel it was made on. Switch away and the mark
waits there rather than following you or being thrown away, and it is back
on the glass when you return. A channel whose scrollback has filled up
loses its mark instead, because the lines it named have started falling off
the top and a mark over the wrong words looks exactly like a mark over the
right ones.

There are two selections here, as there are in every X11 terminal.
Selecting text with the mouse fills the *primary* selection, and the middle
button pastes it: that pair is a gesture of its own, and it leaves the
clipboard alone, so a run marked to paste two lines down does not cost you
what you copied ten minutes ago. `Ctrl`+`Shift`+`C` is what puts the
selection on the *clipboard*, where a browser or an editor looks for it,
and `Ctrl`+`Shift`+`V` is what reads it back. On macOS and Windows, which
have one pasteboard between them and no primary selection, the middle
button pastes the last selection made in this window and the pasteboard
stays where you left it.

Holding `Ctrl` while you middle-click forces bracketed paste onto a program
that did not ask for it. Hold `Ctrl`+`Alt` while you drag to select a
rectangle instead of a run of lines, and `Ctrl` alone (`Cmd` on macOS) to
copy a wrapped command as one unbroken run. Right-click opens the settings
window ([docs/settings-gui.md](settings-gui.md)); a program tracking the
mouse (vim, htop) receives the button instead, as it does the others.

Which model the drag itself follows is `general.selection_model`
([config.md](config.md)): `konsole`, the default, points at a cell and grows
a range of cells; `rio` points at the seam between two cells, so a drag
begun on the right half of a character leaves that character out, and it
brings rio's own word separators and its bracket-matching double click.

On macOS `Ctrl`+click is the right-click, as it is in every Mac terminal,
so it opens the settings window and reaches a mouse-tracking program the
same way the button itself does. That is why `Cmd` and not `Ctrl` holds
the unbroken copy there.

## Links

| Gesture | What it does |
|---|---|
| `Ctrl`+click on a link | Open it with the system's own handler |

`Cmd`+click on macOS, where `Ctrl`+click is the secondary click. A link is
a URL, an e-mail address, or a hyperlink a program declared with OSC 8:
`www.example.com` opens as `http://`, an address as `mailto:`, a hyperlink
as the URI the program gave, whatever text it showed. The terminal starts
`xdg-open`, `open` or the Windows URL handler and never learns which
browser answered.

While the key is held and the pointer rests on a link, the link is
underlined and the pointer becomes a hand; let the key go, move off, or
scroll, and both go. The underline is the glass's own: a copy of the
screen carries no mark the session did not send.

The press is the same one that copies a wrapped run unbroken, so what the
hand does next decides: a click that stays on its cell opens the link, a
drag selects its text and opens nothing. With a program tracking the mouse
(an editor, tmux), the press is the program's; hold `Shift` as well to take
it back, as for any selection.

## The menu bar, on macOS

| Key | What it does |
|---|---|
| `Cmd`+`,` | Open the settings window |
| `Cmd`+`H` | Hide the application |
| `Cmd`+`Option`+`H` | Hide every other application |
| `Cmd`+`Q` | Close the terminal |

macOS draws a menu bar for every application, and these are its items'
shortcuts rather than keys the glass reads: the menu takes them before the
grid sees them. Nothing here exists on Linux or Windows, which draw no such
bar.

## Searching

| Key | What it does |
|---|---|
| `Ctrl`+`Shift`+`F` | Open the find line |
| `Enter` | The next hit, down the history |
| `Shift`+`Enter` | The previous hit, back up it |
| `Esc` | Close the find line and clear the hit |

The find line is a `Find:` prompt on the glass, on the channel it was
raised on, and it takes every key while it stands: what you type is the
text to look for, and `Enter` walks the hits rather than sending a newline
anywhere. The text is looked for literally and without regard to case, so
`.` is a full stop and `ERROR` finds `error`.

Leaving the channel takes the line down, since a line that swallows every
key has no business standing on a screen you are not looking at. The line
opens with the last thing you looked for already in it, ready to be stepped
with `Enter` or typed over.

`Enter` steps forward from the last hit, or from the cursor when there is
no last hit, and wraps once when it reaches the end of the scrollback;
`Shift`+`Enter` walks the same list the other way. A hit is marked the way
a selection is marked, and the view moves the least it can to bring it onto
the screen, so a hit two rows above the top brings those two rows down
rather than jumping.

The query is drawn on the channel's own grid, which is where every other
thing this terminal says to you is drawn. It is therefore in the scrollback
like anything else, and hits on the find line's own rows are the query
reading itself back: those are stepped over, never shown.

## Scrollback

| Key | What it does |
|---|---|
| `Shift`+`Up` / `Down` | Scroll a line |
| `Shift`+`PageUp` / `PageDown` | Scroll a screen |

These move the view only on the primary screen. A full-screen program such
as an editor or a pager is drawn on the alternate screen, which has no
history behind it, so there the same keys are the program's own.

## On an attachment

A channel attached to a tmux server through control mode is a picture to
read and copy from rather than a surface to type at, because that channel's
pty carries the protocol itself. Every key is dropped there, except `Enter`,
which detaches the client: a channel you typed `tmux -CC` in comes home, and
one the terminal started for a session it found closes. Typing `tmux -CC` at
a session that already has a bank opens a second one; tmux allows a second
client and the terminal does not refuse it.

`Ctrl`+`Shift`+`B` folds the bank away and the well takes the whole
window, the cabinet standing around it; the same chord brings the bank back
at its configured width. It is this window's state and not a setting: a
reload leaves a folded bank folded, and a new window opens with its bank.
The digit chords keep naming channels while the bank is folded, as they do
with no chassis drawn.

## Not bound

Font size is a config key rather than a keystroke; see
[config.md](config.md). Nothing here binds `Ctrl`+letter, so `Ctrl`+`C`,
`Ctrl`+`D` and the readline chords reach the program untouched.

## Moving a chord

Every chord in this document is a shipped default, and the `[bindings]`
table of the config file lays its own rows over them: a row on the same key
and modifiers replaces a chord, a row on new ones adds one, and a chord a
program wants back is bound to `pass`. `F11` is the one key the table does
not reach. The table, the key names and the action names are in
[config.md](config.md).
