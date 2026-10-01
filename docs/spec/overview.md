# Restask Spec — System Overview & Repository Layout (§1–§2)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §1 System Overview, Naming & Boundaries

Restask is a local-first, offline-first task system linking Markdown checkboxes to RFC 5545 VTODO objects across devices. The Markdown vault (synced by Syncthing) is the source of truth; Radicale (CalDAV) is both a projection and an ingress point for external clients (Tasks.org, Thunderbird).

### 1.1 Naming map

| Concept | Name | Location |
|---|---|---|
| System/product name | **Restask** | docs, TODO.md marker |
| CLI binary & Rust crate | **`restask`** | `crates/restask/` |
| Per-vault state directory | **`.restask/`** | vault root |
| Vault config | **`restask.toml`** | vault root (synced) |
| Machine config | **`config.toml`** | `$XDG_CONFIG_HOME/restask/` (never synced) |
| Env prefix | `RESTASK_*` | — |
| Systemd unit / container | `restask.service` / `restask` | `contrib/` |

### 1.2 Ports & adapters boundaries

```
┌──────────────────────── PURE DOMAIN (no I/O, fully unit-tested) ────────────────────────┐
│ domain (uid, priority, dates, task, clock trait)                                        │
│ router (note → list resolution)                                                          │
│ markdown (parser, mutator, todo_view)     vtodo (serialize, parse)                      │
│ sync::planner (pure 3-way decision function)                                             │
└──────────────────────────────────────────────────────────────────────────────────────────┘
        ▲ construction & data                ▲ construction & data
┌───────┴───────────────── ADAPTERS (I/O) ───┴─────────────────────────────────────────────┐
│ store (index, cache, tombstones, outbox — filesystem under .restask/)                    │
│ caldav::client (reqwest, rustls)  implements  caldav::port::CaldavPort                    │
│ daemon (notify watcher → debounced events)   cli / setup / tui (process + TTY + ssh)     │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

Rules:
- Pure modules import **nothing** that performs I/O. They receive `now`/timezone via the `Clock` port or parameters.
- All network access goes through the `CaldavPort` trait (static dispatch; mocked in tests).
- The only place allowed to invoke external processes (`ssh`) is `setup.rs` / `doctor` server checks.

### 1.3 Topology & mobile execution model

- **Sync node** (always-on device, e.g. home server): runs `restask daemon` against the vault folder (a Syncthing share) and bridges vault ⇄ Radicale.
- **Phone**: no daemon. Obsidian app edits Markdown; Tasks.org talks CalDAV to Radicale. The Obsidian plugin (§15) mirrors completions into `.restask/tasks/*.ics` so offline state travels with the vault.
- **Multiple daemons are safe**: all state under `.restask/` is reconstructible from vault + Radicale; reconciliation is idempotent.

```
Phone (offline): toggle checkbox ─▶ Obsidian plugin rewrites line, moves under Done,
                                    updates .restask/tasks/<uid>.ics
Reconnect: Syncthing ─▶ vault (md + cache) ─▶ sync node
           Tasks.org  ─▶ CalDAV PUT ─▶ Radicale
           restask daemon ─▶ 3-way plan ─▶ converge (vault == Radicale) ─▶ Syncthing fans out
```

## §2 Repository Layout (exact, normative)

Files marked ⊙ exist today and must not be edited. Everything else is created by the checklist in `AGENTS.md`.

```
.
├── .editorconfig                          ⊙ created T01
├── .github/workflows/ci.yml                 T01 (App. D)
├── .gitignore                               T01
├── AGENTS.md                              ⊙ this repo's agent contract
├── ARCHITECTURE.md                        ⊙ this file
├── INSTALL.md                             ⊙ user install guide
├── LICENSE                                  T01 (MIT)
├── README.md                              ⊙ updated in T29 (routing section)
├── Cargo.toml                              T01 (workspace, App. B)
├── rust-toolchain.toml                     T01 (channel 1.98.1)
├── contrib/
│   ├── config.example.toml                 T02 (machine config sample)
│   ├── restask.example.toml                T02 (vault config sample)
│   ├── restask.service                     T29 (App. E)
│   └── docker/
│       ├── Dockerfile                      T29 (App. E)
│       └── docker-compose.yml              T29 (App. E)
├── crates/restask/
│   ├── Cargo.toml                          T01 (App. B — normative versions)
│   ├── src/
│   │   ├── lib.rs                          T01
│   │   ├── main.rs                         T01
│   │   ├── cli.rs                          T21/T23
│   │   ├── config.rs                       T02
│   │   ├── logging.rs                      T01
│   │   ├── router.rs                       T05
│   │   ├── setup.rs                        T22
│   │   ├── tui.rs                          T22
│   │   ├── daemon.rs                       T20
│   │   ├── domain/
│   │   │   ├── mod.rs  uid.rs  priority.rs  dates.rs  task.rs      T03–T04
│   │   ├── markdown/
│   │   │   ├── mod.rs  parser.rs  mutator.rs  todo_view.rs         T06–T10
│   │   ├── vtodo/
│   │   │   ├── mod.rs  serialize.rs  parse.rs                      T11–T12
│   │   ├── store/
│   │   │   ├── mod.rs  index.rs  cache.rs  tombstones.rs  outbox.rs T14–T15
│   │   ├── caldav/
│   │   │   ├── mod.rs  port.rs  client.rs  protocol.rs             T16–T17
│   │   └── sync/
│   │       ├── mod.rs  planner.rs  engine.rs                       T18–T19
│   └── tests/
│       ├── common/mod.rs                    shared: FixedClock, MockCaldav, vault fixtures
│       ├── config.rs  domain_uid.rs  domain_priority.rs  domain_dates.rs  domain_task.rs
│       ├── router.rs  markdown_parser.rs  markdown_mutator.rs  todo_view.rs
│       ├── vtodo_codec.rs  vtodo_timestamps.rs
│       ├── store.rs  cache.rs  outbox.rs
│       ├── caldav_protocol.rs  caldav_client.rs
│       ├── sync_planner.rs  sync_engine.rs  daemon.rs
│       ├── cli.rs  setup_wizard.rs
│       └── e2e_server.rs                    #[ignore]-gated live server test (T30)
├── docs/
│   ├── contracts/vtodo-golden.ics           T11 (App. A — byte-exact, CRLF)
│   └── EXECUTION_STATE.md                  ⊙ agent session state (seeded)
├── neovim/lua/restask/
│   ├── init.lua                             T28
│   └── toggle.lua                           T28
├── plugins/obsidian/
│   ├── package.json  tsconfig.json  esbuild.config.mjs               T24 (App. C)
│   ├── manifest.json  styles.css                                     T24
│   ├── src/{main.ts, settings.ts, modal.ts, markdown.ts, vtodo.ts}   T24–T27
│   └── test/{markdown.test.ts, modal-filter.test.ts, vtodo.test.ts}  T24–T26
└── test-vault/                            ⊙ READ-ONLY fixtures (never modify, never sync)
    ├── Home Lab Test.md
    ├── Project Alpha Test.md
    └── TODO.md
```
