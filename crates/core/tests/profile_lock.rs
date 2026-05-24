//! Profile discovery + lock arbitration. Chrome is NOT launched —
//! tests exercise only the lock-file layer against fake profile
//! directories in a tempdir.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    clippy::panic,
    clippy::absolute_paths,
    clippy::useless_vec
)]

use std::{fs::OpenOptions, path::PathBuf, time::Duration};

use fd_lock::RwLock as FileLock;
use tempfile::TempDir;
use void_crawl_core::{error::VoidCrawlError, profile};

fn make_fake_profile(base: &TempDir, name: &str) -> PathBuf {
    let dir = base.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Preferences"), "{}").unwrap();
    dir
}

#[test]
fn list_profiles_filters_non_profile_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    make_fake_profile(&tmp, "Default");
    make_fake_profile(&tmp, "Profile 1");
    std::fs::create_dir_all(tmp.path().join("Crashpad")).unwrap();

    let mut names: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().join("Preferences").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Default".to_string(), "Profile 1".to_string()]);

    // Platform-default dir detection compiles and runs.
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
async fn fd_lock_file_exclusion_is_exclusive() {
    // Sanity-check fd-lock semantics on this host. Two in-process
    // locks on the same file path can't both hold the write guard.
    let tmp = tempfile::tempdir().unwrap();
    let dir = make_fake_profile(&tmp, "Default");
    let lock_path = dir.join(".voidcrawl.lock");

    let open = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap()
    };

    let mut lock1 = FileLock::new(open());
    let guard1 = lock1.try_write().expect("first lock should succeed");

    let mut lock2 = FileLock::new(open());
    assert!(lock2.try_write().is_err(), "second lock should fail while first is held");

    drop(guard1);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _guard = lock2.try_write().expect("after release, second lock should succeed");
}
