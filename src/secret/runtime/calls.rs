use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use grammers_tl_types::{Deserializable, Serializable};
use kiaro_secret_chat::{
    enums::SendMessageAction,
    media::{MediaKind, Metadata},
};
use serde_json::json;

impl SecretRuntime {
    /// Dispatch a public secret-chat method using its tool parameter names.
    pub async fn call(&self, api: &dyn Telegram, method: &str, args: Value) -> Result<Value> {
        ensure!(args.is_object(), "Secret-chat arguments must be an object");
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
                self.backend_status()
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
                    let removed = self.history.erase_local_chat(id)?;
                    return Ok(
                        json!({"chat_id":id,"closed":true,"forgotten":true,"protocol_tombstone_retained":true,"messages_removed":removed}),
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
                self.status(id)?;
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
                let honour = args["honour_sender_restriction"].as_bool().unwrap_or(false);
                let reference = self.history.media_with_restriction(id, message, honour)?;
                Ok(
                    json!({"chat_id":lifted(id),"secret_chat_id":id,"message_id":message,"saved":true,"path":self.write_media(&reference,message,path(&args,"destination")).await?}),
                )
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
                self.record_sent(to, id, caption, true)?;
                Ok(json!({"chat_id":to,"message_id":id}))
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
                let sent = self.manager.send_message(target, text, None, None).await?;
                self.record_sent(target, sent, text, false)?;
                Ok(json!({"chat_id":target,"message_id":sent}))
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
                Ok(
                    json!({"chat_id":id,"messages_removed":self.history.delete_saved(id,selected.as_deref())?}),
                )
            }
            "mark_secret_read" => {
                let id = id()?;
                let input = &args["max_date_or_ids"];
                let ids = if !args["message_id"].is_null() {
                    vec![required_int(&args, "message_id")?]
                } else if input.is_array() {
                    ids(input)?
                } else {
                    let cutoff = input.as_i64();
                    self.history
                        .read(id, 1000)?
                        .iter()
                        .filter(|row| {
                            row["is_outgoing"] == false
                                && cutoff.is_none_or(|time| {
                                    row["date"]
                                        .as_str()
                                        .and_then(|date| {
                                            chrono::DateTime::parse_from_rfc3339(date).ok()
                                        })
                                        .is_some_and(|date| date.timestamp() <= time)
                                })
                        })
                        .filter_map(message_id)
                        .collect()
                };
                if !ids.is_empty() {
                    self.manager.mark_read(id, ids.clone()).await?;
                }
                Ok(json!({"chat_id":id,"marked":ids.len()}))
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
            "export_secret_chat" => {
                let id = id()?;
                for field in [
                    "photos",
                    "videos",
                    "voice_messages",
                    "video_messages",
                    "stickers",
                    "gifs",
                    "files",
                    "size_limit_mb",
                    "date_from",
                    "date_to",
                    "peer_user",
                ] {
                    ensure!(
                        args[field].is_null() || args[field].as_str() == Some(""),
                        "Secret-chat export filter {field} is not supported"
                    );
                }
                let destination = path(&args, "destination");
                let format = args["format"]
                    .as_str()
                    .filter(|format| !format.is_empty())
                    .or_else(|| destination.and_then(|path| path.extension()?.to_str()))
                    .unwrap_or("json");
                Ok(
                    json!({"chat_id":id,"path":self.export(id,ExportFormat::parse(format)?,destination,args["include_media"].as_bool().unwrap_or(false)).await?}),
                )
            }
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
                    let wrapper = kiaro_secret_chat::framing::unwrap(&delivery.message.body)?;
                    use kiaro_secret_chat::enums::DecryptedMessage as Message;
                    let message = match wrapper.message {
                        Message::Message(msg) => Some(row(msg.random_id,false,&msg.message,msg.ttl,delivery.message.file.is_some())),
                        Message::Legacy36b091de(msg) => Some(row(msg.random_id,false,&msg.message,msg.ttl,delivery.message.file.is_some())),
                        Message::Legacy204d3878(msg) => Some(row(msg.random_id,false,&msg.message,msg.ttl,delivery.message.file.is_some())),
                        Message::Legacy1f814f1f(msg) => Some(row(msg.random_id,false,&msg.message,0,delivery.message.file.is_some())),
                        _ => None,
                    };
                    Ok::<_,SecretError>(json!({"chat_id":lifted(id),"secret_chat_id":id,"seq_no":delivery.seq_no,"message":message}))
                }).collect::<std::result::Result<_,_>>()?;
                Ok(json!({"chat_id":id,"deliveries":rows}))
            }
            "acknowledge_secret_delivery" => {
                let id = id()?;
                let seq = i32::try_from(required_int(&args, "seq_no")?)?;
                self.manager.acknowledge_delivery(id, seq)?;
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
        if timed && ttl != previous {
            self.manager.set_ttl(id, ttl).await?;
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
        let restoration = if timed && ttl != previous {
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
        if ttl == 0 {
            self.record_sent(id, msg, &formatted, media)?;
        }
        Ok(
            json!({"chat_id":id,"message_id":msg,"ttl_seconds":ttl,"previous_timer":previous,"timer_restored":restoration.is_ok()}),
        )
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

    fn record_sent(&self, id: i32, random_id: i64, text: &str, media: bool) -> Result<()> {
        let ttl = self.manager.status(id)?.ttl;
        if ttl == 0 {
            self.history
                .append(id, row(random_id, true, text, ttl, media), None)?;
        }
        Ok(())
    }

    fn backend_status(&self) -> Result<Value> {
        Ok(
            json!({"account":self.label,"mode":"ingress","ready":true,"durable_ingress":true,"auto_save_folder":self.history.auto_save_folder()?}),
        )
    }
}

fn format_message(
    text: &str,
    mode: Option<&str>,
) -> Result<(String, Option<Vec<tl::enums::MessageEntity>>)> {
    let (text, entities) = match mode.unwrap_or("plain").to_ascii_lowercase().as_str() {
        "plain" | "none" | "" => return Ok((text.into(), None)),
        "markdown" | "md" => grammers_client::parsers::parse_markdown_message(text),
        "html" => grammers_client::parsers::parse_html_message(text),
        _ => bail!("Unsupported secret-chat parse mode"),
    };
    for entity in &entities {
        ensure!(
            kiaro_secret_chat::enums::MessageEntity::from_bytes(&entity.to_bytes()).is_ok(),
            "The secret-chat layer does not support one of the message entities"
        );
    }
    Ok((text, Some(entities)))
}

fn integer(args: &Value, key: &str) -> Option<i64> {
    args[key]
        .as_i64()
        .or_else(|| args[key].as_str()?.parse().ok())
}
fn required_int(args: &Value, key: &str) -> Result<i64> {
    integer(args, key).with_context(|| format!("A valid {key} is required"))
}
fn chat_id(args: &Value, key: &str) -> Result<i32> {
    positive_chat(required_int(args, key)?)
}
const CHAT_OFFSET: i64 = 2_000_000_000_000;
fn lifted(id: i32) -> i64 {
    i64::from(id) - CHAT_OFFSET
}
fn positive_chat(value: i64) -> Result<i32> {
    let id = i32::try_from(if value < 0 {
        value + CHAT_OFFSET
    } else if value >= CHAT_OFFSET {
        value - CHAT_OFFSET
    } else {
        value
    })?;
    ensure!(id > 0, "Invalid secret-chat ID");
    Ok(id)
}
fn alias_chat(args: &Value, field: &str, alias: &str) -> Result<i32> {
    if args[field].is_null() {
        return chat_id(args, alias);
    }
    let id = chat_id(args, field)?;
    ensure!(
        args[alias].is_null() || chat_id(args, alias)? == id,
        "Secret-chat identifiers disagree"
    );
    Ok(id)
}
fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .with_context(|| format!("{key} must be text"))
}
fn path<'a>(args: &'a Value, key: &str) -> Option<&'a Path> {
    args[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(Path::new)
}
fn ids(value: &Value) -> Result<Vec<i64>> {
    let items = value.as_array().context("Message IDs must be an array")?;
    ensure!(items.len() <= 1000, "Too many secret message IDs");
    items
        .iter()
        .map(|v| {
            v.as_i64()
                .or_else(|| v.as_str()?.parse().ok())
                .context("Invalid secret message ID")
        })
        .collect()
}
fn snapshot(chat: ChatSnapshot) -> Value {
    json!({"chat_id":lifted(chat.id),"secret_chat_id":chat.id,"peer_user_id":chat.peer_user_id,"is_outbound":chat.is_outbound,"state":chat.state,"ttl":chat.ttl,"layer":chat.capability_layer,"key_fingerprint":chat.key_fingerprint,"has_previous_key":chat.has_previous_key,"exchange_in_progress":chat.exchange_in_progress,"paused":chat.paused})
}
