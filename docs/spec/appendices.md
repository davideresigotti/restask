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
  setup` — and, on a further machine, `restask setup --join`, §13.2 — installs): `ExecStart=%h/.cargo/bin/restask daemon --vault %h/Vault`,
  `Restart=on-failure`, `RestartSec=5`, `WantedBy=default.target`. `systemctl stop` sends
  `SIGTERM`; the daemon finishes its pass and exits 0.
- `docker/Dockerfile`: built **from the repository root**
  (`docker build -f contrib/docker/Dockerfile .`); `rust:1.98.1` build stage (the tag is
  the toolchain pin; `rust-toolchain.toml` is not copied in) with `--locked` and BuildKit
  cache mounts for the cargo registry and `target/`, so a rebuild compiles what changed;
  `debian:trixie-slim` runtime (the build image's Debian release: the binary links
  against its glibc) with `tzdata` (the daemon stamps local dates and shows
  other clients' timed dues in local time, so `TZ` must resolve), `USER 1000:1000`,
  `HOME=/home/restask`, `RESTASK_VAULT=/vault`, `CMD ["restask", "daemon"]`; the image
  is labelled `org.opencontainers.image.title=restask`.
- `docker/docker-compose.yml`: the sync node as a stack. It is copied into a stack
  directory that holds `.env` (`RESTASK_VAULT_DIR`, the vault's folder on that machine,
  required; `RESTASK_USER`, `uid:gid` the container runs as, default `1000:1000`; `TZ`),
  `src/` (the sources; build context) and `data/` (mounted at
  `/home/restask/.config/restask`: `config.toml` and `radicale.passwd`, 0600, written by
  `restask setup --join` run in the container); mounts the vault at `/vault`;
  `restart: unless-stopped`.
- `node.sh` — the sync node driven over ssh, one connection per run (`ControlMaster`).
  Under `restask setup` that connection is the one setup opened when the host was named
  (§13.2 step 6): its `ControlPath` arrives in `RESTASK_SSH_CONTROL`, `check` and
  `install` share it, and the script leaves it open. Run by itself (`update.sh`) the
  script opens its own, where ssh asks what it needs, and closes it on exit.
  Remote paths are passed as quoted arguments, never spliced into a command line.
  - `check <host> <vault> [<dir>]` (setup's `prepare_node`): ssh works; `docker compose
    version` answers; `<vault>` is a folder; a stack already in `<dir>` mounts that
    folder (`docker compose config`), else it is refused; a running stack is stopped.
  - `install <host> <vault> <dir> <local-vault>` (setup's `install_node`; URL, username
    and password on standard input, one per line): creates `<dir>/data`, owned by the
    owner of the vault's files; when the stack has no compose file yet, writes `.env`
    (that owner, this machine's time zone — completion dates are the user's local days,
    whatever the server's clock says) and the compose file; replaces `src/` with the
    files the image is built from (tracked and new ones under `Cargo.toml`,
    `Cargo.lock`, `crates/`, the Dockerfile; without git, those paths minus `target/`);
    builds. Then the **barrier**: the SHA-256 of every `*.md`, of `restask.toml` and of
    `.restask/**` (not `lock`, conflict copies, temp files, other dot-directories) is
    compared between `<local-vault>` and the node's copy every 3 s, for up to
    `RESTASK_NODE_WAIT` (300) seconds; on a timeout the script fails and lists the files
    that differ. Only an equal copy is joined (`docker compose run --rm -T restask
    restask setup --join --non-interactive --url … --username … --password-stdin`),
    after which `up -d`, the log's last lines, and a failure unless the container runs.
    A pass over files still on their way would be the second writer of §1.1.
  - `update <host> [<dir>]`: `src/`, build, `up -d`, log, running check, and the images
    the build replaced are pruned by label.
  It never rewrites an existing compose file or `.env`. `<dir>` defaults to `restask`
  in the ssh user's home directory.
- `update.sh [<ssh-host> [<stack-dir>]]`: brings what runs restask up to the working
  tree. In order: `cargo install` when a `restask` is on `PATH`; restart of the user unit
  when it is enabled (the script fails if it is not `active` afterwards); the three
  plugin files into `<vault>/.obsidian/plugins/restask/` when they differ — vault from
  `RESTASK_VAULT`, else the unit's `--vault`, else the machine config; never over
  symlinks, never `data.json`; then `node.sh update` for the sync node — host and stack
  from the arguments, else `RESTASK_SERVER` / `RESTASK_SERVER_DIR`, else `[node]` of
  the machine config (§14.2); a host with no stack named anywhere means
  `/opt/docker/restask`; no host, no node step. It never writes `data/` or the stack's
  compose file, and never runs `restask setup`.
