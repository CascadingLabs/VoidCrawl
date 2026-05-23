---
name: verification-plan
description: Use when deciding how to validate a VoidCrawl change across Rust, Python, bindings, and browser-dependent tests.
---
# VoidCrawl Verification Plan
Consider:
- `cargo check`
- `cargo clippy --workspace --all-targets`
- `cargo +nightly fmt --all`
- `./build.sh` when Python bindings are affected
- `uv run pytest tests/ -v`
- relevant Rust integration tests when core behavior changes

Separate cheap static checks from Chrome-dependent or cross-language validation.
