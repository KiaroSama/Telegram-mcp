use super::*;

fn poll() -> Value {
    json!({"_":"poll","question":{"text":"Which?"},"answers":[
        {"option":[0,255],"text":{"text":"First"}},{"option":[7],"text":{"text":"Second"}},
        {"option":[128],"text":{"text":"Third"}}]})
}

#[test]
fn poll_option_lookup_preserves_opaque_bytes_and_rejects_missing_indices() {
    assert_eq!(option(&poll(), 0).unwrap(), json!([0, 255]));
    for index in [-1, 3, i64::MAX] {
        assert!(option(&poll(), index).is_err());
    }
    assert!(option(&json!({"answers":[{}]}), 0).is_err());
}

#[test]
fn poll_description_maps_votes_by_bytes_and_preserves_published_deadline_metadata() {
    let mut poll = poll();
    poll["multiple_choice"] = json!(true);
    poll["public_voters"] = json!(true);
    poll["close_date"] = json!(123456);
    poll["close_period"] = json!(300);
    poll["countries_iso2"] = json!(["IR"]);
    poll["answers"][0]["media"] = json!({"_":"messageMediaPhoto","photo":{"id":"42"}});
    let results = json!({"total_voters":3,"can_view_stats":true,"solution":"Explanation",
        "results":[{"option":[7],"voters":2,"chosen":true},{"option":[0,255],"voters":1,"chosen":true}]});
    let result = describe(&poll, &results);
    assert_eq!(result["your_votes"], json!([0, 1]));
    assert_eq!(result["options"][0]["share_percent"], 33.3);
    assert_eq!(result["options"][1]["share_percent"], 66.7);
    assert!(result["options"][2]["voters"].is_null());
    assert_eq!(result["options"][0]["attachment"]["photo"]["id"], "42");
    assert_eq!(result["solution"], "Explanation");
    assert_eq!(result["countries_iso2"], json!(["IR"]));
    assert_eq!(result["close_date"], 123456);
    assert_eq!(result["close_period"], 300);
    assert_eq!(result["can_view_stats"], true);
}

#[test]
fn unanswered_quiz_does_not_invent_correct_answers_or_zero_vote_percentages() {
    let mut poll = poll();
    poll["quiz"] = json!(true);
    let result = describe(
        &poll,
        &json!({"total_voters":0,"results":[{"option":[0,255],"voters":0}]}),
    );
    assert!(result["correct_option_index"].is_null());
    assert_eq!(result["correct_option_indexes"], json!([]));
    assert!(result["correct_option_note"].is_string());
    assert!(result["options"][0].get("share_percent").is_none());
    assert_eq!(result["your_votes"], json!([]));
}

#[test]
fn quiz_reveals_only_correct_options_supplied_by_telegram() {
    let mut poll = poll();
    poll["quiz"] = json!(true);
    let result = describe(
        &poll,
        &json!({"total_voters":1,"results":[
        {"option":[128],"voters":1,"correct":true},{"option":[0,255],"voters":0,"correct":true}]}),
    );
    assert_eq!(result["correct_option_index"], 0);
    assert_eq!(result["correct_option_indexes"], json!([0, 2]));
    assert_eq!(result["options"][2]["correct"], true);
    assert!(result.get("correct_option_note").is_none());
}

#[test]
fn extreme_and_expired_poll_deadlines_return_errors_without_overflow() {
    assert!(deadline(i64::MIN).is_err());
    assert!(deadline(i64::MAX).is_err());
    assert!(deadline(0).is_err());
    assert!(deadline(chrono::Utc::now().timestamp() + 60).is_ok());
}
