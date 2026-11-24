use super::common::*;

#[tokio::test]
async fn clearing_business_greeting_omits_message() {
    let m = Mock::new();
    run(&m, "set_business_greeting", json!({"shortcut_id":null}))
        .await
        .unwrap();
    assert_eq!(
        m.request("account.updateBusinessGreetingMessage"),
        json!({})
    );
}

#[tokio::test]
async fn business_hours_send_normalized_intervals() {
    let m = Mock::new();
    run(
        &m,
        "set_business_hours",
        json!({"timezone_id":"Asia/Tehran","intervals":[[60,120],[0,60]]}),
    )
    .await
    .unwrap();
    let r = m.request("account.updateBusinessWorkHours");
    assert_eq!(
        r["business_work_hours"]["weekly_open"],
        json!([{"_":"businessWeeklyOpen","start_minute":0,"end_minute":120}])
    );
}

#[tokio::test]
async fn business_coordinate_pair_is_required() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_business_location",
            json!({"address":"Office","latitude":35.0})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn business_coordinates_are_bounded() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_business_location",
            json!({"address":"Office","latitude":95.0,"longitude":50.0})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn business_custom_schedule_rejects_reversed_range() {
    let m = Mock::new();
    assert!(run(&m,"set_business_away_message",json!({"shortcut_id":1,"schedule":"custom","start_date":"2026-01-02T00:00:00Z","end_date":"2026-01-01T00:00:00Z"})).await.is_err());
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn business_greeting_requires_published_inactivity_days() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "set_business_greeting",
            json!({"shortcut_id":1,"no_activity_days":3})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn business_link_removal_extracts_slug() {
    let m = Mock::new();
    run(
        &m,
        "delete_business_chat_link",
        json!({"slug":"https://t.me/m/example"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("account.deleteBusinessChatLink")["slug"],
        "example"
    );
}
