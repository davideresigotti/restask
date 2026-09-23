# Taskres — Architecture (Index & Invariants)

> **Status: NORMATIVE.** This file is the entry point: what the system is, its boundaries, its
> invariants, and the index into the detailed specification in `docs/spec/`. Implementation
> questions are resolved there, or escalated via the Stop-and-Clarify protocol in `AGENTS.md` —
> never by guessing. Supersedes conflicting statements in `README.md` (README updated in T29).
>
> **Agents: never read the whole spec set.** Each checklist task in `AGENTS.md` names the ONE
> `docs/spec/` file to read. Humans: use the index below.

## What Taskres is

Taskres is a local-first, offline-first task system linking Markdown checkboxes to RFC 5545
VTODO objects across devices. The Markdown vault (synced by Syncthing) is the source of truth;
Radicale (CalDAV) is both a projection and an ingress point for external clients (Tasks.org,
Thunderbird).

- **Sync node** (always-on device, e.g. home server): runs `restask daemon` on the vault folder
  (a Syncthing share) and bridges vault ⇄ Radicale. Multiple daemons are safe: `.taskres/`
  state is reconstructible and reconciliation is idempotent.
- **Phone**: no daemon. Obsidian edits Markdown (the plugin mirrors completions into
  `.taskres/tasks/*.ics`, which ride the vault via Syncthing); Tasks.org talks CalDAV to
  Radicale. On reconnection the daemon converges vault and server.

## Naming map

| Concept | Name | Location |
|---|---|---|
| System/product name | **Taskres** | docs, TODO.md marker |
| CLI binary & Rust crate | **`restask`** | `crates/restask/` |
| Per-vault state directory | **`.taskres/`** | vault root |
| Vault config | **`restask.toml`** | vault root (synced) |
| Machine config | **`config.toml`** | `$XDG_CONFIG_HOME/restask/` (never synced) |
| Env prefix | `RESTASK_*` | — |
| Systemd unit / container | `restask.service` / `restask` | `contrib/` |

## Module boundaries (ports & adapters)

```
┌──────────────────────── PURE DOMAIN (no I/O, fully unit-tested) ────────────────────────┐
│ domain (uid, priority, dates, task, clock trait)                                        │
│ router (note → list resolution)                                                          │
│ markdown (parser, mutator, todo_view)     vtodo (serialize, parse)                      │
│ sync::planner (pure 3-way decision function)                                             │
└──────────────────────────────────────────────────────────────────────────────────────────┘
        ▲ construction & data                ▲ construction & data
┌───────┴───────────────── ADAPTERS (I/O) ───┴─────────────────────────────────────────────┐
│ store (index, cache, tombstones, outbox — filesystem under .taskres/)                    │
│ caldav::client (reqwest, rustls)  implements  caldav::port::CaldavPort                    │
│ daemon (notify watcher → debounced events)   cli / setup / tui (process + TTY)             │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

Rules:
- Pure modules import **nothing** that performs I/O; time comes from the `Clock` port or parameters.
- All network access goes through the `CaldavPort` trait (static dispatch; mocked in tests).

## Core invariants (the constitution)

1. **Vault is truth.** Reconciliation conflicts inside a 120 s tie-window resolve LOCAL.
2. **UIDs are eternal.** `taskres-<ULID>` assigned once, never regenerated, preserved across
   edits and list moves; deletion creates a tombstone; UIDs are never reused.
3. **Local-only by default.** Unrouted notes are never parsed (beyond frontmatter), never
   modified, never synced.
4. **Lists = collections.** Tasks route to CalDAV lists via frontmatter (`restask-list`,
   `restask-list-root`, nearest-root-wins); one list = one Radicale collection; bindings to
   existing collections are wizard-confirmed.
5. **Foreign resources are safe.** In bound collections: VEVENTs never touched; foreign VTODOs
   adopted with a fresh UID; only `taskres-*` resources managed.
6. **Timestamps follow the §4 contract** (`docs/spec/domain.md`): date-only Markdown → midnight
   UTC; reverse direction formats the UTC calendar date; timed dues are floating local time.
7. **Pure domain, adapters at the edge** (diagram above). No I/O in `domain`, `router`,
   `markdown`, `vtodo`, `sync::planner`.
8. **Single-writer reconciler.** One task owns all mutations; all state writes are atomic
   (tmp + fsync + rename) and reconstructible from vault + Radicale.
9. **Secrets never travel.** Passwords live only in `~/.config/restask/radicale.passwd` (0600)
   or env — never in synced files, configs, or logs.
10. **Determinism.** Same Task + same `now` → byte-identical VTODO; same task set → identical
    TODO.md render; same inputs → same plan.

## Specification index (`docs/spec/`)

| File | Contents (section numbers preserved) | Checklist tasks |
|---|---|---|
| `docs/spec/overview.md` | §1–2 boundaries, topology, exact repo tree | T01 |
| `docs/spec/domain.md` | §3–4 domain types, emoji/priority tables, timestamp contract | T03, T04, T13 |
| `docs/spec/routing.md` | §5 note routing, list roots, slug→collection mapping | T05 |
| `docs/spec/markdown.md` | §6–7 grammar, mutations, TODO.md view contract | T06–T10 |
| `docs/spec/vtodo.md` | §8 + App. A codec, folding/escaping, golden VTODO | T11–T13 |
| `docs/spec/storage.md` | §9 `.taskres/` layout, index/tombstones/cache/outbox | T14–T15 |
| `docs/spec/caldav.md` | §10 XML protocol, CaldavPort, retry, foreign rules | T16–T17 |
| `docs/spec/sync.md` | §11 snapshots, plan, rules R0–R10 | T18–T19 |
| `docs/spec/runtime.md` | §12–13 errors, logging, daemon, CLI, setup wizard | T20–T23 |
| `docs/spec/config.md` | §14, §17 config reference, security | T02, T23 |
| `docs/spec/integrations.md` | §15–16 Obsidian plugin, Neovim | T24–T28 |
| `docs/spec/appendices.md` | App. B–E Cargo.toml, plugin manifests, CI, deployment | T01, T24, T29 |

## Component tree (summary; full normative tree in `docs/spec/overview.md` §2)

```
crates/restask/src/
├── domain/    uid, priority, dates, task, Clock         (pure)
├── router.rs  note → list resolution                     (pure)
├── markdown/  parser, mutator, todo_view                 (pure)
├── vtodo/     serialize, parse                           (pure)
├── sync/      planner (pure), engine (I/O orchestrator)
├── store/     index, cache, tombstones, outbox           (.taskres/)
├── caldav/    protocol (pure XML), port (trait), client (reqwest)
├── daemon.rs  watcher + debounce + single reconciler
└── cli.rs, setup.rs, tui.rs, config.rs, logging.rs
plugins/obsidian/    TypeScript plugin, zero runtime deps (desktop + mobile)
neovim/lua/restask/  thin wrapper over the restask CLI
```

## Pointers

- **`AGENTS.md`** — agent operating contract, quality gates, 30-task atomic checklist.
- **`docs/EXECUTION_STATE.md`** — agent session state (read first every session).
- **`docs/contracts/vtodo-golden.ics`** — byte-exact cross-language contract (created in T11).
- **`INSTALL.md`** — user install guide. **`test-vault/`** — read-only fixtures.
