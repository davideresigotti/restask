//! Atomic file writes (`fsio`): temp sibling + rename, crash leftovers, mode handling,
//! and change detection.

use std::fs;
use std::path::PathBuf;

use restask::fsio::{write_atomic, write_atomic_mode, write_if_changed};

#[test]
fn write_atomic_renames_a_hidden_tmp_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");

    write_atomic(&path, "hello\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    let entries: Vec<PathBuf> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries, vec![path.clone()]);

    write_atomic(&path, "second\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_leftover_tmp_from_a_crash_never_blocks_later_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.md");
    fs::write(dir.path().join(".note.md.restask-tmp"), "half-written").unwrap();

    write_atomic(&path, "fresh\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "fresh\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn write_if_changed_skips_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    assert!(write_if_changed(&path, "{}\n").unwrap());
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(!write_if_changed(&path, "{}\n").unwrap());
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    assert!(write_if_changed(&path, "{\"a\":1}\n").unwrap());
}

#[cfg(unix)]
#[test]
fn file_modes_are_forced_or_preserved() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    let secret = dir.path().join("config.toml");
    write_atomic_mode(&secret, "a = 1\n", Some(0o600)).unwrap();
    assert_eq!(mode(&secret), 0o600);

    // A plain rewrite keeps whatever mode the file already had.
    let note = dir.path().join("note.md");
    fs::write(&note, "old").unwrap();
    fs::set_permissions(&note, fs::Permissions::from_mode(0o640)).unwrap();
    write_atomic(&note, "new").unwrap();
    assert_eq!(mode(&note), 0o640);
}
