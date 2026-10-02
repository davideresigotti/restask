# restask spec — Configuration & Security (§14, §17)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §14 Configuration

### 14.1 Vault config — `restask.toml` (vault root; synced; no secrets)

```toml
done_heading = "Done"      # heading text that starts a note's completed region
inbox_file   = "TODO.md"   # the engine-managed inbox + view (§7)
inbox_list   = "inbox"     # the list (= collection slug) the inbox file routes to:
                           # where a task typed there without a calendar goes;
                           # setup records the calendar you chose
todo_lists   = ["work"]    # further calendars whose tasks live in the inbox file,
                           # each line naming its own (📁 work, §7.5); default: none,
                           # and the key is then not written
track  = ["**/*.md"]       # globs of files that may be notes
ignore = [".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
```

`ignore` wins over `track`. Paths are vault-relative, `/`-separated; `*` does not cross
`/`, `**` does. Hidden files and folders (§5.1) — `.restask/`, a file sync's
`.stversions/` — are never scanned regardless of these: `track` cannot bring them in,
and they need no `ignore` entry.

### 14.2 Machine config — `$XDG_CONFIG_HOME/restask/config.toml` (0600; never synced)

```toml
[vault]
path = "/opt/docker/syncthing/data/obsidian"   # informational; commands resolve the vault per §13.3

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
dir = "restask"                                # … the directory of its stack there …
vault = "/srv/sync/vault"                      # … and the vault's folder there
url = "http://192.168.1.10:5232"               # the endpoint that daemon syncs with:
username = "me"                                # what `restask lists` prints URLs from
```

`host`, `dir` and `vault` are absent when the daemon was installed by hand
(`--no-daemon`). `contrib/update.sh` reads `host` and `dir`.

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

- **Passwords** live only in `~/.config/restask/radicale.passwd` (0600) or an environment
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
