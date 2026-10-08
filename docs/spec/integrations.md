# restask spec — Obsidian Plugin & Neovim (§15–§16)

> Normative. Index and invariants: `ARCHITECTURE.md`.

A device needs no integration to take part: editing the Markdown is the whole contract
(§1.2), and the daemon repairs hand edits (§6.4). But restask is local-first
(`ARCHITECTURE.md` *Local first*, invariant 12): where an integration is installed, it
does the daemon's local work itself — the same edits, the same bytes, on the device in
hand, at once and offline — and nothing local waits for a round trip of the file sync.
Where this section still leaves something to the daemon, that is an open item
(`docs/EXECUTION_STATE.md`), not the design.

## §15 Obsidian plugin (`plugins/obsidian/`, TypeScript, zero runtime dependencies)

Runs on desktop and mobile. `restask setup` installs and enables it in the vault (§13.2
step 1); `.obsidian/` riding the file sync carries it to the other devices. It **edits
Markdown and nothing else**: no state files, no network. Apart from the notes it reads
one file, `restask.toml`, for the path of the inbox file. (It used to mirror tasks into `.restask/tasks/`; those files are now the
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
| `created` | `➕` — bare; the date is written when the line is settled (§6.4) |
| `calendar` | `📁 ` (then type the calendar's name: a task of TODO.md lives there, §7.5) |
| `today` / `tomorrow` | `📅 <date>` |

- `suggestionsFor(fragment, today)`: entries whose keyword starts with the fragment;
  nothing below two letters (`hi` → high, highest; `low` hidden).
- `triggerAt(lineBeforeCursor, today)`: suggestions open **while typing** only inside a
  task line, when the cursor ends a whole word of ≥ 2 letters that prefixes a keyword.
  Choosing an entry replaces the typed fragment.

### 15.3 `toggle.ts` — complete / reopen

`toggleDone(doc, line, today, doneHeading)`, a pure document transformation:

- completing: flip the box, insert `✅ <today>` in canonical position (ahead of a dated
  `➕`, of a calendar token `📁` that is part of the tail, of `🆔`), move the line —
  un-indented — directly under the done heading (creating `### <heading>` at the end of
  the file if missing);
- reopening: flip the box, remove `✅`, move the line to the bottom of the active list;
- a line already in the right region is edited in place; every other byte is preserved.

It works the same on a TODO.md mirror line; the daemon carries the change to the source
note (§7.1).

### 15.4 `main.ts`, `settings.ts` — the only files using the Obsidian API

- Commands: `Toggle task done` (cursor line; where §15.6 applies it flips the box and
  the extension does the rest, elsewhere it is §15.3 as a whole), `Add metadata` (the
  suggestions as a modal).
- An `EditorSuggest` for §15.2's while-typing behaviour.
- The editor extension and the reading-view post-processor of §15.5, and for §15.6 the
  editor extension with what it needs from Obsidian (the note's file, the frontmatter
  cache, the clock, random bytes), the click listener for reading view, and the edits
  of other files (`Vault.process`).
- Settings: `doneHeading` (must equal `done_heading` in `restask.toml`; setup seeds it
  for a non-default heading when the plugin has no settings yet),
  `suggestWhileTyping` (default on), `hideTaskIds` (default on; switching it takes
  effect in open notes at once), `settleTasks` (`always` — the default —, `mobile`
  or `never`; §15.6), `startTasks` (default on; §15.7).

### 15.5 `conceal.ts`, `editor.ts` — the `🆔` token is not shown

The token is the task's identity and **stays in the file**; the plugin only keeps it off
the screen. Nothing in the engine, the grammar or the vault changes.

- **What is hidden** (`uidToken`, pure): on a task line (§6.1 shape), the token the
  parser reads the UID from — the first `🆔` match, with a valid ULID — together with the
  one blank before it (further blanks are text the user typed, and stay visible: a blank
  typed at the visible end of a line must show and move the cursor). A token on a non-task line, a malformed one and a second one on the
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
| Any edit after which the token would show: its line is no task line any more (the text selected from the visible end back to the line start and deleted, the checkbox deleted or broken, the line joined to prose), or it ends up behind another task's token | removes the token with it |
| A plain line break inside the task's text that would take the token to a line that is no task | the token stays with the head of the task, at the end of its line |

The last two rows are what keeps "not shown" true: such a token is no identity — the engine
reads a line that is not a task as prose (§6.1) and takes the task as deleted — so
keeping it would only put it on the screen.

The filter leaves alone transactions that replay valid documents: a reload from disk
(Obsidian's `set` event — the daemon may rewrite a token, §6.4), undo and redo.
Everything else is guarded, whether or not it carries a user event. With `hideTaskIds`
off neither the decoration nor the filter is installed.

### 15.6 `filing.ts`, `editor.ts`, `main.ts` — the daemon's local edits, made on the device

Phase 1 of a pass (§11.1) and the render need no server, but they happen where the daemon
runs. A phone would show a new line bare, and a checked task where it was, until the
file sync has carried the note there and back. So the plugin makes those edits itself,
at once and offline, as Markdown edits. The daemon then finds a vault it has nothing to
repair in: it pushes, and rewrites no file (`tests/sync_engine.rs`).

**Where.** Only in a note that takes part (invariant 4): the TODO.md view — the file at
`inbox_file` of the vault's `restask.toml` (`TODO.md` when the config names none), known
by its path as in the engine (§7) and by nothing it contains; in a vault without a
`restask.toml` no file is the view —; a
note whose own frontmatter carries `restask-list` or `restask-list-root` (§5.1,
line-scanned like the engine); a note in or below a folder
holding a note with `restask-list-root` (§5.2, from Obsidian's frontmatter cache).
A root note among them holds a view of its own (§7.6): its TODO section, found in the
note's text as the engine finds it (`viewOf`), in which a line is settled as a line of
TODO.md is — everything below about "the view" holds for it, with the section's own
heading rank, the vault's done heading and the seal over the section alone — while the
rest of the note is a note. A mirror line there is told from a line of the note's own by
its exact shape (`isMirrorShaped`). What a note's task changes is brought to every view
that shows it: TODO.md and the root notes of its folder and of the folders above
(`syncMirrors`), and an edit carried from one view reaches the others in the same step.
`track`/`ignore` are not consulted. And only on a device the `settleTasks` setting
names: `always` (default) every device, `mobile` the mobile app only, `never` none. The
settings file rides the file sync, so the value is shared by all devices of the vault.
Plugin and daemon on one machine make the same edits and may both make them; the
outcome is one task either way (a line registered twice keeps the UID that reaches the
vault; the other is deleted from the server), but `mobile` avoids the churn there
without turning the plugin off on the phone as well.

**When.** Only for lines the user edits on the device; the rest of a note stays the
daemon's job.

- *Editor* (`taskFiling`): the extension remembers the lines edited in it (typing,
  paste, a suggestion; not a reload from disk, not undo/redo, not a whole document put
  in place) together with what each line was before, and the UIDs of lines that were
  deleted. An edited line is settled once the cursor is on another line or the editor
  lost the focus — never while it is typed in, nor while an input method composes — and
  at once when the edit was the checkbox itself (a tap in Live Preview, the
  `Toggle task done` command, which in these notes only flips the box).
- *Reading view*: Obsidian changes the note without the editor. The plugin sees the tap
  on the checkbox, compares the note before and after (`toggledLines`: exactly one line,
  differing in its box only), lets Obsidian save, and replaces exactly that saved
  version with the settled one.

**What** (`settled`), for a task line by §6.1/§6.2 that has text:

1. *Register* (`registeredLine`, §6.4). No `🆔` on the line → ` 🆔 restask-<ULID>` is
   appended behind everything else, trailing blanks removed. No creation date is
   written, unless the line asks for one with a bare `➕`: that gets `<today>` behind it,
   where it stands. A bare `➕` on a line that is registered already is left as it is —
   its date is the server's, which the daemon has (§6.4) and the plugin does not; it
   appears when the daemon's edit comes back. The ULID is made on the device (`uidGenerator`: milliseconds + 80
   random bits, monotonic like §3.1). To the engine this is a registered line it has not
   seen: pushed (§11 R2), not rewritten. In the TODO.md view, a line that names no
   priority and stands under a priority section's heading gets that section's emoji,
   ahead of the two tokens (`sectionPriority`, §7.4). A calendar token the line ends
   in (`📁 Work`, §7.5) stays the last thing before the UID and is written as the
   engine writes it — the slug, behind the section's emoji — so that a line typed
   `- [ ] call them 📁 Work` under `## 🔺 Highest Priority` is
   `- [ ] call them 🔺 📁 work 🆔 <uid>` on the device as after the daemon's pass. The
   plugin files a line of the view by its priority and never by its calendar; which
   calendar a line belongs to is read by the engine alone (server work).
2. *A copied line* (§6.4). When other task lines of the same note carry the line's UID,
   the first occurrence keeps it and the others get fresh ones. Copies in other notes
   are the daemon's to find: only it sees the whole vault at one instant.
3. *In a note — the checkbox is the status* (`statusRepaired`, the four rules of §6.4):
   checked outside the done region → stamped `✅ <today>` and moved, un-indented,
   directly under the done heading (created as `### <heading>` at the end when missing);
   checked inside it without a date → stamped; unchecked inside it → moved to the bottom
   of the active list, the date dropped; unchecked with a date → the date dropped. These
   are the edits of §15.3's toggle.
4. *In the TODO.md view — a task of the view's own* (`refiled`): stamped or unstamped as
   in 3, then moved to the section the next render puts it in: `## Done` when completed
   (by date, then UID, both descending), else its priority's section, else
   `## No Priority` (below the priority sections).
   An active line goes behind its section's last task; a missing section is created in
   render order; a section left without content is removed, `## Done` excepted.
5. *In the TODO.md view — a mirror line* (a wikilink right before its `🆔`): what the
   user changed on it is made where the task lives (§7.1). The note the line links to —
   else whichever routed note has the UID — takes over each field that differs between
   the line before the edit and after it (text, priority, repeat rule, start, scheduled,
   due), provided the note still shows the value the line had (`mirrorEdited`, the rule
   of the engine's `mirror_edits`; the note line is rewritten canonically); a checked
   box completes the task there by 3 (`completedByUid`). In the view the line moves to
   its new priority's section, and leaves when the render would no longer show it
   (checked, or no priority left). If no routed note has the task, a checked line is one
   of the view's own after all (4).
   *A mirror line the user deleted* deletes the task: its line is taken out of the note
   (`mirrorDropped`, `dropMirror`), provided the note still shows the task as the
   deleted line did — else the note wins and the line is put back (`restoredSpec`), the
   engine's rule of §7.1. The device knows the deletion was made (it saw the edit), so
   it needs none of the engine's proof. When the line is in the view again — the
   deletion undone, or the cut line pasted — the task's line goes back into its note
   where it was (`putBack`; remembered while the app runs). If the note cannot be
   written, the seal stays broken and the daemon deletes the task by §7.1.
   *A line moved to another section* (§7.4). A registered, active line of the view that
   **arrived** on its line — the line before the user's edit did not hold that task
   (`arrived`): a paste, a move-line command, a drop — is not filed back by its emoji
   but takes the priority of the section it arrived in (`settled(.., moved)`). A task of
   the view's own is rewritten canonically with it; on a mirror line only the emoji is
   replaced (or removed, under `## No Priority`) and the change is carried to the note
   as in 5, the line as it arrived being the earlier text (the cut took the task out
   of its note, the paste put it back — see above — before the change is carried). A
   second mirror line of the same task in the view is taken out. A line edited where it stood
   is filed by its emoji, as before; so is one that was pasted and then given another
   emoji before the cursor left it — unlike in the engine, where the changed emoji wins.
6. *A note task and its mirror line* (`mirrorLine`, `mirrored`). After a task line of a
   note was settled or deleted, the view follows: active with a priority → its mirror
   line is added to the priority's section, or — when it is there and differs —
   rewritten and moved; otherwise the mirror line is removed.

**The seal** (§7.2). After 4–6 the view is a render again, and the plugin seals it — in
the editor once every edit made there since the view was loaded has been settled and
has reached its note, in a file write at once. It seals only a view that was sealed
before the edits: where an edit could not be carried (note not found, not writable,
CRLF, a mirror line whose earlier text is unknown), the seal stays broken and the daemon
carries it by §7.1. And it *rewrites* an existing mirror line (6) only in a sealed view:
in an unsealed one — rendered by a daemon that does not seal yet — a changed mirror line
would be read as an edit of the user's and written into the note. There, lines are
only added and removed — and a view the plugin takes a mirror line out of without
sealing it in the same write (an unsealed view; in the editor, until the seal is
written) gets the seal line that claims no render (`disclaimed`, §7.1): a line missing
from an unsealed view is otherwise the user's deletion of the task.

From a view as the engine rendered it, the result is byte-identical to the next render
when the lines are canonical, a repeat rule is in its canonical spelling, and a new line
sorts last in its section (§7: by source path and line); otherwise the render corrects
it, as it does every hand edit.

**How it is written.** In the editor (`filingOf`, `carriedSpec`, `restoredSpec`, `sealSpec`): one transaction of at
most two changes — a line's tail, or lines taken out in one place and put in at another
— so the text between is not replaced and the cursor stays on its line. It is an
ordinary history entry and passes §15.5's guard untouched: it moves whole lines. Files
not in the editor at hand (the source note of 4, the view of 5, the note in reading
view) are replaced through the vault only if they still hold the text the edit was
computed from; these jobs run one after the other. A note with CRLF line endings is not
edited this way; it waits for the daemon, which keeps its endings.

Not done on the device, and not done by a daemon without its server either: the
roll-forward of a recurring task (§11.6 — the checked line waits under the done
heading) and everything else that is planned from the server's state. Left to the
daemon: lines the user did not edit on the device, duplicate UIDs across notes, and
the exact order and canonical spelling of the view where the plugin's differ.

### 15.7 `filing.ts`, `editor.ts` — a new line in a TODO section starts a task

An editing aid, not a local rule: it changes what the editor puts on a line the user
opens, and nothing about what a line means. No file is touched that the user is not
typing in, and the engine has no part in it.

**The TODO section** (`inTodoSection`) of a note that takes part (as in §15.6: the view,
a note routed by its own frontmatter, a note below a `restask-list-root`):

- in a note, the lines below a heading whose text is `TODO` — any ATX level, any letter
  case, nothing else in the heading — down to the next heading of the same or a higher
  level. Deeper headings stay inside; a later `TODO` heading opens the section again;
- in the TODO.md view, the whole body;
- the done heading ends it in both (`done_heading` in a note, `Done` in the view): what
  is under it is a record, and an unchecked line there would be moved out again (§6.4);
- never the frontmatter, never a fenced block (§6.2).

**The rule** (`taskStart`, a transaction filter): when the user's edit is a line break
and nothing else, and it leaves the cursor on a new, empty line inside the TODO section,
`- [ ] ` is written on that line and the cursor put behind it — in the same transaction,
so one undo takes both away. That is Enter, Shift+Enter and Vim's `o`/`O` on a line that
is no list item: a heading, a blank line, prose. Not touched:

- a line Obsidian continued a list on — it has its marker and box already;
- an indented continuation line (Shift+Enter inside a list item): it is not empty;
- a break inside a line, or in front of one: the cursor is on text;
- text that merely ends in a line break (a paste, a drop, an inserted template), a
  reload from disk, undo, redo, the plugin's own filing;
- several cursors.

Enter on the empty checkbox takes it away again — Obsidian's own rule for an empty list
item — which is how a line that is no task is written in the section. An empty checkbox
left behind is no task to anyone: the plugin does not settle it (§15.6) and the engine
does not register it (§6.4).

The filter runs after §15.5's guard, so a break made in front of a hidden token opens
its line behind the token. The `startTasks` setting turns the rule off; it is
independent of `settleTasks`.

### 15.8 `filing.ts`, `editor.ts`, `main.ts` — the view's fixed lines are skipped, and it opens under `# TODO`

An editing aid for the TODO.md view (the file at the inbox path) only; it decides nothing
about the vault and writes nothing.

- **Locked lines** (`lockedLines`): the frontmatter block and every heading outside a
  fenced block (§6.2). They are the render's, not the user's: the seal, the title and the
  section headings. Blank lines and task lines are free.
- **The cursor skips them** (`viewLock`, a transaction filter; `freeLine`): a selection
  change that puts the cursor on a locked line moves it on to the nearest free line in the
  direction of travel — down when it came from above or from nowhere — and, with none that
  way, the nearest one the other way, so a cursor pushed against the top stays on the first
  free line (`gg` lands there). The column is kept where the line is long enough. Clicks
  (`select.pointer`) and selections that span text are left alone.
- **Opening** (`homeLine`, `main.ts`): when the view is opened in the editor, the cursor
  goes to the line under the first heading (`# TODO`), where a new task is typed at once.

## §16 Neovim (`neovim/lua/restask/`)

A thin wrapper over the CLI — no Markdown logic in Lua beyond recognising a task line,
the `🆔` token and, for `start.lua`, the TODO section of a routed note. The editing aids
of the plugin (§15.2, §15.7) are ported, as aids: they decide nothing about the vault.
The local work (invariant 12) is the engine's own, run when a buffer is written.

- `settle.lua`: on `BufWritePost` of a Markdown file of a vault (a `restask.toml` or
  `.restask/` above it) it runs `restask settle --file <absolute path>` (§13.3) and then
  `checktime`, so every buffer the CLI rewrote — the note, TODO.md in another window —
  shows the result ('autoread', Neovim's default, reloads an unmodified buffer without
  asking; the reload is one undo step). That is all of §6.4 and §7 at once: lines
  registered, a checked box stamped and moved, the views filed and sealed — TODO.md and
  the one each root note holds (§7.6) —, an edit or a deletion made in a view carried
  to its note. No daemon and no network take part,
  and what is left is what the daemon's pass would leave. The CLI is waited for
  (milliseconds; given up after 5 s), so the buffer cannot change between the write and
  the reload. A missing binary is reported once per session, a failed run each time.
  `BufWritePost` does not fire for a write made inside an autocommand that is not
  `nested` — the common auto-save recipe, `update` on `InsertLeave`/`TextChanged` — so
  after `InsertLeave`, `TextChanged`, `TextChangedI`, `BufLeave` and `FocusLost`, once
  their autocommands have run, a saved buffer whose file is not the one last read or
  settled (by modification time) is settled as well (`catch_up`).
  Unlike the plugin (§15.6), which settles the edited line, this settles the vault: a
  line that arrived unsettled from elsewhere is settled by the same write.
- `toggle.lua`: `action_for(line)` classifies the cursor line (`[ ]` → `done`,
  `[x]`/`[X]` → `undone`, else nothing). `toggle()` writes the buffer if modified, runs
  `restask <action> --file <absolute path> --line <n>`, and reloads the buffer. Its
  write is not settled (`settle.without`): the command does the local work itself, and
  `--line` must be the line as written. `add()` prompts and runs `restask add`.
  `register_keymaps()` sets the two keymaps, each with a description, and — where the
  configuration has [which-key.nvim](https://github.com/folke/which-key.nvim) — names
  their prefix `<leader>t` as the group `restask` (`name_group`: `add` of version 3,
  `register` before it), so the hint shown after `<leader>` lists restask by name.
  Without which-key nothing is asked and nothing fails. The keymaps exist once
  `setup()` has run: a configuration that loads the integration for Markdown buffers
  only shows them from the first note on, one that loads it at startup from the start.
- `conceal.lua`: hides the `🆔` token (and the one blank before it, as §15.5) in windows that show a
  Markdown file of a vault (a `restask.toml` or `.restask/` above the file). The file is
  not changed: a window match conceals the text, `conceallevel` is raised to 2 and the
  modes of `concealcursor` (default `nc`) are added to the window's option; all three
  are undone when the window shows something else. The two options are raised again
  whenever something else resets them in such a window (`OptionSet`;
  render-markdown.nvim does on every render). Insert mode is not among the default
  modes, so the line being typed in shows its token — Neovim has no guard like §15.5's.
- `guard.lua` (pure, `lua neovim/test/guard_test.lua`): the editing rules of §15.5's
  table for a buffer, applied by `conceal.lua` after each change — among them the last
  two rows: a token that an edit would leave on a line that is no task (`0D`, a deleted
  or broken checkbox, a join onto prose) goes with the edit, or back to the head of its
  task when a line break cut it off.
- `start.lua` (its rules pure, `lua neovim/test/start_test.lua`): §15.7 for a buffer. A
  line opened in the TODO section — the buffer grew by exactly one line and the cursor
  is on a blank one in insert mode: `o`, `O`, Enter at the end of a line — gets
  `- [ ] ` behind the indentation Neovim gave it, in the undo step of the line. The
  section is §15.7's (`in_todo`, the same cases as the plugin's test). Which note takes
  part is read as the plugin reads it: the file at `inbox_file` of the vault's
  `restask.toml` is the view (`done_heading` comes from there too), a note with
  `restask-list`/`restask-list-root` in its frontmatter is routed, and so is one in or
  below a folder holding a note with `restask-list-root` (looked up when the first line
  is opened, remembered until the buffer is written or read again). Unlike Obsidian,
  Neovim continues no list by itself, so the rule also covers the line opened below a
  task, at its indentation; a second Enter on the empty checkbox opens another one —
  the box is deleted by hand where a line is to stay empty. Keys that arrive as
  typeahead (a macro, a mapping's right-hand side) open plain lines.
- `lock.lua` (its rules pure, `lua neovim/test/lock_test.lua`): §15.8 for a buffer. In the
  TODO.md view, `CursorMoved`/`CursorMovedI` in normal and insert mode send a cursor that
  landed on the frontmatter or a heading on to the next free line (`lock.free`, the plugin's
  `freeLine`; the cases of the two tests are the same), and a view buffer shown for the
  first time puts the cursor on the line under `# TODO` (`lock.home`). `lock = false`
  turns both off.
- `suggest.lua` (its rules pure, `lua neovim/test/suggest_test.lua`), `blink.lua`:
  §15.2 while typing — the same keyword table, `suggestions_for` and `trigger_at` as
  `modal.ts`, with the cases of its test. In a Markdown file of a vault (where the token
  is concealed; routed or not, as in Obsidian), when the cursor in insert mode ends a
  keyword fragment in a task line, the menu of [blink.cmp](https://github.com/Saghen/blink.cmp)
  is opened with the provider `restask` alone (`blink.cmp.show({ providers })`), which
  also works where blink.cmp is configured to show nothing by itself. The provider
  (`restask.blink`) is registered on first use unless the user's configuration defines
  one under that id; its items are the entries in the table's order, each a text edit
  that replaces the fragment by its token. They are accepted and dismissed with
  blink.cmp's own keys. A menu that is open already is not replaced. Without blink.cmp
  nothing is offered.
- `init.lua`: `require("restask").setup({ keymaps = true, conceal = true })` →
  `<leader>td` toggle, `<leader>ta` add, tokens concealed (`conceal = false` turns that
  off, `concealcursor = "…"` chooses the modes), new lines of a TODO section started
  (`start_tasks = false` turns that off), metadata suggested while typing
  (`suggest = false` turns that off), the vault settled on every write
  (`settle = false` turns that off: the lines then wait for the daemon). Errors surface
  through `vim.notify`.
- Gates: `luac -p neovim/lua/restask/*.lua`; `lua neovim/test/toggle_test.lua`.
