use super::common::*;
use serde_json::{Value, json};

fn settings(m: &Mock) {
    m.reply("account.getNotifySettings",json!({"_":"peerNotifySettings","show_previews":false,"silent":false,"mute_until":1900000000,"stories_muted":true,"stories_hide_sender":true,"other_sound":{"_":"notificationSoundRingtone","id":"101"},"stories_other_sound":{"_":"notificationSoundDefault"}}));
}
fn sound(id: i64, name: Option<&str>) -> Value {
    let mut attributes = vec![json!({"_":"documentAttributeAudio","duration":9})];
    if let Some(name) = name {
        attributes.push(json!({"_":"documentAttributeFilename","file_name":name}));
    }
    json!({"_":"document","id":id.to_string(),"access_hash":"70","file_reference":[1,2,3],"attributes":attributes})
}

#[tokio::test]
async fn disabling_incoming_sound_preserves_outgoing_silent_mute_preview_and_story_settings() {
    let m = Mock::new();
    settings(&m);
    run(
        &m,
        "set_chat_sound_disabled",
        json!({"chat":7,"disabled":true}),
    )
    .await
    .unwrap();
    let request = &m.sent("account.updateNotifySettings")[0]["settings"];
    assert_eq!(request["silent"], false);
    assert_eq!(request["mute_until"], 1900000000);
    assert_eq!(request["show_previews"], false);
    assert_eq!(request["stories_muted"], true);
    assert_eq!(request["stories_hide_sender"], true);
    assert_eq!(request["sound"]["_"], "notificationSoundNone");
    assert_eq!(request["stories_sound"]["_"], "notificationSoundDefault");
}

#[tokio::test]
async fn enabling_incoming_sound_restores_default_without_enabling_outgoing_sound() {
    let m = Mock::new();
    m.reply(
        "account.getNotifySettings",
        json!({"silent":true,"mute_until":0,"other_sound":{"_":"notificationSoundNone"}}),
    );
    run(
        &m,
        "set_chat_sound_disabled",
        json!({"chat":7,"disabled":false}),
    )
    .await
    .unwrap();
    let settings = &m.sent("account.updateNotifySettings")[0]["settings"];
    assert_eq!(settings["sound"]["_"], "notificationSoundDefault");
    assert_eq!(settings["silent"], true);
    assert_eq!(settings["mute_until"], 0);
}

#[tokio::test]
async fn unmuting_changes_only_mute_until_and_keeps_the_selected_ringtone() {
    let m = Mock::new();
    settings(&m);
    run(&m, "unmute_chat", json!({"chat_id":7})).await.unwrap();
    let request = &m.sent("account.updateNotifySettings")[0]["settings"];
    assert_eq!(request["mute_until"], 0);
    assert_eq!(request["sound"]["id"], "101");
    assert_eq!(request["show_previews"], false);
}

#[tokio::test]
async fn timed_and_forever_mutes_have_distinct_bounded_deadlines() {
    let m = Mock::new();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    run(
        &m,
        "mute_chat",
        json!({"chat_id":7,"days":1,"hours":2,"minutes":3}),
    )
    .await
    .unwrap();
    let until = m.sent("account.updateNotifySettings")[0]["settings"]["mute_until"]
        .as_i64()
        .unwrap();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let span = 86400 + 7200 + 180;
    assert!((before + span..=after + span).contains(&until));
    let m = Mock::new();
    run(&m, "mute_chat", json!({"chat_id":7})).await.unwrap();
    assert_eq!(
        m.sent("account.updateNotifySettings")[0]["settings"]["mute_until"],
        i32::MAX
    );
}

#[tokio::test]
async fn invalid_or_overflowing_mute_durations_leave_notification_settings_untouched() {
    for fields in [
        json!({"days":-1}),
        json!({"minutes":"fraction"}),
        json!({"hours":i64::MAX}),
        json!({"days":1000000}),
    ] {
        let m = Mock::new();
        let mut args = json!({"chat_id":7});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(run(&m, "mute_chat", args).await.is_err());
        assert!(m.methods().is_empty());
        assert!(m.resolutions().is_empty());
    }
}

#[tokio::test]
async fn saved_sound_names_durations_and_id_fallback_remain_visible_as_untrusted_data() {
    let m = Mock::new();
    m.reply(
        "account.getSavedRingtones",
        json!({"ringtones":[sound(101,Some("Morning.ogg")),sound(303,None)]}),
    );
    let result = run(&m, "list_saved_sounds", json!({})).await.unwrap();
    assert_eq!(result["results"][0]["title"], "Morning.ogg");
    assert_eq!(result["results"][0]["duration"], 9);
    assert_eq!(result["results"][1]["title"], "303");
    assert_eq!(result["untrusted_content"], true);
}

#[tokio::test]
async fn saved_tone_selection_accepts_exact_id_case_insensitive_filename_and_stem() {
    for selector in ["101", "MORNING.OGG", " morning "] {
        let m = Mock::new();
        m.reply(
            "account.getSavedRingtones",
            json!({"ringtones":[sound(101,Some("Morning.ogg"))]}),
        );
        settings(&m);
        run(&m, "set_chat_tone", json!({"chat":7,"tone":selector}))
            .await
            .unwrap();
        let request = &m.sent("account.updateNotifySettings")[0]["settings"];
        assert_eq!(request["sound"]["id"], "101");
        assert_eq!(request["mute_until"], 1900000000);
    }
}

#[tokio::test]
async fn ringtone_upload_saves_the_returned_document_before_selecting_it_for_the_chat() {
    let m = Mock::new();
    m.reply("account.uploadRingtone", sound(101, Some("Alert.opus")));
    m.reply(
        "account.saveRingtone",
        json!({"document":sound(202,Some("Saved.opus"))}),
    );
    settings(&m);
    run(
        &m,
        "set_chat_tone",
        json!({"chat":7,"file_path":"Alert.OpUs"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.methods(),
        [
            "local.upload",
            "account.uploadRingtone",
            "account.saveRingtone",
            "account.getNotifySettings",
            "account.updateNotifySettings"
        ]
    );
    assert_eq!(
        m.sent("account.uploadRingtone")[0]["mime_type"],
        "audio/ogg"
    );
    assert_eq!(m.sent("account.saveRingtone")[0]["unsave"], false);
    assert_eq!(
        m.sent("account.updateNotifySettings")[0]["settings"]["sound"]["id"],
        "202"
    );
}

#[tokio::test]
async fn invalid_tone_inputs_are_refused_before_any_upload_or_notification_mutation() {
    for fields in [
        json!({"tone":"default","file_path":"sound.mp3"}),
        json!({}),
        json!({"file_path":"sound.wav"}),
        json!({"tone":7}),
    ] {
        let m = Mock::new();
        let mut args = json!({"chat":7});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert!(run(&m, "set_chat_tone", args).await.is_err());
        assert!(m.methods().is_empty());
    }
}

#[tokio::test]
async fn unavailable_saved_sound_does_not_silently_select_a_default_tone() {
    let m = Mock::new();
    m.reply(
        "account.getSavedRingtones",
        json!({"ringtones":[sound(101,Some("Morning.ogg"))]}),
    );
    assert!(
        run(&m, "set_chat_tone", json!({"chat":7,"tone":"missing"}))
            .await
            .is_err()
    );
    assert!(m.sent("account.updateNotifySettings").is_empty());
}

#[tokio::test]
async fn removing_saved_sound_resolves_the_exact_document_without_changing_chat_settings() {
    let m = Mock::new();
    m.reply(
        "account.getSavedRingtones",
        json!({"ringtones":[sound(101,Some("Morning.ogg")),sound(303,None)]}),
    );
    run(&m, "remove_saved_sound", json!({"sound":"303"}))
        .await
        .unwrap();
    let sent = &m.sent("account.saveRingtone")[0];
    assert_eq!(sent["id"]["id"], "303");
    assert_eq!(sent["unsave"], true);
    assert!(m.sent("account.updateNotifySettings").is_empty());
}
