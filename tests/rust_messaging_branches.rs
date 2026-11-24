//! Schema-validated native keyboard, rich-send and cursor boundary regressions.
#[path = "common/media.rs"]
mod media_support;
mod rust_messaging_branches {
    mod keyboard;
    mod readers;
    mod send;
    mod support;
}
