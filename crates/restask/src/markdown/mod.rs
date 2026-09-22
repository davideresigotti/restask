//! Markdown grammar, mutation, and TODO.md view (§6–§7). Pure string processing — no I/O.

pub mod mutator;
pub mod parser;

pub use parser::{parse, parse_line, ParsedFile, ParsedTask, TaskDraft};
