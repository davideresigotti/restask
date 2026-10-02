# restask spec — Markdown Grammar, Mutation & the TODO.md View (§6–§7)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §6 Markdown grammar & mutation

### 6.1 Line grammar (`markdown::parser`)

A task is a single-line, unordered-list checkbox:

```regex
^(?P<indent>[ \t]*)(?P<marker>[-*+])[ \t]+\[(?P<check>[ xX])\][ \t]+(?P<body>.*)$
```

Ordered items (`1. [ ]`) and `-[ ]` (no space) are not tasks. LF and CRLF are both read.

Metadata tokens may appear anywhere in `body`, in any order:

| Token | Regex | Field |
|---|---|---|
| Priority | one of 🔺 ⏫ 🔼 🔽 ⏬ as a standalone word | `priority` |
| Repeat | `🔁`, whitespace, a rule in the vault spelling (§3.6) — exactly the words that form the rule | `recurrence` |
| Due | `📅[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)` | `due` |
| Start | `🛫[ \t]+(…same…)` | `start` |
| Scheduled | `⏳[ \t]+(…same…)` | `scheduled` |
| Completed | `✅[ \t]+(\d{4}-\d{2}-\d{2})` | `completed_on` |
| Created | `➕[ \t]+(\d{4}-\d{2}-\d{2})` | `created` |
| Created, asked for | a `➕` as a standalone word with no date behind it | `wants_created` |
| Calendar | `📁[ \t]+([0-9A-Za-z]+(?:-[0-9A-Za-z]+)*)` — a calendar's name as one word, lowercased to its slug | `list` |
| UID | `🆔[ \t]+((?:restask\|taskres)-[0-9a-z]{26})` | `uid` |

`text` = body with every matched token removed, whitespace runs collapsed, trimmed.
`checked` = `[x]`/`[X]`. A token whose value is well-shaped but invalid (impossible date,
non-ULID body) leaves its field empty; the first occurrence of a repeated token wins.

A `🔁` not followed by a rule is ordinary text. A rule is written back in its canonical
spelling whenever its line is rewritten.

A bare `➕` is the request for the task's creation date (§6.4). It is not part of the
text; until it is answered it is written back, bare, in the created token's place.

A `📁` not followed by such a name is ordinary text. The token is read on every task
line and written back wherever its line is rewritten; it *means* something only in the
inbox file (§7.5).

**Canonical tail order** (what the mutator writes): `<priority> 🔁 🛫 ⏳ 📅 ✅ ➕ 📁 🆔`.

### 6.2 File-level rules

- **Frontmatter** (a `---` first line to the next `---`) holds no tasks.
- **Fenced code blocks** (``` or ~~~, including obsidian-tasks ` ```tasks ` queries) hold
  no tasks.
- **Done region**: the first heading whose text equals `vault.done_heading` (default
  `Done`, case-sensitive, any ATX level) starts it; it runs to the end of the file.
- **Headings**: `^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$`; a task's `heading` is the nearest
  preceding one.
- **Subtasks**: a task indented deeper than a previous task of the same section is its
  child (nearest ancestor checkbox). Nesting never crosses a heading. Tasks in the done
  region have no parent — completed records are a flat log.

```rust
pub struct TaskDraft { uid, text, checked, priority, due, start, scheduled, created, completed_on, recurrence, list }
pub struct ParsedTask { line_no, indent_chars, raw, draft, in_done_region, heading }
pub struct ParsedFile { tasks: Vec<ParsedTask>, done_heading_line: Option<usize> }
pub fn parse(contents: &str, cfg: &VaultConfig) -> ParsedFile;        // never fails
pub fn link_parents(tasks: &[ParsedTask]) -> Vec<Option<TaskUid>>;
```

### 6.3 Mutations (`markdown::mutator`) — pure `String → String`

```rust
pub enum Mutation {
    Register { line_no, uid, created },        // append 🆔 (and ➕ <created> when given) to an unregistered line
    SetCreated { uid, created },               // answer a bare ➕; a date the line states is kept
    Reassign { line_no, uid },                 // give a duplicated line its own UID
    SetRecurrence { uid, recurrence },
    Rekey { uid, new_uid },                    // the line becomes a record: new UID, no 🔁 (§11.6)
    SetStatus { uid, checked, completed_on },
    SetPriority { uid, priority },
    SetWhen { uid, field: WhenField, value },
    EditText { uid, text },
    MoveToDone { uid },
    RestoreFromDone { uid },
    Delete { uid },
    Insert { draft, under: Option<TaskUid> },  // a task that came from the server
}
pub fn apply(contents, ops, cfg, clock) -> Result<MutationOutcome, RestaskError>;
```

- Lines are located by their `🆔` token (`Register`/`Reassign` by 1-based line number of
  the input, so they come first in a batch).
- A targeted line is re-rendered canonically; its text changes only through `EditText`.
  Every other line — including its line ending — is preserved byte for byte. Inserted and
  moved lines use the file's dominant line ending.
- `MoveToDone`: directly under the done heading (newest on top). Without a done heading,
  one is created at the end of the file: one blank line, `### <done_heading>`, the line.
- `RestoreFromDone` and `Insert` of an active task: bottom of the active list — after the
  last active task; else above the done heading (and the blank lines before it); else end
  of file. `Insert` of a completed task goes under the done heading.
- A line that moves between regions is un-indented: it leaves its parent's subtree and
  must not nest under whatever precedes it at the destination.
- `Insert { under: Some(parent) }` places the line right below the parent, one indent
  step deeper (a tab if the file indents with tabs, else four spaces).

The mutator performs no I/O; callers persist through `fsio`.

### 6.4 Registration and repair (`vault::scan`)

Every pass scans the routed notes and, in repair mode, rewrites the ones that need it
(one atomic write per file; a file that changed on disk since it was read is left for the
next pass):

- **Register** — a task line without a UID gets a fresh `🆔`, and nothing else.
- **A checkbox without text is not a task yet.** A line of the §6.1 shape whose `text`
  is empty and that has no UID — `- [ ] `, also with metadata tokens only — is the line
  an editor opens for a task still to be written (§15.7, or a list continuation). It is
  left exactly as it is: not registered, not stamped or moved for its box, not pushed.
  With text it is registered like any other line; with a UID (a task whose text was
  deleted, or that came from the server without one) it stays the task it is.
- **The creation date is optional.** A line shows `➕ <date>` only when the user asks for
  it by writing a bare `➕` (the plugin's `created` suggestion writes one, §15.2). The
  request is answered in the same pass, before the task is rendered or pushed:
  on a line that is being registered with **today**; on a registered task with the
  server's `CREATED` as the base snapshot recorded it at the last sync (§9), else — the
  server has never had the task, or the snapshot is not vouched for — with the day its
  UID was minted (`TaskUid::created_on`), else with today (`Scan::created_requests`,
  `SetCreated`). No server round trip is needed. A date written by hand is taken as it
  is; a `➕ <date>` that is on a line stays, and counts as the task's creation date
  (§11.3). Deleting it changes nothing on the server. Lines that restask writes itself —
  tasks pulled from the server, `restask add`, the next occurrence of a repeating task —
  carry no `➕` unless the line they replace had one. A bare `➕` on a mirror line of
  TODO.md asks nothing: the date belongs on the task's own line.
- **Duplicates** — a line sharing its UID with another line (a copied line) gets a fresh
  UID. The line in the note the index knows as the task's source keeps the UID; else the
  first occurrence in path order.
- **The checkbox is the status** — in notes:
  checked outside the done region → stamp `✅ <today>` (if missing) and `MoveToDone`;
  unchecked inside it → `RestoreFromDone` and drop `✅`;
  checked without `✅` → stamp it; unchecked with `✅` → drop it.
  So checking a box in any editor completes the task properly.
- In the **inbox file** only identity is repaired (register, duplicates); placement is the
  render's job (§7). A line registered there under a priority section's heading also
  takes that priority (§7.4). A line there that names a calendar (`📁`) is a task of
  that calendar (§7.5).
- Syncthing conflict copies (`*.sync-conflict-*`) and setup backups (`*.pre-restask-*`)
  are never scanned; `restask doctor` reports the former. Neither is anything hidden
  (§5.1) — a file sync's version archive among it.

## §7 The TODO.md view (`markdown::todo_view`)

The inbox file is engine-owned: it is **fully regenerated** by every render. It holds two
kinds of lines.

- **Inbox lines** — tasks whose source *is* this file (quick captures, tasks created in
  the inbox calendar or in another calendar the file shows, §7.5): full canonical lines.
- **Mirror lines** — views of prioritized tasks that live in notes:
  `- [ ] <text> <priority> <🔁?><🛫?><⏳?><📅?> [[<stem>#<heading>|<stem>]] 🆔 <uid>`
  (`#<heading>` omitted when the task has none; a stem containing `[`, `]`, `|` or `#` is
  written as plain text instead of a wikilink).

```
---
restask-list: inbox
restask-render: 9c1f0e2a7b3d4f56
---
# TODO

## 🔺 Highest Priority
- [ ] Renew the certificate 🔺 [[Home Lab#Tasks|Home Lab]] 🆔 restask-01jz…
- [ ] Update restask README 🔺 📁 work 🆔 restask-01jz…

## 🔽 Low Priority
- [ ] Sort the cables 🔽 🆔 restask-01jz…

## No Priority
- [ ] Buy milk ➕ 2026-09-22 🆔 restask-01jz…
- [ ] Call the bank 🆔 restask-01jz…

## Done
- [x] Take out trash ✅ 2026-09-19 🆔 restask-01jz…
```

The view is the file at `vault.inbox_file`, and it is known by that path alone — to the
engine and to the plugin (§15.6). Nothing in a file makes it the view, and nothing in a
note makes the note one.

Render rules (`render(tasks, cfg)` — same input, byte-identical output):

- Frontmatter: `restask-list: <vault.inbox_list>`, then the seal (§7.2) as a second
  property; directly below its closing `---`, with no blank line between them,
  `# <inbox file stem>`, then one blank. The body holds the view and nothing else
  (§7.3). A view rendered with a blank line above the title loses it at the next
  render.
- `## <emoji> <Name> Priority`, Highest → Lowest: active tasks with that priority — inbox
  lines and mirror lines together — by (source path, line).
- `## No Priority`, below the priority sections: active inbox tasks without a priority,
  by UID (creation order). A view rendered before this section had them in `## Inbox`
  at the top; the next render moves them.
- **Unprioritized note tasks are not shown**: they stay in their note only.
- `## Done`: completed *inbox* tasks, newest first (date desc, then UID desc). Completed
  note tasks are not shown: they live under their note's done heading.
- Empty sections are omitted, except `## Done`, which is always present.
- Anything else in the file — prose, blank lines, reordering — does not survive a render.

### 7.1 The view is editable

A mirror line is the same task as its source line, so editing it must reach the note.
The engine keeps its own last render in `.restask/todo.rendered.md`; at the start of each
pass it compares the live file against it (`mirror_edits`):

- a mirror line equal to the remembered render was not touched (it may be stale; the next
  render fixes that);
- a field the user changed — checkbox, text, priority, repeat rule, due/start/scheduled — is applied to
  the source note as mutations (a checked box becomes `SetStatus` + `MoveToDone`, with
  the line's own `✅` date if it has one), **provided the note still shows the rendered
  value**. If the note changed that field too, the note wins.

**Deleting a mirror line deletes the task**: its line is removed from the source note
(`Mutation::Delete`), and from there it is a deleted vault line like any other (§11:
deleted on the server). A missing line is absence, and absence proves nothing
(invariant 2), so the engine reads it as a deletion only when all of this holds:

- the view is **this engine's last render, edited**: unsealed, and its seal line still
  claims the digest of the remembered render. A view edited from another render — an
  older one that reaches the daemon late, one a device rendered — may never have had
  the line;
- the view is **whole**: it reaches down to the `## Done` heading every render ends
  with. A file cut short lost its lines to no one's decision; a missing or empty file
  has no seal line at all;
- the task's UID is **nowhere** in the view any more (a line mangled into something that
  is not a task is not a deletion);
- the note **still shows the task as it was rendered** (`mirror_line(source)` equals the
  remembered line). If the note changed the task since, the note wins.

Otherwise nothing is deleted and the render puts the line back. A device that takes a
mirror line out of the view itself (§15.6: the task was completed or deleted in its
note) and does not seal the view in the same write sets the seal line to
`restask-render: 0000000000000000` — a claim of no render — so that its own removal is
never read as the user's deletion by a daemon that has the view before the note.

A **sealed** view (§7.2) carries no edits, however much it differs from the remembered
render.

The same comparison finds the lines the user moved to another section (§7.4).

Telling inbox lines from mirror lines when the file is read: a line whose UID is claimed
by a note is a mirror line. A UID no note claims is a mirror line whose source is gone if
the index says the task lived in a note (or, with no index entry, if the line has the
rendered shape — a wikilink right before `🆔`); otherwise it is an inbox task, whatever
its text contains.

### 7.2 The seal

The frontmatter property `restask-render: <digest>`, on the line below `restask-list`,
seals a render: the digest is FNV-1a (64-bit, 16 lowercase hex digits) over the UTF-8
text of the view without that line. A view whose seal matches (`is_sealed`) is exactly
what some device rendered. Any edit made afterwards, in any editor, breaks the seal. The
seal is read from the frontmatter block only: the same text in the body seals nothing.

It exists because the daemon is not the only renderer: a device that settles tasks
itself (§15.6) leaves a TODO.md that differs from this engine's remembered render, and
the notes that explain the difference travel as separate files. Read as edits (§7.1),
those differences would be written into notes whose newer versions are still in
transit — two versions of one note, a file-sync conflict. So:

- a sealed view is never a source of mirror edits; the engine re-renders from the notes
  it has (the view may flip back once and forward again when the notes arrive);
- a device may seal only what holds no edit still to be carried: it carries every edit
  the user makes in the view to the notes first, and leaves the seal broken when it
  cannot — the daemon then carries them by §7.1;
- a view without the property (rendered by an older daemon, which sealed in a comment
  line of the body, or not at all), or with CRLF line endings, is not sealed.

### 7.3 Nothing in the body but the view

restask writes no comment line into any file. Earlier renders opened the body of TODO.md
with `<!-- AUTOGENERATED BY Restask -->` and `<!-- restask-render: <digest> -->`; both
are gone (owner, 2026-10-02):

- a render writes neither, and since it regenerates the file, a view that still has them
  loses them at the next render — with no edit carried on that pass or the next;
- the comment seal seals nothing any more (§7.2);
- the two lines are still *read* in one place: `is_view` — "this inbox file is one of
  restask's", asked by `restask setup` to tell a join from a fresh setup (§13.2) and by
  `restask doctor` — is true for a file whose frontmatter has the seal property, or
  that has one of the two old lines. It is never how a pass or the plugin finds the
  view;
- in a note, such a line is the note's own text: kept, and without meaning.

### 7.4 The section a task is typed in

A task written into TODO.md belongs to the section it was written in:

- A line **without a UID** (a new task) under the heading of a priority section —
  the heading text exactly as the render writes it, `section_priority` — and without a
  priority of its own is registered with that section's priority: `- [ ] call the bank`
  typed under `## 🔺 Highest Priority` becomes
  `- [ ] call the bank 🔺 🆔 <uid>` and stays there.
- A priority written on the line wins: the task is registered with it and the render
  files it in that priority's section.
- Under `## No Priority`, `## Done` or any other heading, and above the first heading,
  a new line gets no priority.
- **Changing the emoji** of a registered line moves the task to that priority's section;
  removing it moves the task to `## No Priority` (a mirror line then leaves the view,
  §7).
- **Moving** a registered, active line to another section — cut and paste, drag, a
  move-line command — with its emoji left as it was gives it that section's priority:
  under a priority section's heading that priority, under `## No Priority` none. The
  line's emoji is rewritten and the line stays where the user put it. For a mirror line
  the change is made in the note (§7.1), so a mirror line moved under `## No Priority`
  leaves the view with the next render.
- When a line was both moved and given another emoji, the emoji wins: it is what the user
  wrote. A move under `## Done` or any other heading, or above the first one, changes
  nothing; the render files the line by its emoji.
- Only in the inbox file: a heading with such a text in a note means nothing.

A new line gets its priority in the write that registers it (`Register` +
`SetPriority`, §6.4), so no pass ever sees the task without it. A move is recognised
like any other edit of the view (§7.1), against the engine's last render: the same UID
under another heading, unchecked in both, with the priority it was rendered with
(`mirror_edits`; for a line of the view's own the mutation is a `SetPriority` on the
inbox file itself). Once applied, the line's emoji differs from the render and the move
is not seen again. A sealed view (§7.2) holds no moves. The plugin does both on the
device (§15.6).

### 7.5 Tasks of other calendars

TODO.md is the organizer for every calendar the user wants in it, not for one. It is
*bound* to one calendar, `vault.inbox_list`, and *shows* the further ones named in
`vault.todo_lists` (§14.1; `restask setup` asks for both, §13.2).

- **A line of the view's own says which calendar it lives in.** With a calendar token
  (`📁 work`, §6.1) the task belongs to that list; without one, to `inbox_list`. The
  line is the evidence, not the sync state: with the state gone (`restask rebuild`) an
  unmarked line of another calendar would be read as a task of the bound one and moved
  there on the server.
- **The render** writes the token on every line of the view's own whose list is not
  `inbox_list` (`inbox_line`), as the last thing before `🆔`, and never on a line of
  the bound calendar — naming that one on a line is the same as naming none, and the
  render drops it. Sections, order and seal are as in §7: a task of another calendar is
  filed by its priority like any line of the view's own, and under `## Done` when
  completed.
- **A task created on the server** in a calendar of `todo_lists` gets its line in
  TODO.md, with the token — unless a note routes to that calendar: then it goes to the
  note that is the list's home (§5.4), as before, and reaches TODO.md as a mirror line
  when it has a priority. A calendar that is neither bound, nor in `todo_lists`, nor
  routed by a note is not looked at.
- **Typing the token** on a new line creates the task in that calendar; **changing**
  it moves the task there (§11 R9: same UID, the server copy moves between
  collections); **removing** it moves the task to the bound calendar. A name that is no
  calendar yet is a new list, created like a routed note's (§5.4). The token routes
  whether or not its calendar is in `todo_lists`: the vault says where a task lives,
  `todo_lists` only says what is brought *into* the view.
- **Taking a calendar out of `todo_lists`** removes nothing: its lines stay, still
  name it and still sync with it; new tasks made there on the server no longer come in.
- **Only in the inbox file.** On a line of a note the token is kept where the line is
  rewritten and decides nothing: a note's tasks live in the list the note routes to. A
  mirror line carries the link to its note instead and no token.
- The whole of it is local work (invariant 12): the token is read by the scan and
  written by the render with no server, and the plugin keeps it in its place when it
  registers, stamps or files a line (§15.6).

```rust
pub fn section_priority(heading: &str) -> Option<Priority>;
pub fn is_view(contents: &str) -> bool;
pub fn is_sealed(contents: &str) -> bool;
pub fn render(tasks: &BTreeMap<TaskUid, Task>, cfg: &VaultConfig) -> String;
pub fn mirror_line(task: &Task) -> String;
pub fn inbox_line(task: &Task, cfg: &VaultConfig) -> String;   // with `📁` for another calendar
pub fn mirror_edits(current, rendered, local, cfg, today) -> BTreeMap<String, Vec<Mutation>>;
```
