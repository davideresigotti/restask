# restask spec — Errors, Logging, Daemon, CLI & Setup (§12–§13)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §12 Errors and logging

### 12.1 Error taxonomy

```rust
pub enum RestaskError {
    Config { path, reason },
    ListConflict { dir, a, b },          // two list roots declared for one folder
    Caldav { kind: CaldavErrorKind, status: Option<u16>, detail },
    Io(std::io::Error),
    Validation { field: &'static str, reason },
}
pub enum CaldavErrorKind { Auth, Network, Protocol, Conflict, Tls }
```

Exit codes: `0` OK · `1` runtime failure · `2` usage (clap) · `3` CalDAV unreachable ·
`4` config invalid / no vault.

### 12.2 Logging

Everything goes to **stderr**; command results go to stdout.

- `restask daemon`: JSON lines at `info` (journald-friendly).
- One-shot commands: compact human lines at `warn`.
- `RUST_LOG` overrides the level in both.

Event names used as messages: `scan_complete`, `task_registered`, `task_completed`,
`task_restored`, `task_deleted`, `task_adopted`, `task_moved`, `todo_rendered`,
`collection_created`, `collection_reset`, `caldav_push`, `caldav_delete`, `caldav_pull`,
`conflict_resolved`, `sync_lag_deferred`, `vault_divergence`, `orphan_subtask`,
`auth_warning`.

## §13 Runtime

### 13.1 Daemon (`daemon.rs`)

```
notify watcher (vault, recursive) ──┐
                                    ├─▶ "dirty" + debounce (300 ms) ─▶ one Engine::reconcile
poll timer (caldav.poll_secs, 300) ─┘
```

- A burst of file events — an editor saving, file sync delivering a batch — coalesces
  into **one** pass; whatever happened, one pass covers it.
- Only events that can change a scan wake the loop: tracked notes and directories.
  Events under `.restask/`, on ignored paths, on temp files, on setup backups and on
  conflict copies do not — so the engine's own state writes never re-trigger it, and its
  note writes cause at most one follow-up pass, which is a no-op.
- The first poll tick is immediate (reconcile on start); later ticks pick up server-side
  changes.
- A failed pass is logged (`auth_warning` for rejected credentials) and never ends the
  daemon.
- `SIGTERM` / `SIGINT` set a `tokio::sync::watch` flag; an in-flight pass finishes, then
  the loop exits 0.

```rust
pub struct DaemonConfig { pub debounce_ms: u64, pub poll_secs: u64, pub once: bool }
pub async fn run_once(vault, machine, clock) -> Result<ReconcileReport, RestaskError>;
pub async fn run(vault, machine, dc, shutdown: watch::Receiver<bool>) -> Result<(), RestaskError>;
// *_with variants take an injected CaldavPort (and clock) for hermetic tests
```

### 13.2 Setup wizard (`setup.rs`, `tui.rs`) — `restask setup`

1. **Vault**: `--vault`, else `RESTASK_VAULT`, else an upward search for `restask.toml` /
   `.restask/`, else the working directory (confirmed interactively; non-interactive
   requires a `TODO.md` there). Writes `restask.toml` if missing; creates `.restask/`.
2. **TODO.md** (every run): an existing inbox file is **renamed** to
   `<stem>.pre-restask-YYYYMMDD-HHMMSS.md` and a fresh §7 scaffold is written. Its lines
   are not migrated: unsynced captures stay in the backup. So that the vanished lines are
   not read as deletions, the sync state of the inbox tasks is forgotten with the file —
   tasks already on the bound calendar flow back in with the first sync.
3. **Credentials**: URL, username, password (hidden). The password goes only to
   `~/.config/restask/radicale.passwd` (0600) or is referenced via an env var — never to
   `config.toml`. A `PROPFIND` verifies; `401` re-prompts (3 tries).
4. **Inbox binding**: the server's calendars are listed; the user types the one TODO.md
   binds to (case-insensitive; a miss re-prompts; no calendars at all aborts). Its slug
   becomes `vault.inbox_list`. Every other list is declared in notes (§5).
5. **First sync.**
6. **Daemon unit**: with a systemd user session, writes
   `$XDG_CONFIG_HOME/systemd/user/restask.service` (absolute, quoted
   `ExecStart=<this binary> daemon --vault <vault>`, `Restart=on-failure`), runs
   `systemctl --user daemon-reload` and `enable --now`, and enables linger. No systemd,
   or a failure, is a warning — never a failed setup. The installer is an injectable port
   (`DaemonInstaller`); tests pass `None`.
7. **Summary.**

Non-interactive: `--url`, `--username`, `--password-env`, `--collection name=collection`
(repeatable; `inbox=<calendar>` sets the inbox binding, any other pair just creates that
collection), `--non-interactive`.

### 13.3 CLI (`cli.rs`)

| Command | Purpose | Server |
|---|---|---|
| `restask setup […]` | §13.2 | required |
| `restask daemon [--once]` | §13.1 | required |
| `restask sync` | one pass; prints the report | required |
| `restask add "<text>" [--priority P] [--due D]` | new inbox task, then a pass | optional |
| `restask done \| undone (--uid ID \| --file F --line N)` | complete / reopen in the source file, then a pass | optional |
| `restask status [--json]` | active per list / priority, done today, pending sync, last sync | none |
| `restask lists` | routed lists: slug, active count, home note, collection URL | none |
| `restask doctor` | config, routing, scan (duplicates, conflict copies), marker, server reachability and auth | probed |
| `restask rebuild` | drop index, base snapshots and remembered render (tombstones stay) | none |

- "optional": the vault-side change is made and kept even if the server is unreachable or
  not configured on this machine; a warning says the server will catch up.
- `--vault` resolution: flag → `RESTASK_VAULT` → upward search from the working directory.
  `done/undone --file <absolute path>` search upward from the file instead, so editor
  integrations work from any directory.
- `status`' *pending* counts tasks whose current vault content the server has not
  confirmed.
- `doctor` exit codes: `4` config invalid, `3` server unreachable, `1` a broken check or a
  server that accepts wrong credentials, `0` otherwise. Duplicated lines and conflict
  copies are warnings (the former are repaired by the next pass).
