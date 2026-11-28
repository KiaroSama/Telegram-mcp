# Native tool catalogue

The catalogue lists 391 native tools: 356 cloud tools and 35 [secret-chat tools](#secret-chat-tools).
MCP `tools/list` returns the schema for tools available to the client after exposure filters.
Use that schema for optional fields and defaults. Account access, owner approvals,
Telegram permissions and file-root restrictions apply regardless of tool annotations.

## CLI entry points

| Command | Purpose |
| --- | --- |
| `telegram-mcp serve` | MCP over stdio |
| `telegram-mcp --config PATH ...` | Explicit private configuration for headless clients |
| `telegram-mcp login --env-file PATH --account LABEL` | Staged login bound to its selected configuration |
| `telegram-mcp session-validate` | Validate a legacy authorization read from stdin without network |
| `telegram-mcp session-health --env-file PATH --key KEY --json` | Check an exact configured source via the shared service |
| `telegram-mcp rename-account --from LABEL --to LABEL` | Recoverable private-state rename |
| `telegram-mcp rename-account --from LABEL --to LABEL --env-file FILE --key KEY` | Rename the exact configuration key and private state in one recoverable transaction |
| `telegram-mcp remove-account --account LABEL --env-file FILE --key KEY` | Archive private account records and remove the exact file-owned configuration key |
| `telegram-mcp serve --http 127.0.0.1:8765` | Authenticated shared `/mcp` service |
| `telegram-mcp bridge` | Attach a stdio client to the shared service |
| `telegram-mcp login --account LABEL` | Interactive native authorization |
| `telegram-mcp accounts --json` | Credential-free account inventory |
| `telegram-mcp health --json` | Local readiness evidence |
| `telegram-mcp state-path` | Actual private state directory |
| `telegram-mcp bind-address` | Resolved shared-service address from the selected configuration |
| `telegram-mcp approve FINGERPRINT` | Separate human-terminal approval |
| `emoji-studio sheet`, `gif`, `index`, `refresh`, `nearest` | Actual emoji images, animation and visual search |
| `emoji-studio lines`, `insert` | Inspect and compose messages through authenticated MCP |
| `emoji-studio accuracy --catalog PATH` | Read-only local accuracy measurement |
| `operator-plugin --source PATH install ...` | Explicit standalone skill copy |
| `operator-plugin --source PATH package ...` | Allowlisted guidance-only ZIP |

Check the installed binary's `--help` for CLI arguments. The
[`telegram-operator` plugin](../plugin.json) installs skills only. [Configure the MCP server separately](../README.md#connect-an-mcp-client).

## MCP tools

### Advanced

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `add_chat_to_folder` | `folder_id`, `chat_id` | No |
| `add_checklist_tasks` | `chat_id`, `message_id`, `tasks` | No |
| `add_poll_option` | `chat_id`, `message_id`, `text` | No |
| `add_quick_reply` | `shortcut`, `text` | No |
| `add_sticker_to_set` | `short_name`, `document_id`, `emoji` | No |
| `analyze_chat_statistics` | `chat_id` | Yes |
| `close_poll` | `chat_id`, `message_id` | No |
| `create_business_chat_link` | `text` | No |
| `create_folder` | `title` | No |
| `create_forum_topic` | `chat_id`, `title` | No |
| `create_poll` | `chat_id`, `question`, `options` | No |
| `delete_business_chat_link` | `slug` | No |
| `delete_fact_check` | `chat_id`, `message_id` | No |
| `delete_folder` | `folder_id` | No |
| `delete_forum_topic` | `chat_id`, `topic_id` | No |
| `delete_poll_option` | `chat_id`, `message_id`, `option_index` | No |
| `delete_quick_reply` | `shortcut_id` | No |
| `delete_story` | `story_id` | No |
| `disable_forum_topics` | `chat_id` | No |
| `edit_business_chat_link` | `slug`, `text` | No |
| `edit_forum_topic` | `chat_id`, `topic_id` | No |
| `edit_quick_reply` | `shortcut_id`, `message_id`, `message` | No |
| `enable_forum_topics` | `chat_id` | No |
| `get_business_settings` | None | Yes |
| `get_channel_statistics` | `chat_id` | Yes |
| `get_chatlist_invite` | `link` | Yes |
| `get_checklist` | `chat_id`, `message_id` | Yes |
| `get_custom_emoji_pack` | `pack` | Yes |
| `get_custom_emoji_packs` | None | Yes |
| `get_fact_check` | `chat_id`, `message_ids` | Yes |
| `get_folder` | `folder_id` | Yes |
| `get_folder_limits` | None | Yes |
| `get_folder_snapshot` | None | Yes |
| `get_gif_search` | `query` | Yes |
| `get_message_effect` | `effect_id` | Yes |
| `get_part_link` | `chat_id`, `message_id` | Yes |
| `get_poll_results` | `chat_id`, `message_id` | Yes |
| `get_poll_statistics` | `chat_id`, `message_id` | Yes |
| `get_poll_voters` | `chat_id`, `message_id` | Yes |
| `get_saved_history` | `peer_id` | Yes |
| `get_stars_receipt` | `message_id` | Yes |
| `get_sticker_sets` | None | Yes |
| `get_stories` | `chat_id`, `story_ids` | Yes |
| `inspect_sticker_set` | None | Yes |
| `install_sticker_set` | None | No |
| `list_business_chat_links` | None | Yes |
| `list_folders` | None | Yes |
| `list_forum_topic_icons` | None | Yes |
| `list_message_effects` | None | Yes |
| `list_peer_stories` | `chat_id` | Yes |
| `list_quick_replies` | None | Yes |
| `list_saved_dialogs` | None | Yes |
| `list_saved_gifs` | None | Yes |
| `list_saved_tags` | None | Yes |
| `list_topics` | `chat_id` | Yes |
| `list_unread_poll_votes` | `chat_id` | Yes |
| `mark_poll_votes_read` | `chat_id` | No |
| `move_sticker_in_set` | `document_id`, `position` | No |
| `name_saved_tag` | None | No |
| `open_mini_app` | `bot` | No |
| `pin_forum_topic` | `chat_id`, `topic_id` | No |
| `post_story` | `file_path`, `privacy` | No |
| `preview_custom_emoji` | `document_id` | Yes |
| `preview_story` | `chat_id`, `story_id` | Yes |
| `react_to_story` | `chat_id`, `story_id` | No |
| `read_quick_reply` | `shortcut_id` | Yes |
| `remove_chat_from_folder` | `folder_id`, `chat_id` | No |
| `remove_checklist_tasks` | `chat_id`, `message_id`, `task_ids` | No |
| `remove_sticker_from_set` | `document_id` | No |
| `rename_quick_reply` | `shortcut_id`, `shortcut` | No |
| `reorder_folders` | `folder_ids` | No |
| `reorder_pinned_chats` | `order` | No |
| `reorder_pinned_topics` | `chat_id`, `topic_ids` | No |
| `reply_to_part` | `chat_id`, `message_id`, `text` | No |
| `resolve_business_chat_link` | `slug` | No |
| `save_gif` | `chat_id`, `message_id` | No |
| `send_checklist` | `chat_id`, `title`, `tasks` | No |
| `send_gif` | `chat_id`, `gif_id` | No |
| `send_quick_reply` | `chat_id`, `shortcut_id` | No |
| `set_business_away_message` | `shortcut_id` | No |
| `set_business_greeting` | `shortcut_id` | No |
| `set_business_hours` | `timezone_id`, `intervals` | No |
| `set_business_intro` | `title` | No |
| `set_business_location` | `address` | No |
| `set_checklist_tasks_done` | `chat_id`, `message_id` | No |
| `set_fact_check` | `chat_id`, `message_id`, `text` | No |
| `suggest_sticker_set_name` | `title` | Yes |
| `translate` | `to_language` | Yes |
| `uninstall_sticker_set` | None | No |
| `unsave_gif` | `document_id` | No |
| `update_folder` | `folder_id`, `patch` | No |
| `upgrade_to_supergroup` | `chat_id` | No |
| `vote_in_poll` | `chat_id`, `message_id`, `option_indexes` | No |
| `vote_poll` | `chat_id`, `message_id`, `option_indices` | No |

### Documents and upload preparation

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `inspect_document` | `chat_id`, `message_id` | Yes |
| `prepare_upload` | `filename` | No |

### Identity

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `add_chat_to_community` | `community`, `chat`, `visibility` | No |
| `add_contact` | None | No |
| `approve_community_link_request` | `community` | No |
| `approve_join_request` | `chat_id`, `user_id` | No |
| `archive_chat` | `chat_id` | No |
| `ban_community_member` | `community`, `user` | No |
| `ban_user` | `chat_id`, `user_id` | No |
| `block_user` | `user_id` | No |
| `check_channel_username` | `chat_id`, `username` | Yes |
| `clear_chat_history` | `chat` | No |
| `convert_to_gigagroup` | `chat_id` | No |
| `create_channel` | `title` | No |
| `create_community` | `first_chat`, `title` | No |
| `create_group` | `title`, `user_ids` | No |
| `create_invite_link` | `chat_id` | No |
| `create_paid_invite_link` | `chat_id`, `monthly_fee_stars` | No |
| `deactivate_channel_usernames` | `chat_id` | No |
| `delete_channel` | `chat_id` | No |
| `delete_chat` | `chat` | No |
| `delete_chat_photo` | `chat_id` | No |
| `delete_community` | `community` | No |
| `delete_community_photo` | `community` | No |
| `delete_contact` | `user_id` | No |
| `delete_profile_photo` | None | No |
| `delete_revoked_invite_links` | `chat_id` | No |
| `demote_admin` | `group_id`, `user_id` | No |
| `edit_admin_rights` | `chat_id`, `user_id` | No |
| `edit_admin_rights_channel` | `channel_id`, `user_id` | No |
| `edit_admin_rights_community` | `community`, `user_id` | No |
| `edit_admin_rights_group` | `group_id`, `user_id` | No |
| `edit_chat_about` | `chat_id`, `about` | No |
| `edit_chat_photo` | `chat_id`, `file_path` | No |
| `edit_chat_title` | `chat_id`, `title` | No |
| `edit_invite_link` | `chat_id`, `link` | No |
| `export_chat_invite` | `chat_id` | No |
| `export_contacts` | None | Yes |
| `get_admins` | `chat_id` | Yes |
| `get_banned_users` | `chat_id` | Yes |
| `get_blocked_users` | None | Yes |
| `get_bot_commands` | None | Yes |
| `get_bot_info` | `bot_username` | Yes |
| `get_chat` | `chat_id` | Yes |
| `get_chats` | None | Yes |
| `get_common_chats` | `user_id` | Yes |
| `get_community_chats` | `community` | Yes |
| `get_community_link_requests` | `community` | Yes |
| `get_contact_chats` | `contact_id` | Yes |
| `get_contact_ids` | None | Yes |
| `get_direct_chat_by_contact` | `contact_query` | Yes |
| `get_full_chat` | `chat_id` | Yes |
| `get_full_user` | `username` | Yes |
| `get_invite_link` | `chat_id` | Yes |
| `get_invite_link_qr` | `link` | No |
| `get_last_interaction` | `contact_id` | Yes |
| `get_me` | None | Yes |
| `get_member_admin_status` | `chat_id`, `user_id` | Yes |
| `get_message_link` | `chat_id`, `message_id` | Yes |
| `get_participants` | `chat_id` | Yes |
| `get_privacy_settings` | None | Yes |
| `get_recent_actions` | `chat_id` | Yes |
| `get_similar_channels` | `chat_id` | Yes |
| `get_user_photos` | `user_id` | Yes |
| `get_user_status` | `user_id` | Yes |
| `import_chat_invite` | `hash` | No |
| `import_contacts` | `contacts` | No |
| `invite_to_group` | `group_id`, `user_ids` | No |
| `invite_users_to_bot` | `bot`, `user_ids` | No |
| `join_chat_by_link` | `link` | No |
| `kick_user` | `chat_id`, `user_id` | No |
| `leave_chat` | `chat_id` | No |
| `list_authorizations` | None | Yes |
| `list_chats` | None | Yes |
| `list_contacts` | None | Yes |
| `list_discussion_candidates` | None | Yes |
| `list_invite_link_admins` | `chat_id` | Yes |
| `list_invite_links` | `chat_id` | Yes |
| `list_join_requests` | `chat_id` | Yes |
| `list_link_joins` | `chat_id`, `link` | Yes |
| `list_my_communities` | None | Yes |
| `list_saved_sounds` | None | Yes |
| `lookup_peer` | `query` | Yes |
| `mark_chat_unread` | `chat` | No |
| `mark_topic_as_read` | `chat_id`, `topic_id` | No |
| `mute_chat` | `chat_id` | No |
| `pin_chat` | `chat` | No |
| `promote_admin` | `group_id`, `user_id` | No |
| `promote_admin_channel` | `channel_id`, `user_id` | No |
| `promote_admin_community` | `community`, `user_id` | No |
| `promote_admin_group` | `group_id`, `user_id` | No |
| `reject_community_link_request` | `community` | No |
| `remove_chat_from_community` | `community`, `chat` | No |
| `remove_member_exception` | `chat_id`, `user_id` | No |
| `remove_saved_sound` | `sound` | No |
| `remove_user` | `chat_id`, `user_id` | No |
| `rename_community` | `community`, `title` | No |
| `reorder_channel_usernames` | `chat_id`, `order` | No |
| `replace_chat_photo` | `chat_id` | No |
| `report_anti_spam_false_positive` | `chat_id`, `message_id` | No |
| `resolve_phone` | `phone` | Yes |
| `resolve_username` | `username` | Yes |
| `revoke_invite_link` | `chat_id`, `link` | No |
| `search_contacts` | `query` | Yes |
| `search_my_chats` | `query` | Yes |
| `search_public_chats` | `query` | Yes |
| `send_contact` | `chat_id`, `phone_number`, `first_name` | No |
| `set_anti_spam` | `chat_id`, `enabled` | No |
| `set_authorization_calls` | `hash`, `accept_calls` | No |
| `set_authorization_secret_chats` | `hash`, `accept_secret_chats` | No |
| `set_autotranslation` | `chat_id`, `enabled` | No |
| `set_boosts_to_unblock` | `chat_id`, `boosts` | No |
| `set_bot_commands` | `commands` | No |
| `set_bot_info` | `bot` | No |
| `set_channel_color` | `chat_id` | No |
| `set_channel_emoji_pack` | `chat_id`, `set_short_name` | No |
| `set_channel_emoji_status` | `chat_id`, `custom_emoji_id` | No |
| `set_channel_location` | `chat_id`, `latitude`, `longitude`, `address` | No |
| `set_channel_stickers` | `chat_id`, `set_short_name` | No |
| `set_channel_username` | `chat_id`, `username` | No |
| `set_chat_sound_disabled` | `chat`, `disabled` | No |
| `set_chat_tone` | `chat` | No |
| `set_community_photo` | `community`, `file_path` | No |
| `set_community_who_can_add` | `community`, `who` | No |
| `set_default_chat_permissions` | `chat_id` | No |
| `set_discussion_group` | `channel_id` | No |
| `set_group_permissions` | `chat_id` | No |
| `set_join_request` | `chat_id`, `enabled` | No |
| `set_join_to_send` | `chat_id`, `enabled` | No |
| `set_member_exception` | `chat_id`, `user_id` | No |
| `set_member_tag` | `chat`, `tag` | No |
| `set_my_username` | `username` | No |
| `set_participants_hidden` | `chat_id`, `hidden` | No |
| `set_prehistory_hidden` | `chat_id`, `hidden` | No |
| `set_privacy_settings` | `key` | No |
| `set_profile_photo` | `file_path` | No |
| `set_secret_chats_only_device` | `hash` | No |
| `set_signatures` | `chat_id`, `enabled` | No |
| `set_view_forum_as_messages` | `chat_id`, `enabled` | No |
| `subscribe_public_channel` | `channel` | No |
| `terminate_authorization` | `hash` | No |
| `toggle_channel_username` | `chat_id`, `username`, `active` | No |
| `toggle_slow_mode` | `chat_id` | No |
| `unarchive_chat` | `chat_id` | No |
| `unban_community_member` | `community`, `user` | No |
| `unban_user` | `chat_id`, `user_id` | No |
| `unblock_user` | `user_id` | No |
| `unmute_chat` | `chat_id` | No |
| `unpin_chat` | `chat` | No |
| `update_profile` | None | No |
| `upgrade_group_to_supergroup` | `chat_id` | No |

### Local

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `add_proxies` | None | No |
| `check_account_session` | `config_key`, `config_digest` | Yes |
| `delete_contact_alias` | `alias` | No |
| `disable_incoming_feed` | None | No |
| `enable_incoming_feed` | None | No |
| `export_unread_messages` | `chat_ids`, `output_path` | Yes |
| `get_connection_route` | None | Yes |
| `get_file_roots_status` | None | Yes |
| `get_ghost_mode` | None | Yes |
| `get_messages_after` | `chat_id` | Yes |
| `incoming_feed_status` | None | Yes |
| `list_accounts` | None | Yes |
| `list_chat_commands` | `chat_id` | Yes |
| `list_contact_aliases` | None | Yes |
| `list_proxies` | None | Yes |
| `remove_proxies` | None | No |
| `revoke_always_approval` | None | No |
| `safeguard_status` | None | Yes |
| `set_contact_alias` | `alias`, `chat_id` | No |
| `set_ghost_mode` | `enabled` | No |
| `test_proxies` | None | No |
| `wait_for_events` | None | Yes |
| `wait_for_new_message` | None | Yes |
| `wait_for_settled_message` | None | Yes |

### Messaging

`transcribe_audio` takes a local `file_path` in an allowed root. It uses the
configured native `whisper-cli` and local model, then returns transcript text and
segment start/end times in seconds. It does not open a Telegram session or send
the file to a hosted engine. `transcribe_voice` handles Telegram voice and video notes.

`view_video` returns sampled frames. Set `start` and `end` in seconds for a shorter clip. Its optional transcript runs through local Whisper and can fail independently of the frames. FFmpeg and FFprobe are required. The tool refuses protected and disappearing videos.

Ordinary message readers return `formatting` when Telegram supplies entities. The
list contains distinct entity kinds and leaves the original text and UTF-16 offsets intact.

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `answer_reply_button` | `chat_id`, `button_index`, `expect_text` | No |
| `cancel_scheduled_message` | `chat_id`, `message_id` | No |
| `clear_draft` | `chat_id` | No |
| `click_button` | `chat_id`, `message_id`, `button_index` | No |
| `copy_message` | `from_chat_id`, `message_id`, `to_chat_id` | No |
| `delete_chat_history` | `chat_id` | No |
| `delete_message` | `chat_id`, `message_id` | No |
| `delete_messages_bulk` | `chat_id`, `message_ids` | No |
| `delete_scheduled_message` | `chat_id`, `message_ids` | No |
| `download_media` | `chat_id` | No |
| `download_rich_media` | `chat_id`, `message_id` | No |
| `edit_message` | `chat_id`, `message_id`, `new_text` | No |
| `edit_message_media` | `chat_id`, `message_id`, `file_path` | No |
| `edit_scheduled_message` | `chat_id`, `message_id` | No |
| `forward_message` | `from_chat_id`, `message_id`, `to_chat_id` | No |
| `forward_messages` | `from_chat_id`, `message_ids`, `to_chat_id` | No |
| `get_custom_emoji` | `document_ids` | Yes |
| `get_drafts` | None | Yes |
| `get_history` | `chat_id` | Yes |
| `get_media_details` | `chat_id`, `message_id` | Yes |
| `get_media_frames` | `chat_id`, `message_id` | Yes |
| `get_media_info` | `chat_id`, `message_id` | Yes |
| `get_media_thumbnail` | `chat_id`, `message_id` | Yes |
| `get_message_context` | `chat_id`, `message_id` | Yes |
| `get_message_reactions` | `chat_id`, `message_id` | Yes |
| `get_message_read_by` | `chat_id`, `message_id` | Yes |
| `get_message_views` | `chat_id`, `message_ids` | Yes |
| `get_messages` | `chat_id` | Yes |
| `get_photo_sheet` | `chat_id` | Yes |
| `get_pinned_messages` | `chat_id` | Yes |
| `get_scheduled_messages` | `chat_id` | Yes |
| `get_send_as` | `chat_id` | Yes |
| `inline_query` | `bot`, `query` | Yes |
| `inspect_buttons` | `chat_id` | Yes |
| `inspect_custom_emoji` | `chat_id`, `message_id` | Yes |
| `inspect_message` | `chat_id`, `message_id` | Yes |
| `inspect_messages` | `chat_id` | Yes |
| `list_calls` | None | Yes |
| `list_disappearing_media` | `chat_id` | Yes |
| `list_inline_buttons` | `chat_id` | Yes |
| `list_messages` | `chat_id` | Yes |
| `list_photos` | `chat_id` | Yes |
| `list_scheduled_messages` | `chat_id` | Yes |
| `list_send_as` | `chat_id` | Yes |
| `list_transcription_models` | None | Yes |
| `mark_as_read` | `chat_id` | No |
| `open_photo` | `chat_id` | Yes |
| `pin_message` | `chat_id`, `message_id` | No |
| `press_inline_button` | `chat_id` | No |
| `press_reply_button` | `chat_id`, `button_index`, `expect_text` | No |
| `rate_voice_transcription` | `chat_id`, `message_id`, `transcription_id`, `good` | No |
| `read_rich_message` | `chat_id`, `message_id` | Yes |
| `remove_reaction` | `chat_id`, `message_id` | No |
| `replace_custom_emoji` | `chat_id`, `message_id`, `new_document_id` | No |
| `reply_to_message` | `chat_id`, `message_id`, `text` | No |
| `save_disappearing_media` | `chat_id`, `message_id` | No |
| `save_draft` | `chat_id`, `message` | No |
| `schedule_message` | `chat_id`, `message`, `when` | No |
| `search_global` | None | Yes |
| `search_messages` | `chat_id` | Yes |
| `search_posts` | None | Yes |
| `send_album` | `chat_id`, `file_paths` | No |
| `send_disappearing_media` | `chat_id`, `file_path` | No |
| `send_file` | `chat_id`, `file_path` | No |
| `send_inline_result` | `chat_id`, `result_id` | No |
| `send_message` | `chat_id`, `message` | No |
| `send_reaction` | `chat_id`, `message_id` | No |
| `send_scheduled_message` | `chat_id`, `message`, `schedule_date` | No |
| `send_sticker` | `chat_id`, `file_path` | No |
| `send_typing` | `chat_id` | No |
| `send_voice` | `chat_id`, `file_path` | No |
| `set_default_send_as` | `chat_id`, `send_as` | No |
| `set_chat_ai_style` | `chat_id` | No |
| `transcribe_audio` | `file_path` | Yes |
| `transcribe_voice` | `chat_id`, `message_id` | Yes |
| `view_video` | `chat_id`, `message_id` | Yes |
| `unpin_all_messages` | `chat_id` | No |
| `unpin_message` | `chat_id`, `message_id` | No |
| `upload_file` | `file_path` | No |

### Visual

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `cancel_export` | `job_id` | No |
| `export_chat_history` | `chat_id` | No |
| `export_recent_actions` | `chat_id` | No |
| `export_status` | None | Yes |
| `get_telegram_frames` | None | Yes |
| `get_telegram_region` | `left`, `top`, `right`, `bottom` | Yes |
| `get_telegram_screen` | None | Yes |
| `list_telegram_windows` | None | Yes |

### Secret-chat tools

[Grammers Secret Chat](https://github.com/KiaroSama/Grammers-Secret-Chat) handles
native MTProto 2.0 secret chats on the existing account connection. On first open,
the server makes a read-only backup and migrates supported legacy stores losslessly
before connecting. See [secret-chat store migration](MIGRATION.md#secret-chat-store-migration).

All 35 tools take an optional `account`. The backend automatically attempts the
handshake when it receives an incoming `ChatRequested` update. To accept a known
pending incoming request explicitly, call `accept_secret_chat` with its `chat_id`.
That call is subject to owner approval. `read_secret_messages` reads local history
without sending a read receipt. `mark_secret_read` sends one and requires approval.
Sending, relaying, and destructive actions also require approval. File operations
stay within configured roots.

#### Lifecycle and status (7)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `secret_chat_status` | None | Yes |
| `list_secret_chats` | None | Yes |
| `create_secret_chat` | `user_id` | No |
| `accept_secret_chat` | `chat_id` | No |
| `set_secret_chat_timer` | `chat_id`, `seconds` | No |
| `close_secret_chat` | `secret_chat_id` | No |
| `forget_secret_chat` | `secret_chat_id` | No |

#### Messaging and media (5)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `send_secret_message` | `chat_id`, `message` | No |
| `send_secret_media` | `chat_id`, `file_path` | No |
| `read_secret_messages` | `chat_id` | Yes |
| `save_secret_media` | `chat_id`, `message_id` | No |
| `forward_secret_media` | `from_chat_id`, `message_id`, `to_chat_id` | No |

#### Timed ephemeral messaging (2)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `send_timed_secret_message` | `chat_id`, `message`, `ttl_seconds` | No |
| `send_timed_secret_media` | `chat_id`, `file_path`, `ttl_seconds` | No |

#### Actions and search (6)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `delete_secret_message` | `chat_id`, `message_ids` | No |
| `clear_secret_history` | `chat_id` | No |
| `mark_secret_read` | `chat_id` | No |
| `send_secret_typing` | `chat_id` | No |
| `search_secret_messages` | `query` | Yes |
| `copy_into_secret_chat` | `from_chat_id`, `message_id`, `to_secret_chat_id` | No |

#### Auto-save, deletion and export (6)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `start_secret_auto_save` | `folder` | No |
| `stop_secret_auto_save` | None | No |
| `delete_secret_chat` | `secret_chat_id` | No |
| `delete_secret_chat_both_sides` | `secret_chat_id` | No |
| `delete_saved_secret_messages` | `chat_id` | No |
| `export_secret_chat` | `chat_id` | No |

#### Custody, pause, delivery and backend control (9)

| Tool | Required arguments | Read only |
| --- | --- | --- |
| `configure_secret_backend` | None | No |
| `get_secret_backend_status` | None | Yes |
| `pause_secret_chat` | `chat_id` | No |
| `get_secret_pause_status` | `chat_id` | Yes |
| `check_secret_custody` | None | Yes |
| `resume_secret_chat` | `chat_id` | No |
| `list_secret_deliveries` | `chat_id` | Yes |
| `acknowledge_secret_delivery` | `chat_id`, `seq_no` | No |
| `receive_secret_media` | `chat_id`, `message_id` | No |
