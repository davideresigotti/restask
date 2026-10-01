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
- **Several daemons** (e.g. server + desktop, each on its own vault copy) are safe: state
  under `.restask/` rides the file sync and is disposable, the merge is idempotent, and a
  vault file that lags behind the synced state is waited on (§11 R1). Two processes on
  the *same* vault folder are serialized by an advisory lock (§9).

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
change, and re-renders TODO.md (§6.4). The plugin and the CLI only make the same edits
immediately instead of at the next pass.

## §2 Repository layout

```
.
├── ARCHITECTURE.md            model, layers, invariants, spec index
├── AGENTS.md                  how to work on this repository
├── README.md  INSTALL.md      what it is for; how to run it
├── Cargo.toml  Cargo.lock  rust-toolchain.toml
├── .github/workflows/ci.yml   App. D
├── contrib/
│   ├── config.example.toml    machine config sample (§14.2)
│   ├── restask.example.toml   vault config sample (§14.1)
│   ├── restask.service        systemd user unit (App. E)
│   └── docker/{Dockerfile,docker-compose.yml}
├── crates/restask/
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
│   └── EXECUTION_STATE.md     state of the work, decisions, open items
├── neovim/
│   ├── lua/restask/{init,toggle}.lua
│   └── test/toggle_test.lua
├── plugins/obsidian/
│   ├── src/{main,settings,markdown,modal,toggle}.ts
│   └── test/{markdown,modal-filter,toggle}.test.ts  test/fixtures/
└── test-vault/                manual sandbox (may be live-synced; tests never read it)
```
