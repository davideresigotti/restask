# restask — Architecture

> **Status: normative.** This file is the entry point: what the system is, how it is
> layered, the model the sync is built on, and the invariants nothing may break. Details
> live in `docs/spec/` (index below). When code and spec disagree, one of them is a bug —
> fix it, and say which in the commit.

## What restask is

restask is a local-first, offline-first task system. Every checkbox in a *routed*
Markdown note is a task, linked by an eternal UID to an RFC 5545 `VTODO` on a CalDAV
server (Radicale), so the same task can be worked on from Obsidian, Neovim, Tasks.org or
Thunderbird.

- **The vault is the source of truth.** It is a folder of Markdown files, carried between
  devices by a file-sync tool (Syncthing). Editing a note is all a device ever has to do.
- **The daemon is the only bridge.** `restask daemon` runs on an always-on device, watches
  the vault and reconciles it with the server. Phones run no daemon: Obsidian edits
  Markdown, Tasks.org talks CalDAV, and the daemon makes both agree.
- **The server is a peer, not a mirror.** Changes made there (complete, reword, re-date,
  create, delete) flow back into the notes.

```
 phone / laptop                        always-on device                    server
┌──────────────────┐   file sync    ┌────────────────────────┐  CalDAV  ┌──────────┐
│ Markdown vault   │◀──────────────▶│ Markdown vault         │          │ Radicale │
│  Obsidian+plugin │                │  restask daemon ───────┼─────────▶│  VTODOs  │
│  Neovim + CLI    │                │  .restask/ (state)     │◀─────────┼──────────│
└──────────────────┘                └────────────────────────┘          └────▲─────┘
                         Tasks.org / Thunderbird ───────────── CalDAV ───────┘
```

## Naming

| Concept | Name |
|---|---|
| Project, CLI binary, Rust crate, plugin id | **`restask`** |
| Per-vault state directory | `.restask/` (vault root, rides the file sync) |
| Vault config | `restask.toml` (vault root, synced) |
| Machine config | `$XDG_CONFIG_HOME/restask/config.toml` (never synced) |
| Environment prefix | `RESTASK_*` |
| Task UID | `restask-<ULID>` |
| Custom VTODO properties | `X-RESTASK-SOURCE`, `X-RESTASK-SCHEDULED` |
| Systemd unit / container | `restask.service` / `restask` |

The project was called *Taskres* in its first weeks. That name survives only as read-side
compatibility: UIDs minted as `taskres-<ULID>` stay valid verbatim (UIDs are eternal) and
`X-TASKRES-*` properties are still parsed. Nothing writes the old names.

## Layers

```
┌───────────────────────────── PURE (no I/O, no clock, no randomness in decisions) ──────┐
│ domain      uid · priority · dates · task · Clock trait                                │
│ router      note → list resolution                                                     │
│ markdown    parser · mutator · todo_view                                               │
│ vtodo       serialize · parse                                                          │
│ sync::merge   field-level three-way merge of one task                                  │
│ sync::planner snapshots → plan (the reconciliation rules)                              │
└────────────────────────────────────────────────────────────────────────────────────────┘
┌───────────────────────────── ADAPTERS (all I/O) ───────────────────────────────────────┐
│ fsio        atomic, change-only file writes                                            │
│ vault       walk + scan + repair of routed notes                                       │
│ store       .restask/: index · base snapshots · tombstones                             │
│ caldav      protocol (pure XML) · port (trait) · client (reqwest) · offline            │
│ sync::engine  one reconciliation pass: scan → plan → apply → record                    │
│ daemon      watcher + poll → debounced single reconciler                               │
│ cli · setup · tui · config · logging                                                   │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

Rules:

- Pure modules import nothing that performs I/O; time arrives through `Clock` or as a
  parameter. They are where the behaviour is, and where most tests are.
- All network access goes through `CaldavPort` (static dispatch). The port is stateless:
  preconditions and timestamps are arguments.
- All vault and state writes go through `fsio` (temp file + fsync + rename; nothing is
  written when the content is unchanged).

## The model the sync is built on

Three versions of every task meet in each pass:

- **local** — the line in the vault;
- **remote** — the `VTODO` on the server;
- **base** — the content both sides last agreed on, kept as `.restask/tasks/<uid>.ics`
  and vouched for by the index.

Reconciliation is a **field-level three-way merge** over the base: a field that changed on
one side only takes that side's value; a field changed on both sides is a conflict,
settled by `LAST-MODIFIED` with a 120 s tie window in the vault's favour. Timestamps decide
*only* conflicts. This matters because the only timestamp the vault has is a file's mtime,
which covers every task in the file and moves on every unrelated edit; it cannot say
whether one given task changed. The base can.

The planner is a pure function `Snapshots → Plan`. The engine executes a plan in an order
that makes a crash at any point harmless: vault edits, then server writes, then the
TODO.md render, then the state. Nothing is queued: an operation that fails is simply
re-planned from fresh snapshots in the next pass.

## Invariants

1. **The vault is truth, and never the victim.** No state file, tombstone or server
   condition may delete or revert what the user has in the vault, with one exception: a
   task that was settled and then disappears from a listed, non-reset collection was
   deleted on the server, and its line is removed. A line that is in the vault outranks a
   tombstone.
2. **Unknown is not empty.** A collection that was not listed, a note that could not be
   read, a collection that lost everything at once — none of these prove a deletion.
3. **UIDs are eternal.** Assigned once, never regenerated, preserved across edits and list
   moves. A copied line gets its own UID; a foreign task is adopted under a UID derived
   from its own, so adopting it twice yields the same task.
4. **Local-only by default.** A note takes part only when routed by frontmatter
   (`restask-list`, `restask-list-root`). Of an unrouted note only the frontmatter block
   is ever read; it is never parsed, modified or synced.
5. **Lists are collections.** List `Home Lab` ⇄ collection `home-lab`. The inbox file
   routes to `vault.inbox_list`.
6. **Other clients' data is not ours to lose.** `VEVENT`s are never touched. Everything in
   a `VTODO` that restask does not manage (description, reminders, recurrence, tags,
   vendor properties) is written back verbatim on every push.
7. **Deterministic.** Same task + same instant → byte-identical `VTODO`; same task set →
   identical TODO.md; same snapshots → same plan.
8. **Idempotent and quiet.** A pass over a converged vault writes no file and sends no
   write request.
9. **One writer per vault per machine** (advisory lock on `.restask/lock`); all writes
   atomic; all state disposable — `restask rebuild` drops it and the next pass
   re-derives it.
10. **Secrets never travel.** Passwords live only in `~/.config/restask/radicale.passwd`
    (0600) or the environment — never in synced files, configs or logs.
11. **Timestamps follow §4.** Markdown dates are device-local; date-only values map to
    midnight UTC and back by UTC calendar date; timed dues are floating local time.

## Specification index

| File | Sections | Contents |
|---|---|---|
| `docs/spec/overview.md` | §1–2 | topology, repository layout |
| `docs/spec/domain.md` | §3–4 | domain types, priority table, timestamp contract |
| `docs/spec/routing.md` | §5 | note routing, list ⇄ collection |
| `docs/spec/markdown.md` | §6–7 | grammar, mutations, repair, the TODO.md view |
| `docs/spec/vtodo.md` | §8, App. A | codec, extras, golden VTODO |
| `docs/spec/storage.md` | §9 | `.restask/` layout and semantics |
| `docs/spec/caldav.md` | §10 | XML protocol, the port, retry |
| `docs/spec/sync.md` | §11 | merge, snapshots, plan, rule table, execution order |
| `docs/spec/runtime.md` | §12–13 | errors, logging, daemon, CLI, setup |
| `docs/spec/config.md` | §14, §17 | configuration, security |
| `docs/spec/integrations.md` | §15–16 | Obsidian plugin, Neovim |
| `docs/spec/appendices.md` | App. B–E | dependencies, plugin manifests, CI, deployment |

## Pointers

- `AGENTS.md` — how to work on this repository (gates, rules, workflow).
- `docs/EXECUTION_STATE.md` — where the work stands; decision log.
- `README.md` — what the project is for. `INSTALL.md` — how to run it.
- `test-vault/` — a sandbox vault for manual trials. Tests never read it.
