# restask spec — Configuration & Security (§14, §17)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §14 Configuration

### 14.1 Vault config — `restask.toml` (vault root; synced; no secrets)

```toml
done_heading = "Done"      # heading text that starts a note's completed region
inbox_file   = "TODO.md"   # the engine-managed inbox + view (§7)
inbox_list   = "inbox"     # the list (= collection slug) the inbox file routes to;
                           # setup records the calendar you chose
track  = ["**/*.md"]       # globs of files that may be notes
ignore = [".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
```

`ignore` wins over `track`. Paths are vault-relative, `/`-separated; `*` does not cross
`/`, `**` does. `.restask/` is never scanned regardless of these.

### 14.2 Machine config — `$XDG_CONFIG_HOME/restask/config.toml` (0600; never synced)

```toml
[vault]
path = "/opt/docker/syncthing/data/obsidian"   # informational; commands resolve the vault per §13.3

[caldav]
url  = "http://192.168.1.10:5232"              # use HTTPS beyond a trusted LAN
username = "me"
password_file = "~/.config/restask/radicale.passwd"   # 0600; or:
# password_env = "RESTASK_CALDAV_PASSWORD"
poll_secs = 300                                # how often the daemon looks at the server
allow_create_lists = true                      # MKCOL a routed list's missing collection
```

- A literal `password = "…"` key anywhere is a **validation error**.
- There is no list configuration: routing lives in the notes (§5). A `[[lists]]` table
  written by earlier versions is accepted and ignored.

### 14.3 Environment

`RESTASK_VAULT` · `RESTASK_CONFIG` (machine config path) · `RESTASK_CALDAV_URL` ·
`RESTASK_CALDAV_USERNAME` · `RESTASK_CALDAV_PASSWORD` · `RUST_LOG`. Environment values
override the file; `RESTASK_CALDAV_PASSWORD` wins over `password_env`, which wins over
`password_file`.

## §17 Security

- **Passwords** live only in `~/.config/restask/radicale.passwd` (0600) or an environment
  variable — never in `config.toml`, never in the vault, never in logs (the
  `Authorization` header is not logged; `CaldavClient`'s `Debug` redacts the password).
- `restask doctor` warns hard (exit 1) when no password is configured, and when the
  server accepts a deliberately wrong password (authentication disabled server-side).
- Plain HTTP is acceptable on a trusted LAN only.
- The engine writes only inside the vault and `~/.config/restask/`; server-supplied
  paths (`X-RESTASK-SOURCE`) are used only when they name a note the scan already found
  routed to that list.
- On the server the engine touches only `VTODO` resources in the lists in scope (§5.4,
  §10.5).
