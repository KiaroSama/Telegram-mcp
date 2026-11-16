use super::*;
use crate::{
    backend::Telegram,
    documents::{IMAGE_WORKERS, check_message_topic, image_work},
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

const MEMORY_PROBE_OUTPUT_LIMIT: usize = 4096;
const MEMORY_PROBE_READY: &[u8] = b"supervisor-ready-before-input";

struct MemoryProbeOutput {
    total: u64,
    prefix: Vec<u8>,
    tail: Vec<u8>,
}

impl MemoryProbeOutput {
    fn new() -> Self {
        Self {
            total: 0,
            prefix: Vec::with_capacity(MEMORY_PROBE_OUTPUT_LIMIT / 2),
            tail: Vec::with_capacity(MEMORY_PROBE_OUTPUT_LIMIT / 2),
        }
    }

    fn retain(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let part_limit = MEMORY_PROBE_OUTPUT_LIMIT / 2;
        let prefix = bytes.len().min(part_limit - self.prefix.len());
        self.prefix.extend_from_slice(&bytes[..prefix]);
        let bytes = &bytes[prefix..];
        if bytes.len() >= part_limit {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - part_limit..]);
        } else {
            let discard = (self.tail.len() + bytes.len()).saturating_sub(part_limit);
            self.tail.drain(..discard);
            self.tail.extend_from_slice(bytes);
        }
    }
}

impl std::fmt::Display for MemoryProbeOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} bytes drained; prefix: {:?}; tail: {:?}",
            self.total,
            String::from_utf8_lossy(&self.prefix),
            String::from_utf8_lossy(&self.tail)
        )
    }
}

struct DrainedMemoryChild {
    child: std::process::Child,
    readers: Vec<std::thread::JoinHandle<io::Result<MemoryProbeOutput>>>,
}

impl DrainedMemoryChild {
    fn join_output(&mut self) -> Vec<io::Result<MemoryProbeOutput>> {
        self.readers
            .drain(..)
            .map(|reader| {
                reader.join().unwrap_or_else(|_| {
                    Err(io::Error::other("Memory probe output reader panicked"))
                })
            })
            .collect()
    }
}

impl Drop for DrainedMemoryChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.join_output();
    }
}

fn drain_memory_probe_output(
    mut pipe: impl io::Read + Send + 'static,
    mut ready: Option<std::sync::mpsc::SyncSender<()>>,
) -> std::thread::JoinHandle<io::Result<MemoryProbeOutput>> {
    std::thread::spawn(move || {
        let mut retained = MemoryProbeOutput::new();
        let mut marker = Vec::with_capacity(MEMORY_PROBE_READY.len() + 1024);
        let mut chunk = [0; 1024];
        loop {
            let count = pipe.read(&mut chunk)?;
            if count == 0 {
                return Ok(retained);
            }
            retained.retain(&chunk[..count]);
            if ready.is_some() {
                marker.extend_from_slice(&chunk[..count]);
                if marker
                    .windows(MEMORY_PROBE_READY.len())
                    .any(|part| part == MEMORY_PROBE_READY)
                    && let Some(ready) = ready.take()
                {
                    let _ = ready.send(());
                }
                let discard = marker.len().saturating_sub(MEMORY_PROBE_READY.len() - 1);
                marker.drain(..discard);
            }
            // Retention is bounded; keep consuming after the limit so neither
            // stdout nor a panic hook's stderr can block the owned supervisor.
        }
    })
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
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
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
        let stress_output = mode == "panic";
        let count = AtomicUsize::new(0);
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = Arc::clone(&ready);
        let _guard = MemoryGuard::start(512, Duration::from_millis(5), move || {
            let index = count.fetch_add(1, Ordering::SeqCst);
            if index < 2 || !gate.load(Ordering::SeqCst) {
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
        if stress_output {
            // Exercise both drains before arming the sampler. Deliberately slow
            // diagnostic IO belongs to startup, not the guard's reaction clock.
            // The real sampler panic below retains Rust's default panic hook.
            let chunk = [b'p'; 4096];
            for mut output in [
                Box::new(std::io::stdout()) as Box<dyn std::io::Write>,
                Box::new(std::io::stderr()) as Box<dyn std::io::Write>,
            ] {
                for _ in 0..32 {
                    std::io::Write::write_all(&mut output, &chunk).unwrap();
                }
                std::io::Write::flush(&mut output).unwrap();
            }
        }
        // Model a caller descheduled after guard startup. Polling must not
        // activate the synthetic failure before its readiness marker exists.
        std::thread::sleep(Duration::from_millis(30));
        println!("supervisor-ready-before-input");
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        ready.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(10));
        panic!("Supervisor failed to terminate its own worker");
    }
    let modes = if cfg!(target_os = "macos") {
        &["excess", "error", "panic", "native"][..]
    } else {
        &["excess", "error", "panic"][..]
    };
    for mode in modes {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "documents::worker::memory::tests::live_memory_supervisor_terminates_only_its_own_child_on_excess_or_accounting_failure", "--nocapture"])
            .env(CHILD, mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let mut probe = DrainedMemoryChild {
            child,
            readers: Vec::new(),
        };
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        probe.readers.push(drain_memory_probe_output(
            probe.child.stdout.take().unwrap(),
            Some(started),
        ));
        probe.readers.push(drain_memory_probe_output(
            probe.child.stderr.take().unwrap(),
            None,
        ));
        let outcome = match ready.recv_timeout(Duration::from_secs(10)) {
            Err(error) => Err(format!(
                "Owned {mode} child did not report readiness: {error}"
            )),
            Ok(()) => {
                // Process startup and readiness observation have their own bound;
                // the original enforcement deadline begins only after this proof.
                let deadline = std::time::Instant::now() + Duration::from_secs(4);
                loop {
                    match probe.child.try_wait() {
                        Ok(Some(status)) => break Ok(status),
                        Err(error) => {
                            break Err(format!("Cannot inspect owned {mode} child: {error}"));
                        }
                        Ok(None) if std::time::Instant::now() > deadline => {
                            break Err(format!(
                                "Memory supervisor did not stop its owned {mode} child"
                            ));
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        };
        if outcome.is_err() {
            let _ = probe.child.kill();
            probe.child.wait().unwrap();
        }
        let output = probe.join_output();
        let stdout = output[0].as_ref().unwrap();
        let stderr = output[1].as_ref().unwrap();
        assert!(stdout.prefix.len() + stdout.tail.len() <= MEMORY_PROBE_OUTPUT_LIMIT);
        assert!(stderr.prefix.len() + stderr.tail.len() <= MEMORY_PROBE_OUTPUT_LIMIT);
        let status = outcome.unwrap_or_else(|error| {
            panic!("{error}; bounded stdout: {stdout}; bounded stderr: {stderr}")
        });
        assert_eq!(status.code(), Some(MEMORY_EXIT), "{mode}: {stderr}");
        if *mode == "panic" {
            assert_eq!(
                stdout.prefix.len() + stdout.tail.len(),
                MEMORY_PROBE_OUTPUT_LIMIT
            );
            assert_eq!(
                stderr.prefix.len() + stderr.tail.len(),
                MEMORY_PROBE_OUTPUT_LIMIT
            );
            assert!(stdout.total >= 128 * 1024);
            assert!(stderr.total >= 128 * 1024);
        }
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
    use std::{future::Future, task::Poll};
    let watchdog = Duration::from_secs(10);
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (finished, mut completed) = tokio::sync::mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut callers = Vec::new();
    for id in 0..2 {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let started = started.clone();
        let finished = finished.clone();
        callers.push(tokio::spawn(image_work(move || {
            started.send(id).unwrap();
            let _ = wait.recv();
            let _ = finished.send(id);
            Ok(())
        })));
    }
    let mut initial = Vec::new();
    for _ in 0..2 {
        initial.push(
            tokio::time::timeout(watchdog, observed.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    initial.sort_unstable();
    assert_eq!(initial, [0, 1]);
    for caller in callers {
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
    }
    let semaphore = IMAGE_WORKERS.get().unwrap();
    assert_eq!(
        semaphore.available_permits(),
        0,
        "Cancelled callers released capacity while their CPU closures were still blocked"
    );
    let mut third = Box::pin(image_work(move || {
        started.send(2).unwrap();
        let _ = finished.send(2);
        Ok(())
    }));
    // Polling proves admission is blocked without a short timer that may be
    // descheduled beyond the real production admission deadline under CI load.
    std::future::poll_fn(|cx| {
        assert!(third.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        observed.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    releases[0].send(()).unwrap();
    // Await the real outcome, so an admission error or panic cannot be masked
    // by an open sender and misreported as a missing readiness notification.
    tokio::time::timeout(watchdog, third)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.try_recv().unwrap(), 2);
    let mut exits = Vec::new();
    for _ in 0..2 {
        exits.push(
            tokio::time::timeout(watchdog, completed.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    exits.sort_unstable();
    assert_eq!(exits, [0, 2]);
    releases[1].send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(watchdog, completed.recv())
            .await
            .unwrap(),
        Some(1)
    );
    let all_capacity = tokio::time::timeout(watchdog, Arc::clone(semaphore).acquire_many_owned(2))
        .await
        .unwrap()
        .unwrap();
    drop(all_capacity);
    assert_eq!(semaphore.available_permits(), 2);
}
