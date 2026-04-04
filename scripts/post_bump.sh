#!/bin/bash
set -euo pipefail

NEW_VERSION="$CZ_PRE_NEW_VERSION"

# Update package versions in Cargo.toml files
sed -i "s/^version = \"[^\"]*\"/version = \"${NEW_VERSION}\"/" crates/core/Cargo.toml
sed -i "s/^version = \"[^\"]*\"/version = \"${NEW_VERSION}\"/" crates/pyo3_bindings/Cargo.toml

# Update the intra-workspace dependency version
sed -i "/^void_crawl_core/s/version = \"[^\"]*\"/version = \"${NEW_VERSION}\"/" crates/pyo3_bindings/Cargo.toml

# Regenerate lock files
cargo generate-lockfile
uv lock

# Amend the bump commit to include all updated files, then re-tag
git add crates/core/Cargo.toml crates/pyo3_bindings/Cargo.toml Cargo.lock uv.lock
git commit --amend --no-edit
git tag -f "$CZ_POST_CURRENT_TAG_VERSION"
