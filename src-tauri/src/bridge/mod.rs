//! Dış dünya köprüleri — MCP sunucusu (Cursor / Claude Desktop / Antigravity).

pub mod mcp_http;
pub mod mcp_server;
pub mod orchestration;
pub mod timeout_manager;
pub mod wait_clock;

pub use mcp_server::{run_stdio, ClientCtx, McpServer};
pub use orchestration::{CallAgentArgs, CancelKind, Orchestrator};
pub use timeout_manager::TimeoutManager;
pub use wait_clock::{ManualWaitClock, SystemWaitClock, WaitClock};
