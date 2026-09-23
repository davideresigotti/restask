# Taskres Spec — Markdown Grammar, Mutation & TODO.md View (§6–§7)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §6 Markdown Grammar & Mutation

### 6.1 Line grammar (parser)

Task line regex (Rust `regex` syntax; applied per line):

```regex
^(?P<indent>[ \t]*)(?P<marker>[-*+])[ \t]+\[(?P<check>[ xX])\][ \t]+(?P<body>.*)$
```

Only single-line, unordered-list checkboxes are tasks. Ordered (`1. [ ]`) and task-list syntax `-[ ]` (no space) are ignored. The parser handles LF and CRLF files; the mutator preserves each file's dominant line ending.

Metadata tokens are extracted from `body` anywhere after the first non-space character (order-insensitive parse):

| Token | Regex | Field |
|---|---|---|
| Priority | one of the five emoji as standalone token | `priority` |
| Due | `📅[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)` | `due` |
| Start | `🛫[ \t]+(…same pattern…)` | `start` |
| Scheduled | `⏳[ \t]+(…same pattern…)` | `scheduled` |
| Completed | `✅[ \t]+(\d{4}-\d{2}-\d{2})` | `completed_on` |
| Created | `➕[ \t]+(\d{4}-\d{2}-\d{2})` | `created` |
| UID | `🆔[ \t]+(taskres-[0-9a-z]{26})` | `uid` |

`text` = body with all matched token spans removed, then end-trimmed and internal runs of spaces/tabs collapsed to single spaces. `checked = true` for `[x]`/`[X]`.

**Canonical metadata tail order** (what the mutator writes): `<priority> 🛫 ⏳ 📅 ✅ ➕ 🆔`.

### 6.2 File-level rules

- **YAML frontmatter**: a `---` at byte 0 opens it; everything to the matching `---` is opaque except the §5.1 markers. No checkboxes inside are tasks.
- **Fenced code blocks**: lines between ``` fences (or ~~~) are never tasks — this includes obsidian-tasks ```tasks query blocks.
- **Done region**: the first heading whose text equals `vault.done_heading` (default `Done`, case-sensitive, any ATX level) starts the completed-records region; it extends to end of file. Tasks there are `Status::Completed` records (their `✅` date is authoritative). Headings: `^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$`; `heading` = nearest preceding heading text.
- **Subtasks**: a task line whose `indent` exceeds a previous task's `indent` is its child (nearest ancestor checkbox). `parent` = ancestor's UID (unknown/unregistered ancestors → `parent = None`, child treated as root). Children serialize to `RELATED-TO;TOREL=PARENT:<uid>`.

```rust
pub struct TaskDraft {
    pub uid: Option<TaskUid>, pub text: String, pub checked: bool,
    pub priority: Option<Priority>, pub due: Option<When>, pub start: Option<When>,
    pub scheduled: Option<When>, pub created: Option<LocalDate>, pub completed_on: Option<LocalDate>,
}
pub struct ParsedTask {
    pub line_no: usize, pub indent_chars: usize, pub raw: String,
    pub draft: TaskDraft, pub in_done_region: bool, pub heading: Option<String>,
}
pub struct ParsedFile { pub tasks: Vec<ParsedTask>, pub done_heading_line: Option<usize> }

/// Pure: never fails; non-task lines are simply absent. Invalid dates inside a matched
/// token are tolerated (token kept verbatim in `raw`, field left None, anomaly counted).
pub fn parse(contents: &str, cfg: &VaultConfig) -> ParsedFile;
pub fn link_parents(tasks: &[ParsedTask]) -> Vec<Option<TaskUid>>;
```

### 6.3 Mutations

```rust
pub enum WhenField { Due, Start, Scheduled }

pub enum Mutation {
    Register { line_no: usize, uid: TaskUid, created: LocalDate },  // append ➕ + 🆔 tokens
    SetStatus { uid: TaskUid, checked: bool, completed_on: Option<LocalDate> },
    SetPriority { uid: TaskUid, priority: Option<Priority> },
    SetWhen { uid: TaskUid, field: WhenField, value: Option<When> },
    EditText { uid: TaskUid, text: String },                        // replaces text, preserves metadata
    MoveToDone { uid: TaskUid },            // under done heading, NEWEST-ON-TOP; creates heading if absent
    RestoreFromDone { uid: TaskUid },       // to bottom of the file's active region
    Delete { uid: TaskUid },                // removes the line entirely
}

pub enum SkipReason { UidNotFound, LineChanged }
pub struct MutationOutcome { pub contents: String, pub applied: Vec<Mutation>, pub skipped: Vec<(Mutation, SkipReason)> }

/// Pure string→string. Locates lines by 🆔 token; rewrites the metadata tail canonically;
/// NEVER alters task text except via EditText; preserves indentation, list marker, and
/// non-task lines byte-for-byte.
pub fn apply(contents: &str, ops: &[Mutation], cfg: &VaultConfig, clock: &dyn Clock) -> Result<MutationOutcome, TaskresError>;

/// Write via `<dir>/.<name>.restask-tmp` + fsync + rename (same directory ⇒ atomic on POSIX).
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), TaskresError>;
```

Done-heading creation: append `\n<blank>\n### Done\n` (level 3) at end of file. The inbox file always carries `## Done` (§7), so creation only applies to vault notes.

### 6.4 Registration

On first sight of an unregistered checkbox in a **routed** note, the engine appends `➕ <today>` and `🆔 <new-uid>` (canonical order). Tasks in the done region are registered too (as `Status::Completed`).

## §7 TODO.md View Contract (engine-owned)

Fixture-derived (`test-vault/TODO.md`). The file is **fully regenerated** by every render and recreated by every `restask setup` (a pre-existing file is renamed to the backup, §13.2); user content is never preserved inside it.

```
---
restask-list: inbox
---

<!-- AUTOGENERATED BY Restask -->

# TODO

## Inbox
- [ ] Buy milk ➕ 2026-09-22 🆔 taskres-01jz…

## 🔺 Highest Priority
- [ ] Setup SSL certificate renew alert 🔺 [[Home Lab Test#TODO|Home Lab Test]] 🆔 taskres-01jz…

## ⏫ High Priority
…
## 🔼 Medium Priority
…
## 🔽 Low Priority
…
## ⏬ Lowest Priority
…

## Done
- [x] Take out trash 🔽 ✅ 2026-09-19 🆔 taskres-01jz…
```

Rules:
- The file opens with the frontmatter block `---` / `restask-list: <vault.inbox_list>` / `---` / blank (§5.2); then the marker line `<!-- AUTOGENERATED BY Restask -->` and a blank; then the `# <inbox-file-stem>` title (e.g. `# TODO`) and a blank. No timestamp callout: the same task set renders byte-identically.
- Section headings are `## Inbox` and `## <emoji> <Name> Priority` for the five priorities — emitted **only when non-empty**, in the fixed order Inbox, Highest→Lowest. `## Done` is **always present**, even when empty.
- **Mirror lines** (tasks whose source is another note): `- [<check>] <text> <priority?> <🛫?><⏳?><📅?> [[<source-stem>#<source-heading>|<source-stem>]] 🆔 <uid>`. `<source-stem>` = filename without `.md`; `#<source-heading>` omitted when the task has no preceding heading. If the stem contains `[`, `]`, `|`, or `#`, the wikilink is replaced by plain text and a `validation` anomaly is logged.
- **Inbox lines** (source = TODO.md): full canonical line, no wikilink.
- `## Done` holds only inbox-sourced completed tasks, **newest-on-top** (completed date desc, then ULID desc). Completed vault tasks disappear from TODO.md and live under their source file's Done heading.
- Ordering within priority sections: (source path lexicographic asc, line asc). Inbox: ULID asc (creation order).
- Completed vault tasks with a priority never appear in priority sections.

```rust
pub const MARKER: &str = "<!-- AUTOGENERATED BY Restask -->";
/// Idempotent full render. `vault_tasks` = every routed, registered task (all lists).
pub fn render(vault_tasks: &std::collections::BTreeMap<TaskUid, Task>, cfg: &VaultConfig) -> String;
pub fn mirror_line(task: &Task) -> String;
pub fn inbox_line(task: &Task) -> String;
```
