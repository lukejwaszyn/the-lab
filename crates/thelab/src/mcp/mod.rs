//! The MCP server (`thelab mcp`).
//!
//! `protocol` holds everything about the wire: JSON-RPC framing, protocol
//! versions and the differences between the modern and legacy eras. `tools`
//! holds what the server can do and knows nothing about the wire.

mod protocol;
mod tools;

pub use protocol::serve;
pub use tools::Groups;
