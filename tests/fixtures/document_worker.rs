//! Standalone Rust protocol peer, compiled only by document transport tests.
use std::io::{Read, Write};

fn main() {
    let executable = std::env::current_exe().unwrap();
    let mode = executable.file_stem().unwrap().to_str().unwrap();
    if mode.ends_with("early-exit") {
        return;
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(36 * 1024 * 1024 + 1)
        .read_to_end(&mut input)
        .unwrap();
    if mode.ends_with("failure") {
        std::process::exit(7);
    }
    if mode.ends_with("malformed") {
        print!("worker-private-canary:invalid-json");
        return;
    }
    if mode.ends_with("oversized") {
        let _ = std::io::stdout().write_all(&vec![b'x'; 2 * 1024 * 1024 * 6 + 4097]);
        return;
    }
    if mode.ends_with("invalid-text") {
        print!("{{\"ok\":true,\"text\":7,\"truncated\":false,\"reason\":null}}");
        return;
    }
    if mode.ends_with("invalid-truncation") {
        print!("{{\"ok\":true,\"text\":\"test\",\"truncated\":\"false\",\"reason\":null}}");
        return;
    }
    if mode.ends_with("invalid-reason") {
        print!("{{\"ok\":true,\"text\":\"test\",\"truncated\":false,\"reason\":\"unknown\"}}");
        return;
    }
    if mode.ends_with("oversized-text") {
        print!(
            "{{\"ok\":true,\"text\":\"{}\",\"truncated\":false,\"reason\":null}}",
            "x".repeat(2 * 1024 * 1024 + 1)
        );
        return;
    }
    let mut keys = std::env::vars_os()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    keys.sort();
    print!(
        "{{\"ok\":true,\"text\":{:?},\"truncated\":false,\"reason\":null}}",
        keys.join(",")
    );
}
