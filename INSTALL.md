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
2. Create a fresh `TODO.md` — if one already exists it is renamed to `TODO.pre-restask-<timestamp>.md` and its tasks are **not** migrated (manage them from the backup yourself in 0.1.0); tasks already on the bound calendar come back with the first sync.
3. Ask for your Radicale URL and credentials (password is stored only in `~/.config/restask/radicale.passwd`, mode `0600`).
4. Show the server's calendars and ask which one `TODO.md` binds to — type its name (e.g. `inbox`). This binding is required for sync; a wrong name re-prompts, and a server with no calendars aborts setup. Every other list is up to you: add `restask-list`/`restask-list-root` frontmatter to your notes and each list syncs to the same-named collection.
5. Run the first sync.
6. Install and enable the systemd user unit (`~/.config/systemd/user/restask.service`) pointing at this vault — the daemon starts immediately and at boot (Linux with systemd; other environments skip this and say so in the summary). Re-running setup refreshes the unit.

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

On any machine with a systemd user session, `restask setup` already installed and enabled the daemon (step 6 of the wizard): it starts immediately, restarts on failure, and survives logout/login (`loginctl enable-linger`).

Run setup **once per machine that owns a vault copy** — your PC and the home server may each run a daemon on their own copy; multiple daemons are safe (`.restask/` state is reconstructible, reconciliation is idempotent). Never run two daemons on the *same* vault folder.

**Adding the always-on server later:** let Syncthing carry the vault there (`restask.toml` + `.restask/` travel with it), then run `restask setup` inside the server's vault copy and re-enter the credentials — the password is machine-local by design. Setup recreates `TODO.md` (existing tasks return from the calendar on the first sync), so do it after the vault has fully synced.

Manual fallback (e.g. a machine where you skip the wizard):

```bash
mkdir -p ~/.config/systemd/user
cp contrib/restask.service ~/.config/systemd/user/   # edit the --vault path
systemctl --user daemon-reload && systemctl --user enable --now restask
loginctl enable-linger $USER
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
git pull && cargo install --path crates/restask --force   # update (the unit keeps working)
systemctl --user disable --now restask                    # stop the daemon
rm ~/.config/systemd/user/restask.service                 # remove the unit setup wrote
cargo uninstall restask                                   # remove binary
```

State lives entirely in `<vault>/.restask/` and `~/.config/restask/` — delete those to fully reset.
