use super::*;

#[test]
fn photo_export_checks_progressive_sizes_then_downloads_largest_area_with_the_right_dc() {
    if isolated(
        "media_edges::photo_export_checks_progressive_sizes_then_downloads_largest_area_with_the_right_dc",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async{
        let api=Fixture::default();let mut options=settings("photo");options.media[0]=true;options.max_bytes=100;
        let photo=json!({"id":3,"out":false,"media":{"photo":{"_":"photo","id":"9007199254740993","access_hash":"7","file_reference":[],"dc_id":4,"sizes":[{"_":"photoSize","type":"s","w":16,"h":16,"size":50},{"_":"photoSizeProgressive","type":"x","w":200,"h":100,"sizes":[100,500,1000]}]}}});
        let mut first=photo.clone();assert!(!media::download_media(&api,&mut first,&options,&options.path).await.unwrap());
        assert_eq!(first["export_file_skipped"],"file exceeds size_limit_mb");assert!(api.requests.lock().unwrap().is_empty());
        options.max_bytes=2000;let mut second=photo;assert!(media::download_media(&api,&mut second,&options,&options.path).await.unwrap());
        let calls=api.requests.lock().unwrap();let location=&calls.last().unwrap().1["location"];
        assert_eq!(location["_"],"inputPhotoFileLocation");assert_eq!(location["thumb_size"],"x");assert_eq!(location["_dc_id"],4);
        assert!(options.path.join(second["export_file"].as_str().unwrap()).is_file());
    });
}

#[test]
fn selected_attachment_categories_preserve_distinct_folders_and_do_not_require_other_media_flags() {
    if isolated(
        "media_edges::selected_attachment_categories_preserve_distinct_folders_and_do_not_require_other_media_flags",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let cases = [
            (
                json!({"_":"documentAttributeVideo","round_message":false}),
                1,
                "video_files",
            ),
            (
                json!({"_":"documentAttributeAudio","voice":true}),
                2,
                "voice_messages",
            ),
            (
                json!({"_":"documentAttributeVideo","round_message":true}),
                3,
                "round_video_messages",
            ),
            (json!({"_":"documentAttributeSticker"}), 4, "stickers"),
            (json!({"_":"documentAttributeAnimated"}), 5, "animations"),
        ];
        for (attribute, index, folder) in cases {
            let mut options = settings(&format!("category-{index}"));
            options.media[index] = true;
            let mut item = document_message(index as u64);
            item["media"]["document"]["attributes"] =
                json!([attribute,{"_":"documentAttributeFilename","file_name":"attachment.bin"}]);
            assert!(
                media::download_media(&api, &mut item, &options, &options.path)
                    .await
                    .unwrap()
            );
            assert!(
                item["export_file"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("{folder}/"))
            );
        }
    });
}

#[test]
fn outgoing_ttl_files_are_exportable_but_unsupported_media_never_creates_fake_files() {
    if isolated(
        "media_edges::outgoing_ttl_files_are_exportable_but_unsupported_media_never_creates_fake_files",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut options = settings("ttl");
        options.media[6] = true;
        let mut outgoing = document_message(7);
        outgoing["out"] = true.into();
        outgoing["media"]["ttl_seconds"] = json!(10);
        assert!(
            media::download_media(&api, &mut outgoing, &options, &options.path)
                .await
                .unwrap()
        );
        for mut item in [
            json!({"id":8}),
            json!({"id":9,"media":{"_":"messageMediaUnsupported"}}),
        ] {
            assert!(
                !media::download_media(&api, &mut item, &options, &options.path)
                    .await
                    .unwrap()
            );
            assert!(item.get("export_file").is_none());
        }
    });
}

#[test]
fn unavailable_custom_emoji_is_recorded_once_and_never_retried_for_each_message() {
    if isolated(
        "media_edges::unavailable_custom_emoji_is_recorded_once_and_never_retried_for_each_message",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async{
        let api=Fixture{fail:true,..Default::default()};let mut options=settings("missing-emoji");options.media[4]=true;let mut cache=std::collections::BTreeMap::new();
        let make=|id|json!({"id":id,"entities":[{"_":"messageEntityCustomEmoji","document_id":"9007199254740993"},{"_":"messageEntityCustomEmoji","document_id":"9007199254740993"},{"_":"messageEntityCustomEmoji","document_id":"invalid"}]});
        let mut first=make(1);assert_eq!(media::hydrate_custom_emoji(&api,&mut first,&options,&options.path,&mut cache).await.unwrap(),0);
        let mut second=make(2);assert_eq!(media::hydrate_custom_emoji(&api,&mut second,&options,&options.path,&mut cache).await.unwrap(),0);
        assert_eq!(first["custom_emoji_unavailable"],json!(["9007199254740993"]));assert_eq!(second["custom_emoji_unavailable"],first["custom_emoji_unavailable"]);
        assert_eq!(api.requests.lock().unwrap().len(),1);assert!(!options.path.exists());
    });
}

#[test]
fn rich_media_deduplicates_repeated_assets_and_keeps_successful_downloads_when_another_is_unselected()
 {
    if isolated(
        "media_edges::rich_media_deduplicates_repeated_assets_and_keeps_successful_downloads_when_another_is_unselected",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async{
        let api=Fixture::default();let mut options=settings("rich-media");options.media[6]=true;
        let doc=document_message(3)["media"]["document"].clone();
        let mut item=json!({"id":3,"rich_message":{"documents":[doc.clone(),doc],"photos":[{"_":"photo","id":"99","sizes":[]}]}});
        assert_eq!(media::download_rich_media(&api,&mut item,&options,&options.path).await.unwrap(),1);
        assert_eq!(item["rich_files"].as_object().unwrap().len(),1);assert_eq!(api.requests.lock().unwrap().len(),1);
    });
}
