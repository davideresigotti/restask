//! The `test-vault/` sandbox as ready-made test input: tests work on a temporary copy
//! and never write to the sandbox itself (AGENTS.md, "Fixtures and the sandbox").

mod common;

use std::fs;
use std::path::Path;

use common::copy_test_vault;

#[test]
fn the_copy_holds_the_visible_notes_and_none_of_the_hidden_state() {
    let copy = copy_test_vault();

    assert!(copy.path().join("TODO.md").is_file());
    let hidden: Vec<_> = fs::read_dir(copy.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(hidden.is_empty(), "hidden entries copied: {hidden:?}");
}

#[test]
fn writing_to_the_copy_leaves_the_sandbox_untouched() {
    let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-vault/TODO.md");
    let before = fs::read(&sandbox).unwrap();

    let copy = copy_test_vault();
    fs::write(copy.path().join("TODO.md"), "changed by a test\n").unwrap();

    assert_eq!(fs::read(&sandbox).unwrap(), before);
}
