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

The maintainer's Radicale lives at `http://192.168.1.10:5232` (user `me`, auth
enforced). **Never contact it** from tests or while developing: everything runs against
`MockCaldav` (`crates/restask/tests/common/mod.rs`). The only exception is the
`#[ignore]`d `tests/e2e_server.rs`, which the owner runs by hand and which touches the
`restask-dev` collection only. Never read `~/.config/restask/radicale.passwd`.

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
- `std::process::Command` is allowed only in `setup.rs` (systemd unit install).
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

**TypeScript** (plugin): `strict: true`; zero runtime dependencies; Obsidian API only in
`src/main.ts` and `src/settings.ts`; everything else pure and tested under Vitest. The
plugin edits Markdown and nothing else — no state files, no network.

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

**Fixtures and the sandbox.** `crates/restask/tests/fixtures/` and
`plugins/obsidian/test/fixtures/` are immutable test inputs. `test-vault/` is the
maintainer's manual sandbox (a real `restask` may be syncing it): never point a test at
it, never commit changes to it, never clean it up.

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
