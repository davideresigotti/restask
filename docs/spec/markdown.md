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
| UID | `🆔[ \t]+((?:restask\|taskres)-[0-9a-z]{26})` | `uid` |

`text` = body with every matched token removed, whitespace runs collapsed, trimmed.
`checked` = `[x]`/`[X]`. A token whose value is well-shaped but invalid (impossible date,
non-ULID body) leaves its field empty; the first occurrence of a repeated token wins.

A `🔁` not followed by a rule is ordinary text. A rule is written back in its canonical
spelling whenever its line is rewritten.

**Canonical tail order** (what the mutator writes): `<priority> 🔁 🛫 ⏳ 📅 ✅ ➕ 🆔`.

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
pub struct TaskDraft { uid, text, checked, priority, due, start, scheduled, created, completed_on, recurrence }
pub struct ParsedTask { line_no, indent_chars, raw, draft, in_done_region, heading }
pub struct ParsedFile { tasks: Vec<ParsedTask>, done_heading_line: Option<usize> }
pub fn parse(contents: &str, cfg: &VaultConfig) -> ParsedFile;        // never fails
pub fn link_parents(tasks: &[ParsedTask]) -> Vec<Option<TaskUid>>;
```

### 6.3 Mutations (`markdown::mutator`) — pure `String → String`

```rust
pub enum Mutation {
    Register { line_no, uid, created },        // append ➕ + 🆔 to an unregistered line
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

- **Register** — a task line without a UID gets `➕ <today>` and a fresh `🆔`.
- **Duplicates** — a line sharing its UID with another line (a copied line) gets a fresh
  UID. The line in the note the index knows as the task's source keeps the UID; else the
  first occurrence in path order.
- **The checkbox is the status** — in notes:
  checked outside the done region → stamp `✅ <today>` (if missing) and `MoveToDone`;
  unchecked inside it → `RestoreFromDone` and drop `✅`;
  checked without `✅` → stamp it; unchecked with `✅` → drop it.
  So checking a box in any editor completes the task properly.
- In the **inbox file** only identity is repaired (register, duplicates); placement is the
  render's job (§7).
- Syncthing conflict copies (`*.sync-conflict-*`) and setup backups (`*.pre-restask-*`)
  are never scanned; `restask doctor` reports the former.

## §7 The TODO.md view (`markdown::todo_view`)

The inbox file is engine-owned: it is **fully regenerated** by every render. It holds two
kinds of lines.

- **Inbox lines** — tasks whose source *is* this file (quick captures, tasks created in
  the inbox calendar): full canonical lines.
- **Mirror lines** — views of prioritized tasks that live in notes:
  `- [ ] <text> <priority> <🔁?><🛫?><⏳?><📅?> [[<stem>#<heading>|<stem>]] 🆔 <uid>`
  (`#<heading>` omitted when the task has none; a stem containing `[`, `]`, `|` or `#` is
  written as plain text instead of a wikilink).

```
---
restask-list: inbox
---

<!-- AUTOGENERATED BY Restask -->

# TODO

## Inbox
- [ ] Buy milk ➕ 2026-09-22 🆔 restask-01jz…

## 🔺 Highest Priority
- [ ] Renew the certificate 🔺 [[Home Lab#Tasks|Home Lab]] 🆔 restask-01jz…

## 🔽 Low Priority
- [ ] Sort the cables 🔽 ➕ 2026-09-22 🆔 restask-01jz…

## Done
- [x] Take out trash ✅ 2026-09-19 ➕ 2026-09-18 🆔 restask-01jz…
```

Render rules (`render(tasks, cfg)` — same task set, byte-identical output):

- Frontmatter `restask-list: <vault.inbox_list>`, blank, the marker line, blank,
  `# <inbox file stem>`, blank.
- `## Inbox`: active inbox tasks without a priority, by UID (creation order).
- `## <emoji> <Name> Priority`, Highest → Lowest: active tasks with that priority — inbox
  lines and mirror lines together — by (source path, line).
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

Deleting a mirror line does nothing (the render restores it); delete the task in its note.

Telling inbox lines from mirror lines when the file is read: a line whose UID is claimed
by a note is a mirror line. A UID no note claims is a mirror line whose source is gone if
the index says the task lived in a note (or, with no index entry, if the line has the
rendered shape — a wikilink right before `🆔`); otherwise it is an inbox task, whatever
its text contains.

```rust
pub const MARKER: &str = "<!-- AUTOGENERATED BY Restask -->";
pub fn render(tasks: &BTreeMap<TaskUid, Task>, cfg: &VaultConfig) -> String;
pub fn mirror_line(task: &Task) -> String;
pub fn inbox_line(task: &Task) -> String;
pub fn mirror_edits(current, rendered, local, cfg, today) -> BTreeMap<String, Vec<Mutation>>;
```
