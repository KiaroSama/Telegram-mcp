use super::*;
use crate::{
    backend::Telegram,
    documents::{check_message_topic, image_work},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[test]
fn memory_supervisor_refuses_unavailable_accounting_before_start() {
    assert!(
        MemoryGuard::start(512, Duration::from_secs(1), || {
            Err(io::Error::other("synthetic accounting failure"))
        })
        .is_err()
    );
}

#[test]
fn memory_supervisor_refuses_an_already_exceeded_budget() {
    assert!(MemoryGuard::start(512, Duration::from_secs(1), || Ok(513)).is_err());
}

#[test]
fn memory_supervisor_keeps_the_exact_boundary_and_stops_without_waiting_for_the_poll() {
    let samples = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&samples);
    let guard = MemoryGuard::start(512, Duration::from_secs(60), move || {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(512)
    })
    .unwrap();
    assert_eq!(
        samples.load(Ordering::SeqCst),
        2,
        "Supervisor was not ready"
    );
    let before = std::time::Instant::now();
    guard.finish().unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(samples.load(Ordering::SeqCst), 3);
}

#[test]
fn final_high_watermark_probe_refuses_a_peak_between_scheduled_samples() {
    let peak = Arc::new(AtomicU64::new(100));
    let observed = Arc::clone(&peak);
    let guard = MemoryGuard::start(512, Duration::from_secs(60), move || {
        Ok(observed.load(Ordering::SeqCst))
    })
    .unwrap();
    peak.store(513, Ordering::SeqCst);
    assert!(
        guard
            .finish()
            .unwrap_err()
            .to_string()
            .contains("physical-memory budget")
    );
}

#[test]
fn final_high_watermark_probe_refuses_lost_accounting() {
    let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let observed = Arc::clone(&available);
    let guard = MemoryGuard::start(512, Duration::from_secs(60), move || {
        if observed.load(Ordering::SeqCst) {
            Ok(100)
        } else {
            Err(io::Error::other("accounting unavailable"))
        }
    })
    .unwrap();
    available.store(false, Ordering::SeqCst);
    assert!(guard.finish().is_err());
}

#[test]
fn dropping_the_supervisor_joins_its_thread_before_releasing_captured_state() {
    let captured = Arc::new(());
    let observed = Arc::clone(&captured);
    let guard = MemoryGuard::start(512, Duration::from_secs(60), move || {
        std::hint::black_box(&observed);
        Ok(100)
    })
    .unwrap();
    assert_eq!(Arc::strong_count(&captured), 2);
    drop(guard);
    assert_eq!(Arc::strong_count(&captured), 1);
}

#[test]
fn live_memory_supervisor_terminates_only_its_own_child_on_excess_or_accounting_failure() {
    const CHILD: &str = "TELEGRAM_TEST_PDF_MEMORY_SUPERVISOR";
    if let Some(mode) = std::env::var_os(CHILD) {
        let mode = mode.to_str().unwrap().to_owned();
        #[cfg(target_os = "macos")]
        if mode == "native" {
            let baseline = physical_footprint().unwrap();
            let _guard = MemoryGuard::start(
                baseline + 8 * 1024 * 1024,
                Duration::from_millis(5),
                physical_footprint,
            )
            .unwrap();
            println!("supervisor-ready-before-input");
            // Touch actual private pages: zero-page reservation alone would
            // exercise virtual memory rather than Darwin's physical ledger.
            let mut allocation = vec![0u8; 32 * 1024 * 1024];
            for byte in allocation.iter_mut().step_by(4096) {
                // SAFETY: each reference points to a live byte in this buffer.
                unsafe { std::ptr::write_volatile(byte, 0x43) };
            }
            std::hint::black_box(&allocation);
            std::thread::sleep(Duration::from_secs(10));
            panic!("Native Darwin accounting did not enforce its physical-memory budget");
        }
        let count = AtomicUsize::new(0);
        let _guard = MemoryGuard::start(512, Duration::from_millis(5), move || {
            let index = count.fetch_add(1, Ordering::SeqCst);
            if index < 2 {
                return Ok(100);
            }
            match mode.as_str() {
                "excess" => Ok(513),
                "error" => Err(io::Error::other("synthetic accounting loss")),
                "panic" => panic!("synthetic accounting panic"),
                _ => unreachable!(),
            }
        })
        .unwrap();
        println!("supervisor-ready-before-input");
        std::thread::sleep(Duration::from_secs(10));
        panic!("Supervisor failed to terminate its own worker");
    }
    let modes = if cfg!(target_os = "macos") {
        &["excess", "error", "panic", "native"][..]
    } else {
        &["excess", "error", "panic"][..]
    };
    for mode in modes {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "documents::worker::memory::tests::live_memory_supervisor_terminates_only_its_own_child_on_excess_or_accounting_failure", "--nocapture"])
            .env(CHILD, mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("Memory supervisor did not stop its owned {mode} child");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            status.code(),
            Some(MEMORY_EXIT),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("supervisor-ready-before-input"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_darwin_accounting_reports_this_process_without_privilege() {
    assert!(physical_footprint().unwrap() > 0);
}

#[tokio::test]
async fn topic_starters_require_the_exact_service_action_and_matching_channel() {
    struct NoRequests;
    #[async_trait::async_trait]
    impl Telegram for NoRequests {
        async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
            panic!("Topic creation service metadata requires no lookup");
        }
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            panic!("The peer is already bound");
        }
    }
    let channel = json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"70"});
    let starter = json!({"_":"messageService","id":8,"peer_id":{"_":"peerChannel","channel_id":"7"},"action":{"_":"messageActionTopicCreate","title":"Synthetic topic","icon_color":0}});
    let mut ordinary = starter.clone();
    ordinary["_"] = json!("message");
    let mut user_constructor = starter.clone();
    user_constructor["_"] = json!("user");
    let mut wrong_peer = starter.clone();
    wrong_peer["peer_id"] = json!({"_":"peerUser","user_id":"7"});
    let mut wrong_id = starter.clone();
    wrong_id["id"] = json!(9);
    let mut wrong_action = starter.clone();
    wrong_action["action"]["_"] = json!("messageActionTopicEdit");
    for (message, allowed) in [
        (starter, true),
        (ordinary, false),
        (user_constructor, false),
        (wrong_peer, false),
        (wrong_id, false),
        (wrong_action, false),
    ] {
        assert_eq!(
            check_message_topic(&NoRequests, &channel, &message, 8)
                .await
                .is_ok(),
            allowed,
            "{message}"
        );
    }
}

#[tokio::test]
async fn cancelled_image_requests_hold_the_worker_budget_until_their_cpu_work_finishes() {
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut callers = Vec::new();
    for id in 0..2 {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let started = started.clone();
        callers.push(tokio::spawn(image_work(move || {
            started.send(id).unwrap();
            let _ = wait.recv();
            Ok(())
        })));
    }
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(2), observed.recv())
            .await
            .unwrap()
            .unwrap();
    }
    for caller in callers {
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
    }
    let third = tokio::spawn(image_work(move || {
        started.send(2).unwrap();
        Ok(())
    }));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), observed.recv())
            .await
            .is_err()
    );
    releases[0].send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), observed.recv())
            .await
            .unwrap(),
        Some(2)
    );
    third.await.unwrap().unwrap();
    releases[1].send(()).unwrap();
}
