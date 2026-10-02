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

The inbox file (`vault.inbox_file`, default `TODO.md`) is engine-managed and always routes
to `vault.inbox_list` (default `inbox`) — the calendar the user bound it to during setup.

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
```

### 5.4 Lists and collections

- List `Home Lab` → slug `home-lab` → collection `<url>/<user>/home-lab/`. There is no
  other mapping: routing is declared in the notes and nowhere else.
- A routed list whose collection does not exist is created on the first pass
  (`MKCOL`, VTODO-only, display name from the slug) when `caldav.allow_create_lists` is
  true; otherwise its tasks stay local and a warning is logged each pass.
- **Home note** of a list: the note that receives tasks created on the server for that
  list — the list's root note if it has one, else its first routed note in path order.
  The inbox list's home is the inbox file.
- A pass looks at: the inbox list, every list a note routes to, and every list the index
  still references (so a move or deletion sees the old copy). Foreign tasks are adopted
  only in lists that have a home (§11 R5).
- Moving a task between differently-routed notes keeps its UID and **moves** the `VTODO`
  between collections (§11 R9).
