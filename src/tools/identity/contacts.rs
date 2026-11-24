use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub const NAMES: &[&str] = &[
    "list_contacts",
    "search_contacts",
    "get_contact_ids",
    "get_direct_chat_by_contact",
    "get_contact_chats",
    "get_last_interaction",
    "add_contact",
    "delete_contact",
    "block_user",
    "unblock_user",
    "import_contacts",
    "export_contacts",
    "get_blocked_users",
    "send_contact",
    "resolve_phone",
];

fn records(response: &Value) -> Vec<Value> {
    response
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|u| {
            let mut record = json!({"id":u["id"],"name":display_name(u)});
            for key in ["username", "phone"] {
                if let Some(v) = u.get(key).filter(|v| !v.is_null()) {
                    record[key] = v.clone();
                }
            }
            record
        })
        .collect()
}

pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    match name {
        "resolve_phone" => {
            let phone = text(a, "phone")?.trim();
            ensure!(phone.len() <= 128, "Phone number exceeds its byte limit");
            let clean: String = phone
                .chars()
                .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')'))
                .collect();
            let digits = clean.strip_prefix('+').context(
                "Phone must include + and its country code; a local country code is never guessed",
            )?;
            ensure!(
                (7..=15).contains(&digits.len())
                    && digits.bytes().all(|c| c.is_ascii_digit())
                    && !digits.starts_with('0'),
                "Phone must contain 7 to 15 international digits with a nonzero country code"
            );
            // This lookup does not import, add or otherwise mutate contacts.
            let mut result = api
                .invoke("contacts.resolvePhone", json!({"phone":digits}))
                .await?;
            if result.is_object() {
                result["untrusted_content"] = true.into();
            }
            Ok(result)
        }
        "list_contacts" | "export_contacts" => {
            let result = api
                .invoke("contacts.getContacts", json!({"hash":0}))
                .await?;
            Ok(json!({"results":records(&result),"untrusted_content":true}))
        }
        "search_contacts" => {
            let query = text(a, "query")?;
            ensure!(!query.trim().is_empty(), "query must not be empty");
            let result = api
                .invoke("contacts.search", json!({"q":query,"limit":50}))
                .await?;
            let mut combined = api
                .alias_matches(query)?
                .as_array()
                .context("Alias matches must be an array")?
                .clone();
            combined.extend(records(&result));
            Ok(json!({"results":combined,"untrusted_content":true}))
        }
        "get_contact_ids" => {
            api.invoke("contacts.getContactIDs", json!({"hash":0}))
                .await
        }
        "get_blocked_users" => {
            api.invoke("contacts.getBlocked", json!({"offset":0,"limit":100}))
                .await
        }
        "get_direct_chat_by_contact" => {
            let query = text(a, "contact_query")?.to_lowercase();
            ensure!(!query.trim().is_empty(), "contact_query must not be empty");
            let contacts = api
                .invoke("contacts.getContacts", json!({"hash":0}))
                .await?;
            let mut peers = Vec::new();
            for u in contacts
                .get("users")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let matches = display_name(u).to_lowercase().contains(&query)
                    || ["username", "phone", "id"].iter().any(|k| {
                        u.get(*k)
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .is_some_and(|s| s.to_lowercase().contains(&query))
                    });
                if matches {
                    peers.push(json!({"_":"inputDialogPeer","peer":{"_":"inputPeerUser","user_id":u["id"],"access_hash":u["access_hash"]}}));
                }
            }
            if peers.is_empty() {
                return Ok(json!({"results":[]}));
            }
            let dialogs = api
                .invoke("messages.getPeerDialogs", json!({"peers":peers}))
                .await?;
            let mut rows = Vec::new();
            for dialog in dialogs
                .get("dialogs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if kind(&dialog["peer"]) != "peerUser" {
                    continue;
                }
                let Some(contact) = contacts
                    .get("users")
                    .and_then(Value::as_array)
                    .and_then(|xs| xs.iter().find(|u| u["id"] == dialog["peer"]["user_id"]))
                else {
                    continue;
                };
                rows.push(json!({"chat_id":contact["id"],"contact":display_name(contact),"username":contact.get("username"),"unread":int(dialog,"unread_count",0)?}));
            }
            Ok(json!({"results":rows,"untrusted_content":true}))
        }
        "get_contact_chats" => {
            let p = peer(api, a, "contact_id").await?;
            let u = user(&p)?;
            let direct = api
                .invoke(
                    "messages.getPeerDialogs",
                    json!({"peers":[{"_":"inputDialogPeer","peer":p}]}),
                )
                .await?;
            let common = api
                .invoke(
                    "messages.getCommonChats",
                    json!({"user_id":u,"max_id":0,"limit":100}),
                )
                .await?;
            let mut results = Vec::new();
            for d in direct
                .get("dialogs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                // getPeerDialogs can answer with dialogFolder; it is not a private chat.
                if d.get("peer").is_some() {
                    results.push(json!({"chat_id":entity_id(&d["peer"])?,"type":"Private","unread":d.get("unread_count").unwrap_or(&json!(0))}));
                }
            }
            for c in common
                .get("chats")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = int_value(req(c, "id")?, "chat id")?;
                let marked = if kind(c) == "channel" {
                    (-1_000_000_000_000_i64 - id).to_string()
                } else {
                    (-id).to_string()
                };
                results.push(json!({"chat_id":marked,"name":display_name(c),"type":if kind(c)=="channel" {"Supergroup"} else {"Group"}}));
            }
            Ok(json!({"results":results,"untrusted_content":true}))
        }
        "get_last_interaction" => {
            let p = peer(api, a, "contact_id").await?;
            user(&p)?;
            api.invoke("messages.getHistory",json!({"peer":p,"offset_id":0,"offset_date":0,"add_offset":0,"limit":5,"max_id":0,"min_id":0,"hash":0})).await
        }
        "add_contact" => {
            let first = string(a, "first_name", "")?;
            let last = string(a, "last_name", "")?;
            let name = string(a, "username", "")?;
            let phone = string(a, "phone", "")?;
            ensure!(
                !name.is_empty() || !phone.is_empty(),
                "Either phone or username must be provided"
            );
            if !name.is_empty() {
                let clean = name.trim().trim_start_matches('@');
                ensure!(!clean.is_empty(), "Username cannot be empty");
                // Keep the exact selector used by the server's private peer
                // binding; a second username RPC could target its new owner.
                let p = api.resolve_peer(&json!(name)).await?;
                api.invoke(
                    "contacts.addContact",
                    json!({"id":user(&p)?,"first_name":first,"last_name":last,"phone":phone}),
                )
                .await
            } else {
                api.invoke("contacts.importContacts",json!({"contacts":[{"_":"inputPhoneContact","client_id":0,"phone":phone,"first_name":first,"last_name":last}]})).await
            }
        }
        "import_contacts" => {
            let entries = array(a, "contacts")?;
            ensure!(
                entries.len() <= 1000,
                "Import at most 1000 contacts in one call"
            );
            let mut contacts = Vec::new();
            for (index, c) in entries.iter().enumerate() {
                contacts.push(json!({"_":"inputPhoneContact","client_id":index.to_string(),"phone":text(c,"phone")?,"first_name":text(c,"first_name")?,"last_name":string(c,"last_name","")?}));
            }
            api.invoke("contacts.importContacts", json!({"contacts":contacts}))
                .await
        }
        "delete_contact" => {
            let p = peer(api, a, "user_id").await?;
            api.invoke("contacts.deleteContacts", json!({"id":[user(&p)?]}))
                .await
        }
        "block_user" | "unblock_user" => {
            let p = peer(api, a, "user_id").await?;
            user(&p)?;
            api.invoke(
                if name == "block_user" {
                    "contacts.block"
                } else {
                    "contacts.unblock"
                },
                json!({"id":p}),
            )
            .await
        }
        "send_contact" => {
            let phone = text(a, "phone_number")?;
            let first = text(a, "first_name")?;
            let p = peer(api, a, "chat_id").await?;
            api.invoke("messages.sendMedia",json!({"peer":p,"media":{"_":"inputMediaContact","phone_number":phone,"first_name":first,"last_name":string(a,"last_name","")?,"vcard":string(a,"vcard","")?},"message":"","random_id":crate::args::random_id()})).await
        }
        _ => unreachable!("identity dispatcher restricts names"),
    }
}
