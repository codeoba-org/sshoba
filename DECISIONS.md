# Decisions

Choices made where the build plan was silent or left a question open.

## General

- **Crossterm via ratatui.** We use `ratatui::crossterm` (the re-export) instead of a
  separate `crossterm` dependency so the versions can never diverge.
- **No `anyhow`.** Core's typed `Error` already carries exit codes, so the binary uses it
  directly; `anyhow` would add nothing.
- **`chrono`** is used for RFC 3339 timestamps in `state.json` (not listed in the plan).
- **Edition 2021** as the plan specifies, even though cargo now defaults to 2024.

## Config

- **`toml_edit` round-tripping works.** Saves merge changes into the parsed document:
  unchanged servers are left byte-for-byte, changed servers keep their comments and key
  order, new servers/forwards are appended after the last table. Comments are only lost
  on lines whose value shhoba itself changed (the value is replaced, its trailing comment
  is kept). Shhoba never rewrites `[settings]`.
- **Unknown keys are errors** (`deny_unknown_fields`), so typos like `hostname = ...`
  are caught with a line number instead of being silently ignored.
- **First-run template** writes `[settings]` with every default value and an explanatory
  comment, rather than commented-out keys: comments in an otherwise empty table end up
  after appended servers in `toml_edit`'s output.
- **`shhoba edit`** edits a temporary copy, validates it on close and only then replaces
  `servers.toml` atomically (with `.bak`). Declining to reopen after an error leaves the
  original untouched.
- **`shhoba set <name> name <new>`** is a rename (updates jump references).
  `extra_args` values are split on whitespace; list fields are comma-separated; an empty
  value clears an optional field.

## SSH

- **Jump-chain semantics follow OpenSSH:** with `ProxyJump a,b`, ssh honours the first
  hop's own `ProxyJump` but overrides later hops'. The displayed route is therefore
  `chain(jump[0]) ++ jump`. Cycle and depth checks still follow every jump reference.
- **`-F` propagation verified** on OpenSSH 9.9: ssh runs `ssh -F <file> -W ... <hop>` for
  jumps (covered by `tests/sshcmd.rs`). The ProxyCommand fallback was not needed.
- **`Host *` before the trailing `Include`s.** An `Include` after a `Host` block is
  conditional on that block, so the generated file opens a `Host *` scope first.
- The user's `~/.ssh/config` `Host` patterns are matched against the `shhoba.<name>`
  alias, so per-host settings keyed by the real hostname don't apply; `Host *` settings do.
- **`shhoba cmd`** prints the exact `ssh -F ... shhoba.<name>` command (runnable as-is);
  `show` prints both it and the readable `-J` form.
- The readable command expands `~` in key paths so it works when pasted into a shell.

## Launching

- **tmux wins over Zellij** when both `$TMUX` and `$ZELLIJ` are set.
- `--tab` with `launch = "current"` still uses a detected multiplexer.
- Connecting in the current terminal from the CLI requires stdin to be a TTY.
- Zellij layouts were checked against the documented 0.40+ syntax; not yet exercised in
  a live Zellij session here.

## TUI

- While a search is active the list is flat and ranked by match quality; groups return
  when the search is cleared.
- Servers without a group appear under an `other` group when any group exists, and the
  list is flat when no server has a group.
- Deleting a server that others jump through asks a confirmation that names the
  dependents; confirming is the TUI equivalent of `rm --force`.
- In the form's key field, `Tab` completes the path when it can, otherwise moves on.
- Shell completion for forward names uses the server named earlier on the command line;
  if none is found it offers every forward name.
