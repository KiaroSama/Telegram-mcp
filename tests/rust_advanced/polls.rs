use super::common::*;

#[tokio::test]
async fn quiz_preserves_correct_answer_index() {
    let m = Mock::new();
    run(&m, "create_poll", quiz_args()).await.unwrap();
    let p = m.request("messages.sendMedia");
    assert_eq!(p["media"]["correct_answers"], json!([1]));
    assert_eq!(p["media"]["poll"]["revoting_disabled"], true);
    assert_eq!(p["media"]["poll"]["answers"][0]["_"], "inputPollAnswer");
    assert_eq!(p["media"]["poll"]["hash"], "0");
}

#[tokio::test]
async fn quiz_refuses_missing_answer() {
    let m = Mock::new();
    let mut a = quiz_args();
    a.as_object_mut().unwrap().remove("correct_option_index");
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn quiz_refuses_outside_answer() {
    let m = Mock::new();
    let mut a = quiz_args();
    a["correct_option_index"] = json!(9);
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn quiz_refuses_revoting() {
    let m = Mock::new();
    let mut a = quiz_args();
    a["allow_revoting"] = json!(true);
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn quiz_refuses_added_options() {
    let m = Mock::new();
    let mut a = quiz_args();
    a["allow_adding_options"] = json!(true);
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn poll_refuses_both_deadline_modes() {
    let m = Mock::new();
    let mut a = quiz_args();
    a["duration_seconds"] = json!(60);
    a["close_date"] = json!((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339());
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn poll_refuses_hundred_day_deadline() {
    let m = Mock::new();
    let mut a = quiz_args();
    a["close_date"] = json!((chrono::Utc::now() + chrono::Duration::days(100)).to_rfc3339());
    assert!(run(&m, "create_poll", a).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn live_app_config_controls_answer_ceiling() {
    let m = Mock::new().respond(
        "help.getAppConfig",
        json!({"config":{"value":[{"key":"poll_answers_max","value":{"value":2.0}}]}}),
    );
    let mut a = quiz_args();
    a["options"] = json!(["A", "B", "C"]);
    assert!(run(&m, "create_poll", a).await.is_err());
    assert_eq!(m.count("messages.sendMedia"), 0);
}

#[tokio::test]
async fn poll_vote_uses_opaque_option_blob() {
    let m = with_poll(poll());
    run(
        &m,
        "vote_in_poll",
        json!({"chat_id":7,"message_id":20,"option_indexes":1}),
    )
    .await
    .unwrap();
    assert_eq!(m.request("messages.sendVote")["options"], json!([[241, 2]]));
}

#[tokio::test]
async fn duplicate_vote_index_is_one_vote() {
    let m = with_poll(poll());
    run(
        &m,
        "vote_in_poll",
        json!({"chat_id":7,"message_id":20,"option_indexes":[1,1]}),
    )
    .await
    .unwrap();
    assert_eq!(m.request("messages.sendVote")["options"], json!([[241, 2]]));
}

#[tokio::test]
async fn empty_vote_list_retracts_vote() {
    let m = with_poll(poll());
    run(
        &m,
        "vote_in_poll",
        json!({"chat_id":7,"message_id":20,"option_indexes":[]}),
    )
    .await
    .unwrap();
    assert_eq!(m.request("messages.sendVote")["options"], json!([]));
}

#[tokio::test]
async fn closed_poll_refuses_vote() {
    let mut p = poll();
    p["closed"] = json!(true);
    let m = with_poll(p);
    assert!(
        run(
            &m,
            "vote_in_poll",
            json!({"chat_id":7,"message_id":20,"option_indexes":0})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.sendVote"), 0);
}

#[tokio::test]
async fn single_choice_refuses_multiple_different_votes() {
    let m = with_poll(poll());
    assert!(
        run(
            &m,
            "vote_in_poll",
            json!({"chat_id":7,"message_id":20,"option_indexes":[0,1]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.sendVote"), 0);
}

#[tokio::test]
async fn poll_tally_matches_option_blobs_instead_of_position() {
    let m = with_poll(poll());
    let r = run(&m, "get_poll_results", json!({"chat_id":7,"message_id":20}))
        .await
        .unwrap();
    assert_eq!(r["options"][0]["voters"], 1);
    assert_eq!(r["options"][1]["voters"], 3);
    assert_eq!(r["options"][1]["share_percent"], 75.0);
    assert_eq!(r["your_votes"], json!([1]));
}

#[tokio::test]
async fn anonymous_poll_does_not_fetch_voter_identities() {
    let mut p = poll();
    p["public_voters"] = json!(false);
    let m = with_poll(p);
    assert!(
        run(&m, "get_poll_voters", json!({"chat_id":7,"message_id":20}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.getPollVotes"), 0);
}

#[tokio::test]
async fn minimal_poll_results_are_refetched() {
    let mut msg = message(poll());
    msg["messages"][0]["media"]["results"]["min"] = json!(true);
    let m=Mock::new().respond("channels.getMessages",msg).respond("messages.getPollResults",json!({"updates":[{"results":{"results":[{"option":[241,2],"chosen":true,"voters":2}],"total_voters":2}}]}));
    let r = run(&m, "get_poll_results", json!({"chat_id":7,"message_id":20}))
        .await
        .unwrap();
    assert_eq!(m.request("messages.getPollResults")["poll_hash"], "0");
    assert_eq!(r["options"][1]["chosen_by_this_account"], true);
}

#[tokio::test]
async fn poll_voter_filter_sends_current_option_bytes() {
    let m = with_poll(poll());
    run(
        &m,
        "get_poll_voters",
        json!({"chat_id":7,"message_id":20,"option_index":1,"offset":"page2"}),
    )
    .await
    .unwrap();
    let r = m.request("messages.getPollVotes");
    assert_eq!(r["option"], json!([241, 2]));
    assert_eq!(r["offset"], "page2");
}
#[tokio::test]
async fn app_config_limit_is_read_once_per_backend_session() {
    let m = Mock::new().cached("advanced-poll-limit-cache").respond(
        "help.getAppConfig",
        json!({"config":{"value":[{"key":"poll_answers_max","value":{"value":12.0}}]}}),
    );
    run(&m, "create_poll", quiz_args()).await.unwrap();
    run(&m, "create_poll", quiz_args()).await.unwrap();
    assert_eq!(m.count("help.getAppConfig"), 1);
    assert_eq!(m.count("messages.sendMedia"), 2);
}
