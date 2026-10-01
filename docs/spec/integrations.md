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
- Settings: `doneHeading` (must equal `done_heading` in `restask.toml`; setup seeds it
  for a non-default heading when the plugin has no settings yet),
  `suggestWhileTyping` (default on).

## §16 Neovim (`neovim/lua/restask/`)

A thin wrapper over the CLI — no Markdown logic in Lua beyond recognising a task line.

- `toggle.lua`: `action_for(line)` classifies the cursor line (`[ ]` → `done`,
  `[x]`/`[X]` → `undone`, else nothing). `toggle()` writes the buffer if modified, runs
  `restask <action> --file <absolute path> --line <n>`, and reloads the buffer. `add()`
  prompts and runs `restask add`.
- `init.lua`: `require("restask").setup({ keymaps = true })` → `<leader>td` toggle,
  `<leader>ta` add. Errors surface through `vim.notify`.
- Gates: `luac -p neovim/lua/restask/*.lua`; `lua neovim/test/toggle_test.lua`.
