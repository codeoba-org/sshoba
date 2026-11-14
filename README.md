# sshoba

A terminal UI and CLI for managing SSH servers, for macOS and Linux.

Save servers once, then connect, check reachability, run port-forward tunnels and move
files with one keypress or one command.

- **Smart connect**: inside tmux, Zellij, kitty or WezTerm a new tab opens running ssh;
  otherwise sshoba replaces itself with ssh in the current terminal.
- **Jump hosts** (ProxyJump chains) where every hop can have its own user, port and key.
- **Live reachability**: TCP + SSH-banner ping next to every server.
- **Port forwards** (local, remote, SOCKS) and detached background tunnels.
- **File transfers** with rsync (or scp) and a two-pane local/remote file browser.
- Every TUI action is also a CLI command, with `--json` output for scripts.

sshoba never implements SSH itself: it runs your system `ssh`, `scp`, `rsync` and `sftp`,
so keys, ssh-agent, `known_hosts`, `~/.ssh/config` and ControlMaster keep working. It
never edits `~/.ssh/config` and never writes passwords to its files: password logins
work as usual (ssh asks in the terminal), or you can save a password in the OS keychain
with `sshoba password set <name>` so sshoba logs in for you.

## Install

With Homebrew (macOS and Linux):

```bash
brew install codeoba-org/tap/sshoba
```

From source, with a Rust toolchain (1.85 or newer):

```bash
cargo install --locked --git https://github.com/codeoba-org/sshoba
```

Or download a binary for your platform from the
[releases page](https://github.com/codeoba-org/sshoba/releases).

Requirements: OpenSSH (`ssh`). Optional: `rsync` (preferred for transfers), `scp`,
`tmux` / `zellij` / kitty / WezTerm for tabs.

## Quick start

```bash
sshoba import ssh-config --dry-run
```

```bash
sshoba import ssh-config
```

```bash
sshoba add web ata@203.0.113.10 -i ~/.ssh/id_ed25519 --tag prod
```

```bash
sshoba add db db@10.0.0.5 -J web
```

```bash
sshoba
```

In the TUI, pick a server and press `⏎` to connect. Or from the shell:

```bash
sshoba c db
```

## The TUI

```
┌ sshoba ─ 12 servers ─ tmux ──────────────────────── 2 tunnels up ┐
│ / search: prod_                                                 │
├──────────────────────────────────┬─────────────────────────────┤
│ ▾ work                           │ db-prod            ★        │
│   ●  23ms  bastion      infra    │ postgres@10.0.0.5:22        │
│ ❯ ●  41ms  db-prod  ★   db prod  │ via bastion › db-prod       │
│   ○  down  web-2        web      │ forwards:                   │
│ ▾ personal                       │   pg  L 5432→localhost:5432 │
│   ●  12ms  atasoy                │ $ ssh -J ata@203… postgres… │
├──────────────────────────────────┴─────────────────────────────┤
│ ⏎ connect  a add  e edit  d del  p ping  t tunnels  f files  ? help │
└───────────────────────────────────────────────────────────────┘
```

Servers are grouped by `group` (favorites first, then most recently used). The dot shows
reachability: green ● up, red ○ down, grey · unknown, yellow ◌ checking. Results from a
previous run are dimmed until refreshed. The details pane hides below 90 columns
(toggle with `i`).

### Keys

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | Move selection |
| `g` / `G` | First / last |
| `/` | Fuzzy search over name, host, tags and group (`Esc` clears) |
| `⏎` | Connect |
| `o` / `Shift+⏎` | Connect with the server's `auto` forwards |
| `a` / `e` | Add / edit server (form) |
| `E` | Edit `servers.toml` in `$EDITOR` |
| `d` / `r` | Delete / rename |
| `s` | Toggle favorite |
| `p` / `P` | Ping selected (full check through jumps) / ping all |
| `t` / `T` | Tunnels for the selected server / all tunnels |
| `f` | Two-pane file browser |
| `x` | Quick send / get |
| `y` | Copy the ssh command to the clipboard (OSC 52) |
| `Space` | Collapse / expand group |
| `i` | Toggle details pane |
| `I` | Import from `~/.ssh/config` |
| `?` | Help (lists the keys of the current screen) |
| `q` / `Ctrl+C` | Quit |

**Form**: `Tab` / `Shift+Tab` move between fields, `Ctrl+S` saves, `Esc` cancels. Paste
`user@host:port` into host to split it. `Tab` in the key field completes the path. `⏎` in
the jump field opens an ordered picker (`Space` picks, `J` / `K` reorder).

**Tunnels screen**: `⏎` starts/stops, `a` adds a forward (`pg -L 5432:localhost:5432`),
`d` deletes, `Esc` goes back.

**File browser**: `Tab` switches pane, `⏎` opens a directory, `Backspace` goes up,
`Space` marks, `c` copies marked files to the other pane, `r` refreshes, `.` toggles
hidden files.

## CLI reference

Run `sshoba <command> --help` for details. Global flags: `--config <path>` (use another
servers.toml), `-q` / `--quiet`.

| Command | What it does |
| --- | --- |
| `sshoba` | Open the TUI |
| `sshoba connect <query> [--here \| --tab] [--with-forwards]` (`c`) | Connect |
| `sshoba list [--tag T] [--group G] [--fav] [--json]` (`ls`) | List servers |
| `sshoba add <name> <[user@]host[:port]> [-i KEY] [-J a,b] [--tag T]... [--group G] [--note N]` | Add a server |
| `sshoba edit <name>` | Edit servers.toml in `$EDITOR` at that server; validated on close |
| `sshoba set <name> <field> <value>` | Change one field (`set db port 2222`; empty value clears) |
| `sshoba rename <old> <new>` | Rename, updating jump references |
| `sshoba rm <name> [--force] [-y]` | Delete (refused for a used jump host without `--force`) |
| `sshoba show <name> [--json]` | Server details, route and ssh commands |
| `sshoba cmd <name>` | Print the exact ssh command |
| `sshoba ping [<query>...] [--all] [--json]` | Reachability check |
| `sshoba fwd add <server> <name> (-L \| -R \| -D) <spec> [--auto]` | Save a forward |
| `sshoba fwd ls [<server>] [--json]` / `fwd rm <server> <name>` | List / delete forwards |
| `sshoba tunnel up <server> [<forward>...] [--all]` | Start detached tunnels |
| `sshoba tunnel down <server> [<forward>...]` / `tunnel down --all` | Stop tunnels |
| `sshoba tunnel ls [--json]` | Running tunnels |
| `sshoba send <server> <local>... [<remote_dir>]` | Upload (default remote dir `~`) |
| `sshoba get <server> <remote>... [<local_dir>]` | Download (default local dir `.`) |
| `sshoba import ssh-config [--file F] [--dry-run]` | Import Host blocks |
| `sshoba export ssh-config [--file F]` | Write `~/.ssh/config.d/sshoba` for plain `ssh <name>` |
| `sshoba config path` | Print config, state and log paths |
| `sshoba completions <bash\|zsh\|fish>` | Print a completion script |

`<query>` is an exact name or a fuzzy match on name, host, tags and group. If several
servers match, a picker appears on a terminal; in scripts the command exits with code 3.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | General error |
| 2 | Server or forward not found |
| 3 | Ambiguous query |
| 4 | Invalid config |
| 5 | Server unreachable (ping) |
| 6 | Required program missing |
| other | `connect` in the current terminal returns ssh's own exit code |

### Shell completion

Completions include your server names and forwards.

```bash
echo 'source <(sshoba completions bash)' >> ~/.bashrc
```

```bash
echo 'source <(sshoba completions zsh)' >> ~/.zshrc
```

```bash
sshoba completions fish > ~/.config/fish/completions/sshoba.fish
```

## Configuration

Servers live in `~/.config/sshoba/servers.toml` (also on macOS; override the directory
with `$SSHOBA_CONFIG_DIR` or `$XDG_CONFIG_HOME`). The file is meant to be edited by hand
and kept in git: sshoba keeps your comments and ordering when it saves, writes atomically
and keeps a `servers.toml.bak`. Runtime data (last connected, ping cache, running
tunnels) lives separately in `~/.local/state/sshoba/state.json` (`$XDG_STATE_HOME`).

```toml
[settings]
launch = "auto"          # auto | tmux | zellij | kitty | wezterm | current
stay_open = true         # keep the TUI open after opening a tab
ping_interval = 30       # seconds; 0 disables auto ping
ping_timeout_ms = 1500
transfer_tool = "auto"   # auto | rsync | scp
control_master = true    # reuse connections via ControlMaster
ssh_bin = "ssh"          # override the ssh path if needed

[[server]]
name = "bastion"           # unique: letters, digits, . _ - (max 64)
host = "203.0.113.10"
user = "ata"               # optional
port = 22                  # optional
key = "~/.ssh/id_ed25519"  # optional; this hop uses it even inside a jump chain
group = "work"             # optional
tags = ["infra"]           # optional
note = "Main jump box"     # optional

[[server]]
name = "db-prod"
host = "10.0.0.5"
user = "postgres"
jump = ["bastion"]         # jump hosts by name, in hop order
favorite = true
extra_args = ["-o", "ServerAliveInterval=30"]

  [[server.forward]]
  name = "pg"
  kind = "local"           # local | remote | socks
  bind = "127.0.0.1"       # optional
  local_port = 5432
  remote = "localhost:5432"  # host:port; omit for socks
  auto = false             # add to `sshoba c db-prod --with-forwards`
```

Every load and save is validated: unique names, existing jump hosts, no jump cycles
(max 8 hops), valid forwards. Errors name the server and field:

```
servers.toml: server "db-prod": jump "bastoin" not found (did you mean "bastion"?)
```

### How connections are made

Before each ssh, scp, rsync or sftp call sshoba regenerates
`~/.local/state/sshoba/ssh_config` (only when servers.toml changed) and runs
`ssh -F <that file> sshoba.<name>`. Each server becomes a `Host sshoba.<name>` block,
so per-hop users, ports and keys work in jump chains. The file ends with
`Include ~/.ssh/config` and `Include /etc/ssh/ssh_config`, so your own settings
(`Host *` defaults, agent, etc.) still apply. `sshoba cmd <name>` prints the exact
command; `sshoba show <name>` also prints a readable `ssh -J ...` equivalent.

To use your servers with plain `ssh <name>` (and other tools), run
`sshoba export ssh-config` and add the printed `Include` line to the top of
`~/.ssh/config`.

## Logs

sshoba never prints while the TUI is open; it logs to `~/.local/state/sshoba/sshoba.log`.
Set `SSHOBA_LOG=debug` for more detail. Background tunnels log to
`~/.local/state/sshoba/tunnels/<server>__<forward>.log`.

## FAQ

**Can sshoba remember my password?**
Yes: `sshoba password set <name>` asks for it once and stores it in the macOS Keychain
(or the Secret Service, e.g. GNOME Keyring, on Linux), keyed by `user@host`. Connects
(CLI and TUI) and background tunnels then log in with it through ssh's `SSH_ASKPASS`;
jump hops with a saved password work too. If the server rejects it, ssh asks in the
terminal as usual. `sshoba password rm <name>` deletes it. Requires OpenSSH 8.4 or
newer. An SSH key (`ssh-copy-id`) is still the safer and faster option when you can
use one.

**My server only accepts passwords. Can I use tunnels?**
Yes, if you save the password (see above). Otherwise background tunnels run with
`BatchMode=yes` and need key or ssh-agent authentication: mark the forwards
`auto = true` and connect with `sshoba c <name> --with-forwards` (or `o` in the TUI) so
the forwards run inside your interactive session, where ssh can ask for the password.

**A background command says "host key not verified".**
Connect once interactively (`sshoba c <name>`) and accept the host key. sshoba never
disables host key checking.

**Transfers on macOS show per-file progress instead of a total.**
macOS ships openrsync (compatible with rsync 2.6.9), which lacks `--info=progress2`.
sshoba detects this and falls back to per-file progress; paths with spaces still work.
Install rsync 3.x (`brew install rsync`) for whole-transfer progress and
`--protect-args`, or set `transfer_tool = "scp"`.

**kitty doesn't open a tab.**
`kitty @ launch` needs `allow_remote_control yes` (or `socket-only` with
`listen_on`) in `kitty.conf`. Otherwise set `launch = "current"`.

**Does sshoba touch `~/.ssh/config`?**
Only reads it (for `import ssh-config`, and through `Include` when connecting). It is
never written; `export ssh-config` writes a separate file.

## Development

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Tests never touch your real `~/.ssh` or `~/.config`. Design choices made where the plan
was silent are recorded in [DECISIONS.md](DECISIONS.md).
