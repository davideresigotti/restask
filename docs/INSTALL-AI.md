# Installing restask (detailed guide)

The full installation reference, for a coding agent or anyone who wants every option. The short, human version is [INSTALL.md](../INSTALL.md).

restask links every checkbox in the routed notes of your Markdown vault to an RFC 5545 VTODO on your own [Radicale](https://radicale.org/) server — so [Obsidian](https://obsidian.md), Neovim, Tasks.org and Thunderbird stay in sync, offline-first.

## 1. Install: three commands, on your computer

```bash
git clone <repo-url> restask && cd restask
cargo install --path crates/restask --locked     # needs Rust 1.98+ (https://rustup.rs)
cd /path/to/your/vault && restask setup
```

`restask setup` is the whole installation — for the computer, the server and the phone. It asks, once:

1. **The Radicale URL, username and password.** Use the address the server has on your network (`http://192.168.1.10:5232`), not `localhost`. A name works too (`https://radicale.example.org`), also one only your home network knows: setup tries the server from the always-on server before it changes anything, and when that machine does not know the name it gives its daemon the address the name has on this computer.
2. **Which calendars `TODO.md` shows** — a checklist of the server's calendars, all ticked: move with the arrows, untick the ones you do not want with the spacebar (`a` ticks or unticks all), Enter to confirm. When you chose more than one it asks **which of them new tasks go to**: a task typed in `TODO.md` without a calendar belongs to that one, a task of another carries `📁 <calendar>` on its line. Every task of the chosen calendars is brought into the vault — completed ones under `Done` — and stays the task its app created.
3. **The ssh host of your always-on server**, if one holds a copy of the vault. Press Enter if there is none: this computer then keeps the vault in sync itself. Setup connects to the server at once: if it logs you in with a password (or your key has a passphrase), ssh asks for it here, once — restask does not keep it.
4. **The vault's folder on that server** (only when you named one).

Then it does the rest by itself:

| Where | What setup puts there |
|---|---|
| The vault | `restask.toml`, the `.restask/` state folder, a fresh `TODO.md` (an existing one is renamed to `TODO.pre-restask-<timestamp>.md`), and the Obsidian plugin in `.obsidian/plugins/restask/`, enabled |
| This computer | the `restask` command (what Neovim calls); a first sync, so your tasks are there when it ends. With a server: nothing else — no daemon, no password kept |
| The server | the restask daemon as a Docker container, built there from this clone, given the credentials you just typed, started, and its first log lines shown |
| The phone | nothing to do: the plugin arrives with the vault |

Without a server the daemon is installed on this computer instead (a systemd user unit, started now and at login) and the password is stored in `~/.config/restask/radicale.passwd` (mode `0600`).

**If Obsidian is open on the vault while setup runs**, the plugin is turned on in it at the end. Obsidian reads a vault's plugins only when it opens the vault, so setup either tells it — when Obsidian's command line interface is on (Settings → General → Command line interface, Obsidian 1.12 or later), nothing closes — or asks `Restart Obsidian now? [Y/n]` and, on Enter, closes Obsidian and starts it again with the vaults it had open (Linux; elsewhere, and from an AppImage or a Flatpak, it asks you to restart it yourself). A vault in restricted mode stays that way: turn it off under Settings → Community plugins and the plugin is on.

### What the server needs

- `ssh <host>` works from this computer (a host from `~/.ssh/config`, or `user@address`), with a key or with a password you type when setup connects.
- Docker with the compose plugin.
- A copy of the vault, kept by your file sync (Syncthing: share the vault's folder with the server and let it finish). `.restask/` must be part of the share.

Setup checks all three before it changes anything. It then waits until the file sync has delivered the vault to the server exactly as it is here, and only then starts the daemon there. The first build on the server compiles restask and takes a few minutes; later updates take seconds.

### What the phone needs

Obsidian, with the vault from the same file sync (`.obsidian/` included). The plugin is already in it. A vault that has community plugins turned off asks you to turn them on once; if Obsidian was open on the phone during setup, close and reopen it there — setup reaches the Obsidian of the computer it runs on only.

## 2. What runs where, and why only one daemon

```
 phone / computer                       always-on machine
 edit the notes        file sync        restask daemon          CalDAV
 Obsidian plugin,   ◀──────────────▶    on its vault copy   ◀───────────▶   Radicale
 Neovim, any editor
```

| Machine | What runs there | Who does restask's work on the notes |
|---|---|---|
| Computer with Obsidian and Neovim | the `restask` command, the plugin, the Neovim integration | in Obsidian the plugin; in Neovim `restask settle`, on every save — at once and offline |
| Phone | Obsidian with the plugin | the plugin, at once and offline |
| Always-on machine (the *sync node*) | `restask daemon` | the daemon: it is the only thing that talks to Radicale |

**One daemon per vault, never two.** A daemon on the computer *and* one on the server would both carry changes between the vault and Radicale, each on its own copy of the files. Whenever one of them is faster than the file sync — which is nearly always — it writes into a note the file sync is about to replace, and you get `sync-conflict` copies of your notes. So restask runs the daemon in one place and makes everything else immediate without it: the plugin and `restask settle` give a task its ID, move a checked task under the done heading and keep `TODO.md` current on the spot. The daemon then carries the result to Radicale and brings back what changed there.

An edit made on the computer with something else — another editor, a script — is not lost: the daemon does the same work when the file reaches it, and the result comes back with the file sync. Run `restask settle` in the vault to have it at once.

## 3. How tasks are routed

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

A root note with a `# TODO` heading shows the prioritized tasks of its whole folder in that section, by priority, like a `TODO.md` of the folder (the rest of the note is left alone).

Unmarked notes are left completely untouched (local-only). Quick tasks live in `TODO.md` → the inbox list. Prioritized tasks from routed notes are mirrored into `TODO.md` automatically, and you can check them off there. Each list syncs with the calendar of the same name (`Home Lab` → `home-lab`), created on first sync.

```bash
restask lists     # every list the vault routes, with its note and the URL other apps connect to
```

## 4. Connect your other apps

All of them use the same URL pattern: `http://<radicale-host>:5232/<user>/<list>/` (`restask lists` prints them).

- **Tasks.org** (Android): Settings → Synchronization → Add account → CalDAV; server URL, username, password. Notes, reminders, tags and recurrence you set there are kept.
- **Thunderbird**: Calendar → New calendar → On the Network → CalDAV; paste the URL, check "offline support".
- **Obsidian**: installed by setup. What the plugin does:
  - a new task gets its ID when you leave the line, a task you check off moves under the done heading (and back), `TODO.md` and the notes follow each other — offline, on the phone as on the computer;
  - the `🆔 restask-…` token of a task line is hidden in the editor and in reading view (it stays in the file; *Settings → restask → Hide task IDs* shows it);
  - suggestions while typing (`hi` → high/highest, `du` → due, `created` → a creation date), a *Toggle task done* command to bind a hotkey to, and a new line under a `TODO` heading starts with `- [ ] `.
  - Leave *Settings → restask → Apply task changes on the device* on *every device*.
- **Neovim**: add `neovim/` to your runtimepath and call `require("restask").setup()` (needs `restask` on `PATH`).
  - Saving a vault note does restask's work at once and offline, and reloads the buffer with the result (`setup({ settle = false })` leaves it to the daemon).
  - `<leader>td` toggles the task under the cursor, `<leader>ta` adds one (`keymaps = false` turns them off).
  - The `🆔` tokens are concealed (`conceal = false`), a line opened under a `TODO` heading starts with `- [ ] ` (`start_tasks = false`), and with [blink.cmp](https://github.com/Saghen/blink.cmp) the same suggestions appear while typing (`suggest = false`).

## 5. Verify

```bash
restask doctor        # config, routing, vault scan; on the sync node also the server and its auth
restask status        # active tasks per list and priority, pending sync, last sync
restask settle        # the local work now, no server: IDs, Done, TODO.md; prints what it did
```

On the computer, `doctor` says which machine runs the daemon and how to read its log. To see the whole chain: type `- [ ] try it 🔺` under a routed note's `TODO` heading and leave the line (Obsidian) or save (Neovim). The line is in `TODO.md` at once, with the network off too, and in Tasks.org a few seconds after the file sync has carried the note to the server.

> ⚠ `doctor` warns hard if your CalDAV server accepts requests **without authentication** — fix that before exposing the server beyond your LAN.

## 6. Other setups

- **A second computer.** Install the `restask` command (the two commands of section 1) if you use Neovim there; nothing else. The vault brings the plugin, the server does the syncing, and no setup is run.
- **No always-on server.** Press Enter at the server question. The computer runs the daemon; the phone's edits reach Radicale whenever the computer is on.
- **A server without Docker.** Install by hand there, then tell the computer that the daemon lives elsewhere:

  ```bash
  # on the server (Rust and a systemd user session):
  git clone <repo-url> restask && cd restask && cargo install --path crates/restask --locked
  # on the computer, in the vault:
  restask setup --no-daemon
  # on the server, in its copy of the vault, once the file sync has delivered it:
  restask setup --join
  ```

  `setup --join` asks for the URL, username and password again, changes nothing in the vault, and installs the daemon as a user unit.
- **The server step failed** (ssh, Docker, the file sync not finished). The vault is set up; setup says what went wrong and prints the command that finishes the job. It is always this one, and it changes nothing in the vault:

  ```bash
  restask setup --join --node <ssh-host> --node-vault <vault folder on the server>
  ```

- **Where things go on the server.** `~/restask` of the ssh user: `docker-compose.yml`, `.env` (vault folder, user, time zone, the stack's name), `src/` (the sources), `data/` (the daemon's config and password, mode `0600`). `--node-dir <dir>` picks another directory. The container runs as the owner of the vault's files, in this computer's time zone.
- **A second vault.** Run `restask setup` in it, like in the first: each vault has its own task server (or its own calendars), its own daemon and its own settings, and they share nothing. On the server the second vault gets a stack beside the first — `~/restask-<vault folder>`, with a container and an image of that name — and setup finds a vault's stack again by itself, so a later run never starts a second daemon for the same vault. On the computer each vault has its own machine config: the first in `~/.config/restask/config.toml`, a further one in `~/.config/restask/vaults/<vault folder>/config.toml`. A command finds the right one from the vault it is run in. A daemon on the computer itself is one unit per vault (`restask.service`, `restask-<vault folder>.service`).
- **Without questions.** Every answer has a flag:

  ```bash
  RESTASK_CALDAV_PASSWORD=… restask setup --non-interactive \
      --url http://192.168.1.10:5232 --username me --password-env RESTASK_CALDAV_PASSWORD \
      --collection inbox=inbox --node myserver --node-vault /srv/sync/vault
  ```

  `--todo-list work` (repeatable) names a further calendar `TODO.md` shows.

- **Another calendar in `TODO.md`, later.** A task calendar you create in any app after setup is added by itself: the daemon puts its name into `todo_lists` in the vault's `restask.toml` and brings its tasks into `TODO.md` (calendars for events only are left out; `todo_new_lists = false` switches this off). For one that existed before, add its name yourself — `todo_lists = ["work"]`, next to `inbox_list` — and let the file sync carry the file. Use the name the calendar shows in your apps, in lowercase with dashes for spaces (`Home Lab` → `home-lab`): it does not matter what address the app gave it on the server. Two calendars with the same name are left alone until one is renamed.

- **Coming from an earlier install.** If the computer ran the daemon, or kept the server's password, run the command of *The server step failed* above: it installs the daemon on the server, rewrites this computer's config without credentials and removes its password file. Then stop the old unit: `systemctl --user disable --now restask`.

## 7. Day-to-day notes

- **Offline**: everything you do to the notes works offline, on every device; the server catches up when the file sync and the daemon meet again.
- **Latency**: a change made in Tasks.org reaches a note in three steps — Tasks.org uploads it (through DAVx⁵ that is Android's sync scheduler: never sooner than 30 s after the edit, and some phones hold it for minutes; a CalDAV account added in Tasks.org itself uploads at once), the daemon notices within 2 s and writes the vault on the server, and the file sync delivers the files. With Syncthing the last step is the longest: a folder's watcher waits `fsWatcherDelayS` (10 s by default) before it looks at a changed file. Set it to `1` for the vault's folder on the server (`syncthing cli config folders <id> fswatcher-delays set 1`; in the web UI under *Actions* → *Advanced* → *Folders* → *Fs Watcher Delay S*), and on your editing devices for the other direction.
- **File versioning**: Syncthing's file versioning can stay on. Its archive (`.stversions/` in the vault's folder) is hidden, and restask reads nothing hidden: the old versions of your notes there are not tasks.
- **Conflict copies**: if Syncthing leaves a `*.sync-conflict-*` file, restask ignores it and `doctor` reports it — merge what you need by hand and delete it.
- **Starting over**: `restask rebuild`, on the machine that runs the daemon, drops the sync bookkeeping (not your notes, not the server); the next sync re-derives it.

## Updating / uninstalling

From the clone on your computer, one command updates everything that runs restask:

```bash
git pull && contrib/update.sh
```

It reinstalls the `restask` command here, restarts this computer's daemons if it has any, refreshes the Obsidian plugin in every vault set up here (reload Obsidian to load it), then sends the sources to each server stack setup installed — one per vault — rebuilds the image and restarts the daemon there, and shows its first log lines. It fails if a daemon does not come up. (`contrib/update.sh <ssh-host> [<dir>]` names the server, or one stack on it, explicitly.)

Uninstalling:

```bash
ssh <host> 'cd restask && docker compose down --rmi local'   # the daemon on the server
systemctl --user disable --now restask                       # a daemon on this computer
rm ~/.config/systemd/user/restask.service                    # … and its unit
cargo uninstall restask                                      # the command
```

A second vault's daemon is `cd restask-<vault folder>` on the server and `restask-<vault folder>.service` on a computer.

State lives entirely in `<vault>/.restask/` and the machine config (`~/.config/restask/` on a computer — a further vault's under `vaults/` there —, the stack's `data/` on the server) — delete those to fully reset.
