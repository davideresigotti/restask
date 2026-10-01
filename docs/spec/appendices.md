# restask spec — Appendices B–E (Dependencies, Manifests, CI, Deployment)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## Appendix B — `crates/restask/Cargo.toml`

```toml
[package]
name = "restask"
version.workspace = true
edition = "2021"
license.workspace = true
description = "restask: Markdown checkboxes ⇄ VTODO (CalDAV) sync"

[dependencies]
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.9"
clap = { version = "4", features = ["derive"] }
fnv = "1.0"
globset = "0.4"
notify = "6"
regex = "1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
rpassword = "7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "signal"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
ulid = "1"

[dev-dependencies]
pretty_assertions = "1"
tempfile = "3"
```

Workspace `Cargo.toml`: `resolver = "2"`, `members = ["crates/restask"]`, version
`0.1.0`, edition 2021, `rust-version = "1.98"`, MIT. `Cargo.lock` is committed (this is an
application). Adding or changing a dependency is a recorded decision (`AGENTS.md` §4).

## Appendix C — Plugin manifests

`package.json`: scripts `lint = tsc --noEmit`, `test = vitest run`,
`build = tsc --noEmit && node esbuild.config.mjs`; dev dependencies `@types/node 22`,
`builtin-modules 5`, `esbuild 0.25`, `obsidian` (API typings), `typescript 5.6`,
`vitest 2.1`; lockfile committed. `manifest.json`: `id: "restask"`, `name: "restask"`,
`minAppVersion: "1.5.0"`, `isDesktopOnly: false`. `tsconfig.json`: `strict: true`,
ES2022, bundler resolution. `esbuild.config.mjs` bundles `src/main.ts` → `main.js` (cjs,
minified; `obsidian`, `@codemirror/state` and `@codemirror/view` external — Obsidian
provides all three at run time, and the CodeMirror typings come with the `obsidian`
package) and then copies `main.js`, `manifest.json` and `styles.css`
to `crates/restask/assets/obsidian/`. `plugins/obsidian/main.js` is git-ignored; the
copies under `assets/` are **committed** — the crate embeds them with `include_str!` so
`restask setup` can install the plugin (§13.2 step 1) from a binary built without Node
(`cargo install`, the Docker image). A plugin change is committed together with its
rebuilt copies; CI fails when they differ from a fresh build.

## Appendix D — CI (`.github/workflows/ci.yml`)

On push and pull request, three jobs on `ubuntu-latest`:

- **rust**: toolchain 1.98.1; `cargo fmt --all --check`;
  `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo test --workspace --locked`.
- **plugin** (`plugins/obsidian`, Node 22): `npm ci`, `npm run lint`, `npm test`,
  `npm run build`, then `git diff --exit-code` on `crates/restask/assets/obsidian` (the
  embedded bundle must be the one the sources build).
- **lua**: `luac5.4 -p neovim/lua/restask/*.lua`; `lua5.4 neovim/test/toggle_test.lua`.

`tests/e2e_server.rs` is `#[ignore]`d and never runs in CI.

## Appendix E — Deployment (`contrib/`)

- `restask.service` (systemd **user** unit, the manual counterpart of what `restask
  setup` installs): `ExecStart=%h/.cargo/bin/restask daemon --vault %h/Vault`,
  `Restart=on-failure`, `RestartSec=5`, `WantedBy=default.target`. `systemctl stop` sends
  `SIGTERM`; the daemon finishes its pass and exits 0.
- `docker/Dockerfile`: built **from the repository root**
  (`docker build -f contrib/docker/Dockerfile .`); `rust:1.98` build stage with
  `--locked`, `debian:bookworm-slim` runtime with `tzdata` (the daemon stamps local dates
  and shows other clients' timed dues in local time, so `TZ` must resolve),
  `USER 1000:1000`, `HOME=/home/restask`, `RESTASK_VAULT=/vault`,
  `CMD ["restask", "daemon"]`.
- `docker/docker-compose.yml`: build context `../..`; mounts the vault at `/vault` and a
  config directory at `/home/restask/.config/restask` (holding `config.toml` and
  `radicale.passwd`, 0600); `TZ`; `restart: unless-stopped`.
