# restask spec — Obsidian Plugin & Neovim (§15–§16)

> Normative. Index and invariants: `ARCHITECTURE.md`.

Integrations are conveniences. A device needs none of them: editing the Markdown is the
whole contract (§1.2), and the daemon repairs hand edits (§6.4).

## §15 Obsidian plugin (`plugins/obsidian/`, TypeScript, zero runtime dependencies)

Runs on desktop and mobile. `restask setup` installs and enables it in the vault (§13.2
step 1); `.obsidian/` riding the file sync carries it to the other devices. It **edits
Markdown and nothing else**: no state files, no network. (It used to mirror tasks into `.restask/tasks/`; those files are now the
engine's base snapshots and no other program may write them.)

### 15.1 `markdown.ts` — grammar port

A port of §6.1/§6.2 (same regexes, same codepoints, same nesting rules), tested against
copies of the Rust fixtures under `test/fixtures/`. The `🔁` token is recognised with the
same boundaries as the engine (`recurrenceLength`, same cases as
`tests/domain_recurrence.rs`); the plugin does not interpret the rule — completing a
recurring task leaves the checked line under the done heading and the daemon rolls the
series forward (§11.6).

### 15.2 `modal.ts` — metadata suggestions

| Keyword (prefix, case-insensitive) | Inserts |
|---|---|
| `highest` / `high` / `medium` / `low` / `lowest` | 🔺 ⏫ 🔼 🔽 ⏬ |
| `due` / `start` / `scheduled` | `📅 ` / `🛫 ` / `⏳ ` |
| `repeat` | `🔁 every ` (then type the rule: `week on Monday`, `month on the 15th`, …) |
| `today` / `tomorrow` | `📅 <date>` |

- `suggestionsFor(fragment, today)`: entries whose keyword starts with the fragment;
  nothing below two letters (`hi` → high, highest; `low` hidden).
- `triggerAt(lineBeforeCursor, today)`: suggestions open **while typing** only inside a
  task line, when the cursor ends a whole word of ≥ 2 letters that prefixes a keyword.
  Choosing an entry replaces the typed fragment.

### 15.3 `toggle.ts` — complete / reopen

`toggleDone(doc, line, today, doneHeading)`, a pure document transformation:

- completing: flip the box, insert `✅ <today>` in canonical position, move the line —
  un-indented — directly under the done heading (creating `### <heading>` at the end of
  the file if missing);
- reopening: flip the box, remove `✅`, move the line to the bottom of the active list;
- a line already in the right region is edited in place; every other byte is preserved.

It works the same on a TODO.md mirror line; the daemon carries the change to the source
note (§7.1).

### 15.4 `main.ts`, `settings.ts` — the only files using the Obsidian API

- Commands: `Toggle task done` (cursor line), `Add metadata` (the suggestions as a
  modal).
- An `EditorSuggest` for §15.2's while-typing behaviour.
- The editor extension and the reading-view post-processor of §15.5.
- Settings: `doneHeading` (must equal `done_heading` in `restask.toml`; setup seeds it
  for a non-default heading when the plugin has no settings yet),
  `suggestWhileTyping` (default on), `hideTaskIds` (default on; switching it takes
  effect in open notes at once).

### 15.5 `conceal.ts`, `editor.ts` — the `🆔` token is not shown

The token is the task's identity and **stays in the file**; the plugin only keeps it off
the screen. Nothing in the engine, the grammar or the vault changes.

- **What is hidden** (`uidToken`, pure): on a task line (§6.1 shape), the token the
  parser reads the UID from — the first `🆔` match, with a valid ULID — together with the
  blanks before it. A token on a non-task line, a malformed one and a second one on the
  same line stay visible: they are not a task's identity, and the user should see them.
  The check is per line (a task-shaped line inside a code fence counts).
- **Editor** (`editor.ts`, the only file that uses CodeMirror — Obsidian's own copy,
  external to the bundle): a replace decoration hides the range in Live Preview and in
  source mode. Copying a line still copies its token, so moving a task keeps its UID.
- **Reading view**: a post-processor removes the token from the text of each rendered
  task item (`stripUid`).

A token nobody sees is easy to destroy, and a destroyed or displaced token is a deleted
task plus a new one on the server. So `editor.ts` also installs a transaction filter
(`uidGuard`) that makes the hidden range behave like the end of the line:

| Situation | Result |
|---|---|
| Cursor sent behind or into the token (End, click, arrow keys) | rests in front of it; moving right from there passes the token |
| Selection started inside the line's text and extended to the line end | ends in front of the token |
| Text typed or inserted inside / right behind the token | goes in front of it |
| Line break at the token (Enter, Shift+Enter, multi-line paste) | goes behind it: the token stays on its line, the new line has none |
| Deletion or replacement that covers part of the line and the token | cut around the token |
| Backspace / Delete that would hit only the token | takes the visible character before / after it (Delete at the line end: nothing) |
| Deletion that covers the task's whole body, the whole line or whole lines | removes the token with it |

The filter leaves alone transactions that replay valid documents: a reload from disk
(Obsidian's `set` event — the daemon may rewrite a token, §6.4), undo and redo.
Everything else is guarded, whether or not it carries a user event. With `hideTaskIds`
off neither the decoration nor the filter is installed.

## §16 Neovim (`neovim/lua/restask/`)

A thin wrapper over the CLI — no Markdown logic in Lua beyond recognising a task line
and the `🆔` token.

- `toggle.lua`: `action_for(line)` classifies the cursor line (`[ ]` → `done`,
  `[x]`/`[X]` → `undone`, else nothing). `toggle()` writes the buffer if modified, runs
  `restask <action> --file <absolute path> --line <n>`, and reloads the buffer. `add()`
  prompts and runs `restask add`.
- `conceal.lua`: hides the `🆔` token (and the blanks before it) in windows that show a
  Markdown file of a vault (a `restask.toml` or `.restask/` above the file). The file is
  not changed: a window match conceals the text, `conceallevel` is raised to 2 and the
  modes of `concealcursor` (default `nc`) are added to the window's option; all three
  are undone when the window shows something else. The two options are raised again
  whenever something else resets them in such a window (`OptionSet`;
  render-markdown.nvim does on every render). Insert mode is not among the default
  modes, so the line being typed in shows its token — Neovim has no guard like §15.5's.
- `init.lua`: `require("restask").setup({ keymaps = true, conceal = true })` →
  `<leader>td` toggle, `<leader>ta` add, tokens concealed (`conceal = false` turns that
  off, `concealcursor = "…"` chooses the modes). Errors surface through `vim.notify`.
- Gates: `luac -p neovim/lua/restask/*.lua`; `lua neovim/test/toggle_test.lua`.
