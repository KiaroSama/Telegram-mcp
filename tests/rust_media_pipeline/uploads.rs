use crate::{local_support, support::*};
use serde_json::{Value, json};
use std::path::Path;
use telegram_mcp::tools::messaging::media::uploads;

async fn send(
    w: &MediaWire,
    state: &telegram_mcp::state::State,
    name: &str,
    a: Value,
) -> anyhow::Result<Value> {
    uploads::run_with_state(w, state, name, &a).await
}
#[tokio::test]
async fn a_photo_send_keeps_utf16_caption_entities_and_does_not_emit_read_actions() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("سلام.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![("messages.sendMedia", json!({"_":"updates","updates":[]}))],
        vec![],
    );
    send(&w,&state,"send_file",json!({"chat_id":7,"file_path":path,"caption":"🙂bold","caption_entities":[{"type":"bold","offset":2,"length":4}]})).await.unwrap();
    let calls = w.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["media"]["_"], "inputMediaUploadedPhoto");
    assert_eq!(calls[0].1["message"], "🙂bold");
    assert_eq!(
        calls[0].1["entities"][0],
        json!({"_":"messageEntityBold","offset":2,"length":4})
    );
}
#[tokio::test]
async fn document_album_normalizes_uploaded_references_and_caps_caption_to_first_item() {
    let (_dir, state) = local_support::fixture();
    let a = state.file_roots()[0].join("a.bin");
    let b = state.file_roots()[0].join("b.bin");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let w = MediaWire::new(
        vec![
            (
                "messages.uploadMedia",
                json!({"document":document(100,"a.bin",1)}),
            ),
            (
                "messages.uploadMedia",
                json!({"document":document(101,"b.bin",1)}),
            ),
            (
                "messages.sendMultiMedia",
                json!({"_":"updates","updates":[]}),
            ),
        ],
        vec![],
    );
    send(&w,&state,"send_album",json!({"chat_id":7,"file_paths":[a,b],"kind":["document","document"],"caption":"**caption**","parse_mode":"markdown","topic_id":55,"reply_to_message_id":8,"send_as":7})).await.unwrap();
    let calls = w.calls();
    let req = &calls[2].1;
    assert_eq!(req["multi_media"][0]["media"]["_"], "inputMediaDocument");
    assert_eq!(req["multi_media"][0]["media"]["id"]["id"], "100");
    assert_eq!(req["multi_media"][1]["message"], "");
    assert_eq!(req["multi_media"][0]["message"], "caption");
    assert_eq!(
        req["multi_media"][0]["entities"][0]["_"],
        "messageEntityBold"
    );
    assert_eq!(req["reply_to"]["top_msg_id"], 55);
    assert_eq!(req["reply_to"]["reply_to_msg_id"], 8);
    assert!(req.get("send_as").is_some());
    assert_eq!(calls[0].1["media"]["force_file"], true);
}
#[tokio::test]
async fn photo_album_uses_normalized_photo_references_and_refuses_missing_server_reference() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("photo.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![
            ("messages.uploadMedia", json!({"photo":photo(90)})),
            (
                "messages.sendMultiMedia",
                json!({"_":"updates","updates":[]}),
            ),
        ],
        vec![],
    );
    send(
        &w,
        &state,
        "send_album",
        json!({"chat_id":7,"file_paths":[path]}),
    )
    .await
    .unwrap();
    assert_eq!(
        w.calls()[1].1["multi_media"][0]["media"]["_"],
        "inputMediaPhoto"
    );
    let w = MediaWire::new(vec![("messages.uploadMedia", json!({}))], vec![]);
    assert!(
        send(
            &w,
            &state,
            "send_album",
            json!({"chat_id":7,"file_paths":[path]})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("references")
    );
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn changing_media_preserves_existing_caption_when_no_new_caption_was_supplied() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("new.png");
    std::fs::write(&path, png()).unwrap();
    let mut old = media_message(json!({"_":"messageMediaPhoto","photo":photo(3)}));
    old["messages"][0]["message"] = json!("old caption");
    old["messages"][0]["entities"] = json!([{"_":"messageEntityItalic","offset":0,"length":3}]);
    let w = MediaWire::new(
        vec![
            ("messages.getMessages", old),
            ("messages.editMessage", json!({"_":"updates","updates":[]})),
        ],
        vec![],
    );
    send(
        &w,
        &state,
        "edit_message_media",
        json!({"chat_id":7,"message_id":8,"file_path":path}),
    )
    .await
    .unwrap();
    let calls = w.calls();
    assert_eq!(calls[1].1["message"], "old caption");
    assert_eq!(calls[1].1["entities"][0]["_"], "messageEntityItalic");
    assert_eq!(calls[1].1["media"]["_"], "inputMediaUploadedPhoto");
}

#[tokio::test]
async fn changing_media_preserves_decoded_unformatted_captions() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("new.png");
    std::fs::write(&path, png()).unwrap();
    let schema = telegram_mcp::tl::Schema::global();
    for caption in ["plain caption", ""] {
        let raw = json!({"_":"message","id":8,"peer_id":{"_":"peerUser","user_id":"7"},"date":123,"message":caption,"media":{"_":"messageMediaPhoto","photo":photo(3)}});
        let original = schema
            .decode("Message", &schema.encode_object(&raw).unwrap())
            .unwrap();
        assert_eq!(original.get("entities"), Some(&Value::Null));
        let w = MediaWire::new(
            vec![
                (
                    "messages.getMessages",
                    json!({"messages":[original],"topics":[],"users":[],"chats":[]}),
                ),
                ("messages.editMessage", json!({"_":"updates","updates":[]})),
            ],
            vec![],
        );
        send(
            &w,
            &state,
            "edit_message_media",
            json!({"chat_id":7,"message_id":8,"file_path":path}),
        )
        .await
        .unwrap();
        let calls = w.calls();
        assert_eq!(calls[1].1["message"], caption);
        assert_eq!(calls[1].1["entities"], json!([]));
        assert_eq!(calls[1].1["media"]["_"], "inputMediaUploadedPhoto");
    }
}
#[tokio::test]
async fn explicit_media_edit_caption_skips_fetch_and_uses_native_html_entities() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("new.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![("messages.editMessage", json!({"_":"updates","updates":[]}))],
        vec![],
    );
    send(&w,&state,"edit_message_media",json!({"chat_id":7,"message_id":8,"file_path":path,"caption":"<b>new</b>","parse_mode":"html"})).await.unwrap();
    assert_eq!(w.calls()[0].1["message"], "new");
    assert_eq!(w.calls()[0].1["entities"][0]["_"], "messageEntityBold");
}
#[tokio::test]
async fn view_once_photo_uses_the_protocol_ttl_sentinel() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("once.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![("messages.sendMedia", json!({"_":"updates","updates":[]}))],
        vec![],
    );
    send(
        &w,
        &state,
        "send_disappearing_media",
        json!({"chat_id":7,"file_path":path,"seconds":0}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].1["media"]["ttl_seconds"], 2147483647);
}
#[tokio::test]
async fn invalid_media_combinations_never_upload_or_send_any_files() {
    let (_dir, state) = local_support::fixture();
    let photo = state.file_roots()[0].join("a.png");
    let doc = state.file_roots()[0].join("a.bin");
    std::fs::write(&photo, png()).unwrap();
    std::fs::write(&doc, b"document").unwrap();
    for (tool, args) in [
        ("send_album", json!({"file_paths":[]})),
        ("send_album", json!({"file_paths":vec![&photo;11]})),
        ("send_album", json!({"file_paths":[photo,doc]})),
        (
            "send_album",
            json!({"file_paths":[photo],"kind":["photo","document"]}),
        ),
        ("send_album", json!({"file_paths":[photo],"kind":"sticker"})),
        ("send_file", json!({"file_path":photo,"kind":"voice_note"})),
        (
            "send_sticker",
            json!({"file_path":photo,"caption":"forbidden"}),
        ),
        (
            "send_file",
            json!({"file_path":photo,"caption":"🙂".repeat(513)}),
        ),
        (
            "send_file",
            json!({"file_path":photo,"caption":"a","parse_mode":"html","caption_entities":[]}),
        ),
        (
            "send_disappearing_media",
            json!({"file_path":photo,"seconds":61}),
        ),
        (
            "send_disappearing_media",
            json!({"file_path":photo,"as_voice":true,"as_video_note":true}),
        ),
        (
            "send_disappearing_media",
            json!({"file_path":doc,"seconds":1}),
        ),
    ] {
        let w = MediaWire::new(vec![], vec![]);
        let mut args = args;
        args["chat_id"] = json!(7);
        assert!(
            send(&w, &state, tool, args.clone()).await.is_err(),
            "accepted {tool} {args}"
        );
        assert!(w.uploads.lock().unwrap().is_empty());
        assert!(w.calls().is_empty());
    }
}
#[tokio::test]
async fn raw_upload_respects_roots_and_returns_the_real_uploaded_handle() {
    let (dir, state) = local_support::fixture();
    let outside = dir.path().join("outside.bin");
    std::fs::write(&outside, b"secret").unwrap();
    let w = MediaWire::new(vec![], vec![]);
    assert!(
        send(&w, &state, "upload_file", json!({"file_path":outside}))
            .await
            .is_err()
    );
    assert!(w.uploads.lock().unwrap().is_empty());
    let file = state.file_roots()[0].join("allowed.bin");
    std::fs::write(&file, b"bytes").unwrap();
    let result = send(&w, &state, "upload_file", json!({"file_path":file}))
        .await
        .unwrap();
    assert_eq!(result["_"], "inputFile");
    assert_eq!(w.uploads.lock().unwrap().as_slice(), [file]);
}
#[tokio::test]
async fn native_audio_probe_carries_duration_title_artist_and_voice_identity() {
    let (_dir, state) = local_support::fixture();
    let wav_path = state.file_roots()[0].join("audio.wav");
    std::fs::write(&wav_path, wav()).unwrap();
    let tagged = state.file_roots()[0].join("music.ogg");
    ffmpeg(&[
        "-i".as_ref(),
        wav_path.as_os_str(),
        "-c:a".as_ref(),
        "libopus".as_ref(),
        "-metadata".as_ref(),
        "title=ترانه".as_ref(),
        "-metadata".as_ref(),
        "artist=خواننده".as_ref(),
        tagged.as_os_str(),
    ]);
    let w = MediaWire::new(vec![], vec![]);
    let media = uploads::uploaded(&w, &tagged, "audio", None).await.unwrap();
    let attr = &media["attributes"][1];
    assert_eq!(attr["voice"], false);
    assert!(attr["duration"].as_i64().unwrap() >= 1);
    assert_eq!(media["mime_type"], "audio/ogg");
    assert_eq!(attr["title"], "ترانه");
    assert_eq!(attr["performer"], "خواننده");
    let media = uploads::uploaded(&w, &tagged, "voice_note", Some(30))
        .await
        .unwrap();
    assert_eq!(media["attributes"][1]["voice"], true);
    assert!(media["attributes"][1].get("title").is_none());
    assert_eq!(media["ttl_seconds"], 30);
    assert_eq!(uploads::infer_kind(&wav_path), "audio");
}
#[tokio::test]
async fn native_video_probe_distinguishes_streaming_round_notes_and_animations() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("clip.mp4");
    ffmpeg(&[
        "-f".as_ref(),
        "lavfi".as_ref(),
        "-i".as_ref(),
        "color=c=red:s=32x32:r=2:d=1".as_ref(),
        "-c:v".as_ref(),
        "mpeg4".as_ref(),
        path.as_os_str(),
    ]);
    let w = MediaWire::new(vec![], vec![]);
    for (kind, round, streaming, animated) in [
        ("video", false, true, false),
        ("video_note", true, false, false),
        ("animation", false, true, true),
    ] {
        let media = uploads::uploaded(&w, &path, kind, None).await.unwrap();
        let video = &media["attributes"][1];
        assert_eq!(video["w"], 32);
        assert_eq!(video["h"], 32);
        assert_eq!(video["round_message"], round);
        assert_eq!(video["supports_streaming"], streaming);
        assert_eq!(
            media["attributes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["_"] == "documentAttributeAnimated"),
            animated
        );
    }
}
#[tokio::test]
async fn malformed_media_fails_codec_validation_before_sending_an_rpc() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("broken.mp4");
    std::fs::write(&path, b"not video").unwrap();
    let w = MediaWire::new(vec![], vec![]);
    assert!(
        send(
            &w,
            &state,
            "send_file",
            json!({"chat_id":7,"file_path":path})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("ffprobe")
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn sticker_uploads_carry_sticker_attributes_instead_of_file_forcing() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("sticker.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![("messages.sendMedia", json!({"_":"updates","updates":[]}))],
        vec![],
    );
    send(
        &w,
        &state,
        "send_sticker",
        json!({"chat_id":7,"file_path":path}),
    )
    .await
    .unwrap();
    let media = &w.calls()[0].1["media"];
    assert_eq!(media["mime_type"], "image/png");
    assert_eq!(media["force_file"], false);
    assert_eq!(media["attributes"][1]["_"], "documentAttributeSticker");
    assert_eq!(
        media["attributes"][1]["stickerset"]["_"],
        "inputStickerSetEmpty"
    );
    assert_eq!(uploads::infer_kind(Path::new("unknown.xyz")), "document");
}

#[tokio::test]
async fn caption_limit_applies_to_parsed_text_before_uploading() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("caption.png");
    std::fs::write(&path, png()).unwrap();
    let w = MediaWire::new(
        vec![("messages.sendMedia", json!({"_":"updates","updates":[]}))],
        vec![],
    );
    send(&w, &state, "send_file", json!({"chat_id":7,"file_path":path,"caption":format!("<b>{}</b>", "a".repeat(1024)),"parse_mode":"html"})).await.unwrap();
    assert_eq!(w.calls()[0].1["message"].as_str().unwrap().len(), 1024);
    assert_eq!(w.calls()[0].1["entities"][0]["length"], 1024);
    let w = MediaWire::new(vec![], vec![]);
    assert!(send(&w, &state, "send_file", json!({"chat_id":7,"file_path":path,"caption":format!("<b>{}</b>", "a".repeat(1025)),"parse_mode":"html"})).await.unwrap_err().to_string().contains("1024"));
    assert!(w.uploads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn heterogeneous_file_lists_preserve_contiguous_groups_order_and_one_caption() {
    let (_dir, state) = local_support::fixture();
    let mut paths = vec![];
    for name in ["first.png", "second.png", "third.bin", "fourth.png"] {
        let path = state.file_roots()[0].join(name);
        std::fs::write(
            &path,
            if name.ends_with("png") {
                png()
            } else {
                b"document".to_vec()
            },
        )
        .unwrap();
        paths.push(path);
    }
    let w = MediaWire::new(
        vec![
            ("messages.uploadMedia", json!({"photo":photo(1)})),
            ("messages.uploadMedia", json!({"photo":photo(2)})),
            (
                "messages.sendMultiMedia",
                json!({"_":"updates","updates":[]}),
            ),
            ("messages.sendMedia", json!({"_":"updates","updates":[]})),
            ("messages.sendMedia", json!({"_":"updates","updates":[]})),
        ],
        vec![],
    );
    let result = send(&w,&state,"send_file",json!({"chat_id":7,"file_path":paths,"caption":"<b>first</b>","parse_mode":"html","topic_id":55,"reply_to_message_id":8,"send_as":7})).await.unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 3);
    assert_eq!(*w.uploads.lock().unwrap(), paths);
    let calls = w.calls();
    assert_eq!(calls[2].1["multi_media"].as_array().unwrap().len(), 2);
    assert_eq!(calls[2].1["multi_media"][0]["message"], "first");
    assert_eq!(
        calls[2].1["multi_media"][0]["entities"][0]["_"],
        "messageEntityBold"
    );
    assert_eq!(calls[2].1["multi_media"][1]["message"], "");
    assert_eq!(calls[3].1["media"]["_"], "inputMediaUploadedDocument");
    assert_eq!(calls[4].1["media"]["_"], "inputMediaUploadedPhoto");
    for (_, req) in &calls[2..] {
        assert_eq!(req["reply_to"]["top_msg_id"], 55);
        assert_eq!(req["send_as"]["user_id"], "7");
    }
    for (_, req) in &calls[3..] {
        assert_eq!(req["message"], "");
        assert_eq!(req["entities"], json!([]));
    }
}

#[tokio::test]
async fn later_group_failure_keeps_completed_results_and_never_retries_a_send() {
    use async_trait::async_trait;
    use telegram_mcp::backend::Telegram;
    struct Failure(MediaWire, std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl Telegram for Failure {
        async fn invoke(&self, method: &str, args: Value) -> anyhow::Result<Value> {
            if method == "messages.sendMedia"
                && self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
            {
                telegram_mcp::tl::Schema::global().encode_call(method, &args)?;
                self.0.calls.lock().unwrap().push((method.to_owned(), args));
                anyhow::bail!("synthetic connection lost after sending");
            }
            self.0.invoke(method, args).await
        }
        async fn resolve_peer(&self, input: &Value) -> anyhow::Result<Value> {
            self.0.resolve_peer(input).await
        }
        async fn upload(&self, path: &Path) -> anyhow::Result<Value> {
            self.0.upload(path).await
        }
    }
    let (_dir, state) = local_support::fixture();
    let paths: Vec<_> = ["first.png", "second.bin", "third.png"]
        .iter()
        .map(|name| {
            let path = state.file_roots()[0].join(name);
            std::fs::write(&path, b"synthetic upload").unwrap();
            path
        })
        .collect();
    let w = Failure(
        MediaWire::new(
            vec![(
                "messages.sendMedia",
                json!({"_":"updates","updates":[],"completed_fixture":true}),
            )],
            vec![],
        ),
        std::sync::atomic::AtomicUsize::new(0),
    );
    let result = uploads::run_with_state(
        &w,
        &state,
        "send_file",
        &json!({"chat_id":7,"file_path":paths}),
    )
    .await
    .unwrap();
    assert_eq!(result["partial"], true);
    assert_eq!(result["completed"][0]["completed_fixture"], true);
    assert_eq!(result["failed_file_index"], 1);
    assert_eq!(w.0.calls().len(), 2);
    assert_eq!(w.1.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn music_tags_require_a_comment_header_and_exact_key() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("input.ogg");
    for (bytes, expected_voice, kind) in [
        (b"".as_slice(), None, "voice_note"),
        (b"OggS".as_slice(), None, "voice_note"),
        (b"garbageTITLE=x".as_slice(), None, "voice_note"),
        (b"OggS OpusHead TITLE=x", None, "voice_note"),
        (b"OggS OpusTags MYALBUM=x", Some(true), "voice_note"),
        (b"OggS OpusTags COMPOSER=x", Some(false), "audio"),
        (b"OggS OpusTags ALBUMARTIST=x", Some(false), "audio"),
        (b"OggS OpusTags GENRE=x", Some(false), "audio"),
        (b"OggS\x03vorbis TRACKNUMBER=2", Some(false), "audio"),
    ] {
        assert_eq!(uploads::looks_like_voice(bytes), expected_voice);
        std::fs::write(&p, bytes).unwrap();
        assert_eq!(uploads::infer_kind(&p), kind);
    }
}

#[tokio::test]
async fn send_sticker_rejects_files_exceeding_10_mib_before_upload() {
    let (_dir, state) = local_support::fixture();
    let path = state.file_roots()[0].join("oversized.webp");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(10 * 1024 * 1024 + 1).unwrap();
    let w = MediaWire::new(vec![], vec![]);
    let err = send(
        &w,
        &state,
        "send_sticker",
        json!({"chat_id":7,"file_path":path}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("File is too large for send_sticker: 10485761 bytes (limit: 10485760 bytes)."),
        "unexpected error: {err}"
    );
    assert!(w.calls().is_empty());
    assert!(w.uploads.lock().unwrap().is_empty());
}
