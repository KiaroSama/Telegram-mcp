mod local_support;
use local_support::fixture;
use serde_json::Value;
use std::time::Duration;
use telegram_mcp::state::IncomingEvent;

fn event(account: &str, chat: &str, id: &str, bot: bool) -> IncomingEvent {
    IncomingEvent {
        account: account.into(),
        chat_id: chat.into(),
        message_id: id.into(),
        name: "نام".into(),
        username: None,
        bot,
    }
}
#[tokio::test]
async fn incoming_wait_preserves_other_account_events() {
    let (_dir, state) = fixture();
    state
        .record_incoming(event("work", "42", "1", false))
        .unwrap();
    let absent = state
        .wait_incoming(
            Some("personal"),
            Some("42"),
            Duration::from_millis(2),
            None,
            50,
        )
        .await
        .unwrap();
    assert_eq!(absent["event"], false);
    let found = state
        .wait_incoming(Some("work"), Some("42"), Duration::from_millis(2), None, 50)
        .await
        .unwrap();
    assert_eq!(found["event"], true);
    assert_eq!(found["pending_chats"][0]["account"], "work");
}
#[tokio::test]
async fn unnamed_wait_skips_bots_but_targeted_wait_returns_them() {
    let (_dir, state) = fixture();
    state
        .record_incoming(event("work", "42", "1", true))
        .unwrap();
    assert_eq!(
        state
            .wait_incoming(Some("work"), None, Duration::from_millis(2), None, 50)
            .await
            .unwrap()["event"],
        false
    );
    assert_eq!(
        state
            .wait_incoming(Some("work"), Some("42"), Duration::from_millis(2), None, 50)
            .await
            .unwrap()["event"],
        true
    );
}
#[tokio::test]
async fn settled_wait_consumes_exact_burst() {
    let (_dir, state) = fixture();
    state
        .record_incoming(event("work", "42", "1", false))
        .unwrap();
    state
        .record_incoming(event("work", "42", "2", false))
        .unwrap();
    let result = state
        .wait_incoming(
            Some("work"),
            Some("42"),
            Duration::from_millis(20),
            Some(Duration::from_millis(1)),
            50,
        )
        .await
        .unwrap();
    assert_eq!(result["message_count"], 2);
    assert_eq!(result["first_message_id"], "1");
    assert_eq!(result["last_message_id"], "2");
    assert_eq!(
        state
            .wait_incoming(Some("work"), Some("42"), Duration::from_millis(1), None, 50)
            .await
            .unwrap()["event"],
        false
    );
}
#[tokio::test]
async fn new_message_wait_does_not_consume_pending_burst() {
    let (_dir, state) = fixture();
    state
        .record_incoming(event("work", "42", "1", false))
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            state
                .wait_incoming(Some("work"), Some("42"), Duration::from_millis(1), None, 50)
                .await
                .unwrap()["event"],
            true
        );
    }
}
#[tokio::test]
async fn event_wait_bounds_are_checked() {
    let (_dir, state) = fixture();
    assert!(
        state
            .wait_incoming(None, None, Duration::ZERO, None, 50)
            .await
            .is_err()
    );
    assert!(
        state
            .wait_incoming(None, None, Duration::from_secs(51), None, 50)
            .await
            .is_err()
    );
    assert!(
        state
            .wait_incoming(None, None, Duration::from_secs(1), None, 0)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn feed_appends_private_metadata_without_message_text() {
    let (_dir, state) = fixture();
    state
        .record_incoming(event("work", "42", "1", false))
        .unwrap();
    state.enable_incoming_feed(1).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !state.root().join("incoming_feed.jsonl").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    state.disable_incoming_feed().unwrap();
    let text = std::fs::read_to_string(state.root().join("incoming_feed.jsonl")).unwrap();
    let row: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(row["account"], "work");
    assert!(row.get("message").is_none());
    assert!(row.get("text").is_none());
}
