# Taskres Spec — Appendices B–E (Dependencies, Manifests, CI, Deployment)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## Appendix B — `crates/restask/Cargo.toml` (normative; no other dependencies allowed)

```toml
[package]
name = "restask"
version.workspace = true
edition = "2021"
license.workspace = true
description = "Taskres: Markdown checkboxes ⇄ VTODO (CalDAV) sync"

[dependencies]
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.9"
fnv = "1.0"
globset = "0.6"
notify = "6"
regex = "1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
rpassword = "7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
ulid = "1"
walkdir = "2"

[dev-dependencies]
pretty_assertions = "1"
tempfile = "3"
```

Workspace `Cargo.toml`: `resolver = "2"`, `members = ["crates/restask"]`, `[workspace.package] version = "0.1.0"`, `edition = "2021"`, `rust-version = "1.98"`, `license = "MIT"`.

## Appendix C — Plugin manifests

`package.json` (scripts: `lint = tsc --noEmit`, `test = vitest run`, `build = tsc --noEmit && node esbuild.config.mjs`; devDependencies pinned: `@types/node 22`, `builtin-modules 5`, `esbuild 0.25`, `obsidian github:obsidianmd/obsidian-api`, `typescript 5.6`, `vitest 2.1`; commit the lockfile). `manifest.json`: `id: "taskres"`, `name: "Taskres"`, `minAppVersion: "1.5.0"`, `isDesktopOnly: false`. `tsconfig.json`: `strict: true`, `target: ES2022`, `module: ESNext`, `moduleResolution: bundler`. `esbuild.config.mjs`: bundles `src/main.ts` → `main.js` (external: `obsidian`, `builtin-modules`), format cjs, minify. `main.js` is git-ignored (built at install time).

## Appendix D — CI (`.github/workflows/ci.yml`)

Three jobs on `ubuntu-latest`, `on: push/PR`: **rust** (`rustup toolchain install 1.98.1 --profile default`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`); **plugin** (`actions/setup-node@v4` node 22, `npm ci`, `npm run lint`, `npm test`, `npm run build`, workdir `plugins/obsidian`); **lua** (`sudo apt-get install -y lua5.4`, `luac5.4 -p neovim/lua/restask/*.lua`). e2e job: none — `tests/e2e_server.rs` stays `#[ignore]`-gated.

## Appendix E — Deployment units (`contrib/`)

`restask.service` (systemd **user** unit): `ExecStart=%h/.cargo/bin/restask daemon --vault %h/Vault`, `Restart=on-failure`, `RestartSec=5`, `WantedBy=default.target`. Docker: `Dockerfile` = two-stage (`rust:1.98` build → `debian:bookworm-slim`, installs the binary, `USER 1000:1000`, `ENV RESTASK_VAULT=/vault`); `docker-compose.yml` mounts the Syncthing vault (`/opt/docker/syncthing/data/obsidian:/vault`) and a config volume (`./data:/home/restask/.config/restask`), `TZ` set, `restart: unless-stopped`, points at `http://<radicale-host>:5232`.
