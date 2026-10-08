# restask spec — System Overview & Repository Layout (§1–§2)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §1 System overview

restask links Markdown checkboxes to RFC 5545 `VTODO` objects across devices. The
Markdown vault (carried by Syncthing) is the source of truth; Radicale (CalDAV) is a peer
that other clients (Tasks.org, Thunderbird) read and write.

### 1.1 Topology

- **Sync node** — an always-on device running `restask daemon` on its copy of the vault.
  It is the only thing that talks to the CalDAV server.
- **Other devices** — edit Markdown (Obsidian with the plugin, Neovim with the CLI, any
  editor) and/or talk CalDAV (Tasks.org, Thunderbird). They need no daemon.
- **A computer with Obsidian and Neovim** is such a device: both work on the one vault
  folder, the plugin doing the local work for an edit made in Obsidian (§15.6) and
  `restask settle` for a buffer written in Neovim (§13.3, §16). Each sees the other's
  result as a file changed on disk. An edit made there with neither falls back to the
  sync node's daemon, whose pass begins with the same local phase (§11.1) — the one
  code path `settle` runs — and reaches the device again through the file sync.
- **One daemon per vault.** `restask setup` puts it in one place (§13.2 step 6): on the
  always-on server that holds a copy of the vault, installed from the computer over ssh,
  or — when there is no such machine — on the computer itself. Every other machine is an
  *editing machine*: it runs no daemon, holds no server credentials (§14.2 `[node]`) and
  makes no pass; `restask sync` and `restask daemon` refuse there, and `add` / `done` /
  `undone` settle the vault and stop (§13.3). What a daemon would add on such a machine —
  immediacy — the integrations already give (§15.6, §16).
- **Several vaults** are several of these, side by side and sharing nothing: each vault
  has its own `restask.toml` and `.restask/`, its own server or calendars, its own
  daemon — on a server one compose stack per vault (§13.2 step 6, App. E), on a
  computer one unit per vault — and, on every machine that works on it, its own machine
  config (§14.2). A machine can be the sync node of one vault and an editing machine of
  another.
- **Why not two.** A vault has two ways to another machine: the file sync, and — through
  two daemons — the server. A pass on machine A pushes an edit at once; the daemon on B
  hears of it from the server watch within seconds (§13.1), before the file sync has
  delivered A's note, and writes the change into B's *older* copy of that note. The file
  sync then holds two versions of one file. They are equal only when the edit was all
  that changed and the mutator happens to rebuild A's bytes; a task typed between two
  others (B appends it at the end of the list), or a sentence of prose saved with it
  (B's copy lacks it), gives different bytes, and the file sync keeps B's as the note and
  A's — the user's — as a conflict copy. `.restask/index.json`, written by both with
  their own `seen_at`, conflicts on every pass. Reproduced with two engines on two
  copies of one vault (T75). The merge still converges on the *server*; it is the files
  that do not. A one-shot pass from an editing machine is the same second writer, which
  is why such a machine keeps no credentials. Two processes on the *same* vault folder
  are serialized by an advisory lock (§9).

```
Phone, offline:  check a box in Obsidian        ─▶ the note changes; nothing else
Reconnect:       Syncthing ─▶ note reaches the sync node
                 Tasks.org ─▶ CalDAV PUT reaches Radicale
Sync node:       daemon ─▶ scan vault ─▶ list server ─▶ three-way merge over the base
                        ─▶ notes and server agree ─▶ Syncthing fans the notes out
```

### 1.2 What a device must do to change a task

Edit the Markdown line. That is the whole contract. Checking a box in any editor is
enough: the daemon stamps `✅ <date>`, moves the line under the done heading, pushes the
change, and re-renders TODO.md (§6.4). The plugin and the CLI (`restask settle`, which
Neovim runs on write) only make the same vault edits immediately instead of at the next
pass; pushing stays the daemon's.

## §2 Repository layout

```
.
├── ARCHITECTURE.md            model, layers, invariants, spec index
├── AGENTS.md                  how to work on this repository
├── README.md  INSTALL.md      what it is for; how to install it (docs/INSTALL-AI.md: the detail)
├── Cargo.toml  Cargo.lock  rust-toolchain.toml
├── .github/workflows/ci.yml   App. D
├── contrib/
│   ├── config.example.toml    machine config sample (§14.2)
│   ├── restask.example.toml   vault config sample (§14.1)
│   ├── restask.service        systemd user unit (App. E)
│   ├── node.sh                the sync node over ssh: check, install, update (App. E)
│   ├── update.sh              update binary, plugin and sync node from the tree (App. E)
│   └── docker/{Dockerfile,docker-compose.yml}
├── crates/restask/
│   ├── assets/obsidian/       built plugin bundle embedded in the binary (App. C)
│   ├── src/
│   │   ├── lib.rs  main.rs  error.rs  logging.rs
│   │   ├── domain/      uid  priority  dates  task                 (pure, §3–4)
│   │   ├── router.rs                                                (pure, §5)
│   │   ├── markdown/    parser  mutator  todo_view                  (pure, §6–7)
│   │   ├── vtodo/       serialize  parse                            (pure, §8)
│   │   ├── sync/        merge  planner (pure) · engine (I/O)        (§11)
│   │   ├── fsio.rs      atomic writes
│   │   ├── vault.rs     walk, scan, repair                          (§5, §6.4)
│   │   ├── store/       index  cache (base snapshots)  tombstones   (§9)
│   │   ├── caldav/      protocol  port  client  offline             (§10)
│   │   ├── daemon.rs                                                (§13.1)
│   │   └── cli.rs  setup.rs  tui.rs  config.rs                      (§13–14)
│   └── tests/
│       ├── common/mod.rs      FixedClock, MockCaldav, temp vault
│       ├── fixtures/          immutable sample notes
│       ├── sync_engine.rs     end-to-end scenarios (incl. every data-loss regression)
│       ├── sync_planner.rs    one test per rule
│       ├── … one suite per module …
│       └── e2e_server.rs      #[ignore]: live server probe, run by hand
├── docs/
│   ├── spec/*.md              this specification
│   ├── contracts/vtodo-golden.ics   byte-exact serializer contract (App. A)
│   └── EXECUTION_STATE.md     state of the work, decisions, open items (private, git-ignored)
├── neovim/
│   ├── lua/restask/{init,toggle}.lua
│   └── test/toggle_test.lua
├── plugins/obsidian/
│   ├── src/{main,settings,markdown,modal,toggle,conceal,editor,filing}.ts
│   └── test/*.test.ts  test/fixtures/
└── test-vault/                manual sandbox (may be live-synced; tests never read it)
```
