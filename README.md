# Overview
[![CI](https://github.com/davideresigotti/restask/actions/workflows/ci.yml/badge.svg)](https://github.com/davideresigotti/restask/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Restask is a self-hosted system that connects selected `.md` notes in your vault to your CalDAV server. Checkboxes in your notes are linked to tasks in your Radicale (or similar) collections, so reminders and calendar tasks on your phone, desktop, etc. stay in sync with your Obsidian vault.

## Videos
https://github.com/user-attachments/assets/7e6384f2-d228-4bfa-92c5-60be04b21f88

https://github.com/user-attachments/assets/3eb60d8b-b00a-4107-a8ad-e4befb525127

## Screenshots
| Obsidian (mobile) | Tasks.org (mobile) | Widget (mobile) |
|---|---|---|
| <img width="250" alt="obsidian_mobile" src="https://github.com/user-attachments/assets/39e3193e-8b27-4b83-9e0a-44babd8c63e6" /> | <img width="250" alt="task_mobile" src="https://github.com/user-attachments/assets/5ae27f72-5fb3-4a7a-a2fb-b1d1d613be5f" /> | <img width="250" alt="widget-mobile" src="https://github.com/user-attachments/assets/d55f784e-c1ae-48ef-ae62-46cbaebfdfa9" /> |

# Disclaimer
- I built restask to solve a problem of my own, and I'm sharing it in the hope that someone else needs it too.
- The code was written with the help of AI coding tools, so review it with that in mind.
- It **rewrites your Markdown notes**, and deleting a task line deletes the task on your server (and the other way round). Backup before the install.

# Getting Started
1. Follow [INSTALL.md](INSTALL.md) to set up the daemon, the plugin and (optionally) Neovim.
2. Route a note to a list (see [Lists & Routing](#lists--routing)).
3. Write checkboxes. Run `restask doctor` if something looks off.

# Tools
- **Obsidian**: dedicated plugin.
- **Neovim**: dedicated Lua config.
- **Syncthing**: file synchronization between devices.
- **Radicale**: self-hosted CalDAV server holding the task collections.

# Task Syntax
The symbols are inspired by [Tasks](https://github.com/obsidian-tasks-group/obsidian-tasks). In Obsidian and Neovim, suggestions appear as you type two letters of a keyword (e.g. `hi` → high priority).

| Symbol | Meaning | Example |
|---|---|---|
| 🔺 ⏫ 🔼 🔽 ⏬ | Priority: highest to lowest | `- [ ] Renew the certificate ⏫` |
| 📅 | Due date, optional time | `📅 2026-09-19 17:00` |
| 🛫 | Start date | `🛫 2026-09-18` |
| ⏳ | Scheduled date | `⏳ 2026-09-18` |
| 🔁 | Repeat | `🔁 every 2 weeks for 5 times` |
| 📁 | Calendar of the task (default calendar if omitted) | `📁 work` |
| ➕ | Creation date; type a bare `➕` and it is filled in | `➕ 2026-09-17` |
| ✅ | Completion date, added when you check the task | `✅ 2026-09-19` |
| 🆔 | Unique ID, added automatically and hidden by the plugin and Neovim | `🆔 a42` |

# How It Works

## `TODO.md`
- The main inbox: tasks you jot down on the fly, plus every prioritized task from the vault.
- Shows the calendars you choose in `restask setup`; a task from another calendar carries its name (`📁 work`), change it to move the task.
- Tasks are grouped by priority (`## No Priority` for the rest). Moving a task to another section changes its priority, and the other way round.
- The file is generated: only the tasks you add or edit there are kept.

## Vault Tasks
- Every checkbox in a routed note is a task; an indented one is a subtask.
- A task with a priority also appears in `TODO.md`. It is the same task: check, reword or re-prioritize it in either place.
- A task without a priority stays in its note only.
- All of this happens on the device you edit on, offline.

## Completion
- A checked task moves under `### Done` with its completion date, whichever app you check it in. The list is newest first, so a mistake is easy to undo.
- A repeating task leaves the completed occurrence under `### Done` and creates the next one.

# Lists & Routing
A note takes part in syncing only when you route it. Unrouted notes are never read, changed or synced.

- **A single note**: `restask-list: University` in its frontmatter.
- **A whole folder**: `restask-list-root: Home Lab` in a root note routes every note below it. The nearest root wins.
- **A root note shows its folder**: give it a `# TODO` heading and that section lists the folder's prioritized tasks, like `TODO.md`.
- **`TODO.md`** goes to the inbox list (`inbox_list` in `restask.toml`).
- A list is a collection on your server, created on first sync. `restask lists` shows them all.
- Tasks created in Tasks.org or Thunderbird land in the list's root note (or `TODO.md` for the inbox).
- Linking a note (`[[Caddy design]]`) gives a link that opens it in Obsidian; the vault needs the same name on every device (`obsidian_vault` in `restask.toml`).

Run `restask doctor` to check config, routing, vault and server.

# Architecture
restask is **local-first**: everything happens on your device's files, and the network is used afterwards, in the background. Details in [ARCHITECTURE.md](ARCHITECTURE.md).

| Where you edit | Who does the local work |
|---|---|
| Obsidian (PC, phone) | The plugin, when you leave the line |
| Neovim | `restask settle`, when you save |
| Any other editor | The daemon, about 10 seconds after you save |

- **Syncthing** carries the notes between devices.
- **The daemon** (one, on your always-on server) makes the vault and Radicale agree, checking the server every two seconds.
- **Conflicts**: a change on one side wins; if both changed the same field, the newer wins. Notes, reminders, tags and recurrence set by other apps are preserved.
- **Deletion**: deleting a task line deletes the task on the server, and the other way round.
- **Compatibility**: tasks use standard VTODO (RFC 5545), so any CalDAV app can read them.

# Real Usage
- **Android**: Tasks.org (with DAVx5) for checking tasks, Obsidian for editing notes.
- **Desktop**: Obsidian for notes and `TODO.md`; Neovim (`<leader>tt` toggles the task under the cursor, `<leader>ta` captures one); Thunderbird as a CalDAV task list.

# Contributing
restask is open source under the [MIT licence](LICENSE). Contributions are welcome: bug reports, ideas, documentation and pull requests. Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Security problems: [SECURITY.md](SECURITY.md).
