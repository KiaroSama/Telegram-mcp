#![forbid(unsafe_code)]

mod export;
mod history;
mod migration;
mod runtime;

pub use export::ExportFormat;
pub use history::SecretHistory;
pub use migration::migrate_legacy;
pub(crate) use migration::{custody_dir, has_legacy, legacy_owner, open_store};
pub use runtime::SecretRuntime;
pub(crate) use runtime::{Prepared, session_binding};

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAX_BYTES: usize = 64 * 1024 * 1024;

#[doc(hidden)]
pub const HISTORY_HANDLER_LABEL: &str = "mcp_history";

pub(crate) const CHAT_OFFSET: i64 = 2_000_000_000_000;

pub(crate) fn wire_chat_id(value: i64) -> Option<i32> {
    let wire = match value {
        ..=-1_000_000_000_000 => value + CHAT_OFFSET,
        1_000_000_000_000.. => value - CHAT_OFFSET,
        _ => value,
    };
    i32::try_from(wire).ok().filter(|id| *id != 0)
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Secret-chat path contains a symbolic link"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "Secret-chat path contains a reparse point"
                );
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Cannot inspect secret-chat path"),
    }
}

fn plain_file(path: &Path) -> Result<bool> {
    let Some(metadata) = metadata(path)? else {
        return Ok(false);
    };
    ensure!(metadata.is_file(), "Secret-chat path is not a regular file");
    Ok(true)
}

fn private_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {
            crate::state::storage::restrict(path, true)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Cannot create secret-chat directory"),
    }
    crate::state::storage::sync_directory(
        path.parent()
            .context("Secret-chat directory has no parent")?,
    )?;
    ensure!(
        metadata(path)?.is_some_and(|m| m.is_dir()),
        "Secret-chat directory is not a regular directory"
    );
    Ok(())
}

fn read_private(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    if !plain_file(path)? {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "Secret-chat file exceeds its byte budget"
    );
    Ok(Some(bytes))
}

fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().context("Secret-chat output has no parent")?;
    plain_file(path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    crate::state::storage::restrict(temp.path(), false)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    if replace {
        temp.persist(path).map_err(|error| error.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|error| error.error)?;
    }
    crate::state::storage::sync_directory(parent).context(
        "Secret-chat file was published but directory durability could not be confirmed",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiaro_secret_chat::{
        enums::DecryptedMessageAction,
        files::FileKey,
        manager::{Event, MessageReceived},
        media::{DocumentMediaInput, MediaKind, MediaReference, Metadata, document_media},
        types::DecryptedMessageActionDeleteMessages,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn test_state(tmp: &tempfile::TempDir) -> (crate::state::State, std::path::PathBuf) {
        let state_dir = tmp.path().join("state");
        let files_dir = tmp.path().join("files");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&files_dir).unwrap();
        let state = crate::state::State::at(state_dir, vec![files_dir.clone()]).unwrap();
        (state, files_dir)
    }

    fn sample_media_ref(chat_id: i32, file_id: i64) -> MediaReference {
        let key = FileKey::generate().unwrap();
        let media = document_media(DocumentMediaInput {
            kind: MediaKind::Document,
            name: "sample.bin",
            mime: "application/octet-stream",
            size: 16,
            layer: 143,
            key: &key,
            caption: "clip",
            metadata: &Metadata::default(),
        })
        .unwrap();
        let file = grammers_tl_types::types::EncryptedFile {
            id: file_id,
            access_hash: 999,
            size: 16,
            dc_id: 2,
            key_fingerprint: key.fingerprint(),
        };
        MediaReference::new(chat_id, &file, &media).unwrap()
    }

    fn received_media(
        id: i64,
        media: kiaro_secret_chat::enums::DecryptedMessageMedia,
    ) -> MessageReceived {
        let key = kiaro_secret_chat::media::file_parts(&media).unwrap().key;
        let file = grammers_tl_types::types::EncryptedFile {
            id: 501,
            access_hash: 999,
            size: 16,
            dc_id: 2,
            key_fingerprint: key.fingerprint(),
        };
        let reference = MediaReference::new(10, &file, &media).unwrap();
        MessageReceived {
            chat_id: 10,
            random_id: id,
            seq_no: 1,
            text: String::new(),
            entities: None,
            ttl: 0,
            media: Some(media),
            file: Some(file),
            reply_to: None,
            media_reference: Some(reference),
        }
    }

    #[test]
    fn history_received_media_captions_survive_restart_and_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, files_dir) = test_state(&tmp);
        let directory = tmp.path().join("secret");
        fs::create_dir(&directory).unwrap();
        let history = SecretHistory::open(&directory, "acc", &BTreeMap::new()).unwrap();
        history
            .set_auto_save_folder(&state, Some(&files_dir.join("archive")))
            .unwrap();

        for (id, body) in [(101, ""), (102, "separate message body")] {
            let key = FileKey::generate().unwrap();
            let media = kiaro_secret_chat::types::DecryptedMessageMediaPhoto {
                thumb: Vec::new(),
                thumb_w: 0,
                thumb_h: 0,
                w: 64,
                h: 64,
                size: 16,
                key: key.key().to_vec(),
                iv: key.iv().to_vec(),
                caption: "Sunset over the lake".into(),
            }
            .into();
            let mut message = received_media(id, media);
            message.text = body.into();
            let row = history.record_message(&message).unwrap();
            assert_eq!(row["caption"], "Sunset over the lake");
            if !body.is_empty() {
                assert_eq!(row["text"], body);
            }
            history.archive_row(&state, 10, &row).unwrap();
        }
        drop(history);

        let reopened = SecretHistory::open(&directory, "acc", &BTreeMap::new()).unwrap();
        assert_eq!(
            reopened.read(10, 2).unwrap()[0]["caption"],
            "Sunset over the lake"
        );
        assert_eq!(reopened.search(Some(10), "lake", 10).unwrap().len(), 2);
        assert_eq!(
            reopened
                .search(Some(10), "separate message", 10)
                .unwrap()
                .len(),
            1
        );
        let archived = reopened.archive_rows(&state, 10).unwrap();
        assert_eq!(archived.len(), 2);
        assert!(
            archived
                .iter()
                .all(|(row, _)| row["caption"] == "Sunset over the lake")
        );
    }

    #[test]
    fn history_received_media_kinds_drive_export_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("secret");
        fs::create_dir(&directory).unwrap();
        let history = SecretHistory::open(&directory, "acc", &BTreeMap::new()).unwrap();
        let cases = [
            (MediaKind::Photo, "photo", "photos"),
            (MediaKind::Video, "video", "videos"),
            (MediaKind::Document, "document", "files"),
            (MediaKind::Audio, "audio", "files"),
            (MediaKind::Animation, "animation", "gifs"),
            (MediaKind::Sticker, "sticker", "stickers"),
            (MediaKind::VideoNote, "video_note", "video_messages"),
            (MediaKind::VoiceNote, "voice_note", "voice_messages"),
        ];
        for (index, (kind, expected, flag)) in cases.into_iter().enumerate() {
            let key = FileKey::generate().unwrap();
            let media = document_media(DocumentMediaInput {
                kind,
                name: "sample.bin",
                mime: "application/octet-stream",
                size: 16,
                layer: 143,
                key: &key,
                caption: "",
                metadata: &Metadata::default(),
            })
            .unwrap();
            let row = history
                .record_message(&received_media(index as i64 + 1, media))
                .unwrap();
            assert_eq!(row["type"], expected);
            let mut options = json!({"include_media": false});
            options[flag] = json!(true);
            let selected = export::ExportFilter::from_args(&options).unwrap();
            assert!(selected.allows_media(&row, 16));
            options[flag] = json!(false);
            assert!(
                !export::ExportFilter::from_args(&options)
                    .unwrap()
                    .allows_media(&row, 16)
            );
        }
    }

    #[test]
    fn wire_chat_id_handles_plain_offset_and_zero() {
        assert_eq!(wire_chat_id(42), Some(42));
        assert_eq!(wire_chat_id(-42), Some(-42));
        assert_eq!(wire_chat_id(CHAT_OFFSET + 77), Some(77));
        assert_eq!(wire_chat_id(-CHAT_OFFSET - 77), Some(-77));
        assert_eq!(wire_chat_id(0), None);
        assert_eq!(wire_chat_id(CHAT_OFFSET), None);
    }

    #[test]
    fn export_format_and_filter_cover_dates_sizes_and_media_kinds() {
        assert_eq!(
            export::ExportFormat::parse("json").unwrap(),
            ExportFormat::Json
        );
        assert_eq!(
            export::ExportFormat::parse("html").unwrap(),
            ExportFormat::Html
        );
        assert_eq!(
            export::ExportFormat::parse("txt").unwrap(),
            ExportFormat::Text
        );
        assert_eq!(
            export::ExportFormat::parse("text").unwrap(),
            ExportFormat::Text
        );
        assert_eq!(
            export::ExportFormat::parse("html_and_json").unwrap(),
            ExportFormat::HtmlAndJson
        );
        assert!(export::ExportFormat::parse("xml").is_err());

        let filter = export::ExportFilter::from_args(&json!({
            "date_from": "2026-01-10",
            "date_to": "2026-01-20",
            "include_media": false,
            "photos": true,
            "gifs": true,
            "files": true,
            "size_limit_mb": 1.0
        }))
        .unwrap();

        assert!(filter.matches_row(&json!({"date": "2026-01-15T12:00:00Z"})));
        assert!(!filter.matches_row(&json!({"date": "2026-01-01T00:00:00Z"})));
        assert!(!filter.matches_row(&json!({"date": "2026-02-01T00:00:00Z"})));
        assert!(!filter.matches_row(&json!({"date": "not-a-date"})));
        assert!(!filter.matches_row(&json!({})));

        assert!(filter.allows_media(&json!({"type": "photo"}), 100));
        assert!(!filter.allows_media(&json!({"type": "video"}), 100));
        assert!(!filter.allows_media(&json!({"type": "voice"}), 100));
        assert!(!filter.allows_media(&json!({"type": "video_note"}), 100));
        assert!(!filter.allows_media(&json!({"type": "sticker"}), 100));
        assert!(filter.allows_media(&json!({"type": "gif"}), 100));
        assert!(filter.allows_media(&json!({"type": "animation"}), 100));
        assert!(!filter.allows_media(&json!({"type": "text"}), 100));
        assert!(!filter.allows_media(&json!({"type": "service"}), 100));
        assert!(filter.allows_media(&json!({"type": "document"}), 100));
        assert!(!filter.allows_media(&json!({"type": "photo"}), 2 * 1024 * 1024));

        let open = export::ExportFilter::from_args(&json!({})).unwrap();
        assert!(open.matches_row(&json!({})));
        assert!(export::ExportFilter::from_args(&json!({"date_from": "bad"})).is_err());
    }

    #[test]
    fn export_outputs_key_picture_and_all_history_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, files_dir) = test_state(&tmp);

        let key_hash = [0x5Au8; 36];
        let png1 = export::key_picture(&state, "secret-key", &key_hash).unwrap();
        let png2 = export::key_picture(&state, "secret-key", &key_hash).unwrap();
        assert_eq!(png1, png2);
        let png_bytes = fs::read(&png1).unwrap();
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");

        let rows = vec![
            json!({"message_id": 1, "is_outgoing": true, "date": "2026-01-10T01:00:00Z", "text": "a < b & c > d"}),
            json!({"message_id": 2, "is_outgoing": false, "date": "2026-01-10T02:00:00Z", "caption": "pic"}),
            json!({"is_outgoing": false, "date": "2026-01-10T03:00:00Z", "action": "FlushHistory"}),
        ];

        let json_out =
            export::write_history(&state, "acc", 77, &rows, ExportFormat::Json, None).unwrap();
        assert_eq!(json_out.len(), 1);
        let parsed: serde_json::Value =
            serde_json::from_slice(&fs::read(&json_out[0]).unwrap()).unwrap();
        assert_eq!(parsed["chat_id"], 77);
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 3);

        let txt_dest = files_dir.join("nested").join("chat.txt");
        let txt_out = export::write_history(
            &state,
            "acc",
            77,
            &rows,
            ExportFormat::Text,
            Some(&txt_dest),
        )
        .unwrap();
        let txt = fs::read_to_string(&txt_out[0]).unwrap();
        assert!(txt.contains("Outgoing\na < b & c > d"));
        assert!(txt.contains("Incoming\npic"));
        assert!(txt.contains("FlushHistory"));

        let html_dir = files_dir.join("bundle");
        fs::create_dir_all(&html_dir).unwrap();
        let both_dir = export::write_history(
            &state,
            "acc",
            77,
            &rows,
            ExportFormat::HtmlAndJson,
            Some(&html_dir),
        )
        .unwrap();
        assert_eq!(both_dir.len(), 2);
        let html = fs::read_to_string(&both_dir[0]).unwrap();
        assert!(html.contains("a &lt; b &amp; c &gt; d"));

        let explicit_html = files_dir.join("explicit").join("out.html");
        let both_file = export::write_history(
            &state,
            "acc",
            77,
            &rows,
            ExportFormat::HtmlAndJson,
            Some(&explicit_html),
        )
        .unwrap();
        assert_eq!(both_file.len(), 2);
        assert!(both_file[1].extension().unwrap() == "json");
    }

    #[test]
    fn export_keeps_distinct_message_text_and_media_caption() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, _) = test_state(&tmp);
        let rows = vec![json!({
            "message_id": 102,
            "is_outgoing": false,
            "date": "2026-01-01T00:00:00Z",
            "text": "separate <message> body",
            "caption": "Sunset & lake",
        })];
        for (format, body, caption) in [
            (
                ExportFormat::Text,
                "separate <message> body",
                "Sunset & lake",
            ),
            (
                ExportFormat::Html,
                "separate &lt;message&gt; body",
                "Sunset &amp; lake",
            ),
        ] {
            let paths = export::write_history(&state, "acc", 10, &rows, format, None).unwrap();
            let output = fs::read_to_string(&paths[0]).unwrap();
            assert!(output.contains(body));
            assert!(output.contains(caption));
        }
    }

    #[test]
    fn history_lifecycle_media_deliveries_search_delete_and_service_events() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, files_dir) = test_state(&tmp);
        let hist_dir = tmp.path().join("secret");
        fs::create_dir_all(&hist_dir).unwrap();

        let legacy_folder = files_dir.join("legacy-archive");
        let mut legacy = BTreeMap::new();
        legacy.insert(
            "auto_save_secret_chats".into(),
            json!({"on": true, "folder": legacy_folder.to_string_lossy()}),
        );
        let hist = SecretHistory::open(&hist_dir, "acc", &legacy).unwrap();
        assert_eq!(hist.auto_save_folder().unwrap(), Some(legacy_folder));

        assert!(hist.append(0, json!({}), None).is_err());
        assert!(hist.append(10, json!("not-object"), None).is_err());

        let mref = sample_media_ref(10, 501);
        let row1 = history::row(101, true, "hello alpha", 0, false);
        hist.append(10, row1.clone(), None).unwrap();
        hist.append(10, row1, None).unwrap();

        let row2 = json!({
            "message_id": "102",
            "is_outgoing": false,
            "date": "2026-01-12T00:00:00Z",
            "caption": "Beta Media"
        });
        hist.append(10, row2, Some(&mref)).unwrap();
        hist.append(
            10,
            json!({"message_id": 102, "date": "2026-01-12T00:00:00Z"}),
            Some(&mref),
        )
        .unwrap();
        assert_eq!(hist.media(10, 102).unwrap().file_id, 501);
        assert!(hist.media(10, 999).is_err());

        let timed_ref = sample_media_ref(10, 502);
        let timed_row = history::row(103, false, "ephemeral", 15, true);
        hist.append(10, timed_row.clone(), Some(&timed_ref))
            .unwrap();
        assert!(hist.media(10, 103).is_err());
        assert_eq!(
            hist.media_with_restriction(10, 103, false).unwrap().file_id,
            502
        );
        assert_eq!(hist.read(10, 10).unwrap().len(), 2);

        let deliv_row = history::row(104, false, "delivered gamma", 0, false);
        assert!(!hist.delivery_committed(10, 1).unwrap());
        hist.commit_delivery(10, 1, deliv_row.clone(), None)
            .unwrap();
        assert!(hist.delivery_committed(10, 1).unwrap());
        hist.commit_delivery(10, 1, deliv_row, None).unwrap();
        assert!(
            hist.commit_delivery(10, 1, history::row(999, false, "conflict", 0, false), None)
                .is_err()
        );
        assert!(hist.commit_delivery(10, 2, timed_row, None).is_err());

        assert!(hist.search(None, "   ", 10).is_err());
        assert_eq!(hist.search(None, "beta", 10).unwrap().len(), 1);
        assert_eq!(hist.search(Some(10), "gamma", 10).unwrap().len(), 1);
        assert_eq!(hist.search(Some(99), "gamma", 10).unwrap().len(), 0);

        let incoming = MessageReceived {
            chat_id: 20,
            random_id: 201,
            seq_no: 1,
            text: "incoming msg".into(),
            entities: None,
            ttl: 0,
            media: None,
            file: None,
            reply_to: None,
            media_reference: None,
        };
        let recorded = hist.record_message(&incoming).unwrap();
        assert_eq!(history::message_id(&recorded), Some(201));

        hist.record_service(Event::ServiceActionReceived {
            chat_id: 20,
            action_name: "DeleteMessages".into(),
            action: DecryptedMessageAction::DeleteMessages(DecryptedMessageActionDeleteMessages {
                random_ids: vec![201],
            }),
            applied: true,
        })
        .unwrap();
        hist.record_service(Event::ServiceActionReceived {
            chat_id: 20,
            action_name: "DeleteMessages".into(),
            action: DecryptedMessageAction::DeleteMessages(DecryptedMessageActionDeleteMessages {
                random_ids: vec![201],
            }),
            applied: false,
        })
        .unwrap();
        hist.record_service(Event::ServiceActionReceived {
            chat_id: 20,
            action_name: "FlushHistory".into(),
            action: DecryptedMessageAction::FlushHistory,
            applied: true,
        })
        .unwrap();
        assert_eq!(hist.read(20, 10).unwrap().len(), 1);

        assert_eq!(hist.delete_saved(10, Some(&[101])).unwrap(), 1);
        assert_eq!(hist.delete_saved(999, Some(&[1])).unwrap(), 0);
        hist.clear_media(10).unwrap();
        assert!(hist.media(10, 102).is_err());
        assert!(hist.erase_local_chat(10).unwrap() >= 1);
        assert!(hist.is_forgotten(10).unwrap());
        hist.append(10, history::row(555, true, "ignored", 0, false), None)
            .unwrap();
        assert!(hist.read(10, 10).unwrap().is_empty());
        assert!(
            hist.commit_delivery(10, 9, history::row(556, true, "blocked", 0, false), None)
                .is_err()
        );
        assert!(
            hist.archive_row(&state, 10, &history::row(557, true, "skip", 0, false))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn history_archive_folders_rows_and_saved_file_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, files_dir) = test_state(&tmp);
        let hist_dir = tmp.path().join("secret");
        fs::create_dir_all(&hist_dir).unwrap();
        let hist = SecretHistory::open(&hist_dir, "acc", &BTreeMap::new()).unwrap();

        assert!(
            hist.archive_row(&state, 30, &history::row(1, true, "no-folder", 0, false))
                .unwrap()
                .is_none()
        );
        assert!(
            hist.archive_row(&state, 30, &history::row(2, true, "ttl", 10, false))
                .unwrap()
                .is_none()
        );

        let f1 = files_dir.join("arch1");
        let f2 = files_dir.join("arch2");
        assert!(
            hist.set_auto_save_folder(&state, Some(&f1))
                .unwrap()
                .is_some()
        );
        let r1 = json!({"message_id": 301, "date": "2026-01-01T00:00:00Z", "text": "first"});
        let dir1 = hist.archive_row(&state, 30, &r1).unwrap().unwrap();
        assert!(hist.archive_row(&state, 30, &r1).unwrap().is_some());
        fs::write(dir1.join("secret-30-301.bin"), b"payload").unwrap();
        fs::write(dir1.join("notes.txt"), b"ignore").unwrap();

        assert!(
            hist.set_auto_save_folder(&state, Some(&f2))
                .unwrap()
                .is_some()
        );
        let r2 = json!({"message_id": 302, "date": "2026-01-02T00:00:00Z", "text": "second"});
        let _dir2 = hist.archive_row(&state, 30, &r2).unwrap().unwrap();

        let rows = hist.archive_rows(&state, 30).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0["message_id"], 301);
        assert_eq!(rows[1].0["message_id"], 302);

        let (removed, failed) = hist.delete_saved_files(&state, 30, Some(&[301])).unwrap();
        assert_eq!(removed, 2);
        assert!(failed.is_empty());

        let (removed_all, failed_all) = hist.delete_saved_files(&state, 30, None).unwrap();
        assert_eq!(removed_all, 1);
        assert!(failed_all.is_empty());

        assert!(hist.set_auto_save_folder(&state, None).unwrap().is_none());
        assert!(hist.auto_save_folder().unwrap().is_none());
    }
}
