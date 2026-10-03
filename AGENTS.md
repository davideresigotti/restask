# AGENTS.md — working on restask

This is the operating contract for anyone changing this repository — a coding agent of any
model, or a human. `ARCHITECTURE.md` (model + invariants) and `docs/spec/*.md` (details)
are normative; this file says how to work. Keep all three true: a change that alters
behaviour updates the spec in the same commit.

## 1. Before you change anything

1. Read `ARCHITECTURE.md` — it is short, and the invariants in it are the review
   checklist for every change.
2. Read `docs/EXECUTION_STATE.md` — where the work stands, open items, past decisions.
3. Read the spec file(s) for the area you touch (index in `ARCHITECTURE.md`) and the code
   itself. Read as much as you need to be sure; a wrong change to the sync core costs
   someone their notes.

Do not read `target/`, `node_modules/`, or `Cargo.lock`.

## 2. Environment

| Tool | Version | Notes |
|---|---|---|
| Rust | **1.98.1** (pinned in `rust-toolchain.toml`) | fmt + clippy included |
| Node / npm | **22 / 10** | plugin only, workdir `plugins/obsidian` |
| Lua | ≥ 5.4 (`luac`, `lua`) | syntax + behaviour gate for `neovim/` |

The maintainer's Radicale lives on their home network (auth enforced; its address is in
the maintainer's machine config). **Never contact it** from tests or while developing: everything runs against
`MockCaldav` (`crates/restask/tests/common/mod.rs`). The only exception is the
`#[ignore]`d `tests/e2e_server.rs`, which the owner runs by hand and which touches the
`restask-dev` collection only. Never read `~/.config/restask/radicale.passwd`.

**Testing uses the `dev` calendar and `test-vault/`, and nothing else** (owner,
2026-10-02, after a trial deleted two tasks of the owner's `inbox` calendar — T75).
There are two kinds of test, and no third:

- *Automated tests* (`cargo test`, Vitest, the Lua tests) are hermetic: temporary
  directories and `MockCaldav`, no real server, no real calendar, no systemd, no ssh. They
  may use `test-vault/` as ready-made input, but only a copy of it in a temporary
  directory (see "Fixtures and the sandbox").
- *A trial of the real thing* — the installed or built `restask` binary, the plugin in
  Obsidian, Neovim, the daemon — happens in `test-vault/`, whose tasks live in the
  calendar `dev` through the sync node's daemon. No other vault, no other calendar.

What follows from it, each point a thing that went wrong once:

- Never run the real binary's `setup`, `daemon`, `sync`, `add`, `done` or `undone` on
  a vault other than `test-vault/` — no throwaway vault, no scratch copy, no
  `--vault` / `RESTASK_VAULT` pointing anywhere else. The machine config of this
  computer names the live server; a vault whose `restask.toml` says `inbox_list =
  "inbox"`, or whose notes route to lists of their own, is synced with the owner's
  real calendars by any pass that finds that config.
- Never run `restask setup` at all, in any form (`--join`, `--non-interactive`, through
  a script, inside a stand-in for a container): it installs and starts the systemd
  unit of this machine, and a unit carries none of the environment (`RESTASK_CONFIG`,
  `RESTASK_VAULT`) that kept the trial away from the real config. Setup is tested
  through `run_setup` with a recording installer (`tests/setup_wizard.rs`); the
  wizard's real run is the owner's.
- No stand-in server, no simulated file sync, no fake `ssh` or `docker` that ends up
  executing the real binary. A fake that only records what it was asked is fine.
- Nothing is started under systemd by an agent, and `~/.config/systemd/user/` is not
  written.
- If a trial needs something this rule does not allow, it is not run: say what was
  not seen, and why, in the session log. That is an acceptable result; a trial outside
  the sandbox is not.

There is one daemon per vault (`docs/spec/overview.md` §1.1), and for the sandbox it does
not run on the development machine. It runs on the sync node — the compose stack
`/opt/docker/restask` on the ssh host `docker`, the machine Radicale is on — and serves
that machine's copy of `test-vault` (Syncthing folder `obsidian-dev`, calendar `dev`).
`contrib/update.sh docker` (§5.4) is how a change gets there. A second daemon — or a
one-shot `restask sync` — on this machine is not redundant but harmful: it races the
file sync and leaves conflict copies of notes (§1.1 *Why not two*).

That is the intended setup, not a gap to work around: restask is local-first
(`ARCHITECTURE.md` *Local first*, invariant 12). On this machine the Obsidian plugin and
the Neovim integration do all the local work themselves, at once and offline. If
something local shows up only after the sync node has seen the file — a `🆔`, a moved
line, a line in TODO.md — the integration has a bug; starting a daemon here hides it.

## 3. Quality gates — all of them, before every commit

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                       # ~10 s; no reason to run less
```

When `plugins/obsidian` changed (workdir `plugins/obsidian`; `npm ci` once):

```
npm run lint && npm test && npm run build
```

The build also refreshes `crates/restask/assets/obsidian/`, the copy of the plugin the
binary embeds for `restask setup`. Commit it with the change; CI fails when it is stale.

When `neovim/` changed:

```
luac -p neovim/lua/restask/*.lua && lua neovim/test/toggle_test.lua
```

A red gate is never committed, and never "fixed" by weakening a test. If a test encodes
behaviour you are deliberately changing, change the test and the spec together and say so
in the commit message.

## 4. Rules

**Architecture**

- Pure modules (`domain`, `router`, `markdown`, `vtodo`, `sync::merge`, `sync::planner`)
  perform no I/O and read no clock; time comes from `Clock` or a parameter. Decisions
  belong there, where they can be tested without a filesystem or a server.
- File writes go through `fsio` only. `std::fs` reads are for the adapters (`vault`,
  `store`, `sync::engine`, `setup`, `cli`, `config`).
- Network goes through `CaldavPort` only. The port stays stateless.
- `std::process::Command` is allowed only in `setup.rs` (the systemd unit install,
  `contrib/node.sh` for the daemon on a node, and the restart of Obsidian).
- Async only where it already is (caldav, engine, daemon, cli).

**Rust** (crate `restask`, edition 2021)

- No `unsafe`. No `.unwrap()` / `.expect()` outside tests. Errors are `RestaskError`,
  propagated with `?`.
- Every public item has a `///` doc comment that says what it is *for*.
- Dependencies are the ones in `docs/spec/appendices.md` App. B. Adding or bumping one is
  a decision: record it in `docs/EXECUTION_STATE.md` and update App. B.
- Tests never depend on the wall clock for their *assertions* and never on the network.
  Server behaviour is tested through `MockCaldav`, which runs bodies through the real
  codec and enforces `If-Match` — do not bypass it with hand-built `RemoteTask`s in
  engine tests.

**Local parity** (invariant 12) — daemon, Obsidian plugin and Neovim behave identically

- A behaviour that needs no server is *local work*: it is done on the device, at once
  and offline, by the plugin in Obsidian and through the CLI in Neovim. "The daemon does
  it at its next pass" is the fallback for edits made without an integration, never the
  design for one made with it.
- A change to a local rule (registration, repair, the checkbox, `➕`, section priority,
  mirror edits, the TODO.md view) is made in the spec, in the engine **and** in the
  plugin in the same task, with a test on each side for the same input and the same
  expected bytes. Neovim gets it through the CLI: no rule is written in Lua.
- The engine is the reference. The plugin recognises what the engine recognises (which
  note is routed, which file is the view, what a task line is) by the same evidence, and
  leaves what the engine would leave. A place where it cannot yet — a line it leaves to
  the daemon, an order the render corrects — is an open item in
  `docs/EXECUTION_STATE.md`, not a design choice.
- Check a change to the plugin, the CLI or the local phase against these questions:
  - Does the result appear without a daemon on the device, and with the network off?
  - Does it appear when the edit is finished, in the note and in TODO.md, without a
    file-sync round trip?
  - Does the engine's pass over the result write no file (`tests/sync_engine.rs`)?
  - Would the same edit in Obsidian, in Neovim and in a plain editor (daemon) end in the
    same bytes?
- Server work (merge, push, pull, adoption, the roll-forward of a recurring series) is
  the daemon's alone. No integration contacts the server.

**TypeScript** (plugin): `strict: true`; zero runtime dependencies; Obsidian API only in
`src/main.ts` and `src/settings.ts`; everything else pure and tested under Vitest. The
plugin writes Markdown and nothing else — no state files, no network. It may read what
the engine's local phase reads (`restask.toml`, the routed notes, `.restask/`), when
parity needs it.

**General**

- UTF-8 everywhere. Emoji are written as literals from the tables in §3.2 / §6.1.
- No `TODO` / `FIXME` / placeholder comments. Unfinished work goes in
  `docs/EXECUTION_STATE.md` under *Open items*.
- The project is called **restask**. The old name must not reappear, except in the
  read-side compatibility code and its tests (`taskres-` UIDs, `X-TASKRES-*`).

**When touching the sync core** (`vault`, `sync::*`, `store`, `markdown::todo_view`), check
the change against these questions — each one was a real data-loss bug:

- Can it delete or revert a vault line on evidence that is merely *absent* (an unlisted
  collection, an unreadable file, a failed request, a missing state file)?
- Does it treat a file's mtime as proof that one particular task changed?
- If the process dies between any two writes, is the next pass still correct?
- Is a second pass over the result a no-op (no file written, no request sent)?
- Does it drop content of a `VTODO` that restask does not manage?

Every bug fix ships with a regression test at the level where the bug showed
(`tests/sync_engine.rs` for end-to-end behaviour, `tests/sync_planner.rs` for rules).

## 5. Workflow

1. One task, one commit, message `T##: <what changed and why>`; take the next free
   number from `docs/EXECUTION_STATE.md`. Commit only your task's files.
2. Close the task in `docs/EXECUTION_STATE.md`: session-log line, any new decision, any
   open item you found but did not fix. Keep the file under 150 lines (trim the oldest
   session-log lines first; decisions that the spec now states can go too).
3. Never push, never amend, never force. Never commit secrets.
4. Update what runs restask. When the task changed the binary (anything under
   `crates/`) or the plugin, and the gates are green, run — without asking, as part of
   closing the task:

   ```
   contrib/update.sh docker
   ```

   It does four things, each skipped when there is nothing to update (App. E): it
   reinstalls the `restask` binary of this machine (the CLI Neovim calls), restarts this
   machine's daemon *if* its unit is enabled (it is not: the daemon runs on the sync
   node only, §1.1), copies the built plugin bundle into the sandbox vault, and ships
   the sources of the working tree to the sync node — the compose stack
   `/opt/docker/restask` on the ssh host `docker` — where the image is rebuilt and the
   daemon restarted.

   Read the output: the script ends with the daemon's log, and the first pass must be
   free of errors. A daemon that does not come up fails the script and is a red gate:
   fix it before you stop. Obsidian picks the new bundle up when the plugin or the app
   is reloaded; say so when you report. Say in the session log what was updated, or why
   not. On the sync node the script replaces `src/` and the image and nothing else:
   never edit the stack's `docker-compose.yml` or `data/` (machine config and password),
   never touch another stack on that host, and use ssh there for this script and for
   reading `docker compose logs` only. The daemon reaches the live server on its own —
   that is its job, and no licence for you to contact it (§2). This step never runs
   `restask setup` (it replaces TODO.md) and touches nothing else in the vault.

   A task that changed local work is not closed on the daemon's log alone: say in the
   session log what was seen working on the device without the daemon (or that it was
   not seen, and why).

**Fixtures and the sandbox.** `crates/restask/tests/fixtures/` and
`plugins/obsidian/test/fixtures/` are immutable test inputs. `test-vault/` is the
sandbox (the sync node's daemon is syncing it, through the file sync, with the calendar
`dev`): the one place a trial of the real thing is made (§2). An automated test may
read it as input, but works on a copy in a temporary directory and never writes to the
folder itself (the file sync would carry the write to the sync node). Never commit
changes to it, never clean it up. What an agent writes there is
the plugin copy of step 4 and the lines a trial needs — task lines it adds for the
trial and says so in the session log, never an edit of a line that was there.

**When to stop and ask.** Implementation details the spec leaves open are yours to
decide — decide, record the decision, move on. Stop and ask the owner when the question
is what the *product* should do (a behaviour users would notice and that neither the
README nor the spec settles), when a change would be destructive for existing vaults or
server data, or when it needs the live server. Ask with the concrete options and your
recommendation.

## 6. Recovery

- **Gates red on files you did not touch:** find the commit that broke them
  (`git log`, `git bisect`) and fix it as its own task before continuing.
- **`docs/EXECUTION_STATE.md` missing or stale:** rebuild it from `git log` and the code;
  history is in git, the file is a convenience.
- **Unsure whether a sync change is safe:** write the scenario as a test in
  `tests/sync_engine.rs` first. If you cannot state the scenario, you do not understand
  the change yet.
