# Keybinding config formats: what the field settled on

Snapshot September 2026, read from each product's own documentation. Capabilities are as their authors describe them, not independently re-verified.

A terminal that lets a user rebind its own chords has to answer about nine questions, and most of them cannot be revisited once a file format is in users' hands. This document reads eight products' answers, marks where they converged, and marks the three places a product had to change its format after shipping. It is the ground the `[bindings]` table of `docs/config.md` was designed on, so a later change to that table can see which of these it is about to repeat.

## The products read

| Product | Where bindings live | Shape |
|---|---|---|
| kitty | `kitty.conf` | `map ctrl+a new_window` |
| Ghostty | `config` | `keybind = ctrl+a=new_window` |
| WezTerm | `wezterm.lua` | `{ key = 'a', mods = 'CTRL', action = ... }` |
| Alacritty | `alacritty.toml` | `[[keyboard.bindings]]` with `key`, `mods`, `mode`, `action`/`chars` |
| Rio | `config.toml` | `[bindings]` with a `keys` array of inline tables: `{ key = "q", with = "super", action = "Quit" }` |
| foot | `foot.ini` | `[key-bindings]`, action on the left: `scrollback-up-page=Shift+Page_Up` |
| Windows Terminal | `settings.json` | `actions` (command + id) and `keybindings` (keys + id), two arrays |
| Konsole | `.keytab` files, plus KDE's shortcut editor | two separate systems, one per layer |

## The nine questions, and the answers that converged

**1. Which side of the line is the trigger?** Seven of the eight put the trigger first and the action second. foot inverts it, one action per line taking a list of combos. foot pays for the inversion in its own manual: "A key combination can only be mapped to one action", and the collision check it needs to enforce that misses `Control+C` against `Control+Shift+c`, which the manual records as a known bug. Trigger-first makes the duplicate check the easy direction, because the trigger is the key of the map.

**2. How does a user turn a default off?** Every product has an answer, and two distinct meanings emerged that products tended to ship one of and then add the other:

- Let the key through to the program. Windows Terminal `unbound` or `"id": null`, Alacritty and Rio `ReceiveChar`, kitty a bare `map ctrl+x` with no action, Ghostty `unbind`.
- Swallow the key and send nothing. Alacritty and Rio `None`, kitty `discard_event`, Ghostty `ignore`.

Rio's documentation draws the pair as sharply as any. `action = "ReceiveChar"` on Super+N disables new-window and lets the character through; `action = "None"` blocks it silently.

Windows Terminal's documentation gives the use case in full: Ctrl+V is paste, vim wants Ctrl+V for blockwise visual mode, so the key has to reach the program. That is the same shape as any clash a user will hit here.

**3. Can a binding send raw text or an escape sequence?** Universal. kitty `send_text`, Ghostty `text:`, `esc:` and `csi:`, Alacritty `chars`, Windows Terminal `sendInput`, Rio `esc`, and foot devotes a whole section to it, `[text-bindings]`, where `\x03 = Mod4+c` remaps Super+c to Ctrl+C. This matters more than it looks: it is the pressure valve that lets a user fix a disagreement with the terminal's key encoding without the encoding tables themselves being configurable.

Rio changed its answer to this question after shipping, and the change is worth reading in full, because Rio's config is TOML like this project's. Until July 2025 a binding had two fields for one job, `text` for a string and `bytes` for a decimal array, and both produced the same internal `Action::Esc`. They collapsed into a single `esc` field taking a string (rio commit `a3630d71`, 30 July 2025).

Three faults went with the two fields. The precedence was undocumented: `bytes` was tested after `text`, so it silently won when a config set both. The array bought nothing: the bytes were run through `std::str::from_utf8` and validated back into a string, with no else arm, so an array that was not valid UTF-8 was silently dropped and the binding did nothing. And the deeper one, which is why the field was renamed rather than merely merged: sending a sequence was implemented as a paste and inherited paste's behaviour. A binding that meant "write these bytes to the program" also cleared the selection, scrolled the view to the bottom, and rewrote `\r\n` to `\r` in its own payload. The commit stripped all three, and the code now routes an `esc` binding through the paste path with bracketing off, where the comment reads "when we explicitly disable bracketed paste don't manipulate with the input, so we pass user input as is."

The rename is the visible half of a semantics fix. `text` invited paste behaviour and got it; `bytes` promised raw bytes and did not deliver them. `esc` names what the field does, which is to write bytes to the pty untouched.

The migration cost is silence. Rio's config structs carry no `deny_unknown_fields`, so a config still holding `bytes = [27, 91, 53, 126]` parses, the unknown key is ignored, and the binding becomes a chord that swallows the key and does nothing, with no message.

TOML also dictates the spelling of whatever such a field holds: Rio's documentation warns that `\x1b` does not work inside a TOML string, and an escape must be written `\u001b`.

**4. Is there an alias for the terminal's own modifier?** Only kitty. `kitty_mod` defaults to `ctrl+shift` and every default binding is written against it, so a user moves the whole scheme with one line. Nobody else has it, and nobody else has a documented single modifier that all their own chords hang off.

**5. Multi-key sequences?** kitty and Ghostty converged on the same character: `ctrl+f>2` in kitty, `ctrl+w>escape` in Ghostty. WezTerm took a different road, a `leader` key plus named key tables. Alacritty, foot and Windows Terminal have none. The `>` form is the one to reserve, whether or not it is implemented.

**6. What are the modifiers called?** Convergent among the newer products: `ctrl`, `shift`, `alt`, `super`, lowercase, with `control`, `opt`/`option` and `cmd`/`command` as accepted aliases naming the same modifier rather than a separate macOS one. WezTerm and Alacritty use capitalised names joined by `|`. foot uses XKB names, `Mod1` and `Mod4`, with `Alt` and `Super` as virtual equivalents.

**7. Physical key or the character the layout produces?** The one question nothing converged on, and the one every product answered after shipping, under bug reports from non-Latin layouts. WezTerm has `phys:A`, `mapped:a`, and a `key_map_preference` setting for what a bare name means. kitty has `--allow-fallback=shifted,ascii`, which all its own defaults use. Alacritty accepts a decimal scancode in the `key` field. Rio documents key names only and says nothing about layouts. A format that fixes what a bare `b` means, and leaves no room to say otherwise, is the corner this list is most likely to hit.

**8. Where do conditions and flags go?** Each product reserved a slot before it needed one. kitty puts `--flag` options between `map` and the action, and later filled the slot with `--when-focus-on`, `--mode` and `--new-mode`. Ghostty puts prefixes before the trigger: `global:`, `all:`, `unconsumed:`, `performable:`. Alacritty has a `mode` field taking `Vi`, `Search`, `AppCursor`, `AppKeypad`, `Alt`, negatable with `~`. Rio has the same field with four modes, `vi`, `alt`, `appcursor` and `appkeypad`, and the same negation: `{ key = "esc", mode = "~vi", action = "ToggleVIMode" }`.

Windows Terminal is the counter-example, and the clearest case of a format changing after it shipped. Binding several keys to one parameterised action, and listing that action in a command palette, needed the action to have a name. The format now splits in two: `actions` holds command and `id`, `keybindings` holds `keys` and `id`, with `Terminal.` and `User.` id namespaces. Every default is written twice, once in each array.

**9. Do user bindings merge with defaults, or replace them?** Merge, everywhere except foot, which replaces per action. Ghostty states the tiebreak outright: "the keybind set later will overwrite the keybind set earlier." kitty says the last line in `kitty.conf` wins. Rio states the rule from the other end: bindings are always filled by default, and one is replaced when a new binding with the same trigger is defined. Merge is also what a file that is a diff against defaults requires.

## The two layers, and who separates them

Konsole is the only product in the list that separates the two things a terminal binds. A `.keytab` file governs what bytes a key sends to the program, and KDE's application shortcut editor governs Konsole's own actions. Alacritty is the opposite, one `[[keyboard.bindings]]` table where an entry either performs an action or sets `chars` to raw bytes.

The separation is worth keeping. What bytes a key sends is protocol behaviour, tested against a reference implementation, and a user editing it breaks programs rather than fixing a clash. What a chord does to the terminal's own furniture is a preference. A binding action that sends raw text, question 3 above, gives the second layer enough reach that the first can stay compiled.

## What the `[bindings]` table took from this

The table is Rio's syntax verbatim: `[bindings]`, `keys`, an array of inline tables with `key`, `with`, `action` and `esc`. Three of the file's own facts decided the rest.

- The file is a diff against defaults, so the shipped chords are the application's data (`crates/app/src/bindings.rs`) and the file's rows lay over them: a row on a shipped trigger replaces it in place, a new trigger joins, the later of two rows on one trigger stands. An absent table is the shipped set exactly.
- The settings window edits the file with a line-oriented editor, `tomledit`, which until 1.1 refused any document holding an inline table or a multi-line array. Rather than deviate from Rio's shape to the `[[bindings.keys]]` spelling the editor already handled, `tomledit` 1.1 admits a multi-line array of one inline table per line as a second shape of row, addressed as `bindings.keys[i].action` like a `[[name]]` row. Both spellings parse on the Rust side.
- A row with a field the table does not have is a parse error, the reader keeping its last good state; a row whose meaning does not parse is logged and left out. Rio's silently ignored `bytes = [...]` is the case the first rule answers.

The rest follows the questions above: trigger first, an exact modifier set on the layout-produced key with case folded, `digit` as a key class, `chord` as the platform's modifier token, `pass` and `swallow` as the two unbinds, one `esc` string field named for what it does, and `F11` the one key outside the table.
