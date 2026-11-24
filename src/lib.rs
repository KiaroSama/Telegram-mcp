//! Telegram MCP's native implementation. Telegram transport is mockable so tests
//! exercise local behavior without credentials or network side effects.
pub mod account_manager;
pub mod args;
pub mod backend;
pub mod documents;
pub mod emoji;
pub mod export;
pub mod logging;
pub mod proxy;
pub mod safeguard;
pub mod server;
pub mod session;
pub mod state;
pub mod tl;
pub mod tools;
pub mod transfer;
