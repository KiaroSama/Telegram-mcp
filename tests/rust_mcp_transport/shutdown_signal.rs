use super::shutdown_signal;
use std::{process::Stdio, time::Duration};

#[tokio::test]
async fn interrupt_is_retained_before_the_shutdown_waiter_is_polled() {
    const CHILD: &str = "TELEGRAM_HTTP_SIGNAL_REGISTRATION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Signal disposition is process-global. A fresh child keeps this check
        // independent of other tests that may already have installed a handler.
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "server::http::shutdown_signal_tests::interrupt_is_retained_before_the_shutdown_waiter_is_polled",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                child.kill().await.unwrap();
                panic!("The owned signal-registration child did not finish");
            }
        };
        assert!(
            status.success(),
            "The shutdown listener must retain an interrupt before its future is polled: {status}"
        );
        return;
    }

    let interrupt = shutdown_signal().unwrap();
    // SAFETY: this isolated test process sends SIGINT only to itself. The
    // production constructor must already own the signal before any await.
    assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
    tokio::time::timeout(Duration::from_secs(20), interrupt)
        .await
        .expect("The registered shutdown listener lost the early interrupt");
}
