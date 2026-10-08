//! The `restask-vault/` sandbox as ready-made test input: tests work on a temporary copy
//! and never write to the sandbox itself (AGENTS.md, "Fixtures and the sandbox").
//!
//! Nothing here asserts what the notes say: the sandbox is where trials are made, so its
//! notes change (routing, `🆔`, whole files) while the tests must stay green.

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
    let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../restask-vault/TODO.md");
    let before = fs::read(&sandbox).unwrap();

    let copy = copy_test_vault();
    fs::write(copy.path().join("TODO.md"), "changed by a test\n").unwrap();

    assert_eq!(fs::read(&sandbox).unwrap(), before);
}

/// Settling a copy of the sandbox — registration, repair, TODO.md and the views its
/// root notes hold (§7.6) — leaves a vault a second settle writes nothing in, whatever
/// the sandbox's notes say today.
#[tokio::test]
async fn settling_the_copy_twice_writes_nothing_the_second_time() {
    use std::sync::Arc;

    use chrono::{FixedOffset, Utc};
    use common::FixedClock;
    use restask::caldav::Offline;
    use restask::config::{MachineConfig, VaultConfig};
    use restask::sync::Engine;

    fn files(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, out);
            } else if let Ok(text) = fs::read_to_string(&path) {
                out.push((path.display().to_string(), text));
            }
        }
        out.sort();
    }

    let copy = copy_test_vault();
    let cfg = VaultConfig::load(&copy.path().join("restask.toml")).unwrap_or_default();
    let clock = Arc::new(FixedClock(Utc::now(), FixedOffset::east_opt(0).unwrap()));
    let engine = Engine::new(copy.path(), cfg, MachineConfig::default(), Offline, clock);
    engine.settle().await.unwrap();
    let mut first = Vec::new();
    files(copy.path(), &mut first);
    engine.settle().await.unwrap();
    let mut second = Vec::new();
    files(copy.path(), &mut second);
    assert_eq!(first, second);
}
