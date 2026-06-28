//! `VoidCrawl` MCP server library.
//!
//! Exposes `void_crawl_core`'s stealth browser pool and session API to
//! Claude Code via the Model Context Protocol. The crate also ships a
//! binary (`voidcrawl-mcp`) that wires the library up over stdio.

pub mod errors;
pub mod install;
pub mod server;
pub mod sessions;
pub mod state;
pub mod tools;

pub const VERSION: &str = "0.3.8.1";

pub use server::VoidCrawlServer;
pub use state::AppState;
