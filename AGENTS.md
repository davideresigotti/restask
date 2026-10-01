# AGENTS.md — Restask Implementation Contract

> **You are the implementation agent for Restask, executed by OpenCode (GLM-5.3-Flash).**
> `ARCHITECTURE.md` (index + invariants) and `docs/spec/*.md` (details) are normative.
> This file is your operating contract. This file is **static** — never edit it.
> If this contract and the specification conflict: STOP and use the Stop-and-Clarify template (§5).

## §0 Prime Directives (read first, always)

1. **Token conservation.** Never scan, list, or dump the repository tree. Never read a file that is not in the current task's file list. Never read `Cargo.lock`, `target/`, `node_modules/`, or log files. Read ONLY the `docs/spec/<file>.md` named by the current task (map in §6) — never the whole spec set, never whole `ARCHITECTURE.md` (it is an index; consult it only if the task's spec file lacks a needed cross-reference).
2. **Minimal diffs.** Use targeted string edits, never whole-file rewrites of existing files. Never reformat code you did not touch. Never paste file contents into your replies — report only what changed.
3. **Narrow verification.** Run ONLY the isolated test command named in the current task. Full suites run in CI and in T30 — never before. Never run `cargo clean`.
4. **Living state.** `docs/EXECUTION_STATE.md` (< 150 lines) is your only session memory. Read it first, update it last, commit with it. Never inspect git history (`git log`) to recover state.
5. **Zero hallucination.** If an edge case, API signature, crate version, or environment detail is not defined in `ARCHITECTURE.md` or the task: HALT and use §5. Never invent defaults, URLs, versions, or file paths.
6. **Fixtures are read-only.** `test-vault/` is never modified, never synced, only read via `include_str!`. `.restask/` inside it must never be created.
7. **Commits are mandated** by the human owner: one commit per completed task. Never push, never amend, never force.

## §1 Environment & Toolchain (verified on maintainer machine)

| Tool | Version | Notes |
|---|---|---|
| Rust | **1.98.1** (pinned in `rust-toolchain.toml`, profile `default`) | fmt+clippy included |
| Node / npm | **22 / 10** | plugin only, workdir `plugins/obsidian` |
| `luac` | any ≥ 5.4 (Fedora: `dnf install lua`) | syntax gate for neovim/ |
| git | 2.x | `main` branch |

External services (development only, read-mostly): Radicale `http://192.168.1.10:5232` user `me` (⚠ auth currently disabled — see ARCHITECTURE.md §17), SSH alias `docker`. E2E tests use the dedicated `Restask-Dev` collection only (env-gated, `#[ignore]`).

## §2 Quality Gates — exact commands

Rust (workdir: repo root):

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test -p restask --test <suite>          # isolated suite — the default gate
```

Plugin (workdir: `plugins/obsidian`):

```
npm ci                                        # first session: npm install, then commit the lockfile
npm run lint                                  # tsc --noEmit
npm test -- test/<file>.test.ts               # single suite
npm run build                                 # T27+ only
```

Lua: `luac -p neovim/lua/restask/*.lua` (CI uses `luac5.4`).

Full-suite policy: `cargo test --workspace` and bare `npm test` are run **only** in T30. All gates must pass before a task's commit.

## §3 Implementation Rules

**Rust** (crate `restask`, edition 2021, MSRV 1.98):
- Dependencies: **only** those in ARCHITECTURE.md Appendix B, at those versions. Adding/changing any dependency = Stop-and-Clarify.
- No `unsafe`. No `.unwrap()`/`.expect()` outside `#[cfg(test)]` and `main.rs` bootstrap. Errors: `RestaskError` (§12.1) via `?`.
- Pure modules (`domain`, `router`, `markdown`, `vtodo`, `sync::planner`) perform **zero I/O**; time comes from `Clock` or parameters.
- Async only where the architecture shows it (caldav/daemon/engine); no blocking calls inside async fns; `std::fs` is allowed only in `store`, `sync::engine`, `setup`, `daemon`.
- `std::process::Command` is allowed **only** for `ssh` in `setup.rs`/doctor.
- All public items documented with `///`; tests never depend on wall-clock time (`FixedClock`) or network (`MockCaldav` in `tests/common/mod.rs`).
- Rustfmt default settings; line length 100 (`rustfmt.toml` not used).

**TypeScript** (plugin): `strict: true`; zero runtime dependencies (only dev deps in App. C); Obsidian API imports only in `src/main.ts`/`src/settings.ts`.

**General**: UTF-8 everywhere; emoji handled as string literals from the exact codepoint tables (§3.2, §6.1) — never escape sequences that can drift. No `TODO`/`FIXME`/placeholder comments anywhere. Every task leaves the repo building and its suite green.

## §4 Execution Workflow (every session, every task)

1. **Restore**: read `docs/EXECUTION_STATE.md`. Identify `Current Task` ID. Do not read anything else.
2. **Targeted read**: read ONLY the spec sections named by the task and the files it lists (if they exist).
3. **Implement**: create/modify exactly the files the task lists — nothing else. Follow §3.
4. **Verify**: write the task's test file cases, run the isolated command (§2). Fix until green, then run `cargo fmt --all` (or `npm run lint`) and the clippy gate.
5. **Close**: update `docs/EXECUTION_STATE.md` (Current Task → next ID; append Session Log line, Verified Commands, Touched Files; resolve blockers). Commit **only** the task's files + `docs/EXECUTION_STATE.md` with message `T##: <short-slug>`.

`EXECUTION_STATE.md` update protocol (hard cap 150 lines — trim Session Log to 5 entries first, then Decision Log to 10):

```markdown
## Current Task
- ID: T##
## Session Log          (latest first)
- YYYY-MM-DD | T## | <one line: what was done / blocker>
## Verified Commands
- cargo test -p restask --test <suite> — green
## Touched Files
- path/to/file.rs
## Open Blockers
- (none) | STOP-AND-CLARIFY: <topic> (awaiting human)
## Decision Log
- D##: <decision> (<source: user|spec|discovery>)
```

## §5 Stop-and-Clarify Template (copy verbatim; then STOP — no file mutations)

```text
⛔ STOP — CLARIFICATION REQUIRED (Task <ID>: <title>)

Ambiguity: <one sentence: the undefined edge case, API signature, or environment detail>

Option A: <description>
  + <pro>
  - <con>
Option B: <description>
  + <pro>
  - <con>

Recommended: <A|B> — <one-line justification>

Files on hold: <paths>
Spec reference: ARCHITECTURE.md §<n>
Awaiting human confirmation before any file mutation.
```

Also record the blocker in `docs/EXECUTION_STATE.md` (`Open Blockers`), commit the state file, and end the session.

## §6 Atomic Checklist (chronological; one responsibility per task)

Every task: implement the listed files → write the listed test cases → run the isolated command → gates (fmt/clippy or lint) → update `EXECUTION_STATE.md` → commit `T##: <slug>`. Spec refs are section numbers inside `docs/spec/` files — read ONLY the file(s) mapped to the task:

| Spec file (read ONLY this) | Sections | Tasks |
|---|---|---|
| `docs/spec/overview.md` | §1–2 | T01 |
| `docs/spec/domain.md` | §3–4 | T03, T04, T13 |
| `docs/spec/routing.md` | §5 | T05 |
| `docs/spec/markdown.md` | §6–7 | T06–T10 |
| `docs/spec/vtodo.md` | §8, App. A | T11–T13 |
| `docs/spec/storage.md` | §9 | T14–T15 |
| `docs/spec/caldav.md` | §10 | T16–T17 |
| `docs/spec/sync.md` | §11 | T18–T19 |
| `docs/spec/runtime.md` | §12–13 | T20–T23 |
| `docs/spec/config.md` | §14, §17 | T02, T23 |
| `docs/spec/integrations.md` | §15–16 | T24–T28 |
| `docs/spec/appendices.md` | App. B–E | T01, T24, T29 |

### Phase 0 — Scaffold

| ID | Deliverable (spec) | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T01 | Repo scaffold, workspace, CI, license | `git init`; `.gitignore` (target/, node_modules/, `.restask/`, `main.js`, `*.restask-tmp`, `TODO.pre-restask-*.md`); `.editorconfig`; `rust-toolchain.toml`; `Cargo.toml`; `crates/restask/Cargo.toml` (App. B verbatim); `src/lib.rs` (empty lib); `src/main.rs` (clap `restask` skeleton, `--version` only); `src/logging.rs` (tracing init); `.github/workflows/ci.yml` (App. D); `LICENSE` (MIT) | — | `cargo build --workspace && cargo test --workspace` (builds, 0 tests) |
| T02 | Config types + loaders (§14) | `src/config.rs`; `contrib/config.example.toml`; `contrib/restask.example.toml` | `tests/config.rs` (defaults, round-trip, literal-password rejection, globset matcher, env overrides) | `cargo test -p restask --test config` |
| T03 | UID + Priority (§3.1–3.2) | `src/domain/mod.rs`; `src/domain/uid.rs`; `src/domain/priority.rs` | `tests/domain_uid.rs` (generate/parse/format/invalid); `tests/domain_priority.rs` (emoji⇄enum⇄ical both directions, all 11 ical inputs) | `cargo test -p restask --test domain_uid --test domain_priority` |
| T04 | Dates, When, Clock, Task, ListSlug (§3.3–3.5) | `src/domain/dates.rs`; `src/domain/task.rs` (incl. `ListSlug`, `Task`, `thumbprint`) | `tests/domain_dates.rs` (parse/format/ical, date vs datetime); `tests/domain_task.rs` (thumbprint stability + sensitivity, slug case) | `cargo test -p restask --test domain_dates --test domain_task` |

### Phase 1 — Routing

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T05 | Note routing (§5) | `src/router.rs` (`scan_frontmatter`, `Router::build/resolve`, `ListConflict`) | `tests/router.rs` (all §5.3 cases, precedence chain, shadowing, both-markers, conflicts, unmarked → LocalOnly) | `cargo test -p restask --test router` |

### Phase 2 — Markdown

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T06 | Line grammar (§6.1) | `src/markdown/mod.rs`; `src/markdown/parser.rs` (line regex, metadata tokens, text extraction) | `tests/markdown_parser.rs` (grammar table incl. CRLF, `[X]`, `*`/`+` markers, non-tasks, unknown emoji ignored) | `cargo test -p restask --test markdown_parser` |
| T07 | File-level parsing (§6.2) | extend `src/markdown/parser.rs` (frontmatter, fences, headings, done region, `link_parents`); fixture reads via `include_str!("../../../test-vault/…")` | append `tests/markdown_parser.rs` (both vault notes + TODO.md: done split, headings, subtask parents, fenced ```tasks block ignored) | `cargo test -p restask --test markdown_parser` |
| T08 | Line mutations (§6.3) | `src/markdown/mutator.rs` (`Register`, `SetStatus`, `SetPriority`, `SetWhen`, `EditText`; canonical tail order; text preservation) | `tests/markdown_mutator.rs` | `cargo test -p restask --test markdown_mutator` |
| T09 | Done-region ops + atomic write (§6.3) | extend `src/markdown/mutator.rs` (`MoveToDone` newest-on-top, `RestoreFromDone`, `Delete`, heading creation, `write_atomic`) | append `tests/markdown_mutator.rs` (incl. tmp+rename semantics, line-ending preservation) | `cargo test -p restask --test markdown_mutator` |
| T10 | TODO.md view (§7) | `src/markdown/todo_view.rs` (`MARKER`, `render`, `mirror_line`, `inbox_line`) | `tests/todo_view.rs` (render full fixture scenario; empty-section omission; ordering; wikilink rules incl. hostile stems) | `cargo test -p restask --test todo_view` |

### Phase 3 — VTODO

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T11 | Serializer + golden (§8.1, App. A) | `src/vtodo/mod.rs`; `src/vtodo/serialize.rs`; `docs/contracts/vtodo-golden.ics` (copy App. A byte-exact, CRLF) | `tests/vtodo_codec.rs` (golden byte-equality; folding >75 octets incl. multi-byte emoji; escaping; property order) | `cargo test -p restask --test vtodo_codec` |
| T12 | Parser + round-trip (§8.2) | `src/vtodo/parse.rs` (`RemoteTask`, unfolding, TZID/UTC/floating, unknown props skipped, VTIMEZONE/VALARM skipped) | append `tests/vtodo_codec.rs` (round-trip property; foreign UID flagging; X-RESTASK-SOURCE) | `cargo test -p restask --test vtodo_codec` |
| T13 | Timestamp contract (§4) | no new source files — conformance layer | `tests/vtodo_timestamps.rs` (every §4 row, both directions, FixedClock; midnight-UTC synthesis; UTC-date reverse; TZID→local) | `cargo test -p restask --test vtodo_timestamps` |

### Phase 4 — Storage

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T14 | Index + tombstones (§9.1) | `src/store/mod.rs`; `src/store/index.rs`; `src/store/tombstones.rs` | `tests/store.rs` (atomic save/load, missing→empty, prune) | `cargo test -p restask --test store` |
| T15 | Cache + outbox (§9.2) | `src/store/cache.rs`; `src/store/outbox.rs` | `tests/cache.rs`; `tests/outbox.rs` | `cargo test -p restask --test cache --test outbox` |

### Phase 5 — CalDAV

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T16 | XML protocol (§10.1) | `src/caldav/mod.rs`; `src/caldav/protocol.rs` | `tests/caldav_protocol.rs` (canned Radicale-style multistatus: etags, collections, prefix-agnostic parsing, unescape) | `cargo test -p restask --test caldav_protocol` |
| T17 | Port + client + retry (§10.2–10.4) | `src/caldav/port.rs`; `src/caldav/client.rs`; `tests/common/mod.rs` (`MockCaldav`, `FixedClock`, temp-vault helper) | `tests/caldav_client.rs` (in-process `std::net::TcpListener` mock: MKCOL/REPORT/GET/PUT etag flows, 401/412/5xx handling, backoff budget, Basic header) | `cargo test -p restask --test caldav_client` |

### Phase 6 — Sync

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T18 | Planner R0–R10 (§11) | `src/sync/mod.rs`; `src/sync/planner.rs` | `tests/sync_planner.rs` (one test per rule row + defer counter + tie-window 120 s + move + adoption) | `cargo test -p restask --test sync_planner` |
| T19 | Engine (§11, §13) | `src/sync/engine.rs` (scan/register, reconcile, fs-change path, complete/uncomplete/add) | `tests/sync_engine.rs` (tempdir routed vault + `MockCaldav`: full converge scenarios, tombstones, moves, outbox park/flush) | `cargo test -p restask --test sync_engine` |
| T20 | Daemon (§13.1) | `src/daemon.rs` (`run_once`, `run`, debounce, shutdown) | `tests/daemon.rs` (run_once end-to-end; debounce coalescing pure test; no real watcher timing deps) | `cargo test -p restask --test daemon` |

### Phase 7 — CLI

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T21 | CLI core (§13.3) | `src/cli.rs` (+ `src/main.rs` wiring; generic `run_with<C: CaldavPort>`) | `tests/cli.rs` (add/done/undone/status/sync/rebuild against tempdir + MockCaldav) | `cargo test -p restask --test cli` |
| T22 | Setup wizard (§13.2) | `src/setup.rs` (`parse_radicale_compose`, `adopt_todo_md`, non-interactive plan); `src/tui.rs` (prompt/select/secret — thin, untested) | `tests/setup_wizard.rs` (compose parse incl. real tomsquest compose text; TODO.md adoption/backup/migration; non-interactive full setup with MockCaldav incl. MKCOL + binding) | `cargo test -p restask --test setup_wizard` |
| T23 | Doctor + exit codes (§13.3, §12.1) | extend `src/cli.rs` | append `tests/cli.rs` (each check, no-auth hard warning, exit codes 0/1/3/4) | `cargo test -p restask --test cli` |

### Phase 8 — Obsidian plugin

| ID | Deliverable | Files | Test file | Isolated verify (workdir `plugins/obsidian`) |
|---|---|---|---|---|
| T24 | Scaffold + grammar port (§15.1) | `package.json`, `tsconfig.json`, `esbuild.config.mjs`, `manifest.json`, `styles.css`, `src/markdown.ts` (run `npm install`, commit lockfile) | `test/markdown.test.ts` (same cases as T06, fixture copies) | `npm test -- test/markdown.test.ts` |
| T25 | Autocomplete modal (§15.2) | `src/modal.ts` | `test/modal-filter.test.ts` (≥2-letter filter, `hi`→high+highest only, today/tomorrow) | `npm test -- test/modal-filter.test.ts` |
| T26 | Cache writer (§15.3) | `src/vtodo.ts` | `test/vtodo.test.ts` (byte-parity with `docs/contracts/vtodo-golden.ics`) | `npm test -- test/vtodo.test.ts` |
| T27 | Commands + settings (§15.4) | `src/main.ts`; `src/settings.ts` | — (logic covered by T24–T26) | `npm run lint && npm run build` |

### Phase 9 — Packaging & release

| ID | Deliverable | Files | Test file | Isolated verify |
|---|---|---|---|---|
| T28 | Neovim integration (§16) | `neovim/lua/restask/init.lua`; `neovim/lua/restask/toggle.lua` | — | `luac -p neovim/lua/restask/*.lua` |
| T29 | Deployment + docs (App. E) | `contrib/restask.service`; `contrib/docker/{Dockerfile,docker-compose.yml}`; update `README.md` (lists/routing section per §5, INSTALL pointers); review `INSTALL.md` | — | `cargo build --release` |
| T30 | Release gate + e2e | `crates/restask/tests/e2e_server.rs` (`#[ignore]`, env `RESTASK_E2E_URL/USERNAME/PASSWORD`, dedicated `Restask-Dev` collection only) | — | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; `npm run lint && npm test && npm run build` (workdir `plugins/obsidian`); `luac -p neovim/lua/restask/*.lua`; optional live: `cargo test -p restask --test e2e_server -- --ignored` |

## §7 Recovery Playbook

- **Task half-done at session end**: note exact status in `EXECUTION_STATE.md` Session Log; next session resumes by reading only the task's file list.
- **Isolated suite red**: fix within the task's files; if the root cause lies in an earlier task's module, fix it only if the fix keeps that module's own suite green, and log a Decision entry.
- **fmt/clippy drift on untouched files**: STOP — that means an earlier task committed non-compliant code; report via Stop-and-Clarify.
- **`EXECUTION_STATE.md` missing/corrupt**: re-seed from this file's §4 template with `Current Task: T01` and rely on git status of task files; never guess progress from `git log -p`.
- **Server unavailable**: all engine/planner/client tests use `MockCaldav` — never "temporarily" hit the live server outside `e2e_server.rs`.

