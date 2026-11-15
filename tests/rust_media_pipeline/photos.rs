use crate::support::*;
use serde_json::{Value, json};
use telegram_mcp::tools::messaging::media;
async fn run(w: &MediaWire, name: &str, a: Value) -> anyhow::Result<Value> {
    media::call(w, name, &a).await.unwrap()
}
#[tokio::test]
async fn avatar_listing_uses_user_photos_and_marks_only_the_current_entry() {
    let w = MediaWire::new(
        vec![(
            "photos.getUserPhotos",
            json!({"photos":[photo(3),photo(2)]}),
        )],
        vec![],
    );
    let result = run(&w, "list_photos", json!({"chat_id":7})).await.unwrap();
    assert_eq!(result["photos"][0]["is_current"], true);
    assert_eq!(result["photos"][1]["is_current"], false);
    assert_eq!(w.calls()[0].1["user_id"]["_"], "inputUser");
    assert_eq!(w.calls()[0].1["limit"], 20);
}
#[tokio::test]
async fn chat_avatar_history_falls_back_to_full_chat_when_search_is_empty() {
    for channel in [false, true] {
        let responses = vec![
            ("messages.search", json!({"messages":[]})),
            (
                if channel {
                    "channels.getFullChannel"
                } else {
                    "messages.getFullChat"
                },
                json!({"full_chat":{"chat_photo":photo(9)}}),
            ),
        ];
        let base = MediaWire::new(responses, vec![]);
        let w = if channel { base.channel() } else { base.chat() };
        let result = run(&w, "list_photos", json!({"chat_id":7,"source":"avatars"}))
            .await
            .unwrap();
        assert_eq!(result["photos"][0]["id"], "9");
        assert_eq!(
            w.calls()[0].1["filter"]["_"],
            "inputMessagesFilterChatPhotos"
        );
        assert!(w.calls()[0].1.get("offset_date").is_none());
    }
}
#[tokio::test]
async fn message_photo_listing_reads_media_without_labeling_them_current_avatars() {
    let w = MediaWire::new(
        vec![(
            "messages.search",
            json!({"messages":[{"media":{"photo":photo(4)}},{"action":{"photo":photo(5)}}]}),
        )],
        vec![],
    );
    let result = run(
        &w,
        "list_photos",
        json!({"chat_id":7,"source":" MESSAGES "}),
    )
    .await
    .unwrap();
    assert_eq!(result["photos"].as_array().unwrap().len(), 2);
    assert_eq!(result["photos"][0]["is_current"], false);
    assert_eq!(w.calls()[0].1["filter"]["_"], "inputMessagesFilterPhotos");
}
#[tokio::test]
async fn open_photo_selects_requested_id_and_returns_actual_pixels_from_memory() {
    let w = MediaWire::new(
        vec![(
            "photos.getUserPhotos",
            json!({"photos":[photo(4),photo(5)]}),
        )],
        png(),
    );
    let result = run(
        &w,
        "open_photo",
        json!({"chat_id":7,"photo_id":5,"max_dimension":64,"max_bytes":4096}),
    )
    .await
    .unwrap();
    assert_eq!(result["metadata"]["photo_id"], "5");
    assert_eq!(result["content"][0]["mimeType"], "image/png");
    assert_eq!(w.downloads.lock().unwrap()[0].0["id"], "5");
    assert_eq!(w.downloads.lock().unwrap()[0].1, 4096);
}
#[tokio::test]
async fn photo_sheet_combines_real_images_and_preserves_photo_order() {
    let w = MediaWire::new(
        vec![(
            "photos.getUserPhotos",
            json!({"photos":[photo(4),photo(5)]}),
        )],
        png(),
    );
    let result = run(&w, "get_photo_sheet", json!({"chat_id":7,"columns":2}))
        .await
        .unwrap();
    assert_eq!(result["metadata"]["photo_ids"], json!(["4", "5"]));
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(w.downloads.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn photo_selection_bounds_and_missing_results_never_download_another_photo() {
    let w = MediaWire::new(vec![], vec![]);
    assert!(
        run(&w, "list_photos", json!({"chat_id":7,"source":"invalid"}))
            .await
            .is_err()
    );
    assert!(w.calls().is_empty());
    for (name, args, photos) in [
        (
            "open_photo",
            json!({"chat_id":7,"photo_id":"99"}),
            vec![photo(4)],
        ),
        ("get_photo_sheet", json!({"chat_id":7}), vec![]),
        (
            "open_photo",
            json!({"chat_id":7,"max_bytes":0}),
            vec![photo(4)],
        ),
        (
            "get_photo_sheet",
            json!({"chat_id":7,"columns":25}),
            vec![photo(4)],
        ),
    ] {
        let w = MediaWire::new(
            vec![("photos.getUserPhotos", json!({"photos":photos}))],
            png(),
        );
        assert!(run(&w, name, args).await.is_err());
        if name == "open_photo" {
            assert!(w.downloads.lock().unwrap().is_empty());
        }
    }
}
