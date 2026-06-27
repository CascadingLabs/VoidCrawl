#![allow(clippy::expect_used, clippy::unwrap_used, clippy::missing_panics_doc, clippy::panic)]

use std::fs::OpenOptions;

use fd_lock::RwLock as FileLock;
use void_crawl_core::{ProfileRegistry, ProfileStatus, VoidCrawlError};

#[test]
fn registry_uses_env_root_and_creates_profile_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());

    let created = registry
        .create_profile("google-001", Some("Google SERP profile".into()), vec!["serp".into()])
        .unwrap();

    assert_eq!(created.profile.id, "google-001");
    assert_eq!(created.profile.description.as_deref(), Some("Google SERP profile"));
    assert_eq!(created.profile.labels, vec!["serp"]);
    assert!(created.profile.path.ends_with("google-001"));
    assert!(created.profile.path.join("Default").join("Preferences").is_file());
    assert_eq!(created.status, ProfileStatus::Available);

    let listed = registry.list_profiles().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].profile.id, "google-001");
}

#[test]
fn delete_rejects_locked_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    let profile = registry.create_profile("busy", None, vec![]).unwrap();
    let lock_path = profile.profile.path.join(".voidcrawl.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    let mut lock = FileLock::new(file);
    let _guard = lock.try_write().unwrap();

    let err = registry.delete_profile("busy").unwrap_err();
    assert!(matches!(err, VoidCrawlError::ProfileBusy { .. }));
}

#[test]
fn pool_validation_and_resolution_preserve_order() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    registry.create_profile("a", None, vec![]).unwrap();
    registry.create_profile("b", None, vec![]).unwrap();

    let pool = registry.create_pool("google-serp", vec!["a".into(), "b".into()], 3).unwrap();
    assert_eq!(pool.profile_ids, vec!["a", "b"]);
    assert_eq!(pool.max_active, 3);

    let resolved = registry.resolve_pool("google-serp").unwrap();
    let ids: Vec<_> = resolved.profiles.into_iter().map(|p| p.profile.id).collect();
    assert_eq!(ids, vec!["a", "b"]);

    let err = registry.create_pool("bad", vec!["missing".into()], 3).unwrap_err();
    assert!(matches!(err, VoidCrawlError::ProfileNotFound { .. }));
}

#[test]
fn pool_leasing_respects_active_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    registry.create_profile("a", None, vec![]).unwrap();
    registry.create_profile("b", None, vec![]).unwrap();
    registry.create_pool("single", vec!["a".into(), "b".into()], 1).unwrap();

    let first = registry.acquire_from_pool("single").unwrap();
    assert_eq!(first.id(), "a");
    let second = registry.acquire_from_pool("single").unwrap();
    assert_eq!(second.id(), "b");

    let err = registry.acquire_from_pool("single").unwrap_err();
    assert!(matches!(err, VoidCrawlError::ProfileBusy { .. }));
}
