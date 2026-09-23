# Taskres Spec — Note Routing — Lists (§5)

> Normative. Split of `ARCHITECTURE.md` (index + invariants live there). Section numbers preserved — `AGENTS.md` references them.

## §5 Note Routing (Lists)

**Local-only by default.** A note participates in syncing only when explicitly routed. The engine never reads (beyond frontmatter), never parses the body of, and never modifies unrouted notes.

### 5.1 Frontmatter markers (line-scanned; no YAML parsing of foreign keys)

The frontmatter block is lines between a `---` at byte 0 and the next `---`. Within it, scan for (case-sensitive, value trimmed):

```yaml
restask-list: University        # this file's tasks → list "University"
restask-list-root: Home Lab     # declares list "Home Lab" for THIS folder (recursive);
                                # the file itself also belongs to it
```

Unknown keys/values are ignored. A file may carry both: `restask-list` wins for the file; `restask-list-root` still declares the folder.

### 5.2 Resolution chain (deterministic, doctor-verifiable)

For a note at vault-relative path `p` (excluding the engine-managed inbox file):

1. `restask-list: X` in `p`'s frontmatter → list `slug(X)`.
2. Nearest enclosing directory (walking up from `p`'s dir to vault root) containing a note with `restask-list-root: X` → list `slug(X)`. Deeper roots shadow shallower ones.
3. Otherwise → **`LocalOnly`**: no UID assignment, no VTODO, no TODO.md mirror, file untouched.

The inbox file (`TODO.md` by default) is engine-managed and routes to the list named by `inbox_list` in `restask.toml` (§14.1, default `inbox`) — the calendar the user bound TODO.md to during setup (§13.2).

```rust
pub enum NoteRouting { LocalOnly, List(ListSlug) }

pub struct NoteMeta { pub path: String, pub file_list: Option<String>, pub folder_list: Option<String> }

pub fn scan_frontmatter(contents: &str) -> (Option<String>, Option<String>); // (file_list, folder_list)

pub struct Router { roots: std::collections::BTreeMap<String, ListSlug> } // dir path → list

impl Router {
    /// dir keys are vault-relative, '/'-separated, no trailing slash; "" = vault root.
    /// Two different roots for the same dir → TaskresError::ListConflict (never silent).
    pub fn build(metas: &[NoteMeta]) -> Result<Router, TaskresError>;
    pub fn resolve(&self, path: &str, file_list: Option<&str>) -> NoteRouting;
}
```

### 5.3 Worked example (user's vault)

```
2. Areas/Home Lab/Home Lab.md      restask-list-root: Home Lab   → list home-lab (root note)
2. Areas/Home Lab/Security.md      (no marker)                   → home-lab (inherited)
2. Areas/Home Lab/Alarm.md         (no marker)                   → home-lab (inherited)
University.md                      restask-list: University      → university (binds to existing calendar)
Inbox.md                           (no marker)                   → LocalOnly
TODO.md                            (engine-managed)              → inbox_list (default inbox)
```

### 5.4 List ⇄ Radicale collection mapping

- List `Home Lab` → slug `home-lab` → CalDAV collection `<url>/<user>/home-lab/`, `MKCOL`'d on first push (displayname `Home Lab`, `supported-calendar-component-set: VTODO` only) when `caldav.allow_create_lists = true`.
- **Setup-confirmed inbox binding**: `restask setup` binds TODO.md to **one existing server calendar** the user types by name (case-insensitive, re-prompted on a miss; aborts when the server has no collections — the binding is required for sync). The typed calendar's slug becomes `vault.inbox_list` (§14.1), TODO.md carries it as `restask-list:` frontmatter, and the binding is recorded in machine config `[[lists]]`. Other lists map to same-named collections via their slug (`MKCOL` on first push when `caldav.allow_create_lists`); users declare them by hand with `restask-list`/`restask-list-root` frontmatter. Bound collections may contain foreign resources — see §10.5 foreign rules.
- Moving a task between differently-routed notes keeps its UID and **moves** the VTODO between collections (§11 R9).
