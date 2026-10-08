# Contributing to restask

Thanks for helping. restask edits people's notes, so changes to the sync core are reviewed carefully, but small fixes, docs and bug reports are always welcome.

## Before you start
- Open an issue first for anything bigger than a small fix, so we can agree on the behaviour.
- Read [ARCHITECTURE.md](ARCHITECTURE.md): its invariants are the review checklist. Details live in [docs/spec/](docs/spec/).
- [AGENTS.md](AGENTS.md) is the working contract (rules, quality gates, sandbox). It applies to humans too.

## You never need a real server
All tests run against `MockCaldav` and temporary directories. Do not point anything at your own calendars while developing; use a throwaway vault and a test calendar if you try the real binary.

## Trying your change: `test-vault/`
`test-vault/` is the sandbox for trying the real thing (the built `restask` binary, the Obsidian plugin, Neovim) by hand. Contributors are expected to use it:
- It is a small mockup vault: `TODO.md` (the inbox view), `Project Alpha.md` (one routed note), `Homelab/` (a root note plus a note below it, with subtasks, priorities, dates, a repeating task and a wikilink) and `Ideas.md` (not routed, so never touched). The notes carry no `🆔` yet: the first pass creates the tasks in your `dev` calendar.
- Open `test-vault/` as the vault, and sync it with a test calendar named `dev` on your own CalDAV server. Never use a vault or calendar that holds real data.
- Add whatever task lines your trial needs. Don't edit or delete the lines that are already there, and don't commit your trial's changes to the folder.
- Automated tests (`cargo test`, Vitest, the Lua tests) can use `test-vault/` as ready-made input. Copy it into a temporary directory first and never write to the folder itself. They run against `MockCaldav`, so they need no server at all.

## Quality gates (all must pass)
```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
If you touched `plugins/obsidian` (`npm ci` once): `npm run lint && npm test && npm run build` (commit the refreshed `crates/restask/assets/obsidian/`).
If you touched `neovim/`: `luac -p neovim/lua/restask/*.lua && lua neovim/test/toggle_test.lua`.

## Pull requests
- One focused change per PR; the maintainer may squash it.
- A behaviour change updates the spec (`docs/spec/`) in the same PR.
- A bug fix includes a regression test (`tests/sync_engine.rs` or `tests/sync_planner.rs` for sync bugs).
- A change to a local rule must be made in the engine **and** the Obsidian plugin, with a test on each side (see "Local parity" in AGENTS.md).
- No new dependency without discussion. Never commit secrets.

## Your execution state file stays private
Keeping a working log in `docs/EXECUTION_STATE.md` is handy: where your work stands, open items, decisions, what you saw in a trial. [AGENTS.md](AGENTS.md) tells coding agents to read and update it. It is yours alone: it ends up naming your machines, server, vault and calendars, so it is listed in `.gitignore` and must never be committed, force-added or pasted into an issue or pull request. Put what others need in the commit message, the spec or `CHANGELOG.md`.

By contributing you agree that your work is released under the [MIT licence](LICENSE).
