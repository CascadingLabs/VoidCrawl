#![allow(clippy::expect_used, reason = "test fixture setup and assertions")]

use std::{fs, path::PathBuf, process::Command};

#[test]
fn path_consumer_resolves_provider_vendored_chromiumoxide_without_root_patch() {
    let core = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temporary = tempfile::tempdir().expect("temporary consumer");
    fs::create_dir(temporary.path().join("src")).expect("consumer source directory");
    fs::write(temporary.path().join("src/lib.rs"), "pub fn consumer() {}\n")
        .expect("consumer source");
    fs::write(
        temporary.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"voidcrawl-path-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nvoid_crawl_core = {{ path = {core:?}, default-features = false }}\n"
        ),
    )
    .expect("consumer manifest");

    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(temporary.path())
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "standalone path consumer metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parsed cargo metadata");
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let chromiumoxide = packages
        .iter()
        .find(|package| package["name"] == "chromiumoxide")
        .expect("chromiumoxide package");
    assert!(chromiumoxide["source"].is_null());
    let manifest = chromiumoxide["manifest_path"].as_str().expect("chromiumoxide manifest path");
    assert!(manifest.contains("vendor/chromiumoxide/Cargo.toml"));
}
