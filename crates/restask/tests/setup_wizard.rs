//! Setup wizard tests (§13.2): compose parsing (real tomsquest text), TODO.md adoption,
//! and the non-interactive full setup against the in-memory CalDAV mock.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tempfile::TempDir;

use common::{FixedClock, MockCaldav};
use restask::cli;
use restask::config::{MachineConfig, VaultConfig};
use restask::markdown::MARKER;
use restask::setup::{
    adopt_todo_md, parse_collections, parse_radicale_compose, run_setup, ComposeFacts, SetupArgs,
};
use restask::store::Index;
use restask::TaskresError;

fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock(
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ))
}

/// A bare vault: legacy TODO.md without the marker, no restask.toml, no `.taskres/`.
fn legacy_vault() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("TODO.md"),
        "# My tasks\n\n- [ ] water the plants\n\nSome prose.\n\n```tasks\n- [ ] inside tasks block\n```\n",
    )
    .unwrap();
    dir
}

fn args(vault: &TempDir) -> SetupArgs {
    SetupArgs {
        vault: vault.path().to_path_buf(),
        config_path: vault.path().join("machine.toml"),
        url: "http://radicale.local:5232".to_string(),
        username: "me".to_string(),
        password_env: Some("RESTASK_TEST_PASS".to_string()),
        password_file: None,
        collections: vec![("Home".to_string(), "home".to_string())],
    }
}

/// The canonical compose file from the tomsquest/docker-radicale README.
const TOMSQUEST_COMPOSE: &str = r#"version: "3"

services:
  radicale:
    image: tomsquest/docker-radicale
    container_name: radicale
    ports:
      - 5232:5232
    init: true
    read_only: true
    security_opt:
      - no-new-privileges:true
    healthcheck:
      test: ["CMD", "curl", "--fail", "http://localhost:5232"]
      interval: 30s
      timeout: 10s
      retries: 3
    volumes:
      - ./data:/data
      - ./config:/config
"#;

#[test]
fn parses_the_tomsquest_compose() {
    let facts = parse_radicale_compose(TOMSQUEST_COMPOSE);
    assert_eq!(facts.host_port, Some(5232));
    assert_eq!(facts.container_port, Some(5232));
}

#[test]
fn compose_edges_never_guess() {
    let none = ComposeFacts::default();

    // Empty input and invalid YAML yield no facts.
    assert_eq!(parse_radicale_compose(""), none);
    assert_eq!(parse_radicale_compose("\tbroken: ["), none);

    // Radicale present but ports behind a reverse proxy: no mapping to propose.
    let proxied =
        "services:\n  radicale:\n    image: tomsquest/docker-radicale\n    networks: [web]\n";
    assert_eq!(parse_radicale_compose(proxied), none);

    // Unrelated services are ignored.
    let other = "services:\n  web:\n    image: nginx\n    ports:\n      - 5232:5232\n";
    assert_eq!(parse_radicale_compose(other), none);

    // Long syntax mapping.
    let long = "services:\n  radicale:\n    image: tomsquest/docker-radicale\n    ports:\n      - target: 5232\n        published: 6000\n";
    let facts = parse_radicale_compose(long);
    assert_eq!(facts.host_port, Some(6000));
    assert_eq!(facts.container_port, Some(5232));

    // Bind address prefix and protocol suffix.
    let bound = "services:\n  radicale:\n    image: tomsquest/docker-radicale\n    ports:\n      - \"127.0.0.1:5232:5232/tcp\"\n";
    let facts = parse_radicale_compose(bound);
    assert_eq!(facts.host_port, Some(5232));
    assert_eq!(facts.container_port, Some(5232));
}

#[test]
fn adoption_backs_up_and_migrates_only_unfenced_checkboxes() {
    let legacy =
        "# My tasks\n\n- [ ] water the plants\n\n```tasks\n- [ ] inside tasks block\n```\n";
    let outcome = adopt_todo_md(legacy, clock().as_ref());

    assert_eq!(outcome.backup, "TODO.pre-restask-20260922-140000.md");
    assert_eq!(outcome.migrated, vec!["water the plants".to_string()]);
}

#[test]
fn backup_name_uses_the_device_local_stamp() {
    // Fixed clock: 2026-09-22 12:00 UTC with +02:00 → 14:00 local.
    let outcome = adopt_todo_md("", clock().as_ref());
    assert_eq!(outcome.backup, "TODO.pre-restask-20260922-140000.md");
}

#[test]
fn collection_flags_must_be_list_equals_collection() {
    assert_eq!(
        parse_collections(&["Home=home".to_string()]).unwrap(),
        vec![("Home".to_string(), "home".to_string())]
    );
    assert!(matches!(
        parse_collections(&["Home".to_string()]),
        Err(TaskresError::Validation { .. })
    ));
}

#[tokio::test]
async fn non_interactive_setup_end_to_end() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();

    let summary = run_setup(args(&vault), mock.clone(), clock())
        .await
        .unwrap();

    // Step 1: §14-default restask.toml written, `.taskres/` created.
    let cfg = VaultConfig::load(&vault.path().join("restask.toml")).unwrap();
    assert_eq!(cfg.done_heading, "Done");
    assert!(vault.path().join(".taskres").is_dir());

    // Step 2: verbatim backup, fresh marker scaffold, migrated line present with a UID.
    let backup_name = summary.backup.clone().unwrap();
    let backup = std::fs::read_to_string(vault.path().join(&backup_name)).unwrap();
    assert!(backup.contains("- [ ] water the plants"));
    assert!(backup.contains("```tasks"));
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert!(todo.contains(MARKER));
    assert!(todo.contains("## Inbox"));
    assert!(todo.contains("- [ ] water the plants"));
    assert!(todo.contains("🆔 taskres-"));
    assert!(!todo.contains("inside tasks block"));
    assert_eq!(summary.migrated, 1);

    // Steps 3–5: machine config records the endpoint and the env-var reference (never
    // the secret), the vault path, and the Home→home binding.
    let machine = MachineConfig::load(&vault.path().join("machine.toml")).unwrap();
    assert_eq!(
        machine.caldav.url.as_deref(),
        Some("http://radicale.local:5232")
    );
    assert_eq!(machine.caldav.username.as_deref(), Some("me"));
    assert_eq!(
        machine.caldav.password_env.as_deref(),
        Some("RESTASK_TEST_PASS")
    );
    assert!(machine.caldav.password_file.is_none());
    assert_eq!(machine.vault.path.as_deref(), Some(vault.path()));
    assert_eq!(machine.lists.len(), 1);
    assert_eq!(machine.lists[0].name, "Home");
    assert_eq!(machine.lists[0].collection, "home");

    // MKCOL happened for the binding, and the first sync ensured the inbox collection.
    assert!(mock.collection_names().iter().any(|slug| slug == "home"));
    assert!(mock.collection_names().iter().any(|slug| slug == "inbox"));

    // Step 6: the migrated task is registered and pushed.
    let index = Index::load(&vault.path().join(".taskres")).unwrap();
    assert_eq!(index.entries.len(), 1);
    let entry = index.entries.values().next().unwrap();
    assert!(entry.caldav_etag.is_some());
    assert_eq!(entry.list.as_str(), "inbox");
    assert_eq!(mock.resource_names("inbox").len(), 1);
}

#[tokio::test]
async fn setup_is_non_destructive_on_rerun() {
    let vault = legacy_vault();
    let mock = MockCaldav::new();
    run_setup(args(&vault), mock.clone(), clock())
        .await
        .unwrap();

    let second = run_setup(args(&vault), mock.clone(), clock())
        .await
        .unwrap();

    // The marker is present now: no second backup, no re-migration, no new pushes.
    assert!(second.backup.is_none());
    assert_eq!(second.migrated, 0);
    let index = Index::load(&vault.path().join(".taskres")).unwrap();
    assert_eq!(index.entries.len(), 1);
    assert_eq!(mock.resource_names("inbox").len(), 1);
    let todo = std::fs::read_to_string(vault.path().join("TODO.md")).unwrap();
    assert_eq!(
        todo.matches("water the plants").count(),
        1,
        "no duplicate migrated lines"
    );
}

#[test]
fn setup_vault_falls_back_to_cwd_with_todo_md() {
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(vault.path().join("TODO.md"), "# Taskres\n").unwrap();

    assert_eq!(
        cli::resolve_setup_vault_with(None, None, vault.path(), true).unwrap(),
        vault.path().to_path_buf()
    );

    let bare = tempfile::tempdir().unwrap();
    assert_eq!(
        cli::resolve_setup_vault_with(None, None, bare.path(), false).unwrap(),
        bare.path().to_path_buf()
    );
    assert!(matches!(
        cli::resolve_setup_vault_with(None, None, bare.path(), true),
        Err(TaskresError::Config { .. })
    ));
}

#[test]
fn setup_vault_prefers_flags_env_and_markers_over_the_fallback() {
    let vault = tempfile::tempdir().unwrap();
    std::fs::write(vault.path().join("restask.toml"), "").unwrap();
    std::fs::write(vault.path().join("TODO.md"), "").unwrap();
    let nested = vault.path().join("sub");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        cli::resolve_setup_vault_with(None, Some("/env/vault"), Path::new("/nowhere"), true)
            .unwrap(),
        PathBuf::from("/env/vault")
    );
    assert_eq!(
        cli::resolve_setup_vault_with(None, None, &nested, true).unwrap(),
        vault.path().to_path_buf()
    );
    assert_eq!(
        cli::resolve_setup_vault_with(
            Some(Path::new("/srv/other")),
            Some("/env/vault"),
            &nested,
            true
        )
        .unwrap(),
        PathBuf::from("/srv/other")
    );
}
