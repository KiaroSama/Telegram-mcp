use super::common::*;

#[tokio::test]
async fn story_listing_never_marks_seen() {
    let m = Mock::new().respond(
        "stories.getPeerStories",
        json!({"stories":{"max_read_id":1,"stories":[{"_":"storyItem","id":2}]}}),
    );
    let r = run(&m, "list_peer_stories", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(r["marked_read"], false);
    assert_eq!(m.count("stories.readStories"), 0);
    assert_eq!(m.count("stories.incrementStoryViews"), 0);
}

#[tokio::test]
async fn conflicting_story_reactions_are_refused() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "react_to_story",
            json!({"chat_id":7,"story_id":1,"emoji":"❤","custom_emoji_id":"9"})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("stories.sendReaction"), 0);
}

#[tokio::test]
async fn empty_story_reaction_is_removal() {
    let m = Mock::new();
    run(&m, "react_to_story", json!({"chat_id":7,"story_id":1}))
        .await
        .unwrap();
    assert_eq!(
        m.request("stories.sendReaction")["reaction"]["_"],
        "reactionEmpty"
    );
}

#[tokio::test]
async fn story_privacy_is_required_before_upload() {
    let m = Mock::new();
    assert!(
        run(&m, "post_story", json!({"file_path":"/tmp/photo.jpg"}))
            .await
            .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn posted_story_uses_explicit_privacy_and_period() {
    let m = Mock::new();
    let state = telegram_mcp::state::State::global().unwrap();
    let temp = tempfile::tempdir_in(&state.file_roots()[0]).unwrap();
    let photo = temp.path().join("photo.jpg");
    std::fs::write(&photo, b"synthetic uploaded photo bytes").unwrap();
    run(
        &m,
        "post_story",
        json!({"file_path":photo,"privacy":"contacts","hours":12,"pin_to_profile":true}),
    )
    .await
    .unwrap();
    let p = m.request("stories.sendStory");
    assert_eq!(
        p["privacy_rules"],
        json!([{"_":"inputPrivacyValueAllowContacts"}])
    );
    assert_eq!(p["period"], 43200);
    assert_eq!(p["pinned"], true);
    assert_eq!(p["media"]["_"], "inputMediaUploadedPhoto");
}
#[tokio::test]
async fn missing_stories_are_reported_without_guessing_why() {
    let m = Mock::new().respond(
        "stories.getStoriesByID",
        json!({"stories":[{"_":"storyItem","id":2}]}),
    );
    let r = run(&m, "get_stories", json!({"chat_id":7,"story_ids":[2,3]}))
        .await
        .unwrap();
    assert_eq!(r["not_returned"], json!([3]));
    assert!(
        r["not_returned_reason"]
            .as_str()
            .unwrap()
            .contains("does not distinguish")
    );
    assert_eq!(m.count("stories.readStories"), 0);
}
