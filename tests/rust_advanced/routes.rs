//! Requests are asserted against public TL contracts and encoded by the mock.
use super::common::*;
fn standard() -> Mock {
    Mock::new()
        .forum()
        .respond("messages.getDialogFilters", folder(json!([]), json!([])))
        .respond("messages.getDialogFilters", folder(json!([]), json!([])))
        .respond(
            "messages.getQuickReplies",
            json!({"quick_replies":[{"shortcut_id":5,"shortcut":"hello","count":1}]}),
        )
        .respond(
            "messages.getQuickReplyMessages",
            json!({"messages":[{"id":10}]}),
        )
        .respond("messages.getStickerSet", json!({"set":{"count":1}}))
        .respond(
            "stickers.suggestShortName",
            json!({"short_name":"test_pack"}),
        )
        .respond("channels.getMessages", {
            let mut p = poll();
            p["open_answers"] = json!(true);
            message(p)
        })
        .respond(
            "channels.exportMessageLink",
            json!({"link":"https://t.me/c/7/20"}),
        )
        .respond(
            "stories.getStoriesByID",
            json!({"stories":[{"_":"storyItem","id":2}]}),
        )
        .respond(
            "users.getFullUser",
            json!({"full_user":{"business_intro":{"title":"Hello"}}}),
        )
}
macro_rules! request {
    ($name:ident,$tool:expr,$args:tt,$method:expr,$expected:tt) => {
        #[tokio::test]
        async fn $name() {
            let m = standard();
            run(&m, $tool, json!($args)).await.unwrap();
            let actual = m.request($method);
            assert!(actual.is_object(), "Expected {} for {}", $method, $tool);
            for (key, value) in json!($expected).as_object().unwrap() {
                assert_eq!(&actual[key], value, "{} argument {key}", $method);
            }
        }
    };
}
request!(topic_creation_uses_live_peer_route,"create_forum_topic",{"chat_id":7,"title":"News","icon_color":123},"messages.createForumTopic",{"title":"News","icon_color":123,"peer":{"_":"inputPeerChannel","channel_id":"7","access_hash":"99"}});
request!(topic_icon_catalogue_uses_default_topic_icon_set,"list_forum_topic_icons",{},"messages.getStickerSet",{"stickerset":{"_":"inputStickerSetEmojiDefaultTopicIcons"},"hash":0});
request!(
    folder_list_uses_filters_query,
    "list_folders",
    {},
    "messages.getDialogFilters",
    {}
);
request!(folder_get_looks_up_existing_filters,"get_folder",{"folder_id":2},"messages.getDialogFilters",{});
request!(folder_creation_allocates_unused_id,"create_folder",{"title":"New"},"messages.updateDialogFilter",{"id":3});
request!(folder_reorder_sends_complete_order,"reorder_folders",{"folder_ids":[2]},"messages.updateDialogFiltersOrder",{"order":[2]});
request!(pinned_chat_order_uses_dialog_peers,"reorder_pinned_chats",{"order":[7],"folder_id":0,"force":true},"messages.reorderPinnedDialogs",{"folder_id":0,"force":true,"order":[{"_":"inputDialogPeer","peer":{"_":"inputPeerChannel","channel_id":"7","access_hash":"99"}}]});
request!(saved_dialog_list_is_zero_hash_query,"list_saved_dialogs",{},"messages.getSavedDialogs",{"offset_date":0,"offset_id":0,"offset_peer":{"_":"inputPeerSelf"},"limit":50,"hash":0});
request!(saved_history_addresses_sender_bucket,"get_saved_history",{"peer_id":7,"limit":20},"messages.getSavedHistory",{"peer":{"_":"inputPeerChannel","channel_id":"7","access_hash":"99"},"limit":20,"add_offset":0,"max_id":0,"min_id":0});
request!(saved_tag_list_is_zero_hash_query,"list_saved_tags",{},"messages.getSavedReactionTags",{"hash":0});
request!(saved_tag_name_uses_custom_document_id,"name_saved_tag",{"custom_emoji_id":"1234567890123456789","title":"Work"},"messages.updateSavedReactionTag",{"reaction":{"_":"reactionCustomEmoji","document_id":"1234567890123456789"},"title":"Work"});
request!(quick_reply_catalogue_is_zero_hash_query,"list_quick_replies",{},"messages.getQuickReplies",{"hash":0});
request!(quick_reply_read_uses_shortcut_id,"read_quick_reply",{"shortcut_id":5},"messages.getQuickReplyMessages",{"shortcut_id":5,"hash":0});
request!(quick_reply_send_resolves_all_stored_message_ids,"send_quick_reply",{"chat_id":7,"shortcut_id":5},"messages.sendQuickReplyMessages",{"shortcut_id":5,"id":[10]});
request!(quick_reply_rename_keeps_shortcut_id,"rename_quick_reply",{"shortcut_id":5,"shortcut":"/renamed"},"messages.editQuickReplyShortcut",{"shortcut_id":5,"shortcut":"renamed"});
request!(sticker_set_lookup_uses_named_reference,"inspect_sticker_set",{"short_name":"test_pack"},"messages.getStickerSet",{"stickerset":{"_":"inputStickerSetShortName","short_name":"test_pack"},"hash":0});
request!(sticker_set_numeric_reference_preserves_long_ids,"inspect_sticker_set",{"set_id":"1234567890123456789","access_hash":"987654321098765432"},"messages.getStickerSet",{"stickerset":{"_":"inputStickerSetID","id":"1234567890123456789","access_hash":"987654321098765432"}});
request!(suggested_sticker_name_is_checked_before_reporting,"suggest_sticker_set_name",{"title":"Test pack"},"stickers.checkShortName",{"short_name":"test_pack"});
request!(adding_sticker_carries_emoji_and_reference,"add_sticker_to_set",{"short_name":"test_pack","document_id":"123","access_hash":"456","emoji":"❤"},"stickers.addStickerToSet",{"sticker":{"_":"inputStickerSetItem","document":{"_":"inputDocument","id":"123","access_hash":"456","file_reference":[]},"emoji":"❤"}});
request!(sticker_removal_uses_input_document,"remove_sticker_from_set",{"document_id":"123","access_hash":"456"},"stickers.removeStickerFromSet",{"sticker":{"_":"inputDocument","id":"123","access_hash":"456","file_reference":[]}});
request!(sticker_position_is_zero_based,"move_sticker_in_set",{"document_id":"123","access_hash":"456","position":2},"stickers.changeStickerPosition",{"position":2});
request!(install_sticker_preserves_archive_choice,"install_sticker_set",{"short_name":"test_pack","archived":true},"messages.installStickerSet",{"archived":true});
request!(uninstall_sticker_uses_selected_set,"uninstall_sticker_set",{"short_name":"test_pack"},"messages.uninstallStickerSet",{"stickerset":{"_":"inputStickerSetShortName","short_name":"test_pack"}});
request!(sticker_catalogue_uses_sticker_route,"get_sticker_sets",{"kind":"stickers"},"messages.getAllStickers",{"hash":0});
request!(mask_catalogue_uses_mask_route,"get_sticker_sets",{"kind":"masks"},"messages.getMaskStickers",{"hash":0});
request!(emoji_catalogue_uses_emoji_route,"get_sticker_sets",{"kind":"emoji"},"messages.getEmojiStickers",{"hash":0});
request!(saved_gif_list_reads_account_collection,"list_saved_gifs",{},"messages.getSavedGifs",{"hash":0});
request!(closed_poll_edit_preserves_original_poll,"close_poll",{"chat_id":7,"message_id":20},"messages.editMessage",{"id":20});
request!(added_poll_option_is_formatted_input_answer,"add_poll_option",{"chat_id":7,"message_id":20,"text":"C"},"messages.addPollAnswer",{"msg_id":20,"answer":{"_":"inputPollAnswer","text":{"_":"textWithEntities","text":"C","entities":[]}}});
request!(deleted_poll_option_uses_live_option_bytes,"delete_poll_option",{"chat_id":7,"message_id":20,"option_index":1},"messages.deletePollAnswer",{"msg_id":20,"option":[241,2]});
request!(unread_poll_votes_have_bounded_query,"list_unread_poll_votes",{"chat_id":7,"limit":9},"messages.getUnreadPollVotes",{"offset_id":0,"add_offset":0,"limit":9,"max_id":0,"min_id":0});
request!(poll_vote_mark_read_is_explicit_mutation,"mark_poll_votes_read",{"chat_id":7},"messages.readPollVotes",{"peer":{"_":"inputPeerChannel","channel_id":"7","access_hash":"99"}});
request!(poll_part_reply_addresses_option_not_whole_message,"reply_to_part",{"chat_id":7,"message_id":20,"option_index":1,"text":"About B"},"messages.sendMessage",{"message":"About B","reply_to":{"_":"inputReplyToMessage","reply_to_msg_id":20,"poll_option":[241,2]}});
request!(part_link_uses_channel_message_export,"get_part_link",{"chat_id":7,"message_id":20,"option_index":1},"channels.exportMessageLink",{"id":20,"channel":{"_":"inputChannel","channel_id":"7","access_hash":"99"}});
request!(get_stories_queries_explicit_ids,"get_stories",{"chat_id":7,"story_ids":[2]},"stories.getStoriesByID",{"id":[2]});
request!(story_delete_accepts_multiple_explicit_ids,"delete_story",{"chat_id":7,"story_id":[2,4]},"stories.deleteStories",{"id":[2,4]});
request!(business_read_uses_full_self_user,"get_business_settings",{},"users.getFullUser",{"id":{"_":"inputUserSelf"}});
request!(away_message_refers_to_stored_shortcut,"set_business_away_message",{"shortcut_id":5,"offline_only":true},"account.updateBusinessAwayMessage",{"message":{"_":"inputBusinessAwayMessage","shortcut_id":5,"schedule":{"_":"businessAwayMessageScheduleAlways"},"recipients":{"_":"inputBusinessRecipients","existing_chats":true,"new_chats":true,"contacts":true,"non_contacts":true},"offline_only":true}});
request!(outside_hours_away_schedule_uses_dedicated_constructor,"set_business_away_message",{"shortcut_id":5,"schedule":"outside_hours","recipients":["contacts"]},"account.updateBusinessAwayMessage",{"message":{"_":"inputBusinessAwayMessage","shortcut_id":5,"schedule":{"_":"businessAwayMessageScheduleOutsideWorkHours"},"recipients":{"_":"inputBusinessRecipients","contacts":true},"offline_only":false}});
request!(business_greeting_preserves_selected_recipient_group,"set_business_greeting",{"shortcut_id":5,"no_activity_days":14,"recipients":["new_chats"]},"account.updateBusinessGreetingMessage",{"message":{"_":"inputBusinessGreetingMessage","shortcut_id":5,"recipients":{"_":"inputBusinessRecipients","new_chats":true},"no_activity_days":14}});
request!(business_intro_carries_optional_sticker_reference,"set_business_intro",{"title":"Hello","description":"Welcome","sticker_document_id":"123","sticker_access_hash":"456"},"account.updateBusinessIntro",{"intro":{"_":"inputBusinessIntro","title":"Hello","description":"Welcome","sticker":{"_":"inputDocument","id":"123","access_hash":"456","file_reference":[]}}});
#[tokio::test]
async fn clearing_business_intro_omits_intro() {
    let m = standard();
    run(&m, "set_business_intro", json!({"title":null}))
        .await
        .unwrap();
    assert_eq!(m.request("account.updateBusinessIntro"), json!({}));
}
request!(business_location_address_only_has_no_geo_point,"set_business_location",{"address":"Office"},"account.updateBusinessLocation",{"address":"Office"});
#[tokio::test]
async fn clearing_business_location_omits_address_and_geo() {
    let m = standard();
    run(&m, "set_business_location", json!({"address":null}))
        .await
        .unwrap();
    assert_eq!(m.request("account.updateBusinessLocation"), json!({}));
}
request!(
    business_link_list_reads_all_links,
    "list_business_chat_links",
    {},
    "account.getBusinessChatLinks",
    {}
);
request!(business_link_creation_preserves_text_and_title,"create_business_chat_link",{"text":"Hello","title":"Sales"},"account.createBusinessChatLink",{"link":{"_":"inputBusinessChatLink","message":"Hello","title":"Sales"}});
request!(business_link_edit_extracts_slug_and_keeps_new_text,"edit_business_chat_link",{"slug":"https://t.me/m/example","text":"Updated"},"account.editBusinessChatLink",{"slug":"example","link":{"_":"inputBusinessChatLink","message":"Updated"}});
request!(business_link_resolve_calls_visible_view_counter_route,"resolve_business_chat_link",{"slug":"example"},"account.resolveBusinessChatLink",{"slug":"example"});
request!(fact_check_query_sends_message_ids,"get_fact_check",{"chat_id":7,"message_ids":[20,21]},"messages.getFactCheck",{"msg_id":[20,21]});
request!(fact_check_edit_uses_text_with_entities,"set_fact_check",{"chat_id":7,"message_id":20,"text":"Context"},"messages.editFactCheck",{"msg_id":20,"text":{"_":"textWithEntities","text":"Context","entities":[]}});
request!(fact_check_deletion_targets_exact_message,"delete_fact_check",{"chat_id":7,"message_id":20},"messages.deleteFactCheck",{"msg_id":20});
