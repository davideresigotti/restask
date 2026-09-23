# Taskres Spec — Obsidian Plugin & Neovim (§15–§16)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §15 Obsidian Plugin (`plugins/obsidian/`, TypeScript, zero runtime deps)

Runs on desktop **and mobile**; it is the phone-side half of the offline promise (mutates Markdown + `.taskres/tasks/*.ics`; never talks CalDAV — that is the daemon's job).

### 15.1 `markdown.ts` — grammar port

Faithful port of §6.1/§6.2 (same regexes, same codepoints, same metadata tail order). Parity is proven by testing against copies of `test-vault/` fixtures and by the golden contract below.

### 15.2 `modal.ts` — autocomplete

Triggered on typing in a task line; suggestions appear once ≥ 2 letters of a keyword are typed; the list filters dynamically to matches only (README behavior: `hi` → high + highest, low hidden). Keyword table:

| Keyword (prefix, case-insensitive) | Inserts |
|---|---|
| `highest` / `high` / `medium` / `low` / `lowest` | 🔺 ⏫ 🔼 🔽 ⏬ |
| `due` / `start` / `scheduled` | `📅 ` / `🛫 ` / `⏳ ` (+ date picker → `YYYY-MM-DD`) |
| `today` / `tomorrow` | `📅 <today+0/1>` |

Filter rule: an option matches iff its keyword `startsWith` the typed fragment (case-insensitive). Zero matches → menu hides.

### 15.3 `vtodo.ts` — cache writer

TS port of §8.1 (serialize only) for the exact subset the plugin produces (complete/uncomplete, priority). Emits the same bytes as Rust for the same task — asserted against `docs/contracts/vtodo-golden.ics` in `test/vtodo.test.ts`. Writes `.taskres/tasks/<uid>.ics` via `app.vault.adapter` (creating `.taskres/tasks/` as needed). Absent `.taskres/` (fresh mobile device) → skip silently.

### 15.4 `main.ts` — commands & settings

Commands: `Taskres: Toggle task done` (cursor line: flips checkbox, adds/removes `✅ <today>`, moves under Done heading newest-on-top, updates cache file), `Taskres: Add metadata` (opens modal), `Taskres: Sync now` (best-effort cache refresh of visible tasks). Settings: `enableCacheMirror` (default true), `doneHeading` (default "Done", must equal vault config). Obsidian API calls are isolated in `src/main.ts`; `markdown.ts`/`modal.ts`/`vtodo.ts` stay API-free and unit-testable under Vitest.

## §16 Neovim Integration (`neovim/lua/restask/`)

Thin wrapper over the CLI (no parsing in Lua — zero drift):

- `toggle.lua`: reads cursor line/file → `[ ]` ⇒ `restask done --file <bufname> --line <lnum>`; `[x]` ⇒ `restask undone …`; refreshes buffer. Keymaps: `<leader>td` toggle, `<leader>ta` prompt → `restask add "<text>"`.
- `init.lua`: `require("restask").setup({ keymaps = true })`, `vim.notify`-based error surface.
- Gate: `luac -p neovim/lua/restask/*.lua` (Fedora: `luac`; CI: `luac5.4`).
