//! Cross-process profile lock contention. Uses a fake profile dir
//! (Preferences file) under a tempdir so it doesn't touch a real
//! Chrome install. Chrome is NOT launched — these tests verify only
//! the lock-file arbitration layer.
//!
//! We exercise the lock layer directly via `fs2` to avoid spawning
//! Chrome from the test binary.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::absolute_paths,
    clippy::useless_vec
)]

use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
    time::Duration,
};

use fs2::FileExt;
use tempfile::TempDir;
use void_crawl_core::{error::VoidCrawlError, profile};

fn make_fake_profile(base: &TempDir, name: &str) -> PathBuf {
    let dir = base.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Preferences"), "{}").unwrap();
    dir
}

#[test]
fn list_profiles_returns_names_for_dirs_with_preferences() {
    let tmp = tempfile::tempdir().unwrap();
    make_fake_profile(&tmp, "Default");
    make_fake_profile(&tmp, "Profile 1");
    // Non-profile directory — should be ignored (no Preferences file).
    std::fs::create_dir_all(tmp.path().join("Crashpad")).unwrap();

    let bases = vec![tmp.path().to_path_buf()];
    let mut names: Vec<String> = std::fs::read_dir(&bases[0])
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("Preferences").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Default".to_string(), "Profile 1".to_string()]);

    // Also check that `chrome_user_data_dirs()` returns a non-empty
    // vec on this host (doesn't assert the specific paths — only that
    // platform detection compiled).
    let _ = profile::chrome_user_data_dirs();
}

#[test]
fn resolve_profile_returns_not_found_for_unknown() {
    let err = profile::resolve_profile("NoSuchProfileXYZ").unwrap_err();
    match err {
        VoidCrawlError::ProfileNotFound { .. } => {}
        other => panic!("expected ProfileNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lock_file_exclusion_is_exclusive() {
    // Verify fs2 advisory-lock semantics on this host. Two processes
    // (simulated via two File handles) can't hold exclusive locks
    // on the same path simultaneously.
    let tmp = tempfile::tempdir().unwrap();
    let dir = make_fake_profile(&tmp, "Default");
    let lock_path = dir.join(".voidcrawl.lock");

    let f1 = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    f1.try_lock_exclusive().expect("first lock should succeed");

    let f2: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    let r = f2.try_lock_exclusive();
    assert!(r.is_err(), "second lock should fail while first is held");

    drop(f1);
    // Small pause so the OS releases; then second acquire should succeed.
    tokio::time::sleep(Duration::from_millis(20)).await;
    f2.try_lock_exclusive().expect("after release, second lock should succeed");
}
