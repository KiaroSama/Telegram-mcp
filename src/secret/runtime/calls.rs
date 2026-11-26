use super::{accept::snapshot, arguments::*, *};
use base64::{Engine, engine::general_purpose::STANDARD};

use kiaro_secret_chat::{
    enums::SendMessageAction,
    media::{MediaKind, Metadata},
};
use serde_json::json;

impl SecretRuntime {
    /// Dispatch a public secret-chat method using its tool parameter names.
    pub async fn call(&self, api: &dyn Telegram, method: &str, args: Value) -> Result<Value> {
        ensure!(args.is_object(), "Secret-chat arguments must be an object");
        if method == "accept_secret_chat" {
            let id = args["chat_id"]
                .as_i64()
                .and_then(crate::secret::wire_chat_id)
                .context("Invalid secret-chat ID")?;
            if args["account"]
                .as_str()
                .is_some_and(|account| account != self.label)
            {
                return Ok(json!({"status":"refused","code":"not_found"}));
            }
            return Ok(self.accept_incoming(id, None).await);
        }
        let mutations = Arc::clone(&self.mutations).lock_owned().await;
        let id = || alias_chat(&args, "chat_id", "secret_chat_id");
        let close_id = || alias_chat(&args, "secret_chat_id", "chat_id");
        let limit = || integer(&args, "limit").unwrap_or(50).clamp(1, 1000) as usize;
        match method {
            "secret_chat_status"
                if args["chat_id"].is_null() && args["secret_chat_id"].is_null() =>
            {
                self.backend_status()
            }
            "get_secret_backend_status" => self.backend_status(),
            "configure_secret_backend" => {
                let mode = args["mode"].as_str().unwrap_or("ingress");
                ensure!(
                    matches!(mode, "ingress" | "strong" | "normal"),
                    "The native backend cannot switch to bounded custody; strong ingress remains active"
                );
                ensure!(
                    args["options"].is_null()
                        || args["options"]
                            .as_object()
                            .is_some_and(|options| options.is_empty()),
                    "Changing the running strong-ingress limits is unsupported"
                );
                for key in [
                    "records",
                    "bytes",
                    "age_seconds",
                    "chats",
                    "frame_bytes",
                    "operation_seconds",
                    "tasks",
                ] {
                    ensure!(
                        args[key].is_null(),
                        "Changing the running strong-ingress limit {key} is unsupported"
                    );
                }
                let mut status = self.backend_status()?;
                if mode == "normal" {
                    status["unchanged"] = json!(true);
                    status["note"] = json!("The native backend keeps durable ingress enabled");
                }
                Ok(status)
            }
            "secret_chat_status" => {
                let id = id()?;
                let mut status = snapshot(self.status(id)?);
                status["key_picture_path"] = json!(self.key_picture(id).await?);
                Ok(status)
            }
            "list_secret_chats" => {
                let mut rows = Vec::new();
                for chat in self.manager.list()? {
                    if !self.history.is_forgotten(chat.id)? {
                        rows.push(snapshot(chat));
                    }
                }
                Ok(json!({"chats":rows}))
            }
            "create_secret_chat" => {
                let peer = api.resolve_peer(&args["user_id"]).await?;
                ensure!(
                    peer["_"].as_str() == Some("inputPeerUser"),
                    "Secret chat requires another Telegram user"
                );
                let user = tl::types::InputUser {
                    user_id: required_int(&peer, "user_id")?,
                    access_hash: required_int(&peer, "access_hash")?,
                };
                Ok(snapshot(self.manager.create(user.into()).await?))
            }
            "set_secret_chat_timer" => {
                let id = id()?;
                let seconds = i32::try_from(required_int(&args, "seconds")?)?;
                self.manager.set_ttl(id, seconds).await?;
                Ok(json!({"chat_id":id,"seconds":seconds}))
            }
            "close_secret_chat"
            | "delete_secret_chat"
            | "delete_secret_chat_both_sides"
            | "forget_secret_chat" => {
                let id = close_id()?;
                if method == "forget_secret_chat" {
                    ensure!(
                        self.manager.status(id)?.state == ChatState::Closed,
                        "Only a closed secret chat can be forgotten"
                    );
                    self.history.forget_chat(id)?;
                    return Ok(
                        json!({"chat_id":id,"closed":true,"forgotten":true,"protocol_tombstone_retained":true,"messages_removed":0}),
                    );
                }
                let both = method == "delete_secret_chat_both_sides";
                self.manager.close(id).await?;
                let result = self
                    .client
                    .invoke(&tl::functions::messages::DiscardEncryption {
                        delete_history: both,
                        chat_id: id,
                    })
                    .await;
                let removed = if method != "close_secret_chat" {
                    self.history.erase_local_chat(id)?
                } else {
                    0
                };
                let remote_confirmed = matches!(result, Ok(true));
                Ok(
                    json!({"chat_id":id,"closed":true,"remote_confirmed":remote_confirmed,"remote_outcome":if remote_confirmed {"confirmed"} else {"unconfirmed"},"messages_removed":removed,"protocol_tombstone_retained":true}),
                )
            }
            "send_secret_message"
            | "send_secret_media"
            | "send_timed_secret_message"
            | "send_timed_secret_media" => {
                let chat = id()?;
                if method.starts_with("send_timed_") {
                    let runtime = self.clone();
                    let method = method.to_owned();
                    return tokio::spawn(async move {
                        let _mutations = mutations;
                        runtime.send(chat, &method, &args).await
                    })
                    .await
                    .context("Timed secret-chat send worker did not finish")?;
                }
                self.send(chat, method, &args).await
            }
            "read_secret_messages" => {
                let id = id()?;
                if !self.history.is_forgotten(id)? {
                    self.manager.status(id)?;
                }
                Ok(json!({"chat_id":id,"messages":self.history.read(id,limit())?}))
            }
            "search_secret_messages" => {
                let id = if args["chat_id"].is_null() {
                    None
                } else {
                    Some(id()?)
                };
                Ok(json!({"messages":self.history.search(id, string(&args,"query")?,limit())?}))
            }
            "receive_secret_media" if !args["max_bytes"].is_null() => {
                let id = id()?;
                let message = required_int(&args, "message_id")?;
                let maximum = usize::try_from(required_int(&args, "max_bytes")?)?;
                ensure!(
                    (1..=MAX_BYTES).contains(&maximum),
                    "Secret media byte budget must be between 1 byte and 64 MiB"
                );
                let reference = self.history.media(id, message)?;
                let bytes = Zeroizing::new(
                    self.manager
                        .receive_file_with_limit(&reference, maximum)
                        .await?,
                );
                Ok(
                    json!({"secret_chat_id":id,"chat_id":lifted(id),"message_id":message,"encoding":"base64","content":STANDARD.encode(&*bytes),"bytes":bytes.len(),"max_bytes":maximum,"content_file_created":false,"acknowledged":false}),
                )
            }
            "save_secret_media" | "receive_secret_media" => {
                let id = id()?;
                let message = required_int(&args, "message_id")?;
                let honour = method == "receive_secret_media"
                    || args["honour_sender_restriction"].as_bool().unwrap_or(false);
                let reference = self.history.media_with_restriction(id, message, honour)?;
                let (saved_path, durable) = self
                    .write_media(&reference, message, path(&args, "destination"))
                    .await?;
                let mut out = json!({"chat_id":lifted(id),"secret_chat_id":id,"message_id":message,"saved":true,"path":saved_path,"durability_confirmed":durable});
                if !durable {
                    out["warning"] = json!(
                        "Destination was published, but directory durability could not be confirmed."
                    );
                }
                Ok(out)
            }
            "forward_secret_media" => {
                let from = chat_id(&args, "from_chat_id")?;
                let to = chat_id(&args, "to_chat_id")?;
                let reference = self
                    .history
                    .media(from, required_int(&args, "message_id")?)?;
                let caption = args["caption"].as_str().unwrap_or_default();
                let id = self
                    .manager
                    .forward_file(
                        to,
                        &reference,
                        caption,
                        integer(&args, "reply_to_message_id"),
                    )
                    .await?;
                Ok(self.sent_result(to, id, caption, true))
            }
            "copy_into_secret_chat" => {
                let target = alias_chat(&args, "to_chat_id", "to_secret_chat_id")?;
                let peer = api.resolve_peer(&args["from_chat_id"]).await?;
                let source_id = i32::try_from(required_int(&args, "message_id")?)?;
                let input = json!({"_":"inputMessageID","id":source_id});
                let result = if peer["_"].as_str() == Some("inputPeerChannel") {
                    let channel = json!({"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]});
                    api.invoke(
                        "channels.getMessages",
                        json!({"channel":channel,"id":[input]}),
                    )
                    .await?
                } else {
                    api.invoke("messages.getMessages", json!({"id":[input]}))
                        .await?
                };
                let expected = crate::server::access::marked_peer(&peer)
                    .context("Source chat cannot be verified")?;
                let source = result["messages"]
                    .as_array()
                    .and_then(|rows| {
                        rows.iter().find(|message| {
                            integer(message, "id") == Some(i64::from(source_id))
                                && crate::server::access::marked_peer(&message["peer_id"])
                                    .as_deref()
                                    == Some(&expected)
                        })
                    })
                    .context("Source message is unavailable in the requested chat")?;
                ensure!(
                    source["noforwards"] != true,
                    "The source message cannot be copied"
                );
                ensure!(
                    source["media"].is_null()
                        || source["media"]["_"].as_str() == Some("messageMediaEmpty"),
                    "Copying cloud media into a secret chat is not supported by the pinned backend"
                );
                let text = string(source, "message")?;
                let sent = self
                    .manager
                    .send_message(target, text, Some(Vec::new()), None)
                    .await?;
                Ok(self.sent_result(target, sent, text, false))
            }
            "delete_secret_message" => {
                let id = id()?;
                let ids = if args["message_id"].is_null() {
                    ids(&args["message_ids"])?
                } else {
                    vec![required_int(&args, "message_id")?]
                };
                ensure!(
                    !ids.is_empty(),
                    "Select at least one secret message to delete"
                );
                self.manager.delete_messages(id, ids.clone()).await?;
                Ok(
                    json!({"chat_id":id,"messages_removed":self.history.delete_saved(id,Some(&ids))?}),
                )
            }
            "clear_secret_history" => {
                let id = id()?;
                ensure!(
                    chat_id(&args, "confirm_chat_id")? == id,
                    "Secret chat confirmation does not match"
                );
                self.manager.flush_history(id).await?;
                Ok(
                    json!({"chat_id":lifted(id),"secret_chat_id":id,"messages_removed":self.history.clear(id)?}),
                )
            }
            "delete_saved_secret_messages" => {
                let id = id()?;
                let selected = if args["message_ids"].is_null() {
                    None
                } else {
                    Some(ids(&args["message_ids"])?)
                };
                let (deleted, failed) =
                    self.history
                        .delete_saved_files(&self.state, id, selected.as_deref())?;
                Ok(
                    json!({"chat_id":lifted(id),"secret_chat_id":id,"deleted":deleted,"failed":failed}),
                )
            }
            "mark_secret_read" => {
                let id = id()?;
                let input = &args["max_date_or_ids"];
                let (ids, cutoff) = if args["message_id"].is_null() && input.is_array() {
                    (ids(input)?, None)
                } else {
                    let rows = self.history.read(id, usize::MAX)?;
                    let received = rows
                        .iter()
                        .filter(|row| row["is_outgoing"] == false)
                        .filter_map(|row| {
                            Some((
                                message_id(row)?,
                                row["date"]
                                    .as_str()?
                                    .parse::<chrono::DateTime<chrono::Utc>>()
                                    .ok()?,
                            ))
                        });
                    let cutoff = if !args["message_id"].is_null() {
                        let wanted = required_int(&args, "message_id")?;
                        Some(
                            received
                                .clone()
                                .find(|(id, _)| *id == wanted)
                                .context("No received message with that ID and date is retained")?
                                .1,
                        )
                    } else if let Some(time) = input.as_i64() {
                        Some(
                            chrono::DateTime::from_timestamp(time, 0)
                                .context("Invalid secret-chat read cutoff")?,
                        )
                    } else {
                        received.clone().map(|(_, date)| date).max()
                    };
                    (
                        received
                            .filter(|(_, date)| cutoff.is_some_and(|cutoff| *date <= cutoff))
                            .map(|(id, _)| id)
                            .collect(),
                        cutoff,
                    )
                };
                let marked = ids.len();
                if marked > 0 {
                    self.manager.mark_read(id, ids).await?;
                }
                Ok(
                    json!({"chat_id":id,"marked":marked,"read_up_to_date":cutoff.map(|date| date.to_rfc3339())}),
                )
            }
            "send_secret_typing" => {
                let id = id()?;
                use kiaro_secret_chat::types::*;
                let name = args["action"].as_str().unwrap_or("typing");
                let action: SendMessageAction = match name {
                    "typing" => SendMessageTypingAction {}.into(),
                    "recording_voice" => SendMessageRecordAudioAction {}.into(),
                    "recording_video" => SendMessageRecordVideoAction {}.into(),
                    "uploading_photo" => SendMessageUploadPhotoAction {}.into(),
                    "uploading_video" => SendMessageUploadVideoAction {}.into(),
                    "uploading_document" => SendMessageUploadDocumentAction {}.into(),
                    "cancel" => SendMessageCancelAction {}.into(),
                    _ => bail!("Unsupported secret-chat typing action"),
                };
                self.manager.set_typing(id, Some(action)).await?;
                Ok(json!({"chat_id":lifted(id),"secret_chat_id":id,"shown":name}))
            }
            "start_secret_auto_save" => Ok(
                json!({"enabled":true,"folder":self.history.set_auto_save_folder(&self.state,Some(Path::new(string(&args,"folder")?)))?}),
            ),
            "stop_secret_auto_save" => {
                self.history.set_auto_save_folder(&self.state, None)?;
                Ok(json!({"enabled":false}))
            }
            "export_secret_chat" => self.start_export(id()?, &args),
            "pause_secret_chat" | "get_secret_pause_status" | "resume_secret_chat" => {
                let id = id()?;
                match method {
                    "pause_secret_chat" => self.manager.pause_chat(id)?,
                    "resume_secret_chat" => self.manager.resume_chat(id).await?,
                    _ => {}
                }
                let pause = self.manager.pause_status(id)?;
                Ok(
                    json!({"chat_id":id,"paused":pause.paused,"resumable":pause.resumable,"reason":pause.reason.map(|reason|format!("{reason:?}")),"records":pause.records,"bytes":pause.bytes}),
                )
            }
            "check_secret_custody" => {
                Ok(json!({"paused_chats":self.manager.check_custody()?,"mode":"ingress"}))
            }
            "list_secret_deliveries" => {
                let id = id()?;
                let offset = integer(&args, "offset").unwrap_or_default().clamp(0, 4096) as usize;
                let rows: Vec<_> = self.manager.pending_deliveries(id)?.into_iter().skip(offset).take(limit()).map(|delivery| {
                    let message = delivery_snapshot(&delivery)?.map(|(row, _)| row);
                    Ok::<_, anyhow::Error>(json!({"chat_id":lifted(id),"secret_chat_id":id,"seq_no":delivery.seq_no,"message":message}))
                }).collect::<Result<_>>()?;
                Ok(json!({"chat_id":id,"deliveries":rows}))
            }
            "acknowledge_secret_delivery" => {
                let id = id()?;
                let seq = i32::try_from(required_int(&args, "seq_no")?)?;
                self.status(id)?;
                if let Some(delivery) = self
                    .manager
                    .pending_deliveries(id)?
                    .into_iter()
                    .find(|delivery| delivery.seq_no == seq)
                {
                    let (row, reference) = delivery_snapshot(&delivery)?
                        .context("No message snapshot for this secret-chat delivery")?;
                    self.history
                        .commit_delivery(id, seq, row, reference.as_ref())?;
                    self.manager.acknowledge_delivery(id, seq)?;
                } else {
                    ensure!(
                        self.history.delivery_committed(id, seq)?,
                        "Unknown secret-chat delivery sequence"
                    );
                }
                Ok(json!({"chat_id":id,"seq_no":seq,"acknowledged":true}))
            }
            _ => bail!("Unknown secret-chat method"),
        }
    }

    async fn send(&self, id: i32, method: &str, args: &Value) -> Result<Value> {
        let media = method.ends_with("media");
        let text = if media {
            args["caption"].as_str().unwrap_or_default()
        } else {
            string(args, "message")?
        };
        let source = if media {
            Some(
                self.state
                    .resolve_file(Path::new(string(args, "file_path")?), FileAccess::Read)?,
            )
        } else {
            None
        };
        let reply = integer(args, "reply_to_message_id");
        let (formatted, entities) = if media {
            (text.to_owned(), None)
        } else {
            format_message(text, args["parse_mode"].as_str())?
        };
        let options = if media {
            Some(self.file_options(args, text, reply)?)
        } else {
            None
        };
        let previous = self.status(id)?.ttl;
        let timed = method.starts_with("send_timed_");
        let ttl = if timed {
            let ttl = i32::try_from(if args["seconds"].is_null() {
                required_int(args, "ttl_seconds")?
            } else {
                required_int(args, "seconds")?
            })?;
            ensure!((1..=604800).contains(&ttl), "Invalid timed secret-chat TTL");
            ttl
        } else {
            previous
        };
        if timed
            && ttl != previous
            && let Err(error) = self.manager.set_ttl(id, ttl).await
        {
            let outcome = match self.manager.status(id) {
                Ok(chat) if chat.ttl == previous => "the local timer is unchanged",
                Ok(_) if self.manager.set_ttl(id, previous).await.is_ok() => {
                    "the prior timer was restored"
                }
                _ => "restoration of the prior timer could not be confirmed",
            };
            return Err(anyhow::Error::new(error)
                .context(format!("Timed secret-chat message was not sent; {outcome}")));
        }
        if timed {
            let live_ttl = self.manager.status(id)?.ttl;
            if live_ttl != ttl {
                if live_ttl != previous {
                    let _ = self.manager.set_ttl(id, previous).await;
                }
                bail!(
                    "Secret-chat timer changed concurrently from {ttl}s to {live_ttl}s before sending; message was not sent"
                );
            }
        }
        let sent = if let Some((path, options)) = source.zip(options) {
            self.manager
                .send_file(id, FileSource::Path { path, name: None }, options)
                .await
        } else {
            self.manager
                .send_message(id, &formatted, entities, reply)
                .await
        };
        let observed_ttl = self.manager.status(id).map(|chat| chat.ttl).unwrap_or(ttl);
        let restoration = if timed && observed_ttl != previous {
            self.manager.set_ttl(id, previous).await
        } else {
            Ok(())
        };
        let msg = match sent {
            Ok(id) => id,
            Err(error) => {
                return Err(anyhow::Error::new(error).context(if restoration.is_ok() {
                    "Secret-chat send did not confirm"
                } else {
                    "Secret-chat send did not confirm and the prior timer could not be restored"
                }));
            }
        };
        let mut result = if observed_ttl == 0 {
            self.sent_result(id, msg, &formatted, media)
        } else {
            json!({"chat_id":id,"message_id":msg})
        };
        result["ttl_seconds"] = json!(observed_ttl);
        if timed && observed_ttl != ttl {
            result["requested_ttl_seconds"] = json!(ttl);
            result["ttl_changed_concurrently"] = json!(true);
        }
        result["previous_timer"] = json!(previous);
        result["timer_restored"] = json!(restoration.is_ok());
        Ok(result)
    }

    fn file_options(
        &self,
        args: &Value,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<SendFileOptions> {
        ensure!(
            integer(args, "self_destruct_seconds").unwrap_or(0) == 0,
            "Per-message media TTL is unsupported; use the timed media tool"
        );
        let kind = if args["as_voice"] == true {
            Some(MediaKind::VoiceNote)
        } else {
            args["kind"].as_str().map(MediaKind::parse).transpose()?
        };
        let number = |name| -> Result<Option<i32>> {
            integer(args, name)
                .map(i32::try_from)
                .transpose()
                .map_err(Into::into)
        };
        let text_field = |name: &str| args[name].as_str().map(str::to_owned);
        let mut metadata = Metadata {
            duration: number("duration")?,
            width: number("width")?,
            height: number("height")?,
            title: text_field("title"),
            performer: text_field("performer"),
            sticker_alt: text_field("sticker_alt"),
            ..Default::default()
        };
        if !args["waveform"].is_null() {
            metadata.waveform = Some(
                serde_json::from_value(args["waveform"].clone())
                    .context("Invalid voice waveform")?,
            );
        }
        if let Some(path) = path(args, "thumbnail_path") {
            let path = self.state.resolve_file(path, FileAccess::Read)?;
            let bytes = super::super::read_private(&path, 32 * 1024)?
                .context("Secret media thumbnail is unavailable")?;
            let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()?
                .into_dimensions()?;
            metadata.thumbnail_size = Some((width.try_into()?, height.try_into()?));
            metadata.thumbnail = Some(bytes);
        }
        Ok(SendFileOptions {
            caption: text.into(),
            kind,
            reply_to,
            metadata,
            ..Default::default()
        })
    }

    fn sent_result(&self, id: i32, random_id: i64, text: &str, media: bool) -> Value {
        let mut result = json!({"chat_id":id,"message_id":random_id});
        if self.record_sent(id, random_id, text, media).is_err() {
            result["history_warning"] =
                json!("Telegram confirmed the send, but local history could not be saved");
        }
        result
    }

    fn record_sent(&self, id: i32, random_id: i64, text: &str, media: bool) -> Result<()> {
        if self.manager.status(id)?.ttl == 0 {
            let row = row(random_id, true, text, 0, media);
            self.history.append(id, row.clone(), None)?;
            self.history.archive_row(&self.state, id, &row)?;
        }
        Ok(())
    }

    fn backend_status(&self) -> Result<Value> {
        Ok(
            json!({"account":self.label,"mode":"ingress","ready":true,"durable_ingress":true,"auto_save_folder":self.history.auto_save_folder()?}),
        )
    }
}
