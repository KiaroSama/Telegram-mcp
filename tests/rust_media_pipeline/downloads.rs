use crate::{local_support, support::*};
use serde_json::{Value, json};
use telegram_mcp::tools::messaging::media::downloads;
async fn save(
    w: &MediaWire,
    s: &telegram_mcp::state::State,
    name: &str,
    a: Value,
) -> anyhow::Result<Value> {
    downloads::run_with_state(w, s, name, &a).await
}
#[tokio::test]
async fn ordinary_download_uses_the_exact_document_dc_reference_and_writes_requested_path() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("saved.bin");
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"_":"messageMediaDocument","document":document(99,"file.bin",4)})),
        )],
        b"data".to_vec(),
    );
    let result = save(
        &w,
        &state,
        "download_media",
        json!({"chat_id":7,"message_id":8,"file_path":out,"max_bytes":4}),
    )
    .await
    .unwrap();
    assert_eq!(result["path"], out.to_string_lossy().as_ref());
    assert_eq!(std::fs::read(&out).unwrap(), b"data");
    let calls = w.downloads.lock().unwrap();
    assert_eq!(calls[0].0["_"], "inputDocumentFileLocation");
    assert_eq!(calls[0].0["id"], "99");
    assert_eq!(calls[0].0["_dc_id"], 2);
    assert_eq!(calls[0].1, 4);
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn incoming_filenames_are_sanitized_inside_the_download_directory() {
    let (_dir, state) = local_support::fixture();
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":document(99,"../bad:name?.bin",1)})),
        )],
        vec![1],
    );
    let result = save(
        &w,
        &state,
        "download_media",
        json!({"chat_id":7,"message_id":8}),
    )
    .await
    .unwrap();
    let path = std::path::Path::new(result["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "_bad_name_.bin");
    assert_eq!(
        path.parent().unwrap(),
        state.file_roots()[0].join("downloads")
    );
    assert!(path.is_file());
}
#[tokio::test]
async fn photo_download_into_a_directory_uses_an_image_suffix_and_largest_size() {
    let (_dir, state) = local_support::fixture();
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"photo":photo(88)})),
        )],
        png(),
    );
    let result = save(
        &w,
        &state,
        "download_media",
        json!({"chat_id":7,"message_id":8,"file_path":state.file_roots()[0]}),
    )
    .await
    .unwrap();
    let path = std::path::Path::new(result["path"].as_str().unwrap());
    assert_eq!(path.extension().unwrap(), "jpg");
    assert_eq!(w.downloads.lock().unwrap()[0].0["thumb_size"], "x");
    assert_eq!(std::fs::read(path).unwrap(), png());
}
#[tokio::test]
async fn declared_and_actual_download_sizes_are_both_enforced_without_publication() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("large.bin");
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":document(3,"large.bin",5)})),
        )],
        b"12345".to_vec(),
    );
    assert!(
        save(
            &w,
            &state,
            "download_media",
            json!({"chat_id":7,"message_id":8,"file_path":out,"max_bytes":4})
        )
        .await
        .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
    assert!(!out.exists());
    let mut doc = document(3, "large.bin", 0);
    doc.as_object_mut().unwrap().remove("size");
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":doc})),
        )],
        b"12345".to_vec(),
    );
    assert!(
        save(
            &w,
            &state,
            "download_media",
            json!({"chat_id":7,"message_id":8,"file_path":out,"max_bytes":4})
        )
        .await
        .is_err()
    );
    assert!(!out.exists());
}
#[tokio::test]
async fn download_refuses_existing_files_and_paths_outside_allowed_roots() {
    let (dir, state) = local_support::fixture();
    let existing = state.file_roots()[0].join("exists.bin");
    std::fs::write(&existing, b"original").unwrap();
    for path in [existing.clone(), dir.path().join("outside.bin")] {
        let w = MediaWire::new(
            vec![(
                "messages.getMessages",
                media_message(json!({"document":document(3,"a.bin",1)})),
            )],
            vec![1],
        );
        assert!(
            save(
                &w,
                &state,
                "download_media",
                json!({"chat_id":7,"message_id":8,"file_path":path})
            )
            .await
            .is_err()
        );
        assert!(w.downloads.lock().unwrap().is_empty());
    }
    assert_eq!(std::fs::read(existing).unwrap(), b"original");
}
#[tokio::test]
async fn disappeared_or_non_media_messages_do_not_create_a_download() {
    let (_dir, state) = local_support::fixture();
    for response in [
        json!({"messages":[]}),
        json!({"messages":[{"_":"message","id":8,"message":"plain"}]}),
    ] {
        let w = MediaWire::new(vec![("messages.getMessages", response)], vec![1]);
        assert!(
            save(
                &w,
                &state,
                "download_media",
                json!({"chat_id":7,"message_id":8})
            )
            .await
            .is_err()
        );
        assert!(w.downloads.lock().unwrap().is_empty());
    }
    for cap in [0, 512 * 1024 * 1024 + 1] {
        let w = MediaWire::new(vec![], vec![]);
        assert!(
            save(
                &w,
                &state,
                "download_media",
                json!({"chat_id":7,"message_id":8,"max_bytes":cap})
            )
            .await
            .is_err()
        );
        assert!(w.calls().is_empty());
    }
}
#[tokio::test]
async fn story_download_selects_the_requested_story_without_marking_it_viewed() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("story.bin");
    let w = MediaWire::new(
        vec![(
            "stories.getStoriesByID",
            json!({"stories":[{"id":9,"media":{"photo":photo(4)}},{"id":8,"media":{"document":document(3,"story.bin",1)}}]}),
        )],
        vec![8],
    );
    save(
        &w,
        &state,
        "download_media",
        json!({"chat_id":7,"story_id":8,"file_path":out}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["id"], "3");
    assert_eq!(w.calls()[0].1["id"], json!([8]));
    assert_eq!(w.calls().len(), 1);
    let w = MediaWire::new(
        vec![("stories.getStoriesByID", json!({"stories":[]}))],
        vec![],
    );
    assert!(
        save(
            &w,
            &state,
            "download_media",
            json!({"chat_id":7,"story_id":8})
        )
        .await
        .is_err()
    );
}
fn rich(blocks: Value) -> Value {
    json!({"messages":[{"id":8,"rich_message":{"blocks":blocks,"photos":[photo(40)],"documents":[document(50,"rich.bin",1)]}}]})
}
#[tokio::test]
async fn rich_media_requires_unambiguous_block_selection_and_exact_reference() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("rich.bin");
    let blocks = json!([{"photo_id":"40"},{"audio_id":"50"}]);
    let w = MediaWire::new(
        vec![("messages.getRichMessage", rich(blocks.clone()))],
        vec![1],
    );
    assert!(
        save(
            &w,
            &state,
            "download_rich_media",
            json!({"chat_id":7,"message_id":8,"file_path":out})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("block_index")
    );
    let w = MediaWire::new(vec![("messages.getRichMessage", rich(blocks))], vec![1]);
    save(
        &w,
        &state,
        "download_rich_media",
        json!({"chat_id":7,"message_id":8,"block_index":1,"file_path":out}),
    )
    .await
    .unwrap();
    assert_eq!(w.downloads.lock().unwrap()[0].0["id"], "50");
    let w = MediaWire::new(
        vec![("messages.getRichMessage", rich(json!([{"photo_id":"40"}])))],
        png(),
    );
    save(
        &w,
        &state,
        "download_rich_media",
        json!({"chat_id":7,"message_id":8,"file_path":out.with_extension("png")}),
    )
    .await
    .unwrap();
    assert_eq!(
        w.downloads.lock().unwrap()[0].0["_"],
        "inputPhotoFileLocation"
    );
}
#[tokio::test]
async fn malformed_rich_blocks_are_refused_before_any_media_transfer() {
    let (_dir, state) = local_support::fixture();
    for (response, index) in [
        (json!({"messages":[{"id":8}]}), None),
        (rich(json!([{"text":"plain"}])), None),
        (rich(json!([{"photo_id":"999"}])), None),
        (rich(json!([{"photo_id":"40"}])), Some(8)),
        (rich(json!([{"photo_id":"40"}])), Some(-1)),
    ] {
        let w = MediaWire::new(vec![("messages.getRichMessage", response)], vec![]);
        assert!(
            save(
                &w,
                &state,
                "download_rich_media",
                json!({"chat_id":7,"message_id":8,"block_index":index})
            )
            .await
            .is_err()
        );
        assert!(w.downloads.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn disappearing_download_never_issues_a_read_or_listened_acknowledgement() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("once.png");
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"photo":photo(3),"ttl_seconds":30})),
        )],
        png(),
    );
    let result = save(
        &w,
        &state,
        "save_disappearing_media",
        json!({"chat_id":7,"message_id":8,"file_path":out}),
    )
    .await
    .unwrap();
    assert_eq!(result["content"][0]["type"], "image");
    assert_eq!(w.calls().len(), 1);
    assert!(out.is_file());
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"document":document(3,"ordinary.bin",1)})),
        )],
        vec![1],
    );
    assert!(
        save(
            &w,
            &state,
            "save_disappearing_media",
            json!({"chat_id":7,"message_id":8})
        )
        .await
        .is_err()
    );
    assert!(w.downloads.lock().unwrap().is_empty());
}
#[tokio::test]
async fn a_failed_optional_preview_preserves_a_successful_disappearing_save() {
    let (_dir, state) = local_support::fixture();
    let out = state.file_roots()[0].join("once.png");
    let w = MediaWire::new(
        vec![(
            "messages.getMessages",
            media_message(json!({"photo":photo(3),"ttl_seconds":1})),
        )],
        png(),
    );
    let result = save(
        &w,
        &state,
        "save_disappearing_media",
        json!({"chat_id":7,"message_id":8,"file_path":out,"max_dimension":"invalid-integer"}),
    )
    .await
    .unwrap();
    assert!(result["preview_error"].is_string());
    assert_eq!(std::fs::read(out).unwrap(), png());
}
