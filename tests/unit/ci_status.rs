use super::*;
#[test]
fn concurrent_push_and_pull_request_statuses_cannot_overwrite_each_other() {
    let push = status_context(
        "tests.yml",
        "push",
        "Native release and tests (linux-aarch64)",
    )
    .unwrap();
    let pull = status_context(
        "tests.yml",
        "pull_request",
        "Native release and tests (linux-aarch64)",
    )
    .unwrap();
    assert_ne!(
        push, pull,
        "A late pending status from another trigger can overwrite final evidence"
    );
    assert_eq!(
        push,
        "CI/tests/push/Native release and tests (linux-aarch64)"
    );
    assert!(status_context("tests.yml", "pull_request/forged", "job").is_err());
}
#[test]
fn status_rejects_ancestor_commit_and_stale_attempt() {
    let sha = "ab".repeat(20);
    assert!(run_matches(
        &json!({"head_sha":sha,"run_attempt":2}),
        &sha,
        2
    ));
    assert!(!run_matches(
        &json!({"head_sha":"cd".repeat(20),"run_attempt":2}),
        &sha,
        2
    ));
    assert!(!run_matches(
        &json!({"head_sha":sha,"run_attempt":1}),
        &sha,
        2
    ));
}
#[test]
fn status_rejects_job_from_another_commit_attempt_or_run() {
    let sha = "ab".repeat(20);
    let url = "https://github.com/owner/repo/actions/runs/100";
    let valid =
        json!({"head_sha":sha,"run_attempt":2,"run_id":100,"html_url":format!("{url}/job/123")});
    assert!(job_matches(&valid, 100, 2, &sha, url));
    // The actual GitHub jobs API uses the singular /job/ URL segment.
    let mut unsupported = valid.clone();
    unsupported["html_url"] = json!(format!("{url}/jobs/123"));
    assert!(!job_matches(&unsupported, 100, 2, &sha, url));
    for field in ["head_sha", "run_attempt", "run_id", "html_url"] {
        let mut invalid = valid.clone();
        invalid[field] = match field {
            "head_sha" => json!("cd".repeat(20)),
            "html_url" => json!("https://example.org/job/123"),
            _ => json!(1),
        };
        assert!(!job_matches(&invalid, 100, 2, &sha, url));
    }
}
#[test]
fn status_cannot_report_skipped_cancelled_or_running_as_success() {
    for verdict in ["skipped", "cancelled", "failure", "timed_out", ""] {
        assert_ne!(
            conclusion(&json!({"status":"completed","conclusion":verdict})).0,
            "success"
        );
    }
    assert_ne!(
        conclusion(&json!({"status":"in_progress","conclusion":"success"})).0,
        "success"
    );
    assert_ne!(conclusion(&json!({})).0, "success");
    assert_eq!(
        conclusion(&json!({"status":"completed","conclusion":"success"})).0,
        "success"
    );
}
