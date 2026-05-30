//! Tool implementations for the `VoidCrawl` MCP server.
//!
//! Each sub-module declares a `#[tool_router]` impl block on
//! `VoidCrawlServer` and returns its own named router function
//! (e.g. `fetch_router`). `server.rs` composes them in `new()`.

pub mod actions;
pub mod download;
pub mod fetch;
pub mod introspect;
pub mod screenshot;
pub mod session;
pub mod wait;
