# Installing Taskres

Taskres links every routed Markdown checkbox in your [Obsidian](https://obsidian.md) vault to an RFC 5545 VTODO on your own [Radicale](https://radicale.org/) server — so Tasks.org, Thunderbird, and Obsidian stay in sync, offline-first.

## 1. Requirements

- **Rust 1.98+** (engine): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- A **Markdown vault** (any folder; ideally synced across devices with Syncthing)
- Optional: a **Radicale** server for CalDAV (self-hosted), **Syncthing** for vault sync

## 2. Install the engine

From a clone of this repository:

```bash
git clone <repo-url> restask && cd restask
cargo install --path crates/restask
restask --version
```

## 3. First-time setup (one command)

Run the wizard **inside your vault**:

```bash
cd /path/to/your/vault
restask setup
```

The wizard will:

1. Create `restask.toml` (vault defaults) and the `.restask/` state directory.
2. Adopt an existing `TODO.md` — the old file is backed up to `TODO.pre-restask-<timestamp>.md`, and its checkbox lines are migrated into the new `## Inbox`.
3. Ask for your Radicale URL and credentials (password is stored only in `~/.config/restask/radicale.passwd`, mode `0600`).
4. Show the server's calendars and ask which one `TODO.md` binds to — type its name (e.g. `inbox`). This binding is required for sync; a wrong name re-prompts, and a server with no calendars aborts setup. Every other list is up to you: add `restask-list`/`restask-list-root` frontmatter to your notes and each list syncs to the same-named collection.
5. Run the first sync.

### How tasks are routed

Add frontmatter to a note to route **that note**:

```yaml
---
restask-list: University
---
```

or to route **the whole folder** (this note becomes the list's root note):

```yaml
---
restask-list-root: Home Lab
---
```

Unmarked notes are left completely untouched (local-only). Quick tasks live in `TODO.md` → the **Inbox** list. Prioritized tasks from routed notes are mirrored into `TODO.md` automatically.

## 4. Keep it running (sync node)

Run the daemon on one always-on device that has the vault (e.g. your home server).

**systemd (user unit):**

```bash
mkdir -p ~/.config/systemd/user
cp contrib/restask.service ~/.config/systemd/user/   # edit the --vault path
systemctl --user daemon-reload && systemctl --user enable --now restask
```

**Docker** (matches an `/opt/docker` style server — see `contrib/docker/docker-compose.yml`):

```bash
cd contrib/docker
# edit docker-compose.yml: vault path, Radicale URL, timezone
docker compose up -d
```

## 5. Connect your clients

All clients use the same URL pattern: `http://<radicale-host>:5232/<user>/<list>/`

- **Tasks.org** (Android): Settings → Synchronization → Add account → CalDAV; enter the list URL, username, password. Tasks appear under each list name.
- **Thunderbird**: Calendar → New calendar → On the Network → CalDAV; paste the URL, check "offline support".
- **Obsidian**: copy `plugins/obsidian` into `<vault>/.obsidian/plugins/taskres/`, `npm install && npm run build` there, enable "Taskres" in Community Plugins. Provides task autocomplete (`hi` → high/highest) and offline completion mirroring.
- **Neovim**: add `neovim/lua/restask` to your runtimepath; `<leader>td` toggles the task under the cursor (requires `restask` on PATH).

## 6. Verify

```bash
restask doctor        # config, routing, vault scan, Radicale reachability
restask status        # task counts per list, pending outbox
```

> ⚠ `doctor` warns hard if your CalDAV server accepts requests **without authentication** — fix that before exposing the server beyond your LAN.

## Updating / uninstalling

```bash
git pull && cargo install --path crates/restask --force   # update
systemctl --user disable --now restask                    # stop
cargo uninstall restask                                   # remove binary
```

State lives entirely in `<vault>/.restask/` and `~/.config/restask/` — delete those to fully reset.
