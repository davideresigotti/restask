# Taskres Spec — Configuration & Security (§14, §17)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §14 Configuration Reference

### 14.1 Vault config — `restask.toml` (vault root, synced, safe to commit)

```toml
done_heading = "Done"      # heading text that starts the completed-records region
inbox_file   = "TODO.md"   # engine-managed inbox + aggregation view
track  = ["**/*.md"]       # globset patterns (applied after ignore)
ignore = [".restask/**", ".obsidian/**", ".trash/**", ".git/**"]
```

`ignore` wins over `track`. Vault-relative paths only.

### 14.2 Machine config — `$XDG_CONFIG_HOME/restask/config.toml` (chmod 600, NEVER synced)

```toml
# vault (optional; usually discovered per §13.3)
[vault]
path = "/opt/docker/syncthing/data/obsidian"

[caldav]
url  = "http://192.168.1.10:5232"   # LAN example; use HTTPS when exposed beyond LAN
username = "me"
password_file = "~/.config/restask/radicale.passwd"  # chmod 600; OR password_env below
# password_env = "RESTASK_CALDAV_PASSWORD"            # alternative source
poll_secs = 300
allow_create_lists = true           # MKCOL missing collections on first push

# Wizard-recorded bindings (list display name → Radicale collection)
[[lists]]
name = "Inbox"
collection = "inbox"

[[lists]]
name = "University"
collection = "university"
```

A literal `password = "…"` key is a **validation error** — secrets never live in config files.

### 14.3 Environment variables

`RESTASK_VAULT` (vault path) · `RESTASK_CONFIG` (machine config path override) · `RESTASK_CALDAV_URL` · `RESTASK_CALDAV_USERNAME` · `RESTASK_CALDAV_PASSWORD` · `RUST_LOG`. Env caldav values override the file; the password env always wins over `password_file`.

## §17 Security & Secrets

- **Passwords** live only in `~/.config/restask/radicale.passwd` (0600) or an env var. Never in `config.toml`, never in the vault, never in logs (the `Authorization` header is never logged).
- **FINDING (2026-09-22, maintainer lab):** Radicale at `http://192.168.1.10:5232` accepts any/no credentials (`PROPFIND` returns 207 with a wrong password). Auth is disabled in the container config (`/config/config`). **Fix before exposing further.** `restask doctor` detects and hard-warns on unauthenticated CalDAV.
- Plain HTTP on a trusted LAN is accepted; beyond the LAN, HTTPS is required (documented in INSTALL.md).
- The engine only ever touches VTODO resources it owns (or adopts) inside collections bound to lists — see §10.5.
