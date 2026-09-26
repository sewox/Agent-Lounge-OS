//! Dış dünya köprüleri — MCP sunucusu (Cursor / Claude Desktop).

pub mod mcp_http;
pub mod mcp_server;

pub use mcp_server::{run_stdio, McpServer};
