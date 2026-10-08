# restask spec — Note Routing (§5)

> Normative. Index and invariants: `ARCHITECTURE.md`.

## §5 Note routing (lists)

**Local-only by default.** A note takes part in syncing only when explicitly routed. Of an
unrouted note the engine reads the frontmatter block and nothing else: the body is never
loaded, parsed or modified.

### 5.1 Frontmatter markers

The frontmatter block is the lines between a `---` on the first line and the next `---`
(at most 512 lines; an unterminated block is not frontmatter). Within it, line-scanned
(no YAML parsing; keys case-sensitive, values trimmed, first occurrence wins):

```yaml
restask-list: University        # this note's tasks → list "University"
restask-list-root: Home Lab     # list "Home Lab" for THIS folder, recursively;
                                # the note itself belongs to it too (the list's root note)
```

A note may carry both: `restask-list` wins for the note; `restask-list-root` still
declares the folder.

**Hidden files and folders are not part of the vault.** A file or directory whose name
starts with `.` is never walked, at any depth and whatever `track` says: no frontmatter
is read there, no folder is declared from there, no line is registered there. Such
places hold other tools' copies of notes, markers and all — a file sync's version archive
(Syncthing's `.stversions/`, with `Home Lab~20261002-195517.md` next to an archived
`TODO~….md`), `.trash/`, `.git/` — and a copy of a routed note is not a note: scanned, an
archived root note routes its archived folder, every archived task becomes a second task
on the server and a mirror line that links to the copy. Obsidian does not show such
files either, so the plugin never sees them.

### 5.2 Resolution

For a note at vault-relative path `p` (other than the inbox file):

1. `restask-list: X` in its own frontmatter → list `slug(X)`.
2. Else the nearest enclosing directory (walking up to the vault root) that contains a
   note with `restask-list-root: X` → list `slug(X)`. Deeper roots shadow shallower ones.
3. Else **local-only**.

The inbox file (`vault.inbox_file`, default `TODO.md`) is engine-managed and routes to
`vault.inbox_list` (default `inbox`) — the calendar the user bound it to during setup —
except for a line that names another calendar itself (`📁 <name>`, §7.5): that task
lives in the calendar it names.

```rust
pub enum NoteRouting { LocalOnly, List(ListSlug) }
pub struct NoteMeta { pub path: String, pub file_list: Option<String>, pub folder_list: Option<String> }
pub fn scan_frontmatter(contents: &str) -> (Option<String>, Option<String>);
impl Router {
    pub fn build(metas: &[NoteMeta]) -> Result<Router, RestaskError>; // two roots in one dir → ListConflict
    pub fn resolve(&self, path: &str, file_list: Option<&str>) -> NoteRouting;
}
```

Two different roots declared for the same directory are a hard `ListConflict`
(`restask doctor` reports it; nothing syncs until it is resolved).

### 5.3 Example

```
2. Areas/Home Lab/Home Lab.md      restask-list-root: Home Lab   → home-lab (root note)
2. Areas/Home Lab/Security.md      (no marker)                   → home-lab (inherited)
University.md                      restask-list: University      → university
Journal.md                         (no marker)                   → local-only, untouched
TODO.md                            (engine-managed)              → vault.inbox_list
TODO.md, a line with `📁 work`     (the line says so, §7.5)      → work
```

### 5.4 Lists and collections

- List `Home Lab` → slug `home-lab` → the server's calendar of that name
  (`caldav::resolve_list`, pure), looked for in this order:
  1. the collection at the list's own path, `<url>/<user>/home-lab/` — asked for
     first, with the list's `REPORT`, so a list that is there costs no other request;
  2. else, in the account's listing: the one task calendar whose path spells the name
     another way (`Home-Lab`), else the one whose **display name** does (`Home Lab`).
     A calendar made in another client (DAVx⁵, Tasks.org, Thunderbird, Radicale's own
     page) has a generated path; its name is the only one the user ever gave it.
  Calendars that hold no tasks are found by their path only. Two calendars that answer
  alike are not chosen between: the list is left out of the pass (unknown, not empty)
  and a warning names them. Routing is declared in the notes and nowhere else; where
  a list was found is remembered in `.restask/calendars.json` (§9) only to notice that
  it is somewhere else now — such a list is a *reset* one for that pass (§11.4): what
  is missing in its new collection was not deleted.
- A routed list that **no** calendar answers to is created on the first pass, at its
  own path (`MKCOL`, VTODO-only, display name from the slug), when
  `caldav.allow_create_lists` is true; otherwise its tasks stay local and a warning is
  logged each pass. Setup creates by the same rule (`ensure_list`).
- **Home note** of a list: the note that receives tasks created on the server for that
  list — the list's root note if it has one, else its first routed note in path order.
  The inbox list's home is the inbox file. So is the home of a list named in
  `vault.todo_lists` (§7.5, §14.1) that no note routes to: the further calendars
  TODO.md shows. A list in `todo_lists` that has notes keeps its note as home.
- A pass looks at: the inbox list and the lists of `todo_lists`, every list a note
  routes to or a line of the inbox file names, and every list the index still
  references (so a move or deletion sees the old copy). Foreign tasks are adopted only
  in lists that have a home (§11 R5).
- Moving a task between differently-routed notes keeps its UID and **moves** the `VTODO`
  between collections (§11 R9).
