//! File-roots status reporting exposing `free_folders`, `always_allowed_folders`,
//! and per-request client-advertised MCP roots (distinguishing unavailable vs empty).
use crate::state::State;
use serde_json::Value;
use std::path::PathBuf;

pub fn get_file_roots_status(state: &State, client_roots: Option<Vec<PathBuf>>) -> Value {
    crate::server::roots_status_payload(state, client_roots)
}
