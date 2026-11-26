//! Schema-validated native keyboard, rich-send and cursor boundary regressions.
#[path = "common/media.rs"]
mod media_support;
mod rust_messaging_branches {
    mod general_and_layer234;
    mod keyboard;
    mod readers;
    mod search_safety;
    mod send;
    mod support;
}
