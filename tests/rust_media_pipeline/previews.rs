use crate::support::*;
use serde_json::{Value, json};
use telegram_mcp::tools::messaging::media;
async fn run(w: &MediaWire, name: &str, a: Value) -> anyhow::Result<Value> {
    media::call(w, name, &a).await.unwrap()
}
#[tokio::test]
async fn server_thumbnail_download_uses_the_selected_declared_type() {
    let mut doc = document(7, "thumb.bin", 100);
    doc["thumbs"] = json!([{"_":"photoSize","type":"m","w":32,"h":16,"size":80},{"_":"photoSize","type":"x","w":64,"h":32,"size":120}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        png(),
    );
    let result = run(
        &w,
        "get_media_thumbnail",
        json!({"chat_id":7,"message_id":8,"thumb_index":0,"max_bytes":4096}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["thumb_size"], "m");
    assert_eq!(result["metadata"]["kind"], "thumbnail");
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn inline_cached_thumbnails_render_without_any_download() {
    let mut doc = document(7, "thumb.bin", 100);
    doc["thumbs"] = json!([{"_":"photoCachedSize","type":"m","w":32,"h":16,"bytes":png()}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        vec![],
    );
    let result = run(
        &w,
        "get_media_thumbnail",
        json!({"chat_id":7,"message_id":8}),
    )
    .await
    .unwrap();
    assert_eq!(result["content"][0]["type"], "image");
    assert!(w.downloads.lock().unwrap().is_empty());
}
#[tokio::test]
async fn thumbnail_selection_rejects_out_of_range_or_missing_declared_sizes() {
    for thumbs in [
        json!([]),
        json!([{"_":"photoSize","type":"m","w":1,"h":1,"size":1}]),
    ] {
        let mut doc = document(7, "a.bin", 1);
        doc["thumbs"] = thumbs;
        let w = MediaWire::new(
            vec![(
                "messages.getMessages",
                media_message(json!({"document":doc})),
            )],
            vec![],
        );
        assert!(
            run(
                &w,
                "get_media_thumbnail",
                json!({"chat_id":7,"message_id":8,"thumb_index":8})
            )
            .await
            .is_err()
        );
        assert!(w.downloads.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn premium_effect_frames_select_video_thumb_f_instead_of_the_sticker_document() {
    let mut doc = document(7, "sticker.webm", 100);
    doc["video_thumbs"] = json!([{"_":"videoSize","type":"f","w":32,"h":16,"size":100}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        png(),
    );
    let result = run(
        &w,
        "get_media_frames",
        json!({"chat_id":7,"message_id":8,"premium_effect":true}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["thumb_size"], "f");
    assert_eq!(result["metadata"]["static"], true);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":document(7,"plain.webm",1)})),
        )],
        png(),
    );
    assert!(
        run(
            &w,
            "get_media_frames",
            json!({"chat_id":7,"message_id":8,"premium_effect":true})
        )
        .await
        .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
}
#[tokio::test]
async fn custom_emoji_id_and_preview_budget_validation_precedes_the_network() {
    for args in [
        json!({"document_ids":[]}),
        json!({"document_ids":["0"]}),
        json!({"document_ids":["9223372036854775808"]}),
        json!({"document_ids":vec!["3";25]}),
        json!({"document_ids":["3"],"max_bytes":0}),
    ] {
        let w = MediaWire::new(vec![], vec![]);
        assert!(run(&w, "get_custom_emoji", args).await.is_err());
        assert!(w.calls().is_empty());
    }
    for response in [json!({}), json!([])] {
        let w = MediaWire::new(vec![("messages.getCustomEmojiDocuments", response)], vec![]);
        assert!(
            run(&w, "get_custom_emoji", json!({"document_ids":["3"]}))
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn invalid_thumbnail_pixels_and_transfer_budgets_return_honest_errors() {
    let mut doc = document(7, "a.bin", 1);
    doc["thumbs"] = json!([{"_":"photoSize","type":"m","w":1,"h":1,"size":1}]);
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        vec![1, 2, 3],
    );
    assert!(
        run(
            &w,
            "get_media_thumbnail",
            json!({"chat_id":7,"message_id":8})
        )
        .await
        .is_err()
    );
    let w = MediaWire::new(vec![], vec![]);
    assert!(media::download_bytes(&w, json!({}), 0).await.is_err());
    assert!(
        media::download_bytes(&w, json!({}), 512 * 1024 * 1024 + 1)
            .await
            .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
}
