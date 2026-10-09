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
- **Every device is complete on its own.** Whatever restask does to the vault that needs
  no server — registering a task, filing it, completing it, keeping TODO.md current —
  happens on the device the edit is made on, at once and offline, in Obsidian and in
  Neovim alike. No device waits for the daemon, or for the file sync, to see the result
  of its own edit (*Local first*, below; invariant 12).
- **The daemon is the only bridge to the server — and nothing more.** `restask daemon`
  runs on one always-on device (the sync node), watches the vault and reconciles it with
  the server. No other device runs one, computers included, and none holds the server's
  credentials: Obsidian and Neovim edit Markdown, Tasks.org talks CalDAV, and the daemon
  makes both agree. It is one because a vault reaches another machine twice — through
  the file sync and through the server — and a second bridge writes into notes the file
  sync has not delivered yet (§1.1 *Why not two*). `restask setup` puts it in place: on
  the server, over ssh, from the computer it is run on (§13.2). It has no local
  behaviour of its own: on the vault it does what the integrations do, for the edits
  that reached it without one.
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
| Machine config | `$XDG_CONFIG_HOME/restask/config.toml` (never synced); one per vault — a further vault's is `vaults/<name>/config.toml` there (§14.2) |
| Environment prefix | `RESTASK_*` |
| Task UID | `restask-<device tag><number>` (`restask-a42`); a line shows `🆔 a42` |
| Custom VTODO properties | `X-RESTASK-SOURCE`, `X-RESTASK-SCHEDULED`, `X-RESTASK-UID`, `X-RESTASK-OF`, `X-RESTASK-TEXT` |
| Systemd unit / container | `restask.service` / `restask`; a further vault's are `restask-<name>.service` / `restask-<vault folder>` |

The project was called *Taskres* in its first weeks. That name survives only as read-side
compatibility: UIDs minted as `taskres-<ULID>` are still read and `X-TASKRES-*`
properties are still parsed. Nothing writes the old names.

Until 2026-10 a UID was `restask-<ULID>`, 34 characters on every task line. Such *long*
UIDs are still read everywhere; the sync node gives each task of the vault a counted
one, once, and the task's resource keeps the long UID as its `UID` (§11.7).

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
│ store       .restask/: index · base snapshots · tombstones · wire names · devices      │
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

## Local first: one behaviour, on every device

The work of a pass (§11.1) is of two kinds, and only one of them is the daemon's.

- **Local work** — everything that is decided from the vault alone: registering a new
  line (`🆔`), separating a copied line, repairing hand edits, the checkbox as the status
  (stamp, move under the done heading, and back), the creation date a line asks for, the
  priority of the TODO.md section a task is typed in or moved to, an edit on a TODO.md
  mirror line carried to its note, and the TODO.md view itself — mirror lines, sections,
  order, seal (§6.4, §7). The view a root note holds of its folder (§7.6) is the same
  work, in one section of a note.
- **Server work** — everything that needs the CalDAV server: the three-way merge, pushes
  and pulls, adoption, the roll-forward of a recurring series, and the state under
  `.restask/` that records what both sides agreed on.

Local work belongs to the device the edit is made on. It is done there, immediately, with
no network, no server and no daemon — neither on that device nor reachable from it:

| Where the edit is made | Who does the local work | When |
|---|---|---|
| Obsidian, desktop and mobile | the plugin — a port of the engine's rules (§15.6) | when the edit of a line is finished (the cursor leaves it); a checkbox at once |
| Neovim | the engine itself, through the CLI (`restask settle`) — no rules in Lua (§16) | when the buffer is written; a toggle at once |
| Any other editor; a file that arrives through the file sync | the daemon's local phase (§11.1) | at its next pass |

The last row is the fallback for the first two, on a computer as on a phone: an edit
that no integration settled is settled by the sync node, by the same local phase, and
is only later. The sync node needs nothing else for it — it runs the one daemon, and
`restask settle` is that daemon's phase 1 and render as a command, without the server.

Three places, **one behaviour**. The engine's pure modules are the reference; the plugin
is a port of them, and Neovim calls them. For the same edit all three leave the same
bytes in the same files — the note *and* TODO.md — so it makes no difference to the user
which of them got there first, and the daemon's pass over a vault an integration has
settled writes no file (invariant 8). A difference between them is a bug, in whichever
side departs from the spec. A local rule therefore never exists in one place only: it is
specified once (§6–§7) and implemented in the engine and in the plugin in the same task.

What a device cannot do alone is server work, and only that may wait for the sync node.
"It appears once the sync has gone there and back" is acceptable for a change made *on
the server* and for nothing else.

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
3. **A task keeps its UID, and a UID is one task's.** Assigned once, preserved across
   edits and list moves, and never given to another task: every device mints under a
   tag of its own (§3.1, §9.4) and uses each number once. A copied line gets its own
   UID. A resource keeps its `UID` too: a task another client created stays that
   client's resource — its name, its `UID` — and is linked to its line, never replaced
   by a copy; the name it goes by is recorded before its line is written (§9.5), so
   adopting it twice yields the same task. The one time a line's UID changes is the
   renumbering of the long UIDs of before the counters (§11.7) — the sync node's, once
   per task, with nothing created or deleted on the server.
4. **Local-only by default.** A note takes part only when routed by frontmatter
   (`restask-list`, `restask-list-root`). Of an unrouted note only the frontmatter block
   is ever read; it is never parsed, modified or synced.
5. **Lists are collections.** List `Home Lab` ⇄ the server's calendar of that name: the
   collection at `home-lab`, else the one calendar other clients show as `Home Lab`,
   wherever its client put it (§5.4). Two of one name are not chosen between. The inbox file
   routes to `vault.inbox_list`; a line in it that names another calendar (`📁 work`)
   lives in that one, and the line — not the state — is what says so (§7.5).
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
10. **Secrets never travel.** Passwords live only in `radicale.passwd` beside the vault's
    machine config (0600) or the environment — never in synced files, configs or logs.
11. **Timestamps follow §4.** Markdown dates are device-local; date-only values map to
    midnight UTC and back by UTC calendar date; timed dues are floating local time.
12. **Local work is local, and the same everywhere.** Nothing that can be decided from
    the vault alone waits for the daemon, the file sync or the server: the Obsidian
    plugin and the Neovim integration do it on the device, at once and offline, and
    leave what the daemon's local phase would leave — byte for byte, in the note and in
    TODO.md. The daemon is required for server work only.

## Specification index

| File | Sections | Contents |
|---|---|---|
| `docs/spec/overview.md` | §1–2 | topology, repository layout |
| `docs/spec/domain.md` | §3–4 | domain types, priority table, timestamp contract |
| `docs/spec/routing.md` | §5 | note routing, list ⇄ collection |
| `docs/spec/markdown.md` | §6–7 | grammar, mutations, repair, the TODO.md view, the view of a root note |
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
- `docs/EXECUTION_STATE.md` — where the work stands; decision log. Private to each
  contributor and git-ignored: it is not in the repository.
- `README.md` — what the project is for. `INSTALL.md` — how to install it (`docs/INSTALL-AI.md` is the detailed guide).
- `restask-vault/` — a sandbox vault for manual trials. Tests may use a temporary copy of it (`copy_test_vault`), never the folder itself.
