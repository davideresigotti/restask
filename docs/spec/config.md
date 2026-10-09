# restask spec — Configuration & Security (§14, §17)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §14 Configuration

### 14.1 Vault config — `restask.toml` (vault root; synced; no secrets)

```toml
done_heading = "Done"      # heading text that starts a note's completed region
inbox_file   = "TODO.md"   # the engine-managed inbox + view (§7)
inbox_list   = "inbox"     # the list (a calendar's name, §5.4) the inbox file routes to:
                           # where a task typed there without a calendar goes;
                           # setup records the calendar you chose
todo_lists   = ["work"]    # further calendars whose tasks live in the inbox file,
                           # each line naming its own (📁 work, §7.5); default: none,
                           # and the key is then not written
todo_new_lists = true      # a calendar made in another client later is added to
                           # todo_lists by the sync node (§7.5); default true, and the
                           # key is then not written
obsidian_vault = "Obsidian"   # the vault's name in Obsidian: a task's wikilinks reach
                           # other clients as links that open the note there (§8.4);
                           # setup records the vault folder's name; absent: plain text.
                           # One name for all devices: Obsidian follows a link only
                           # into a vault of that name (on Android, case-insensitive)
track  = ["**/*.md"]       # globs of files that may be notes
ignore = [".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
```

`ignore` wins over `track`. Paths are vault-relative, `/`-separated; `*` does not cross
`/`, `**` does. Hidden files and folders (§5.1) — `.restask/`, a file sync's
`.stversions/` — are never scanned regardless of these: `track` cannot bring them in,
and they need no `ignore` entry.

### 14.2 Machine config — `$XDG_CONFIG_HOME/restask/config.toml` (0600; never synced)

**One per vault.** A machine may work on several vaults, each with its own server, its
own sync node, its own daemon; a command run in one never loads what belongs to another
(`config::machine_config_of`). A config is known by the vault its `[vault] path` names:

1. a further config, `$XDG_CONFIG_HOME/restask/vaults/<name>/config.toml`, that names
   the vault the command works on (links resolved);
2. else `config.toml` itself — when it does not exist yet, names this vault, names no
   vault, or names a folder this machine does not have (a hand-written path, a vault
   that was moved). A machine with one vault has this one file, as before;
3. else — `config.toml` is another vault's — a further config of this vault's own:
   `<name>` is the vault folder's name in lower case, runs of other characters a dash
   (`Work Notes` → `work-notes`), with `-2`, `-3` … when a vault of the same folder name
   holds that directory. Until setup writes it the vault has no machine config (the
   defaults: no server), never a borrowed one.

`RESTASK_CONFIG` names the file outright and ends the search. What lies beside a config
is per vault with it: the password file (§17), the name of the daemon's unit on
this machine (`setup::daemon_unit_name`: `restask.service` beside `config.toml`,
`restask-<name>.service` beside a further config), and the file `device` — the
identity this machine mints UIDs under in that vault (§9.4), which restask writes
itself, also on a machine that was never set up. It is no part of the config and is
never synced: copied to another machine, the two would mint the same UIDs.

```toml
[vault]
path = "/opt/docker/syncthing/data/obsidian"   # the vault this config is for; commands resolve the vault per §13.3

[caldav]
url  = "http://192.168.1.10:5232"              # use HTTPS beyond a trusted LAN
username = "me"
password_file = "~/.config/restask/radicale.passwd"   # 0600; or:
# password_env = "RESTASK_CALDAV_PASSWORD"
poll_secs = 300                                # a pass at least this often, changed or not
watch_secs = 2                                 # how often the daemon asks the server whether
                                               # another client wrote there (§13.1); 0 = never
allow_create_lists = true                      # MKCOL a routed list's missing collection
```

On an **editing machine** (§1.1) setup writes no endpoint and no password source, and a
`[node]` section instead — the mark of such a machine (§13.3):

```toml
[vault]
path = "/home/me/Vault"

[node]                                         # this vault's daemon runs elsewhere
host = "myserver"                              # ssh host of the sync node …
dir = "restask"                                # … the directory of this vault's stack there …
vault = "/srv/sync/vault"                      # … and the vault's folder there
url = "http://192.168.1.10:5232"               # the endpoint that daemon syncs with:
username = "me"                                # what `restask lists` prints URLs from
```

`host`, `dir` and `vault` are absent when the daemon was installed by hand
(`--no-daemon`). `contrib/update.sh` reads `host` and `dir` of every config of the
machine.

- A literal `password = "…"` key anywhere is a **validation error**.
- There is no list configuration: routing lives in the notes (§5). A `[[lists]]` table
  written by earlier versions is accepted and ignored.

### 14.3 Environment

`RESTASK_VAULT` · `RESTASK_CONFIG` (machine config path) · `RESTASK_CALDAV_URL` ·
`RESTASK_CALDAV_USERNAME` · `RESTASK_CALDAV_PASSWORD` · `RUST_LOG` · `RESTASK_SOURCE`
(the restask sources setup builds a node's daemon from; default: the clone the binary
was built from) · `RESTASK_NODE_WAIT` (seconds `contrib/node.sh` waits for the file sync,
default 300). Environment values
override the file; `RESTASK_CALDAV_PASSWORD` wins over `password_env`, which wins over
`password_file`.

## §17 Security

- **Passwords** live only in `radicale.passwd` (0600) beside the vault's machine config
  (§14.2: `~/.config/restask/`, a further vault's directory under `vaults/` there) or an environment
  variable — never in `config.toml`, never in the vault, never in logs (the
  `Authorization` header is not logged; `CaldavClient`'s `Debug` redacts the password).
  Only the sync node has one. Setup hands it to a node through standard input — of
  `contrib/node.sh`, of `ssh`, of the container's `restask setup --password-stdin` —
  never in an argument list or a temporary file, and in memory it is a `setup::Secret`,
  whose `Debug` prints a placeholder.
- `restask doctor` warns hard (exit 1) when no password is configured, and when the
  server accepts a deliberately wrong password (authentication disabled server-side).
- Plain HTTP is acceptable on a trusted LAN only.
- The engine writes only inside the vault and `~/.config/restask/`; server-supplied
  paths (`X-RESTASK-SOURCE`) are used only when they name a note the scan already found
  routed to that list.
- On the server the engine touches only `VTODO` resources in the lists in scope (§5.4,
  §10.5).
