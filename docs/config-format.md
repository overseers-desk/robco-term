# Config file: the machine-write contract

This document is for anyone writing a tool that edits the terminal's
configuration file programmatically: a settings GUI, a `dotfiles` sync
script, a linter, a migration script bumping a version field, anything
that opens the file, changes something, and saves it back out. It states
the rules your writer must follow, not the settings schema itself (the
keys, their types, their defaults). It covers only the file mechanics
that any writer, regardless of the keys it touches, must get right.

For the keys themselves, and for the human's view of the same file, see
[`config.md`](config.md).

The format is [TOML](https://toml.io/). The file is read by the running
terminal and by any number of external tools, potentially at the same
time; the rules below exist so that all of them can coexist without
corrupting the file, losing each other's data, or racing the terminal's
live-reload.

## The file is a diff against defaults

Every setting has a built-in default. A key's absence from the file means
"use the default," not "use zero," "use empty," or "unset." Consequently:

- **Do not write a key merely to record its default value.** A writer
  that "fills in" every key it knows about, even at default, turns a
  four-line file into a two-hundred-line file and defeats the point of
  defaulting: a user's file should show only what they changed.
- **Do not delete a key just because your tool is about to write the
  default value for it.** If the user's file already pins a key away
  from its default and your tool is asked to set that key back to the
  default, the correct edit is to remove the key (letting it fall back
  to the default), not to write the default value explicitly. Writing it
  explicitly is not wrong, but it is not the minimal edit, and this
  contract prefers minimal edits (see below).
- A file that does not exist at all is equivalent to a file present but
  empty: every setting takes its default. Do not treat a missing file as
  an error condition in a writer; treat it as an empty document to edit.
  The same goes for the directory: if it isn't there yet, make it. A
  fresh install has no config directory until something writes one.

### What "the default" is depends on two keys

The `[screen]` and `[chassis]` tables each have a `name` key, and it is
not a label: it **selects which built-in preset the rest of that table is
a diff against**. `[screen] name = "Deep Blue"` followed by nothing else
means the whole Deep Blue preset; add `bloom = 0.9` and it means Deep Blue
with one value moved. A `name` naming no built-in preset (a look the user
saved under a name of their own) falls back to the shipped default as the
base.

Two consequences for a writer, and they are the ones that bite:

- **Removing a key does not always restore the shipped default.** The
  rule above ("the minimal edit for setting a key back to its default is
  to remove it") still holds, but "its default" means *the value the
  named preset gives it*, not the value a table with no `name` would
  give it. If your tool wants a specific value regardless of the preset,
  write that value explicitly.
- **Writing `name` rewrites the meaning of every other key in the
  table.** Changing `screen.name` from `"Deep Blue"` to `"Vintage"` does
  not just relabel the screen: every screen key the file does *not* pin
  now resolves to Vintage's value instead of Deep Blue's. A tool
  offering the user a preset picker should therefore decide deliberately
  whether it is switching presets (write `name` alone, drop the
  overrides) or renaming a look (write `name` and pin every value the
  user currently sees).

The two tables resolve independently, which is what makes a look two
axes: any screen can sit in any chassis.

## Writers write atomically

A writer must never let a reader observe a partially written file. The
required sequence is:

1. Write the complete new file contents to a temporary file **in the
   same directory** as the config file, not a system temp directory:
   same-directory placement is what makes the following rename atomic
   on the filesystem, rather than merely "usually fine."
2. Flush and sync that temporary file to disk.
3. Rename the temporary file over the config file's path.

The rename is what makes this atomic: at every point in time, the path
that readers open resolves either to the complete old file or the
complete new file, never to a half-written one. This matters twice over:
it protects against corruption if the writing process is killed or the
machine loses power mid-write, and it is the only write pattern the
terminal's live file watch is designed around. A writer that opens the
config file and writes into it in place, instead of writing a temp file
and renaming, will at best cause the terminal to reload a truncated or
torn file, and at worst will race the terminal's own watcher against a
half-written read.

Concurrent writers (your tool and the terminal's own settings editor
running at the same time, say) are not sequenced by this contract beyond
atomicity: the last rename to land wins, in full. If your tool needs
"read the current file, change one thing, write it back" semantics
without losing a concurrent edit, keep that window as short as possible
and accept last-writer-wins as the resolution model: there is no lock
file or transaction log in this design.

## Preserve everything you don't understand

The file will routinely contain keys, tables, comments, and formatting
that your tool has no opinion about: settings your tool predates, that
belong to a different tool, or that a human hand-edited and annotated. A
compliant writer:

- **Preserves comments.** A user's `# why I set this` note above a key
  survives every edit that doesn't touch that key, and survives edits
  that do touch that key's value as long as the comment isn't attached
  specifically to the old value.
- **Preserves formatting.** Blank lines, key ordering, indentation,
  quoting style, and inline-vs-table-array layout are not yours to
  normalize as a side effect of an unrelated edit. If a human or another
  tool laid the file out a certain way, an edit to one key should not
  reformat the other nine hundred bytes of the file.
- **Preserves the one array shape the file carries.** `[bindings]`'s
  `keys` is a multi-line array of one inline table per line
  ([`config.md`](config.md)). A writer that edits by lines treats each
  element line as a row and the opener and closer as its brackets; a
  writer that materialises a tree keeps the array's line layout and the
  comments between rows as it keeps every other byte.
- **Preserves unknown keys and tables.** A key your tool doesn't
  recognize is not invalid input and not your tool's to drop. It may be
  a setting introduced by a newer version of the terminal than your tool
  knows about, or a setting owned by a different tool entirely. Round-trip
  it unchanged.

Put together: **a writer that changes one key should change only that
key's bytes on disk.** Every other byte in the file (every comment,
every blank line, every unrelated key) should be identical before and
after the edit, down to the byte. This is the bar a writer is held to,
not "produces an equivalent TOML document" or "produces a file a human
would call basically the same."

## Reference implementation, not a mandate

The terminal's own Rust code satisfies every rule above using
[`toml_edit`](https://docs.rs/toml_edit), a format-preserving TOML
parser: it represents the file as an editable document that retains
comments, whitespace, and key order, so a single-field edit really does
touch only that field's bytes, and it writes changes out through the
atomic temp-file-then-rename sequence described above. That is a
statement about what the terminal happens to use, not a requirement on
you. Write your tool in whatever language you like, using whatever TOML
library you like (or none at all, if you're comfortable hand-editing
text). The only thing that matters is that the file it produces obeys
the rules in this document. A library billed as "format-preserving" for
TOML is worth checking for regardless of language; libraries that parse
TOML into a plain map and re-serialize it generally do not preserve
comments or formatting, and will fail the byte-identity bar above even
if the resulting document is semantically equivalent.

## If your tool triggers a reload

The running terminal watches the config file's directory (not the file
path directly, precisely so that the write-temp-then-rename pattern
above is observed reliably rather than losing the watch across the
rename) and reloads automatically once your write lands. You do not need
to signal it. If the terminal fails to parse the file your tool just
wrote, it keeps its last-known-good settings in memory and logs the
parse error loudly. It does not fall back to defaults, and it does not
fail silently. Treat a reported parse failure as a bug in your tool's output,
not as something the terminal should have recovered from more gracefully.
