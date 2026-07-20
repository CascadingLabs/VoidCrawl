#![allow(clippy::expect_used, clippy::unwrap_used, clippy::missing_panics_doc, clippy::panic)]

use std::{
    fs::{self, OpenOptions},
    process,
};

use fd_lock::RwLock as FileLock;
use void_crawl_core::{MAX_PROFILE_SPLIT_COPIES, ProfileRegistry, ProfileStatus, VoidCrawlError};

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
fn busy_profile_reports_voidcrawl_owner_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    registry.create_profile("owned", None, vec![]).unwrap();
    let _lease = registry.acquire_profile("owned").unwrap();

    let error = registry.acquire_profile("owned").unwrap_err();
    match error {
        VoidCrawlError::ProfileBusy { pid, acquired_at, .. } => {
            assert_eq!(pid, Some(process::id()));
            assert!(acquired_at.is_some());
        }
        other => panic!("expected ProfileBusy, got {other:?}"),
    }
}

#[test]
fn snapshot_is_isolated_and_cleans_up_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    let profile = registry.create_profile("source", None, vec![]).unwrap();
    fs::write(profile.profile.path.join("Default").join("Cookies"), "cookie-data").unwrap();
    fs::write(profile.profile.path.join("SingletonLock"), "transient").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tmp.path().join("outside-secret");
        fs::write(&outside, "must-not-be-copied").unwrap();
        symlink(outside, profile.profile.path.join("ExternalLink")).unwrap();
    }

    let snapshot = registry.snapshot_profile("source").unwrap();
    let snapshot_path = snapshot.path().to_path_buf();
    assert_eq!(
        fs::read_to_string(snapshot_path.join("Default").join("Cookies")).unwrap(),
        "cookie-data"
    );
    assert!(!snapshot_path.join(".voidcrawl.lock").exists());
    assert!(!snapshot_path.join("SingletonLock").exists());
    #[cfg(unix)]
    assert!(!snapshot_path.join("ExternalLink").exists());
    drop(snapshot);
    assert!(!snapshot_path.exists());
}

#[test]
fn split_profile_creates_same_baseline_in_unique_cleanup_scoped_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    let profile = registry.create_profile("source", None, vec![]).unwrap();
    fs::write(profile.profile.path.join("Default").join("Cookies"), "shared-baseline").unwrap();

    let split = registry.split_profile("source", 2).unwrap();
    let paths = split.iter().map(|snapshot| snapshot.path().to_path_buf()).collect::<Vec<_>>();
    assert_ne!(paths[0], paths[1]);
    for path in &paths {
        assert_eq!(
            fs::read_to_string(path.join("Default").join("Cookies")).unwrap(),
            "shared-baseline"
        );
        assert!(!path.join(".voidcrawl.lock").exists());
    }

    drop(split);
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn fork_native_profile_normalizes_default_and_copies_local_state() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path().join("registry"));
    let user_data = tmp.path().join("chrome-user-data");
    let native = user_data.join("Profile 7");
    fs::create_dir_all(&native).unwrap();
    fs::write(native.join("Preferences"), r#"{"profile":{"name":"Work"}}"#).unwrap();
    fs::write(native.join("Cookies"), "authenticated-state").unwrap();
    fs::write(user_data.join("Local State"), "encryption-context").unwrap();

    let forks = registry.fork_profile(native.to_str().unwrap(), 2).unwrap();
    let paths = forks.iter().map(|fork| fork.path().to_path_buf()).collect::<Vec<_>>();
    assert_ne!(paths[0], paths[1]);
    for path in &paths {
        assert_eq!(
            fs::read_to_string(path.join("Default").join("Cookies")).unwrap(),
            "authenticated-state"
        );
        assert_eq!(fs::read_to_string(path.join("Local State")).unwrap(), "encryption-context");
        assert!(!path.join("Profile 7").exists());
    }

    drop(forks);
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn fork_native_profile_rejects_a_running_chrome_root() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path().join("registry"));
    let user_data = tmp.path().join("chrome-user-data");
    let native = user_data.join("Default");
    fs::create_dir_all(&native).unwrap();
    fs::write(native.join("Preferences"), "{}").unwrap();
    fs::write(user_data.join("SingletonLock"), "active").unwrap();

    let error = registry.fork_profile(native.to_str().unwrap(), 2).unwrap_err();
    assert!(matches!(error, VoidCrawlError::ChromeProfileBusy { .. }));
}

#[test]
fn split_profile_rejects_unbounded_copy_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = ProfileRegistry::new(tmp.path());
    registry.create_profile("source", None, vec![]).unwrap();

    assert!(registry.split_profile("source", 1).is_err());
    assert!(registry.split_profile("source", MAX_PROFILE_SPLIT_COPIES + 1).is_err());
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
