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
`task_restored`, `task_deleted`, `task_adopted`, `task_moved`, `task_recurred`,
`todo_rendered`,
`collection_created`, `collection_reset`, `caldav_push`, `caldav_delete`, `caldav_pull`,
`conflict_resolved`, `sync_lag_deferred`, `vault_divergence`, `orphan_subtask`,
`auth_warning`, `server_changed`.

## §13 Runtime

### 13.1 Daemon (`daemon.rs`)

```
notify watcher (vault, recursive) ───┐
server watch (caldav.watch_secs, 2) ─┼─▶ "dirty" + debounce (300 ms) ─▶ one Engine::reconcile
poll timer (caldav.poll_secs, 300) ──┘
```

- A burst of file events — an editor saving, file sync delivering a batch — coalesces
  into **one** pass; whatever happened, one pass covers it.
- Only events that can change a scan wake the loop: tracked notes and directories.
  Events under `.restask/`, on ignored paths, on temp files, on setup backups and on
  conflict copies do not — so the engine's own state writes never re-trigger it, and its
  note writes cause at most one follow-up pass, which is a no-op.
- **Server watch.** The server sends no events, so the daemon asks: every
  `caldav.watch_secs` one `PROPFIND` (the collection listing of §10.2) returns the change
  tag (`getctag`) of every task collection, and an answer that differs from the previous
  one (`server_tags`) marks the vault dirty, logged as `server_changed`. A task added,
  changed or deleted in another client is therefore in the vault within the interval, not
  at the next poll. The watch only starts a pass; what changed is found by the pass, from
  full snapshots as always, and no decision rests on a tag. The first look is taken
  before the start-up pass, so a write that lands during that pass is not missed. A look
  that fails is skipped. The daemon's own writes change the tags too: a pass that pushed
  is followed by one more, which finds nothing to do (invariant 8). `watch_secs = 0`, or
  a server that reports no change tags, leaves server-side changes to the poll.
  The default is 2 s: the look is one small request, and the interval is the whole of
  the daemon's share in how long a server-side change takes to show (the rest is the
  other client's upload and the file sync's own delay, INSTALL *Latency*).
- The first poll tick is immediate (reconcile on start); later ticks are the net under
  the two watches — a missed file event, a server without change tags.
- A failed pass is logged (`auth_warning` for rejected credentials) and never ends the
  daemon.
- `SIGTERM` / `SIGINT` set a `tokio::sync::watch` flag; an in-flight pass finishes, then
  the loop exits 0.

```rust
pub struct DaemonConfig { pub debounce_ms: u64, pub poll_secs: u64, pub watch_ms: u64, pub once: bool }
pub fn server_tags(collections: &[CollectionInfo]) -> Vec<(String, String)>;   // (slug, ctag), sorted
pub async fn run_once(vault, machine, clock) -> Result<ReconcileReport, RestaskError>;
pub async fn run(vault, machine, dc, shutdown: watch::Receiver<bool>) -> Result<(), RestaskError>;
// *_with variants take an injected CaldavPort (and clock) for hermetic tests
```

### 13.2 Setup wizard (`setup.rs`, `tui.rs`) — `restask setup`

1. **Vault**: `--vault`, else `RESTASK_VAULT`, else an upward search for `restask.toml` /
   `.restask/`, else the working directory (confirmed interactively; non-interactive
   requires a `TODO.md` there). Writes `restask.toml` if missing; creates `.restask/`.
   Then installs the **Obsidian plugin** (`install_obsidian_plugin`): the bundle compiled
   into the binary (App. C) is written to `.obsidian/plugins/restask/` (`main.js`,
   `manifest.json`, `styles.css`) and `"restask"` is added to
   `.obsidian/community-plugins.json`. A re-run refreshes those three files and nothing
   else: other entries of the list keep their place, an existing `data.json` (the user's
   plugin settings) is never written, unchanged files are not rewritten. `data.json` is
   created only when it is missing and `done_heading` is not the default, holding
   `doneHeading` (§15.4). A plugin folder or file that is a symlink is a hand-managed
   development install and is left alone. A `community-plugins.json` that is not a JSON
   list is left untouched and the summary asks to enable the plugin by hand. Any failure
   here is a warning — never a failed setup. The result (`PluginInstall`: listed or
   not, and whether this run wrote any of it) is what step 7 works from.
2. **TODO.md** (every run): an existing inbox file is **renamed** to
   `<stem>.pre-restask-YYYYMMDD-HHMMSS.md` and a fresh §7 scaffold is written. Its lines
   are not migrated: unsynced captures stay in the backup. So that the vanished lines are
   not read as deletions, the sync state of the inbox tasks is forgotten with the file —
   tasks already on the bound calendar flow back in with the first sync.
3. **Credentials**: URL, username, password (hidden) — asked once, whichever machine
   ends up using them. A `PROPFIND` verifies; `401` re-prompts (3 tries). On the sync
   node the password goes only to `~/.config/restask/radicale.passwd` (0600) or is
   referenced via an env var — never to `config.toml`. On an editing machine (step 6)
   it is kept nowhere: it is used for this run's requests, handed to the node's
   installer, and a password file an earlier setup left beside the config is removed.
4. **Inbox binding**: the server's calendars are listed; the user types the one TODO.md
   binds to (case-insensitive; a miss re-prompts; no calendars at all aborts). Its slug
   becomes `vault.inbox_list`. Every other list is declared in notes (§5).
5. **First sync** — from this machine, also when the daemon goes elsewhere: the vault
   leaves it converged, and the node's first pass finds nothing to do in it.
6. **The daemon** (`SetupArgs::daemon`, a `DaemonHost`). The interactive wizard asks
   for the ssh host of an always-on server that holds a copy of the vault (Enter: this
   computer) and for the vault's folder there; flags answer instead (`DaemonFlags`).
   A server is **connected to as soon as it is named** (`connect_node`): one ssh
   master connection is opened, on the terminal, so ssh itself asks whatever it needs
   to log in — a password where no key is set up, a key's passphrase, a new host's
   fingerprint — right after the host and before the next question, once. Everything
   setup then does on the server goes through that connection; it is closed when the
   run ends. restask never sees or stores what was typed there. A typed host that
   cannot be logged in to is asked for again (Enter: this computer); one given by
   `--node` fails the run. Without a terminal ssh can ask nothing: an unattended run
   needs a key.
   - **`Here`** (no server; the default without a terminal). With a systemd user
     session, writes `$XDG_CONFIG_HOME/systemd/user/restask.service` (absolute, quoted
     `ExecStart=<this binary> daemon --vault <vault>`, `Restart=on-failure`), runs
     `systemctl --user daemon-reload` and `enable --now`, and enables linger. No
     systemd, or a failure, is a warning — never a failed setup.
   - **`Node`** (`--node <ssh-host> --node-vault <path> [--node-dir <dir>]`). Before
     step 1 — before anything is written — `prepare_node` checks the host (ssh, Docker
     with compose, the vault folder, a stack directory that is free or serves this
     vault) and stops a daemon running there, so that this machine's first sync is the
     only pass while the vault changes; a URL that names this machine (`localhost`,
     `127.x`, `::1`) is refused, the node could not reach it. After step 5
     `install_node` creates the stack, builds the image from the restask sources,
     waits until the file sync has made the node's copy of the vault equal to this one,
     joins the vault there with the credentials of step 3 and starts the daemon
     (`contrib/node.sh`, App. E). A failure is the run's error and names the command
     that finishes the job (`restask setup --join --node …`); nothing syncs until it
     does.
   - **`Elsewhere`** (`--no-daemon`): the daemon is installed by hand on another
     machine (`restask setup --join` there). A unit an earlier run installed here is
     neither refreshed nor removed.

   With `Node` and `Elsewhere` this is an *editing machine* (§1.1): its machine config
   gets a `[node]` section and no credentials (§14.2), written only once the daemon is
   in place, so a run that failed is repeated — a plain `restask setup` then joins —
   and not taken for done. Whichever machine this is, setup creates the directory of
   the machine config when it is missing (a computer restask was never set up on has
   none). The installer is an injectable port (`DaemonInstaller`:
   `install`, `connect_node`, `prepare_node`, `install_node`; the real one is
   `SystemInstaller`); tests record instead.
7. **The plugin in a running Obsidian** (`SetupSummary::load_plugin`,
   `load_obsidian_plugin`). Obsidian reads a vault's plugins when it opens the vault
   and never again, and it writes the list back from memory when a plugin is switched
   in its settings: under an Obsidian that has the vault open, step 1 alone leaves the
   plugin off. So when step 1 wrote something (a plugin file, the entry in the list)
   and an Obsidian on this computer has the vault open, that Obsidian is made to load
   it — last, when the vault is converged:
   - *Is it open?* The vault's entry in Obsidian's own `obsidian.json` (in its
     configuration folder; also the Flatpak's) is marked `open`, and the socket of
     Obsidian's command line interface (`$XDG_RUNTIME_DIR/.obsidian-cli.sock`) takes
     a connection. No such Obsidian: nothing is done — it reads the list when it
     opens the vault.
   - *Its command line interface is on* (Obsidian ≥ 1.12, Settings → General):
     setup sends what Obsidian's own `obsidian` command sends — one JSON line
     (`argv`, `tty: false`, `cwd`) with the vault named by its id — and nothing
     closes. `plugins:restrict` first: in restricted mode nothing is loaded, the
     summary says how to turn it off (that decision is the user's). Then the
     gentlest command that works: `plugin:reload id=restask` (it was on; its new
     files are loaded), `plugin:enable id=restask filter=community` (Obsidian knows
     the folder, it was off), `reload` (the folder is new to this Obsidian: the
     vault's window is reloaded and reads the list).
   - *It is off* (the default): Obsidian must start again. The interactive wizard
     asks — `Restart Obsidian now? [Y/n]`, Enter is yes — because every window of
     the user's closes; a non-interactive run never restarts. On Linux the restart
     is: `SIGTERM` to Obsidian's main process (executable `obsidian`, no `--type=`
     argument; exactly one of this user's), wait up to 15 s for it to end — it is
     never killed —, then the same program with the same arguments and working
     directory, in the wizard's environment, in a process group of its own;
     Obsidian reopens the vaults it had open. Not restarted, with the reason in the
     output: a terminal outside the desktop session (no `WAYLAND_DISPLAY` /
     `DISPLAY`), an AppImage or sandboxed install (its program is not a file that
     can be started again), two Obsidians, another system than Linux.

   Nothing here fails setup; what was not done is in the summary (`PluginLoad`:
   `NotNeeded`, `Loaded`, `Reloaded`, `Restarted`, `Restricted`, `Pending`). A join
   installs no plugin and does none of this. The app is an injectable port
   (`ObsidianApp`: `has_open`, `command`, `restart`; the real one is
   `SystemObsidian`); tests record instead. Other devices are out of reach: a phone
   loads the plugin when Obsidian starts there.
8. **Summary.**

Non-interactive: `--url`, `--username`, a password source (`--password-env <VAR>`, or
`--password-stdin`: one line on standard input, stored in the machine's password file
when the daemon runs here), `--collection name=collection` (repeatable;
`inbox=<calendar>` sets the inbox binding, any other pair just creates that collection),
`--non-interactive`. `--node` (then `--node-vault` is required), `--node-dir` and
`--no-daemon` apply to interactive and non-interactive runs, and to a join.

**Joining** (`join_and_sync`) — a second machine, typically the always-on server. The
vault it serves was set up elsewhere and arrived through the file sync; running steps 1,
2 and 4 there would replace TODO.md on every device. A run *joins* (`setup::joins`) when

- `--join` is given, or
- the vault is set up and this machine has no machine config file.

*Set up* means: `restask.toml` exists and loads, and the inbox file it names is a restask
view (`is_view`, §7.3). `--join` on a vault that is not set up is a validation error, never
a fresh setup — on a machine the file sync has not reached yet, a fresh setup would write
files that collide with the ones arriving. A machine that has a config and re-runs plain
`restask setup` gets the full run above, as before.

A join performs step 3 (credentials, verified), step 5 and step 6, and prints the
summary. It writes nothing into the vault itself: no `restask.toml`, no scaffold, no
backup, no plugin install, no `MKCOL` of its own, no inbox prompt (`vault.inbox_list` is in
the vault). What the first sync does is what any pass does (§11). Order: `PROPFIND`
(endpoint and credentials proven) → first sync → machine config → daemon unit. The machine
config is written only after the sync succeeded, so a failed join leaves the machine
without one and the next run joins again. Non-interactive: `--join --non-interactive
--url … --username … --password-env …` (or `--password-stdin`, which is how the node's
container is joined); `--collection` is refused with `--join`.

A join with `--node` or `--no-daemon` joins an *editing machine*: step 5 is skipped —
the pass is the sync node's (§1.1) — and nothing but the credentials check reaches the
server. With `--node` it is the way to install the daemon for a vault that is already
set up, or to finish a setup whose node step failed: `prepare_node`, `PROPFIND`,
`install_node`, machine config.

### 13.3 CLI (`cli.rs`)

| Command | Purpose | Server |
|---|---|---|
| `restask setup [--join] [--node HOST --node-vault PATH] [--no-daemon] […]` | §13.2; `--node` installs the daemon on an always-on server over ssh; `--join` connects a machine to a vault that is already set up; `--no-daemon` leaves the daemon to a machine set up by hand | required |
| `restask daemon [--once]` | §13.1; the sync node only | required |
| `restask sync` | one pass; prints the report; the sync node only | required |
| `restask add "<text>" [--priority P] [--due D] [--repeat "every …"]` | new inbox task, then a pass — on an editing machine, then the local work (`settle`) | optional |
| `restask done \| undone (--uid ID \| --file F --line N)` | complete / reopen in the source file, then a pass — on an editing machine, then the local work | optional |
| `restask settle [--file F]` | the local work of a pass and nothing else: phase 1 of §11.1 and the render (`Engine::settle`). What editor integrations run on save (§16) | never contacted |
| `restask status [--json]` | active per list / priority, done today, pending sync, last sync | none |
| `restask lists` | routed lists: slug, active count, home note, collection URL | none |
| `restask doctor` | config, routing, scan (duplicates, conflict copies), TODO.md is a restask view, server reachability and auth | probed |
| `restask rebuild` | drop index, base snapshots and remembered render (tombstones stay) | none |

- "optional": the vault-side change is made and kept even if the server is unreachable or
  not configured on this machine; a warning says the server will catch up.
- An *editing machine* — one whose machine config has a `[node]` section (§14.2) — does
  no server work: `sync` and `daemon` fail there (exit 4) with the name of the machine
  that runs the daemon, and `add` / `done` / `undone` settle the vault without building
  a client, whatever else the config holds. `doctor` reports the server check as left to
  the node, with the command that shows its log; `lists` prints the collection URLs from
  the endpoint recorded in `[node]`.
- `--vault` resolution: flag → `RESTASK_VAULT` → upward search from the working directory.
  `done/undone --file <absolute path>` and `settle --file <absolute path>` search upward
  from the file instead, so editor integrations work from any directory.
- `settle` builds no client, also on a machine that has an endpoint configured, and
  writes no sync state (index, base snapshots, tombstones): only vault files and the
  remembered render. It prints `scanned N registered N normalized N`. A second run over
  its result writes nothing, and so does the vault side of the daemon's pass.
- `status`' *pending* counts tasks whose current vault content the server has not
  confirmed.
- `doctor` exit codes: `4` config invalid, `3` server unreachable, `1` a broken check or a
  server that accepts wrong credentials, `0` otherwise. Duplicated lines and conflict
  copies are warnings (the former are repaired by the next pass).
