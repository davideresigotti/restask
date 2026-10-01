# Installing restask

restask links every checkbox in the routed notes of your Markdown vault to an RFC 5545 VTODO on your own [Radicale](https://radicale.org/) server — so [Obsidian](https://obsidian.md), Neovim, Tasks.org and Thunderbird stay in sync, offline-first.

## 1. Requirements

- **Rust 1.98+** (engine): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- A **Markdown vault** (any folder; ideally synced across devices with Syncthing)
- A **Radicale** server (self-hosted CalDAV) with at least one calendar for the inbox

## 2. Install the engine

From a clone of this repository:

```bash
git clone <repo-url> restask && cd restask
cargo install --path crates/restask --locked
restask --version
```

## 3. First-time setup (one command)

Run the wizard **inside your vault**:

```bash
cd /path/to/your/vault
restask setup
```

The wizard will:

1. Create `restask.toml` (vault defaults) and the `.restask/` state directory, and install the Obsidian plugin into `.obsidian/plugins/restask/`, enabled in `.obsidian/community-plugins.json` (your other plugins and the plugin's settings are kept).
2. Create a fresh `TODO.md` — an existing one is renamed to `TODO.pre-restask-<timestamp>.md`. Its lines are not migrated: tasks already on the bound calendar come back with the first sync, anything else stays in the backup for you to move.
3. Ask for your Radicale URL and credentials (the password is stored only in `~/.config/restask/radicale.passwd`, mode `0600`).
4. Show the server's calendars and ask which one `TODO.md` binds to — type its name (e.g. `inbox`). A wrong name re-prompts; a server with no calendars aborts setup.
5. Run the first sync.
6. Install and enable the systemd user unit (`~/.config/systemd/user/restask.service`) pointing at this vault — the daemon starts immediately and at boot (Linux with systemd; other environments skip this and say so). Re-running setup refreshes the unit.

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

Unmarked notes are left completely untouched (local-only). Quick tasks live in `TODO.md` → the inbox list. Prioritized tasks from routed notes are mirrored into `TODO.md` automatically, and you can check them off there. Each list syncs with the collection of the same name (`Home Lab` → `home-lab`), created on first sync.

```bash
restask lists     # every list the vault routes, with its note and collection URL
```

## 4. Keep it running (sync node)

On any machine with a systemd user session, `restask setup` already installed and enabled the daemon (step 6): it starts immediately, restarts on failure, and survives logout (`loginctl enable-linger`). The always-on server daemon is the anchor: it keeps vault ⇄ Radicale converging even when every other device is off.

Your PC and the server may each run a daemon on their own copy of the vault. The daemon and CLI commands on one machine take turns automatically (a lock under `.restask/`).

**Adding the server daemon (no `setup` re-run):** Syncthing already carries the whole vault — `restask.toml` and `.restask/` included — so the vault itself needs nothing. Only the machine-local pieces must exist on the server, because secrets never ride the vault:

```bash
# 1. the binary (a clone + cargo install, or copy the built binary)
cargo install --path /path/to/restask/crates/restask --locked

# 2. the machine config, once, from the PC (endpoint + password, modes preserved)
rsync -a ~/.config/restask/ server:'.config/restask/'

# 3. the user unit, pointed at the server's vault copy
scp contrib/restask.service server:'.config/systemd/user/restask.service'
ssh server
  $EDITOR ~/.config/systemd/user/restask.service   # fix the --vault path
  systemctl --user daemon-reload && systemctl --user enable --now restask
  loginctl enable-linger $USER
```

The unit's `--vault` path decides which vault the daemon serves. Verify from the server's vault copy with `restask doctor`.

**Docker** (matches an `/opt/docker` style server — see `contrib/docker/docker-compose.yml`):

```bash
cd contrib/docker
# edit docker-compose.yml: vault path, and set TZ to your timezone
mkdir -p data    # put config.toml and radicale.passwd (chmod 600) here
docker compose up -d --build
```

Set `TZ`: completion dates are stamped in local time, and timed due dates created in other apps are shown in local time.

## 5. Connect your clients

All clients use the same URL pattern: `http://<radicale-host>:5232/<user>/<list>/` (`restask lists` prints them).

- **Tasks.org** (Android): Settings → Synchronization → Add account → CalDAV; enter the server URL, username, password. Tasks appear under each list name. Notes, reminders, tags and recurrence you set there are kept.
- **Thunderbird**: Calendar → New calendar → On the Network → CalDAV; paste the URL, check "offline support".
- **Obsidian**: nothing to install — `restask setup` put the plugin in the vault and enabled it, and it reaches your other devices with the vault if `.obsidian/` is synced. Reload Obsidian if the vault was open during setup; a vault that has community plugins turned off (restricted mode) asks you to turn them on once. Re-run `restask setup` after updating restask to refresh the plugin (or copy `crates/restask/assets/obsidian/*` into `<vault>/.obsidian/plugins/restask/` by hand). It adds metadata suggestions while typing (`hi` → high/highest) and a *Toggle task done* command (bind a hotkey). The plugin is optional: checking a box by hand works too, the daemon tidies it up.
- **Neovim**: add `neovim/` to your runtimepath and call `require("restask").setup()`; `<leader>td` toggles the task under the cursor, `<leader>ta` adds one (requires `restask` on PATH).

## 6. Verify

```bash
restask doctor        # config, routing, vault scan, Radicale reachability and auth
restask status        # active tasks per list and priority, pending sync, last sync
restask sync          # one pass now; prints what it did
```

> ⚠ `doctor` warns hard if your CalDAV server accepts requests **without authentication** — fix that before exposing the server beyond your LAN.

## 7. Day-to-day notes

- **Offline**: `restask add`, `done` and `undone` always save to the vault; the server catches up at the next sync. A phone needs nothing but the notes.
- **Conflict copies**: if Syncthing leaves a `*.sync-conflict-*` file, restask ignores it and `doctor` reports it — merge what you need by hand and delete it.
- **Starting over**: `restask rebuild` drops the sync bookkeeping (not your notes, not the server); the next sync re-derives it.

## Updating / uninstalling

```bash
git pull && cargo install --path crates/restask --locked --force   # update (the unit keeps working)
systemctl --user restart restask                                    # run the new binary
systemctl --user disable --now restask                              # stop the daemon
rm ~/.config/systemd/user/restask.service                           # remove the unit setup wrote
cargo uninstall restask                                             # remove the binary
```

State lives entirely in `<vault>/.restask/` and `~/.config/restask/` — delete those to fully reset.
