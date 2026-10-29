# SSH

`robco-term --ssh [user@]host[:port]` opens the first channel on an SSH connection the terminal itself owns, instead of a local shell. The user defaults to `$USER` and the port to 22, the same spelling `ssh` reads. An address made of colons is bracketed to carry a port, `[2001:db8::1]:2222`, and stands bare without one. The connection is a bank of its own: its channel sits at slot 1, the bank strip shows it beside the home bank, and closing its last channel hangs the connection up. Connection progress, authentication results and every failure are printed on the channel's own glass, dim and bracketed, in scrollback like everything else that happened there.

A channel whose remote shell exits ends the way a local shell's channel does, and so does every channel of a connection you hang up with `Alt`+`Shift`+`D`. Any other end keeps the channel until you close it (`Ctrl`+`Shift`+`W`), because the slot is the only place the reason is readable. A connection that fails before it ever produces a remote byte keeps its channel wearing the refusal. A connection that drops after working, a network gone or the server's sshd killed, keeps each of its channels with the screen it last held and `connection lost` under it. A `tmux -CC` attachment carried by such a channel ends with it, and the channel's strip reads `tmux -CC ended: connection lost` until you type there.

Where a session starts is configuration: the `[ssh]` table in `config.toml` holds the pre-configured servers as `[[ssh.host]]` rows and names which of them is the default (`docs/config.md`). The settings window is to list those rows as its SSH tab's radios, localhost first, showing which of them is the default; the terminal reads the table at launch, so a change applies to the next session started. `--ssh` on the command line outranks the configured default for its own invocation.

`Shift`+`Alt`+`T` raises a picker on the glass, and it is where a session that is not the default one is chosen. A digit opens a channel: `1` is localhost, `2` to `9` are the configured rows in the order the file lists them. `Esc` steps out of the page. A destination cannot be typed here: a hostname alone names neither a user nor a key file, so naming one is the settings window's job and this page lists what it was given.

`Tab` ticks the checkbox on the same page: make this the default connection. Leave it clear and the picker writes nothing at all. Tick it and the destination you just chose becomes `ssh.default` before the connection is dialled, so a connection that fails is not also a preference lost. The settings window's SSH radios set the same default from the bail-out side; on the glass, this checkbox is where it is set.

The sections below say exactly what this build will and will not do.

## Host keys

Trust is read from `~/.ssh/known_hosts`, then `/etc/ssh/ssh_known_hosts`. Only the first is ever written: the machine-wide file is the administrator's statement about the machine, and one user answering a prompt is not an administrator. Three outcomes:

* **Match**: a recorded key for the host matches the presented one, and the connection proceeds.
* **Unknown**: no key is recorded. The `SHA256:` fingerprint goes on the channel's glass and the terminal asks whether to accept and record it. Type `yes` and the key is written to `~/.ssh/known_hosts` and the connection goes on; type `no` and it is refused. Anything shorter re-asks: the full word is the friction a trust decision is owed, and it is the same rule `ssh` applies for the same reason.
* **Mismatch**: a key is recorded and the presented one differs. Refused, always, with both fingerprints and the offending `file:line`; no flag, key, environment variable or answer overrides it, and nothing is asked.

The question is typed on the terminal's own grid through the terminal's own keyboard, like everything else here. `Esc` withdraws it, which refuses the key and ends the connection.

The host-key algorithms recorded for a host lead the negotiation order, so a host known only under `ssh-rsa` connects rather than reading as unknown when the server also holds a newer key.

Ceilings of the reader, each of which costs a spurious refusal and never a false accept, because the policy refuses by default:

* Glob and negation host patterns (`*.example.com`, `!host`) are compared literally.
* `@cert-authority` lines never match, and a server presenting a certificate host key is refused outright.
* A tab-separated or double-spaced line does not parse.
* `@revoked` does better than the ceiling: any presented key a revocation line names is refused, whatever host it is filed under.

## tmux over the connection

Type `tmux -CC` on an SSH channel and the attachment arrives exactly as it does locally, and [controls.md](controls.md#on-an-attachment) says how it behaves and how it is let go. Because the connection multiplexes, each SSH channel can carry its own attachment: several remote tmux sessions over one wire, one `tmux -CC` each. Sessions on the remote server are not discovered automatically; the discovery mechanism is local by construction.

## Authentication

Public keys, from three sources, in intent order: the keys the configuration names for this destination (a `[[ssh.host]]` row's `key`, with `~/` expanded to home, or every `IdentityFile` line `~/.ssh/config` gives the host, tried in the order the file lists them), the ssh-agent (`SSH_AUTH_SOCK` on Unix; the OpenSSH Authentication Agent's named pipe, then Pageant, on Windows), and, only when no key is named at all, the default files `ssh` itself would try: `~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa`. Agent identities are tried in the order the agent lists them, silently; in the common case you never see an authentication message at all.

An encrypted key file is asked about on the channel's own glass. The passphrase is typed there and never echoed, not even as asterisks: a count of asterisks is a length, and a length is something about your passphrase. Three wrong ones give that key up and the sequence moves on. `Esc` withdraws the question, and withdrawing ends the whole attempt rather than moving to the next method: declining to connect is an answer about connecting.

Then the prompted methods, in `ssh`'s own `PreferredAuthentications` order: keyboard-interactive, then password.

Keyboard-interactive is the server's own challenge. It composes the questions and this asks them in its order, one at a time, with the echo flag it set on each: an employee number or a one-time code appears as you type it, a passphrase does not. A challenge can have several rounds, and the server ends them by accepting or refusing.

Password asks for `user@host's password`, never echoed, three tries, `permission denied, please try again` between them.

Both are skipped when the server does not offer them, which is what the opening `none` probe is for and what every rejection since has refreshed. A key-only server never asks you for anything. A lost cause closes with a line naming what the server would have accepted.

## `~/.ssh/config`

The file is read whole or not at all. Honouring `HostName` while passing over `ProxyJump` connects, confidently, to the wrong place; `HostKeyAlias` silently changes which `known_hosts` entry a key is checked against. Both failures look like success from the glass, so there are two outcomes here and no third: either the block matching your destination yields nothing this build cannot carry out, and every word of it is taken, or it carries one word this build cannot carry out, and the whole file's counsel is set aside out loud.

It is read once, on the way to the wire, and asked only about the destination you are dialling. `%USERPROFILE%\.ssh\config` is where it is read on Windows, the same home directory the default key files come from. A file that is not there, or that the terminal may not open, says nothing and costs nothing: an appliance whose user has no `~/.ssh/config` starts, looks and behaves like one built without a reader for it.

**Honoured**: `HostName`, `User`, `Port`, `IdentityFile`. All the `IdentityFile` lines the matched blocks give, in the file's own order, `~/` expanded, each landing where a `[[ssh.host]]` row's `key` lands: the named-key stage, tried ahead of the agent, with the passphrase asked for on the glass if the key is encrypted.

**Precedence** is `ssh`'s own. A field spelled on the `[[ssh.host]]` row or in the `--ssh` destination outranks the file: a user before the `@`, a port after the `:`, a row's `key`. The file fills only what was left unsaid, which includes the name `$USER` would otherwise supply and the port 22 would. `HostName` is the exception that is not one: it replaces the host you spelled, because `Host` names a lookup rather than a machine, and that is what the file is being read for.

**Refused, out loud**: anything that decides where a connection goes, whom it authenticates as, which identity it offers, which key it trusts, or what the far side runs, and that this build cannot carry out. `ProxyJump`, `ProxyCommand`, `HostKeyAlias`, `UserKnownHostsFile`, `LocalForward`, `RemoteCommand`, `IdentityAgent`, `Ciphers` and their kind, plus the boolean settings whose one honourable value is the behaviour this build already has: `ForwardAgent no` costs nothing, `ForwardAgent yes` is a promise this build cannot keep, and `StrictHostKeyChecking` is honoured at `ask` and nowhere else, because asking is what the host-key policy above does. The refusal names the directive on the channel's own glass, and the connection then proceeds to the literal destination, exactly as if the file did not exist. `HostName` beside a refused directive is not taken either: half a block is the failure this rule exists to prevent.

`Include` and `Match` are refused wherever in the file they stand, matched block or not. An `Include` means the file in front of the reader is not the whole file; a `Match` block is invisible to the parser, which would fold its directives into the `Host` block above it. Either way what would be read is not what you wrote.

Tuning and cosmetics decide none of those things and are passed over in silence, as `ssh` passes over a setting for a feature it was built without: `ServerAliveInterval`, `LogLevel`, `VisualHostKey` and the like.

A file that exists and does not parse is a notice on the glass and a connection to the literal destination, never a crash and never a blocked window. The parse is `russh-config`'s, the same family as the pinned russh, so the `Host` patterns, their negations and the merge order are one library's reading of OpenSSH's rules rather than this repository's guess at them. What that library hands back is only what it knows how to name, though, and a directive it has never heard of is dropped without a word, so the matched block is read here as text as well and every directive in it is held against the list above before any of the parse is believed.
