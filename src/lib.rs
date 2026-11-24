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
pub mod secret;
pub mod server;
pub mod session;
pub mod state;
pub mod tl;
pub mod tools;
pub mod transfer;

pub(crate) fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(DIGITS[(byte >> 4) as usize] as char);
        hex.push(DIGITS[(byte & 15) as usize] as char);
    }
    hex
}
