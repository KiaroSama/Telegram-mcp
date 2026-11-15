mod local_support;
use local_support::fixture;
use std::sync::Arc;
use telegram_mcp::{safeguard::approve_from_terminal, state::State};

#[test]
fn piped_input_cannot_authorize_a_pending_operation() {
    if std::env::var_os("TERMINAL_PIPE_CHILD").is_some() {
        let state = Arc::new(State::from_environment().unwrap());
        assert!(
            approve_from_terminal(state, &"a".repeat(64))
                .unwrap_err()
                .to_string()
                .contains("interactive owner terminal")
        );
        return;
    }
    let (directory, state) = fixture();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "piped_input_cannot_authorize_a_pending_operation",
            "--nocapture",
        ])
        .env("TERMINAL_PIPE_CHILD", "1")
        .env("TELEGRAM_STATE_DIR", state.root())
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(state.file_roots()).unwrap(),
        )
        .current_dir(directory.path().join("installation"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!state.root().join("approval-grants.json").exists());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use serde_json::json;
    use std::{
        fs::File,
        io::{Read, Write},
        os::fd::{AsRawFd, FromRawFd},
        path::Path,
        process::Stdio,
        time::{Duration, Instant},
    };
    use telegram_mcp::safeguard::{Decision, Safeguard};
    const NAME: &str = "unix::genuine_terminal_approval_is_single_use_or_scoped_durable";

    fn drain(master: &mut File, transcript: &mut Vec<u8>) {
        let mut buffer = [0; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    transcript.extend_from_slice(&buffer[..count]);
                    assert!(
                        transcript.len() <= 65536,
                        "Synthetic terminal exceeded its transcript budget"
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("Synthetic terminal read failed: {error}"),
            }
        }
    }

    fn review(
        state: &State,
        installation: &Path,
        fingerprint: &str,
        reply: &str,
        accepted: bool,
    ) -> String {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: output fd pointers are valid and optional termios/winsize are
        // null, so the OS creates a fresh isolated PTY with its own defaults.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: openpty returned two new owned descriptors, each wrapped once.
        let mut master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        // SAFETY: master is an owned live descriptor. Nonblocking reads let us
        // capture the review before sending input while retaining a deadline.
        unsafe {
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            assert!(flags >= 0);
            assert_eq!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
                0
            );
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", NAME, "--nocapture"])
            .env("TERMINAL_PTY_CHILD", "1")
            .env(
                "TERMINAL_EXPECT_ACCEPTED",
                if accepted { "yes" } else { "no" },
            )
            .env("TERMINAL_FINGERPRINT", fingerprint)
            .env("TELEGRAM_STATE_DIR", state.root())
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(state.file_roots()).unwrap(),
            )
            .current_dir(installation)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        let mut child = command.spawn().unwrap();
        drop(command);
        let mut transcript = Vec::new();
        let mut replied = false;
        let prompt = b"Type APPROVE for this request once";
        let started = Instant::now();
        let status = loop {
            drain(&mut master, &mut transcript);
            if !replied
                && transcript
                    .windows(prompt.len())
                    .any(|window| window == prompt)
            {
                master.write_all(format!("{reply}\n").as_bytes()).unwrap();
                replied = true;
            }
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > Duration::from_secs(15) {
                child.kill().unwrap();
                panic!("Synthetic owner terminal did not return within its deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        // Keep one slave open until the final drain: Darwin may discard unread
        // terminal output when its last slave closes. This descriptor belongs
        // only to this test and never grants terminal approval to the MCP side.
        drain(&mut master, &mut transcript);
        drop(slave);
        let transcript = String::from_utf8_lossy(&transcript).into_owned();
        assert!(status.success(), "{transcript}");
        transcript
    }

    fn fingerprint(decision: Decision) -> String {
        match decision {
            Decision::NeedsApproval { fingerprint, .. } => fingerprint,
            other => panic!("Expected a pending owner request, got {other:?}"),
        }
    }

    #[test]
    fn genuine_terminal_approval_is_single_use_or_scoped_durable() {
        if std::env::var_os("TERMINAL_PTY_CHILD").is_some() {
            let state = Arc::new(State::from_environment().unwrap());
            let fingerprint = std::env::var("TERMINAL_FINGERPRINT").unwrap();
            let result = approve_from_terminal(state, &fingerprint);
            assert_eq!(
                result.is_ok(),
                std::env::var("TERMINAL_EXPECT_ACCEPTED").unwrap() == "yes",
                "{result:?}"
            );
            return;
        }
        let (directory, state) = fixture();
        let installation = directory.path().join("installation");
        let guard = Safeguard::new(state.clone());
        let args = json!({"account":"work","chat_id":42,"message_id":7});
        let pending = fingerprint(guard.check("delete_message", &args, false, true).unwrap());
        let transcript = review(&state, &installation, &pending, "APPROVE", true);
        assert!(transcript.contains("Review this exact operation"));
        assert!(transcript.contains("delete_message"));
        assert!(transcript.contains("Approved once"));
        assert_eq!(
            guard.check("delete_message", &args, false, true).unwrap(),
            Decision::Allow
        );
        let pending = fingerprint(guard.check("delete_message", &args, false, true).unwrap());
        assert!(
            review(&state, &installation, &pending, "ALWAYS", true)
                .contains("Always approval saved")
        );
        review(&state, &installation, &pending, "ALWAYS", true);
        assert_eq!(
            state.always_approvals()["grants"].as_array().unwrap().len(),
            1
        );
        assert!(state.approval_is_granted("work", "delete_message", &json!(42)));
        assert!(!state.approval_is_granted("personal", "delete_message", &json!(42)));
        assert!(!state.approval_is_granted("work", "delete_message", &json!(99)));
        assert!(
            state
                .revoke_approval("work", "delete_message", &json!(42))
                .unwrap()
        );
        let pending = fingerprint(guard.check("delete_message", &args, false, true).unwrap());
        review(&state, &installation, &pending, "DENY", false);
        assert!(matches!(
            guard.check("delete_message", &args, false, true).unwrap(),
            Decision::NeedsApproval { .. }
        ));
        state
            .update_json("approval-pending.json", json!({}), |requests| {
                requests[&pending]["expires"] = json!(0);
                Ok(())
            })
            .unwrap();
        review(&state, &installation, &pending, "APPROVE", false);
        review(
            &state,
            &installation,
            "invalid-fingerprint",
            "APPROVE",
            false,
        );
        review(&state, &installation, &"b".repeat(64), "APPROVE", false);
        let pending = fingerprint(guard.check("delete_message", &args, false, true).unwrap());
        state
            .update_json("approval-pending.json", json!({}), |requests| {
                requests[&pending]["arguments"]["chat_id"] = json!(99);
                Ok(())
            })
            .unwrap();
        review(&state, &installation, &pending, "APPROVE", false);
        let global = fingerprint(
            guard
                .check(
                    "update_profile",
                    &json!({"account":"work","first_name":"Synthetic"}),
                    false,
                    true,
                )
                .unwrap(),
        );
        review(&state, &installation, &global, "ALWAYS", false);
        assert!(
            state.always_approvals()["grants"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
