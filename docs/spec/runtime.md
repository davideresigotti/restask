# Taskres Spec — Errors, Logging, Daemon, CLI & Setup (§12–§13)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §12 Failure Modes, Errors & Logging

### 12.1 Error taxonomy

```rust
#[derive(Debug, thiserror::Error)]
pub enum TaskresError {
    #[error("config invalid: {path}: {reason}")]
    Config { path: String, reason: String },
    #[error("parse error in {path} at byte {offset}: {reason}")]
    Parse { path: String, offset: usize, reason: String },
    #[error("uid conflict: {uid} claimed by {a} and {b}")]
    UidConflict { uid: TaskUid, a: String, b: String },      // same UID in two routed lines
    #[error("list conflict in {dir}: {a} vs {b}")]
    ListConflict { dir: String, a: String, b: String },      // two roots for one folder
    #[error("caldav {kind:?} (status {status:?}): {detail}")]
    Caldav { kind: CaldavErrorKind, status: Option<u16>, detail: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation: `{field}`: {reason}")]
    Validation { field: &'static str, reason: String },
}
pub enum CaldavErrorKind { Auth, Network, Protocol, Conflict, Tls }
```

Exit codes: `0` OK · `1` runtime failure · `2` usage (clap default) · `3` CalDAV unreachable · `4` config invalid.

### 12.2 Logging schema

Daemon: JSON lines to **stderr** (journald captures), `RUST_LOG` filter (default `info`). One-shot commands: compact human format to stderr, results to stdout. Fields: `ts` (RFC 3339 UTC), `level`, `event`, plus optional `uid`, `path`, `list`, `ms`, `count`, `detail`, `status`.

Event registry (closed set): `fs_event`, `scan_complete`, `note_routed`, `task_registered`, `task_completed`, `task_restored`, `task_deleted`, `task_adopted`, `task_moved`, `todo_rendered`, `collection_created`, `list_bound`, `caldav_push`, `caldav_delete`, `caldav_pull`, `conflict_resolved`, `sync_lag_deferred`, `retry_scheduled`, `outbox_flushed`, `orphan_subtask`, `auth_warning`, `vault_divergence`, `error`.

## §13 Runtime: Daemon, CLI & Setup Wizard

### 13.1 Daemon (`src/daemon.rs`) — single-writer reconciler

```
notify watcher (vault, recursive, *.md) ─┐
                                         ├─ mpsc channel → debounce 300 ms by path → Engine::handle_fs_change
poll timer (poll_secs, default 300) ─────┘   (full reconcile via Engine::reconcile)
```

- **One reconciler task owns all mutations.** Producers only send events. `SIGTERM`/`SIGINT` → `tokio::sync::watch` → flush state files → exit 0.
- `run_once` (used by `restask sync` and tests) performs a single full reconcile.

```rust
pub struct DaemonConfig { pub debounce_ms: u64 /*300*/, pub poll_secs: u64 /*300*/, pub once: bool }
pub async fn run_once(vault: &Path, machine: &MachineConfig, clock: std::sync::Arc<dyn Clock>) -> Result<ReconcileReport, TaskresError>;
pub async fn run(vault: PathBuf, machine: MachineConfig, dc: DaemonConfig, shutdown: tokio::sync::watch::Receiver<bool>) -> Result<(), TaskresError>;
```

### 13.2 Setup wizard (`src/setup.rs` + `src/tui.rs`) — `restask setup`

Non-destructive; every destructive step prompts. Steps:

1. **Vault**: resolve (`--vault`, else `RESTASK_VAULT`, else upward search for `restask.toml`/`.restask/`, else cwd must contain `TODO.md` or be confirmed). Write `restask.toml` (§14 defaults). Create `.restask/`.
2. **TODO.md recreation** (every run): if the inbox file exists it is **renamed** to `<stem>.pre-restask-YYYYMMDD-HHMMSS.md` (verbatim backup; scanned never — `.pre-restask-` files are skipped by the vault walk), then the fresh §7 scaffold is written. Tasks in the old file are **not migrated** (0.1.0 — the user reconciles the backup manually); tasks already on the bound calendar flow back in through the first sync.
3. **CalDAV credentials**: URL, username, password (hidden, `rpassword`). Password is stored ONLY in `~/.config/restask/radicale.passwd` (chmod 600) or referenced via env — never in `config.toml`. `PROPFIND` verifies; 401 re-prompts (3 tries).
4. **Inbox binding** (required for sync): `PROPFIND` user root → print the server's calendars, prompt `Bind TODO.md to (insert one of the calendars above):` and match the typed name case-insensitively against the collections (wrong name → error line + re-prompt; empty input never matches). Zero collections on the server aborts setup with a validation error — the user creates a calendar and re-runs. The typed calendar's slug becomes `vault.inbox_list` (§14.1), TODO.md carries it as `restask-list:` frontmatter, and the binding is recorded in machine config `[[lists]]` (name = collection slug). No vault-list discovery, no per-list prompts: other lists are declared by hand with `restask-list`/`restask-list-root` frontmatter (§5) and map to same-named collections (§5.4).
5. **First sync**: full reconcile (creates collections, registers UIDs, pushes all routed tasks).
6. **Summary**: client wiring URLs (`<url>/<user>/<slug>/`), systemd/docker next steps, Syncthing reminder (vault folder must include `.restask/`).

Flags for non-interactive use: `--url`, `--username`, `--password-env`, `--collection <list=slug>` (repeatable; a binding named `inbox` — e.g. `--collection inbox=Tasks` — binds TODO.md and sets `vault.inbox_list`), `--non-interactive` (fails rather than prompting). Server discovery via SSH was removed by owner decision — the URL and credentials are always entered manually.

### 13.3 CLI surface (`src/cli.rs`)

| Command | Purpose |
|---|---|
| `restask setup [--vault P] [--url U] [--username U] [--password-env V] [--collection L=C]... [--non-interactive]` | §13.2 wizard |
| `restask daemon [--vault P] [--once]` | §13.1 |
| `restask sync [--vault P]` | one-shot reconcile |
| `restask add "<text>" [--priority highest..lowest] [--due YYYY-MM-DD[ HH:MM]]` | append to TODO.md Inbox, register, push |
| `restask done / undone (--uid ID \| --file F --line N)` | complete/uncomplete + done-region move + push |
| `restask status [--json]` | counts by list/priority, done-today, outbox backlog, last sync |
| `restask doctor` | config, routing (incl. ListConflict), vault scan, TODO marker, CalDAV reachability, no-auth hard warning, sync-conflict files |
| `restask rebuild` | re-derive index+cache from vault (remote untouched) |
| `restask list show / create <name> / bind <name> <collection>` | manage list bindings |

`done/undone --file --line` resolve the UID by parsing that file at that line (used by the Neovim integration). `--vault` resolution order: flag → `RESTASK_VAULT` → upward search from cwd → error exit 4.
