//! `VoidCrawl` MCP server library.
//!
//! Exposes `void_crawl_core`'s stealth browser pool and session API to
//! Claude Code via the Model Context Protocol. The crate also ships a
//! binary (`voidcrawl-mcp`) that wires the library up over stdio.

pub mod errors;
pub mod server;
pub mod sessions;
pub mod state;
pub mod tools;

pub use server::VoidCrawlServer;
pub use state::AppState;
