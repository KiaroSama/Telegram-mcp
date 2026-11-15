use super::support::*;
use serde_json::{Value, json};

#[tokio::test]
async fn message_formatting_and_send_identity_are_encoded_in_the_actual_request() {
    let api = Api::new(vec![("messages.sendMessage", updates())]).identity(
        json!("sender"),
        json!({"_":"inputPeerChannel","channel_id":"9","access_hash":"19"}),
    );
    run(&api,"send_message",json!({"chat_id":7,"message":"<b>سلام</b> 🙂","parse_mode":"html","send_as":"sender","effect_id":"9223372036854775807"})).await.unwrap();
    let req = &api.calls()[0].1;
    assert_eq!(req["message"], "سلام 🙂");
    assert_eq!(
        req["entities"][0],
        json!({"_":"messageEntityBold","offset":0,"length":4})
    );
    assert_eq!(req["send_as"]["channel_id"], "9");
    assert_eq!(req["effect"], "9223372036854775807");
    let api = Api::new(vec![("messages.editMessage", updates())]);
    run(
        &api,
        "edit_message",
        json!({"chat_id":7,"message_id":8,"new_text":"**edited**","parse_mode":"markdown"}),
    )
    .await
    .unwrap();
    assert_eq!(api.calls()[0].1["message"], "edited");
    assert_eq!(api.calls()[0].1["id"], 8);
    assert!(api.calls()[0].1.get("random_id").is_none());
}
#[tokio::test]
async fn rich_modes_keep_native_source_flags_and_accept_bodies_beyond_plain_text_limits() {
    for (mode, field, constructor) in [
        ("rich_html", "html", "inputRichMessageHTML"),
        ("rich_markdown", "markdown", "inputRichMessageMarkdown"),
    ] {
        let api = Api::new(vec![("messages.sendMessage", updates())]);
        let text = "paragraph ".repeat(500);
        run(&api,"send_message",json!({"chat_id":7,"message":text,"parse_mode":mode,"rich_rtl":true,"rich_no_autolink":true})).await.unwrap();
        let calls = api.calls();
        let rich = &calls[0].1["rich_message"];
        assert_eq!(rich["_"], constructor);
        assert_eq!(rich[field], text);
        assert_eq!(rich["rtl"], true);
        assert_eq!(rich["noautolink"], true);
    }
}
#[tokio::test]
async fn rich_and_plain_formatting_validation_prevents_ambiguous_sends() {
    for args in [
        json!({"message":" "}),
        json!({"message":"🙂".repeat(2049)}),
        json!({"message":"text","parse_mode":"rich_invalid"}),
        json!({"message":"text","parse_mode":"rich_html","entities":[]}),
        json!({"message":"text","parse_mode":"rich_html","rich_files":[]}),
        json!({"message":"text","parse_mode":"html","entities":[]}),
        json!({"message":"text","rich_files":{"attachment":"unused"}}),
    ] {
        let api = Api::new(vec![]);
        let mut args = args;
        args["chat_id"] = json!(7);
        assert!(run(&api, "send_message", args).await.is_err());
        assert!(api.calls().is_empty());
    }
    let api = Api::new(vec![]);
    let files: serde_json::Map<String, Value> =
        (0..21).map(|i| (i.to_string(), json!("unused"))).collect();
    assert!(
        run(
            &api,
            "send_message",
            json!({"chat_id":7,"message":"text","parse_mode":"rich_html","rich_files":files})
        )
        .await
        .is_err()
    );
    assert!(api.calls().is_empty());
}
#[tokio::test]
async fn scheduled_sends_encode_the_future_date_and_recurrence_exactly_once() {
    let when = telegram_mcp::tools::messaging::helpers::unix_time() + 3600;
    for (name, field, repeat, period) in [
        ("send_scheduled_message", "schedule_date", "daily", 86400),
        ("schedule_message", "when", "weekly", 604800),
    ] {
        let api = Api::new(vec![("messages.sendMessage", updates())]);
        let mut args = json!({"chat_id":7,"message":"later","repeat":repeat});
        args[field] = json!(when);
        run(&api, name, args).await.unwrap();
        assert_eq!(api.calls()[0].1["schedule_date"], when);
        assert_eq!(api.calls()[0].1["schedule_repeat_period"], period);
    }
    let api = Api::new(vec![]);
    assert!(
        run(
            &api,
            "send_scheduled_message",
            json!({"chat_id":7,"message":"later"})
        )
        .await
        .is_err()
    );
    assert!(api.calls().is_empty());
}
#[tokio::test]
async fn scheduled_date_only_edit_preserves_body_formatting_and_recurrence() {
    let mut queued = message(8, "stored body");
    queued["schedule_repeat_period"] = json!(604800);
    queued["entities"] = json!([{"_":"messageEntityBold","offset":0,"length":6}]);
    let api = Api::new(vec![
        ("messages.getScheduledHistory", messages(vec![queued])),
        ("messages.editMessage", updates()),
    ]);
    let when = telegram_mcp::tools::messaging::helpers::unix_time() + 3600;
    run(
        &api,
        "edit_scheduled_message",
        json!({"chat_id":7,"message_id":8,"when":when}),
    )
    .await
    .unwrap();
    let calls = api.calls();
    let req = &calls[1].1;
    assert_eq!(req["id"], 8);
    assert_eq!(req["schedule_date"], when);
    assert_eq!(req["message"], "stored body");
    assert_eq!(
        req["entities"],
        json!([{"_":"messageEntityBold","offset":0,"length":6}])
    );
    assert_eq!(req["schedule_repeat_period"], 604800);
    assert_eq!(calls[0].1["hash"], "0");
}
#[tokio::test]
async fn scheduled_text_edit_keeps_the_original_date_even_if_the_queue_is_overdue() {
    let queued = message(8, "old text");
    let api = Api::new(vec![
        ("messages.getScheduledHistory", messages(vec![queued])),
        ("messages.editMessage", updates()),
    ]);
    run(
        &api,
        "edit_scheduled_message",
        json!({"chat_id":7,"message_id":8,"message":"new text"}),
    )
    .await
    .unwrap();
    let req = &api.calls()[1].1;
    assert_eq!(req["schedule_date"], 123);
    assert_eq!(req["message"], "new text");
    assert!(req.get("entities").is_none());
}
#[tokio::test]
async fn scheduled_edits_refuse_ids_outside_the_scheduled_queue_before_mutation() {
    let api = Api::new(vec![(
        "messages.getScheduledHistory",
        messages(vec![message(9, "a different queued ID")]),
    )]);
    assert!(
        run(
            &api,
            "edit_scheduled_message",
            json!({"chat_id":7,"message_id":8,"message":"replacement"})
        )
        .await
        .is_err()
    );
    assert_eq!(api.calls().len(), 1);
}
#[tokio::test]
async fn scheduled_edits_refuse_unreadable_bodies_and_missing_dates_without_sending() {
    let mut missing_date = message(8, "body");
    missing_date.as_object_mut().unwrap().remove("date");
    for queued in [message(8, ""), missing_date] {
        let api = Api::new(vec![(
            "messages.getScheduledHistory",
            messages(vec![queued]),
        )]);
        assert!(
            run(
                &api,
                "edit_scheduled_message",
                json!({"chat_id":7,"message_id":8})
            )
            .await
            .is_err()
        );
        assert_eq!(api.calls().len(), 1);
    }
}
#[tokio::test]
async fn scheduled_repeat_off_does_not_reapply_the_old_period_and_bodyless_entities_are_ignored() {
    let mut queued = message(8, "stored body");
    queued["schedule_repeat_period"] = json!(86400);
    queued["entities"] = json!([{"_":"messageEntityItalic","offset":0,"length":6}]);
    let api = Api::new(vec![
        ("messages.getScheduledHistory", messages(vec![queued])),
        ("messages.editMessage", updates()),
    ]);
    run(&api,"edit_scheduled_message",json!({"chat_id":7,"message_id":8,"repeat":"off","entities":[{"type":"bold","offset":999,"length":1}]})).await.unwrap();
    let req = &api.calls()[1].1;
    assert!(req.get("schedule_repeat_period").is_none());
    assert_eq!(req["entities"][0]["_"], "messageEntityItalic");
    assert_eq!(req["message"], "stored body");
}
#[tokio::test]
async fn unchanged_message_edits_return_the_idempotent_native_outcome() {
    let api = Api::new(vec![]);
    api.error("messages.editMessage", "MESSAGE_NOT_MODIFIED");
    let result = run(
        &api,
        "edit_message",
        json!({"chat_id":7,"message_id":8,"new_text":"same"}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"edited_message_id":8,"not_modified":true}));
    assert_eq!(api.calls().len(), 1);
}
#[tokio::test]
async fn plain_quote_fallback_preserves_utf16_offsets_and_clips_existing_formatting() {
    let mut original = message(8, "🙂before quoted tail");
    original["entities"] = json!([{"_":"messageEntityBold","offset":2,"length":6},{"_":"messageEntityItalic","offset":9,"length":6},{"_":"messageEntityUnderline","offset":16,"length":4}]);
    let api = Api::new(vec![]);
    api.error("messages.getRichMessage", "MESSAGE_ID_INVALID");
    api.push("messages.getMessages", messages(vec![original]));
    api.push("messages.sendMessage", updates());
    run(
        &api,
        "reply_to_message",
        json!({"chat_id":7,"message_id":8,"text":"response","quote_text":"fore quoted"}),
    )
    .await
    .unwrap();
    let calls = api.calls();
    let reply = &calls[2].1["reply_to"];
    assert_eq!(reply["quote_text"], "fore quoted");
    assert_eq!(reply["quote_offset"], 4);
    assert_eq!(
        reply["quote_entities"],
        json!([{"_":"messageEntityBold","offset":0,"length":4},{"_":"messageEntityItalic","offset":5,"length":6}])
    );
}
#[tokio::test]
async fn rich_quote_sends_exact_rich_fragments_without_fetching_the_plain_message() {
    let mut original = message(8, "");
    original["rich_message"] = json!({"blocks":[{"_":"pageBlockParagraph","text":{"_":"textBold","text":{"_":"textPlain","text":"rich quoted"}}}]});
    let api = Api::new(vec![
        ("messages.getRichMessage", messages(vec![original])),
        ("messages.sendMessage", updates()),
    ]);
    run(&api,"reply_to_message",json!({"chat_id":7,"message_id":8,"text":"response","quote_text":"quoted","quote_block_index":0})).await.unwrap();
    let reply = &api.calls()[1].1["reply_to"];
    assert_eq!(reply["quote_offset"], 5);
    assert_eq!(
        reply["quote_rich"],
        json!([{"_":"textBold","text":{"_":"textPlain","text":"quoted"}}])
    );
    assert!(reply.get("quote_text").is_none());
    assert_eq!(api.calls().len(), 2);
}
#[tokio::test]
async fn invalid_quote_types_offsets_and_source_failures_never_send_a_reply() {
    for quote in [json!(7), json!("")] {
        let api = Api::new(vec![]);
        assert!(
            run(
                &api,
                "reply_to_message",
                json!({"chat_id":7,"message_id":8,"text":"response","quote_text":quote})
            )
            .await
            .is_err()
        );
        assert!(api.calls().is_empty());
    }
    for args in [
        json!({"quote_text":"word","quote_offset":-1}),
        json!({"quote_text":"word","quote_offset":1}),
        json!({"quote_text":"missing"}),
    ] {
        let api = Api::new(vec![]);
        api.error("messages.getRichMessage", "MESSAGE_ID_INVALID");
        api.push("messages.getMessages", messages(vec![message(8, "word")]));
        let mut args = args;
        args["chat_id"] = json!(7);
        args["message_id"] = json!(8);
        args["text"] = json!("response");
        assert!(run(&api, "reply_to_message", args).await.is_err());
        assert_eq!(api.calls().len(), 2);
    }
    let api = Api::new(vec![(
        "messages.getRichMessage",
        messages(vec![message(8, "word")]),
    )]);
    assert!(run(&api,"reply_to_message",json!({"chat_id":7,"message_id":8,"text":"response","quote_text":"word","quote_block_index":0})).await.is_err());
    assert_eq!(api.calls().len(), 1);
    let api = Api::new(vec![]);
    api.error("messages.getRichMessage", "CHANNEL_PRIVATE");
    assert!(
        run(
            &api,
            "reply_to_message",
            json!({"chat_id":7,"message_id":8,"text":"response","quote_text":"word"})
        )
        .await
        .is_err()
    );
    assert_eq!(api.calls().len(), 1);
}
fn isolated(test: &str) -> bool {
    if std::env::var("MESSAGING_BRANCH_CHILD").ok().as_deref() == Some(test) {
        return true;
    }
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let mut command =
        telegram_mcp::tools::messaging::media::child(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env("MESSAGING_BRANCH_CHILD", test)
        .env("TELEGRAM_STATE_DIR", dir.path().join("state"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&[&files]).unwrap(),
        );
    let output = command.as_std_mut().output().unwrap();
    assert!(
        output.status.success(),
        "Isolated native rich-file test failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let proof: Value = serde_json::from_slice(
        &std::fs::read(files.join("verified-rich-request.json"))
            .expect("The isolated test must execute and publish its actual wire request"),
    )
    .unwrap();
    assert_eq!(proof["rich_message"]["files"][0]["id"], "a-photo");
    assert_eq!(proof["rich_message"]["files"][1]["id"], "b-document");
    false
}
#[tokio::test]
async fn rich_file_uploads_bind_named_photo_and_document_references_within_private_roots() {
    if !isolated(
        "rust_messaging_branches::send::rich_file_uploads_bind_named_photo_and_document_references_within_private_roots",
    ) {
        return;
    }
    let state = telegram_mcp::state::State::global().unwrap();
    let dir = tempfile::tempdir_in(&state.file_roots()[0]).unwrap();
    let photo = dir.path().join("photo.png");
    let document = dir.path().join("document.bin");
    std::fs::write(&photo, crate::media_support::png()).unwrap();
    std::fs::write(&document, b"attachment").unwrap();
    let api = Api::new(vec![
        (
            "messages.uploadMedia",
            json!({"_":"messageMediaPhoto","photo":crate::media_support::photo(3)}),
        ),
        (
            "messages.uploadMedia",
            json!({"_":"messageMediaDocument","document":crate::media_support::document(4,"document.bin",10)}),
        ),
        ("messages.sendMessage", updates()),
    ]);
    run(&api,"send_message",json!({"chat_id":7,"message":"<p>attachments</p>","parse_mode":"rich_html","rich_files":{"a-photo":photo,"b-document":document}})).await.unwrap();
    let calls = api.calls();
    let files = &calls[2].1["rich_message"]["files"];
    std::fs::write(
        state.file_roots()[0].join("verified-rich-request.json"),
        serde_json::to_vec(&calls[2].1).unwrap(),
    )
    .unwrap();
    assert_eq!(files[0]["_"], "inputRichFilePhoto");
    assert_eq!(files[0]["id"], "a-photo");
    assert_eq!(files[0]["photo"]["id"], "3");
    assert_eq!(files[1]["_"], "inputRichFileDocument");
    assert_eq!(files[1]["document"]["id"], "4");
    assert_eq!(api.uploaded.lock().unwrap().len(), 2);
    for files in [
        json!({"bad":3}),
        json!({"bad":state.root().join("private.json")}),
    ] {
        let api = Api::new(vec![]);
        assert!(run(&api,"send_message",json!({"chat_id":7,"message":"rich","parse_mode":"rich_markdown","rich_files":files})).await.is_err());
        assert!(api.uploaded.lock().unwrap().is_empty());
    }
    let api = Api::new(vec![(
        "messages.uploadMedia",
        json!({"_":"messageMediaEmpty"}),
    )]);
    assert!(run(&api,"send_message",json!({"chat_id":7,"message":"rich","parse_mode":"rich_markdown","rich_files":{"file":document}})).await.unwrap_err().to_string().contains("reference"));
    assert_eq!(api.calls().len(), 1);
}
#[tokio::test]
async fn forwarding_and_draft_mutations_preserve_topics_schedule_and_source_bounds() {
    let when = telegram_mcp::tools::messaging::helpers::unix_time() + 3600;
    let api = Api::new(vec![("messages.forwardMessages", updates())]);
    run(&api,"forward_messages",json!({"from_chat_id":7,"to_chat_id":7,"message_ids":[8,9],"topic_id":55,"when":when,"repeat":"daily","drop_author":true,"drop_captions":true,"send_as":7})).await.unwrap();
    let req = &api.calls()[0].1;
    assert_eq!(req["id"], json!([8, 9]));
    assert_eq!(req["random_id"].as_array().unwrap().len(), 2);
    assert_eq!(req["top_msg_id"], 55);
    assert_eq!(req["schedule_date"], when);
    assert_eq!(req["schedule_repeat_period"], 86400);
    let api = Api::new(vec![
        ("messages.saveDraft", json!(true)),
        ("messages.saveDraft", json!(true)),
        ("messages.deleteScheduledMessages", updates()),
    ]);
    run(
        &api,
        "save_draft",
        json!({"chat_id":7,"message":"draft","reply_to_msg_id":8,"topic_id":55,"no_webpage":true}),
    )
    .await
    .unwrap();
    run(&api, "clear_draft", json!({"chat_id":7}))
        .await
        .unwrap();
    run(
        &api,
        "cancel_scheduled_message",
        json!({"chat_id":7,"message_id":8}),
    )
    .await
    .unwrap();
    let calls = api.calls();
    assert_eq!(calls[0].1["reply_to"]["reply_to_msg_id"], 8);
    assert_eq!(calls[0].1["reply_to"]["top_msg_id"], 55);
    assert_eq!(calls[1].1["message"], "");
    assert_eq!(calls[2].1["id"], json!([8]));
}
