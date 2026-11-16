use super::super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const KEYS: &[(&str, &str)] = &[
    ("status", "inputPrivacyKeyStatusTimestamp"),
    ("phone", "inputPrivacyKeyPhoneNumber"),
    ("profile_photo", "inputPrivacyKeyProfilePhoto"),
    ("chat_invite", "inputPrivacyKeyChatInvite"),
    ("phone_call", "inputPrivacyKeyPhoneCall"),
    ("phone_p2p", "inputPrivacyKeyPhoneP2P"),
    ("forwards", "inputPrivacyKeyForwards"),
    ("voice_messages", "inputPrivacyKeyVoiceMessages"),
    ("about", "inputPrivacyKeyAbout"),
    ("birthday", "inputPrivacyKeyBirthday"),
    ("added_by_phone", "inputPrivacyKeyAddedByPhone"),
    ("saved_music", "inputPrivacyKeySavedMusic"),
];
const RULES: &[(&str, &str)] = &[
    ("everyone_allowed", "AllowAll"),
    ("contacts_allowed", "AllowContacts"),
    ("close_friends_allowed", "AllowCloseFriends"),
    ("premium_allowed", "AllowPremium"),
    ("everyone_disallowed", "DisallowAll"),
    ("contacts_disallowed", "DisallowContacts"),
    ("users_allowed", "AllowUsers"),
    ("users_disallowed", "DisallowUsers"),
    ("chats_allowed", "AllowChatParticipants"),
    ("chats_disallowed", "DisallowChatParticipants"),
    ("bots_allowed", "AllowBots"),
    ("bots_disallowed", "DisallowBots"),
];

pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let key = string(a, "key", "status")?;
    let input_key=KEYS.iter().find(|(name,_)|*name==key).map(|(_,value)|*value).context("Unsupported privacy key; use status, phone, profile_photo, chat_invite, phone_call, phone_p2p, forwards, voice_messages, about, birthday, added_by_phone or saved_music")?;
    if name == "get_privacy_settings" {
        let r = api
            .invoke("account.getPrivacy", json!({"key":{"_":input_key}}))
            .await?;
        let described = r
            .get("rules")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|r| {
                let raw = kind(r).strip_prefix("privacyValue").unwrap_or(kind(r));
                let name = RULES
                    .iter()
                    .find(|(_, suffix)| *suffix == raw)
                    .map(|(n, _)| *n)
                    .unwrap_or(raw);
                let mut row = json!({"rule":name});
                copy_present(r, &mut row, &["users", "chats"]);
                row
            })
            .collect::<Vec<_>>();
        return Ok(
            json!({"results":described,"metadata":{"key":key,"rule_count":described.len()}}),
        );
    }
    let mut built = Vec::new();
    if let Some(described) = a.get("rules").filter(|v| !v.is_null()) {
        let rows = described.as_array().context("rules must be an array")?;
        ensure!(
            !rows.is_empty(),
            "rules was empty; account.setPrivacy would clear every rule"
        );
        // Validate all rule kinds before resolving even one user.
        for r in rows {
            let rule = text(r, "rule")?;
            ensure!(
                RULES.iter().any(|(name, _)| *name == rule),
                "Unknown privacy rule kind: {rule}"
            );
            for key in ["users", "chats"] {
                if let Some(v) = r.get(key) {
                    ensure!(v.is_array(), "{key} must be an array");
                }
            }
        }
        for r in rows {
            let rule = text(r, "rule")?;
            let suffix = RULES
                .iter()
                .find(|(name, _)| *name == rule)
                .context("validated rule")?
                .1;
            let mut input = json!({"_":format!("inputPrivacyValue{suffix}")});
            if rule == "users_allowed" || rule == "users_disallowed" {
                let values = r
                    .get("users")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                input["users"] = json!(users(api, &values).await?);
            }
            if rule == "chats_allowed" || rule == "chats_disallowed" {
                let values = r
                    .get("chats")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut ids = Vec::new();
                for v in values {
                    ids.push(int_value(&v, "chat id")?.to_string());
                }
                input["chats"] = ids.into();
            }
            built.push(input);
        }
    } else {
        let policy=text(a,"base_policy").context("base_policy is required: account.setPrivacy replaces the entire rule set; use everyone, contacts or nobody, or pass the complete rules list")?.trim().to_lowercase();
        let base = match policy.as_str() {
            "everyone" => "inputPrivacyValueAllowAll",
            "contacts" => "inputPrivacyValueAllowContacts",
            "nobody" => "inputPrivacyValueDisallowAll",
            _ => anyhow::bail!("base_policy must be everyone, contacts or nobody"),
        };
        let allow = a
            .get("allow_users")
            .filter(|v| !v.is_null())
            .map(|v| v.as_array().context("allow_users must be an array"))
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let disallow = a
            .get("disallow_users")
            .filter(|v| !v.is_null())
            .map(|v| v.as_array().context("disallow_users must be an array"))
            .transpose()?
            .cloned()
            .unwrap_or_default();
        ensure!(
            !allow.iter().any(|v| disallow.contains(v)),
            "A user appears on both allow_users and disallow_users"
        );
        let allowed = users(api, &allow).await?;
        let disallowed = users(api, &disallow).await?;
        let identity = |user: &Value| -> Result<String> {
            if kind(user) == "inputUserSelf" {
                return Ok("self".into());
            }
            Ok(int_value(req(user, "user_id")?, "user id")?.to_string())
        };
        let allowed_ids = allowed
            .iter()
            .map(identity)
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        for user in &disallowed {
            ensure!(
                !allowed_ids.contains(&identity(user)?),
                "A resolved user appears on both allow_users and disallow_users"
            );
        }
        if !allow.is_empty() {
            built.push(json!({"_":"inputPrivacyValueAllowUsers","users":allowed}));
        }
        if !disallow.is_empty() {
            built.push(json!({"_":"inputPrivacyValueDisallowUsers","users":disallowed}));
        }
        // Telegram uses the first matching rule, so exceptions must precede the base policy.
        built.push(json!({"_":base}));
    }
    api.invoke(
        "account.setPrivacy",
        json!({"key":{"_":input_key},"rules":built}),
    )
    .await
}
