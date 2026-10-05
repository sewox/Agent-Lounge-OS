//! Dış dünya köprüleri — MCP sunucusu (Cursor / Claude Desktop / Antigravity).

pub mod mcp_http;
pub mod mcp_server;
pub mod nats_terminal;
pub mod orchestration;
pub mod session_id;
pub mod timeout_manager;
pub mod wait_clock;

pub use mcp_server::{run_stdio, ClientCtx, McpServer};
pub use nats_terminal::NatsTerminalHub;
pub use orchestration::{CallAgentArgs, CancelKind, Orchestrator, ProgressSink, WaitOpts};
pub use session_id::{is_valid_session_id, normalize_or_mint_session_id};
pub use timeout_manager::{ClientProfile, ProfileSource, TimeoutManager};
pub use wait_clock::{ManualWaitClock, SystemWaitClock, WaitClock};
