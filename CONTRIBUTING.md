# Contributing to VoidCrawl

Thanks for contributing! This guide covers the development workflow for the Rust/PyO3 browser automation layer.

## Objectives

VoidCrawl is a Rust-native CDP (Chrome DevTools Protocol) wrapper with Python bindings via PyO3. Contributions that improve browser automation reliability, add new CDP capabilities, or improve the Python binding ergonomics are welcome.

## Clone & Setup

```bash
git clone https://github.com/CascadingLabs/VoidCrawl.git
cd VoidCrawl
```

**Prerequisites:**

| Tool | Version | Install |
|------|---------|---------|
| Rust | >= 1.86 | [rustup.rs](https://rustup.rs) |
| Python | >= 3.10 | System or [mise](https://mise.jdx.dev) |
| Chrome/Chromium | Any recent | System package manager |
| maturin | >= 1.7 | `cargo install maturin` |
| uv | Latest | `curl -LsSf https://astral.sh/uv/install.sh \| sh` |

### Install pre-commit hooks

```bash
uvx prek install
```

[Prek](https://github.com/thesuperzapper/prek) is a Rust-based pre-commit runner that executes git hooks automatically on every `git commit`, catching issues before they reach CI. It reads the same `.pre-commit-config.yaml` format. In this repo the hooks run cargo fmt and ruff (lint + format) on commit, check for secrets via gitleaks, and enforce conventional commit messages via commitizen. Clippy, cargo deny, and strict ruff are run in the manual stage (CI). To run all hooks manually:

```bash
uvx prek run --all-files
```

### Build the extension

```bash
./build.sh
```

This runs `maturin develop --release` which compiles the Rust code and installs the Python extension into the active venv.

### Run Rust tests

```bash
cargo test -p void_crawl_core -- --test-threads=1
```

Tests **must** run serially (`--test-threads=1`) because each test launches a Chromium process that uses a shared profile directory. Parallel launches cause `SingletonLock` conflicts.

### Build the MCP server

`voidcrawl-mcp` ships as a standalone package on PyPI (prebuilt binary wheels) and crates.io. Inside this repo it's a workspace member at `crates/mcp_server/` with its own `pyproject.toml` that uses maturin's `bindings = "bin"` mode — the Rust binary becomes a console script in the wheel.

For local dev:

```bash
cargo build --release -p voidcrawl-mcp
# binary at ./target/release/voidcrawl-mcp
```

Build the standalone wheel locally:

```bash
maturin build --release --manifest-path crates/mcp_server/Cargo.toml
# wheel at ./target/wheels/voidcrawl_mcp-*.whl
```

End users install one of:

```bash
uv tool install voidcrawl-mcp            # binary only
pipx install voidcrawl-mcp               # binary only
pip install 'voidcrawl[mcp]'             # Python lib + MCP binary
cargo install voidcrawl-mcp              # from crates.io, builds from source
```

Wire it into Claude Code:

```json
{
  "mcpServers": { "voidcrawl": { "command": "voidcrawl-mcp" } }
}
```

Restart Claude Code and the `voidcrawl` MCP server + its tools (`fetch`, `session_open`, `click`, `click_visual_coords`, `detect_captcha`, ...) become available.

### Use the `voidcrawl` Claude Code skill

A skill lives at `.claude/skills/voidcrawl/SKILL.md`. Claude Code auto-discovers skills in any project that has a `.claude/skills/` directory — you don't need to do anything beyond having `.mcp.json` + the built binary on disk. The skill tells Claude when to pick `voidcrawl` over `claude-in-chrome`, how to chain `click_visual_coords` for React forms, and how to react to `CaptchaDetected` errors.

For full protocol + tool reference see [`docs/mcp-server.md`](docs/mcp-server.md).

### Run Python tests

```bash
uv run pytest tests/unit/core/fetcher/test_browser.py -v
```

### Full CI check

```bash
uv run poe ci-check
```

## Linting & Formatting

### Rust

```bash
cargo check --workspace
cargo clippy --workspace
cargo fmt --check
```

- Follow standard Rust conventions (`rustfmt` defaults)
- Clippy config lives in `clippy.toml` -enforces cognitive complexity thresholds, MSRV 1.86
- `print!`/`println!` are disallowed -use `tracing` instead
- Use `thiserror` for error types -every new error variant goes in `error.rs`
- Map chromiumoxide errors to `YosoiError` at the boundary, not deep inside methods
- Builders use the owned-self pattern: `.method(self) -> Self`

### Python

```bash
uv run ruff check .
uv run ruff format --check .
uv run mypy .
```

- Follow the project's `ruff` config (see root `pyproject.toml`)
- Single quotes, 120-char line length
- Google-style docstrings
- Never use `unittest` -always `pytest`
- Use `tenacity` for retries -never `time.sleep()` in loops

CI runs clippy, cargo test, and ruff on every push and PR. Your PR must pass all checks.

## Issues

We use [GitHub issue forms](https://github.com/CascadingLabs/VoidCrawl/issues/new/choose) for all issues. Pick the template that fits:

- **Bug Report** -something is broken or behaving unexpectedly.
- **Feature Request** -suggest a new feature or improvement.
- **Question** -ask a question about usage or internals.
- **Ticket** -internal planning ticket for tracked work.

Blank issues are disabled -please use a template so we have the context we need to help.

## Pull Request Rules

1. **Branch from `main`** -create a feature branch (`feat/...`, `fix/...`, `docs/...`).
2. **Keep PRs focused** -one logical change per PR.
3. **Pass CI** -Rust compilation, clippy, tests, and Python lint must all pass.
4. **Use the PR template** -every PR auto-fills a template. Fill in all sections:
   - **Intent** -what the PR does and why.
   - **Changes** -a summary of what was changed.
   - **GenAI usage** -check the box and describe how AI was used, if applicable. All AI-generated code must be reviewed line-by-line.
   - **Risks** -any risks or side effects this PR might introduce.
5. **Link an issue** -reference the issue your PR addresses with `Closes #<number>`.

### Commit Conventions

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(driver): add cookie management to Page
fix(driver): handle Chrome SingletonLock race condition
test(driver): add stealth config integration test
```

## Project Layout

```
void_crawl/
├── crates/
│   ├── core/              # void_crawl_core -pure Rust CDP wrapper
│   │   ├── src/           # Library source
│   │   └── tests/         # Integration tests (require Chrome)
│   └── pyo3_bindings/     # void_crawl -PyO3 extension module
│       └── src/lib.rs     # Python class definitions
├── Cargo.toml             # Workspace root
├── pyproject.toml         # maturin build config
├── build.sh               # Build helper
└── void_crawl.pyi         # Python type stubs
```

## Adding a New Page Method

1. **Rust core** (`crates/core/src/page.rs`): Add the async method on `Page`
2. **PyO3 binding** (`crates/pyo3_bindings/src/lib.rs`): Add corresponding `#[pymethods]` on `PyPage`
3. **Type stub** (`void_crawl.pyi`): Add the async signature
4. **Test**: Add a Rust test in `crates/core/tests/integration.rs` and a Python test in `tests/unit/core/fetcher/test_browser.py`

## Adding a New BrowserSession Option

1. Add the field to `BrowserSessionBuilder` in `session.rs`
2. Wire it through `connect_or_launch()`
3. Expose it as a kwarg on `PyBrowserSession.__init__` in the bindings
4. Update the type stub and README

## PyO3 Binding Conventions

- Every Python-facing async method uses `pyo3_async_runtimes::tokio::future_into_py`
- The inner Rust object is wrapped in `Arc<Mutex<Option<T>>>` -`Option` for clean shutdown semantics
- Error conversion: `YosoiError` -> `PyRuntimeError` via `to_py_err()`
- Keep the binding layer thin -business logic belongs in `crates/core/`

## License

Contributions are licensed under Apache-2.0, matching the project.
