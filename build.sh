#!/usr/bin/env bash
# Build and install the Python extension (voidcrawl) into the current venv,
# plus the standalone voidcrawl-mcp binary in target/release/ for local use.
#
# The two artifacts ship as separate packages now — `voidcrawl-mcp` has its
# own pyproject at crates/mcp_server/pyproject.toml. For a local editable
# MCP install, run:
#   maturin develop --release --manifest-path crates/mcp_server/pyproject.toml
# or just use the binary directly at target/release/voidcrawl-mcp.
set -euo pipefail
cd "$(dirname "$0")"

# 1. Build the MCP server binary for local dev use.
cargo build --release -p voidcrawl-mcp

# 2. Build + install the Python extension (editable).
maturin develop --release --manifest-path crates/pyo3_bindings/Cargo.toml
