# Installing restask
If you are using an AI agent to install restask, point it to [docs/INSTALL-AI.md](docs/INSTALL-AI.md): it has every step and option.

# What you need
- **A CalDAV server** such as [Radicale](https://radicale.org/), reachable from your computer and your phone.
- **A file sync** for the vault between your devices, e.g. Syncthing.
- **An always-on machine** (optional) with `ssh` and Docker, holding a copy of the vault. It runs the daemon, the only part that talks to the server. Without one, your computer runs it.
- **Rust 1.98+** on your computer ([rustup.rs](https://rustup.rs)).

# Install
Three commands, on your computer:

```bash
git clone https://github.com/Davide-Resigotti/restask.git && cd restask
cargo install --path crates/restask --locked
cd /path/to/your/vault && restask setup
```

`restask setup` is the whole installation. It asks, once:
1. **The server's URL, username and password.** Use the address it has on your network (`http://192.168.1.10:5232`), not `localhost`.
2. **Which calendars `TODO.md` shows**, and which of them receives new tasks.
3. **The ssh host of your always-on machine.** Press Enter if there is none.

Then it sets up everything by itself: the config and a fresh `TODO.md` in the vault, the Obsidian plugin, the `restask` command, and the daemon.

If Obsidian is open on the vault, setup may ask to restart it so the plugin loads. If the plugin is missing, copy `main.js`, `manifest.json` and `styles.css` from `crates/restask/assets/obsidian/` into `<vault>/.obsidian/plugins/restask/` and enable it.

# Phone
Open the same vault, synced with your file sync (`.obsidian/` included). The plugin is already in it: turn on community plugins when Obsidian asks.

# Connect your other apps
Every list is a calendar at `http://<radicale-host>:5232/<user>/<list>/`. `restask lists` prints them.
- **Tasks.org** (Android): Settings → Synchronization → Add account → CalDAV or DAVx5.
- **Thunderbird**: Calendar → New calendar → On the Network → Your credentials.
- **Neovim**: add `neovim/` to your runtimepath and call `require("restask").setup()`.

# Check
```bash
restask doctor
```
Then type `- [ ] try it 🔺` under a `TODO` heading in a routed note: it should show up in `TODO.md` at once, and in Tasks.org a few seconds after the file sync has delivered the note.

> ⚠ `doctor` warns you if your CalDAV server accepts requests without authentication. Fix that before exposing the server beyond your LAN.

# Update
```bash
git pull && contrib/update.sh
```

# Uninstall
```bash
ssh <host> 'cd restask && docker compose down --rmi local'   # the daemon on the server
cargo uninstall restask                                      # the command
```
To fully reset, delete `<vault>/.restask/` and `~/.config/restask/`.
