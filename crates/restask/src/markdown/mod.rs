//! Markdown grammar, mutation, and TODO.md view (§6–§7). Pure string processing — no I/O.

pub mod mutator;
pub mod parser;
pub mod todo_view;

pub use parser::{parse, parse_line, ParsedFile, ParsedTask, TaskDraft};
pub use todo_view::{inbox_line, mirror_edits, mirror_line, render, MARKER};
