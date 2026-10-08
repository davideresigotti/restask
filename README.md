# Overview
[![CI](https://github.com/davideresigotti/restask/actions/workflows/ci.yml/badge.svg)](https://github.com/davideresigotti/restask/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Restask is a self-hosted system that connects selected `.md` notes in your vault to your CalDAV server. Checkboxes in your `.md` files are linked to tasks in your Radicale (or similar) collections, so reminders and calendar tasks on your phone, desktop, etc. stay in sync with your Obsidian vault notes.

> **Status: early-stage, open source, contributions welcome.** See [Contributing & Disclaimer](#contributing--disclaimer).

# Examples
A task line in a routed note:

```md
- [ ] Renew the certificate ⏫ 📅 2026-09-19 17:00 🔁 every year
```

It shows up in `TODO.md` under the high-priority section and in Tasks.org / Thunderbird as a task due at that time. Check it off anywhere and it is checked everywhere.

# Usage
1. Follow [INSTALL.md](INSTALL.md) to set up the daemon, the plugin and (optionally) Neovim.
2. Route a note to a list (see [Lists & Routing](#lists--routing)).
3. Write checkboxes. Run `restask doctor` if something looks off.

# Tools
- **Obsidian**: Dedicated plugin.
- **NeoVim**: Dedicated lua config.
- **Syncthing**: Peer-to-peer file synchronization.
- **Radicale**: Self-hosted CalDAV server managing VTODO collections.

# UI
The symbols for priority, dates and other metadata are inspired by [Tasks](https://github.com/obsidian-tasks-group/obsidian-tasks). In Obsidian and in Neovim, suggestions appear as soon as you type two letters of a keyword in a task line (e.g., `hi` → high priority, `me` → medium priority). The list filters as you type, so `hi` shows only high and highest.

| Symbol | Meaning | Example |
|---|---|---|
| 🔺 ⏫ 🔼 🔽 ⏬ | Priority: highest, high, medium, low, lowest | `- [ ] Renew the certificate ⏫` |
| 📅 | Due date, with an optional time | `📅 2026-09-19`, `📅 2026-09-19 17:00` |
| 🛫 | Start date | `🛫 2026-09-18` |
| ⏳ | Scheduled date | `⏳ 2026-09-18` |
| 🔁 | Repeat | `🔁 every week`, `🔁 every month on the 15th`, `🔁 every 2 weeks for 5 times` |
| 📁 | Calendar the task belongs to (type `calendar` in the suggestions). Without it, the task goes to the default calendar | `📁 work` |
| ➕ | Creation date. Optional: type a bare `➕` (`created` in Obsidian) and restask fills in the date | `➕ 2026-09-17` |
| ✅ | Completion date, added when you check the task off | `✅ 2026-09-19` |
| 🆔 | The task's unique ID. Added automatically, so leave it alone (the plugin and the Neovim integration hide it) | `🆔 restask-01jz…` |

# Task Structure & Workflow

## Main File: `TODO.md`
- `TODO.md` is the main inbox. It holds the tasks you jot down on the fly, plus every prioritized task gathered from the vault.
- It shows all the calendars you choose, not just one. `restask setup` asks which calendars appear and which one receives new tasks. A task from another calendar carries its name on the line (`- [ ] Update README 🔺 📁 work`); change the name to move the task.
- Tasks are grouped by priority. Those without a priority sit under `## No Priority`.
- The section and the priority emoji always agree: type a task under a heading and it gets that priority, move it to another section and the priority follows, and the other way round.
- The file is generated, so only the tasks you add or edit there are kept. Besides the tasks, it contains just two frontmatter properties, `restask-list` and `restask-render`. Leave them alone; the Obsidian plugin and the Neovim integration hide `restask-render`.

## Vault Tasks & Subtasks
- Every checkbox in a routed note is a task. An indented checkbox is a subtask of the one above it.
- A task with a priority also appears in `TODO.md`. It is not a copy but the same task: check it off, reword it or re-prioritize it in either place.
- A task without a priority stays in its note only (ideas, loose thoughts).
- A new line under a `TODO` heading starts with `- [ ] `, ready for your text. A checkbox with no text is not a task and is not synced.
- None of this waits for the network: it happens on the device you edit on, offline, with no daemon running there.

## Completion
- A checked task moves under a `### Done` heading in its note (or in `TODO.md`) with its completion date, e.g. `✅ 2026-09-19`. This works the same whether you check it in Obsidian, in any editor or in Tasks.org.
- `### Done` is sorted newest first, so a task checked by mistake is right at the top: uncheck it and it comes back.
- A repeating task is never finished for good. Checking it off leaves the completed occurrence under `### Done` and creates the next one with its new date.

# Lists & Routing
A note takes part in syncing only when you route it. Notes you don't route are never read, changed or synced.

- **A single note**: add `restask-list: University` to its frontmatter and its tasks go to the list `University`.
- **A whole folder**: add `restask-list-root: Home Lab` to a note and it becomes the folder's root note. Every note below it is routed to that list. The nearest root wins.
- **A root note shows its folder.** Give it a `# TODO` heading and that section becomes the folder's own `TODO.md`: every prioritized task of the notes in the folder and below, by priority, next to the root note's own tasks. It works like `TODO.md` — tick, edit or delete a line there and the task follows in its note. The rest of the note stays yours; a root note without the heading is left alone.
- **`TODO.md`** goes to the inbox list, the calendar that receives new tasks (`inbox_list` in `restask.toml`, chosen during `restask setup`).
- A list is a collection on your server, created on first sync (`Home Lab` → `home-lab`). `restask lists` shows them all.
- A task created in Tasks.org or Thunderbird lands in the list's root note, or in `TODO.md` for the inbox. From then on, changes on either side update the other.
- Moving a task to a note with a different route moves it to the matching calendar.
- A task linking a note (`- [ ] [[Caddy design]]`) reaches Tasks.org with a link that opens that note in Obsidian. For this to work, the vault must have the same name on every device (`obsidian_vault` in `restask.toml`).

Run `restask doctor` to check the whole chain: config, routing, vault and server. [INSTALL.md](INSTALL.md) covers the setup, which is three commands on your computer.

# Architecture
restask is **local-first and offline-first**. Everything you do to a task happens on your device's files, and the network is only used afterwards, in the background. The full design is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Who does the work, where
- **Local part** (task ID, `Done`, `TODO.md`): needs only the vault, so it runs on the device you edit on, at once and offline.
- **Server part** (everything involving Radicale): handled by a single daemon on your always-on server.

| Where you edit | Who does the local part |
|---|---|
| Obsidian (PC, phone) | The plugin, when you leave the line |
| Neovim | `restask settle`, when you save the note |
| Any other editor | The daemon, at its next pass |

A PC with both Obsidian and Neovim needs no daemon: the one you edit in does the work and the other sees the change on disk.

## Syncing
- **Syncthing** carries the notes between your devices.
- **The daemon** reads the vault and the server and makes them agree. It checks the server every two seconds, so a task changed in Tasks.org reaches the vault in moments.

## Conflicts
- restask remembers what the vault and the server last agreed on per task. A change made on only one side simply wins: completing a task in Tasks.org while rewording it in Obsidian keeps both.
- If the same field changed on both sides, the newer change wins (the vault wins when they are less than two minutes apart).
- Deleting a task line deletes the task on the server, and the other way round. A line is never removed for any other reason.
- Notes, reminders, tags and recurrence set by other apps are preserved.

## Compatibility
restask uses the standard VTODO format (RFC 5545), so any CalDAV app can read the tasks. A task with `📅 2026-09-19` shows as an all-day item, and with `📅 2026-09-19 17:00` at that exact time. Every task is tied to its vault line by its unique ID, which prevents duplicates. Copying a line creates a new task.

# Real Usage

## Mobile (Android)
- **Tasks.org**: connects to your Radicale server to manage and check off tasks. (I personally use it with the DAVx5 app.)
- **Obsidian**: edits the vault notes directly, including `TODO.md`.

Changes made in either app show up in the other.

## Desktop
- **Obsidian**: view and manage tasks in your notes and in `TODO.md`.
- **Neovim**: saving a note settles it on the spot. `<leader>tt` toggles the task under the cursor, `<leader>ta` captures a new one.
- **Thunderbird**: view and edit the tasks as a regular CalDAV task list.

# Contributing & Disclaimer
restask is open source under the [MIT licence](LICENSE), and contributions are welcome: bug reports, ideas, documentation and pull requests. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; the design rules are in [ARCHITECTURE.md](ARCHITECTURE.md). Security problems: see [SECURITY.md](SECURITY.md).

**Disclaimer.**
- restask is early-stage software, maintained on a best-effort basis. Replies and reviews can take time.
- It reads and **rewrites your Markdown notes**, and deleting a task line deletes the task on your server (and the other way round). It is provided "as is", without warranty of any kind, as stated in the MIT licence.
- **Back up your vault** (Git, Syncthing versioning or similar) before using it, and try it first in a throwaway vault with a test calendar.
- restask is not affiliated with or endorsed by Obsidian, Obsidian Tasks, Radicale, Tasks.org, Thunderbird or Syncthing.
