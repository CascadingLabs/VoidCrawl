See @AGENTS.md

Always use Context7 MCP for library docs without having to be asked.

Use `cargo check` to type-check Rust. Use `cargo clippy --workspace --all-targets` for linting.
Use `cargo +nightly fmt --all` to format Rust.
Use `uv run ruff check` and `uv run ruff format` for Python files.
Use `uv run mypy` for Python type checking.

Run `uv run pytest tests/ -v` for Python tests (requires built extension via `./build.sh`).

NEVER use unittest — always pytest + pytest-asyncio.
NEVER use `time.sleep()` — always `asyncio.sleep()` or tenacity.
