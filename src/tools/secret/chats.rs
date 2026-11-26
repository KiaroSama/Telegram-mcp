use crate::{args, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub const TOOL_NAMES: &[&str] = &[
    "secret_chat_status",
    "accept_secret_chat",
    "create_secret_chat",
    "list_secret_chats",
    "set_secret_chat_timer",
    "close_secret_chat",
    "forget_secret_chat",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some(dispatch(api, name, args).await)
}

async fn dispatch(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let mut params = payload(name, args)?;
    if name == "create_secret_chat" {
        let peer = api.resolve_peer(&params["user_id"]).await?;
        ensure!(
            peer["_"].as_str() == Some("inputPeerUser"),
            "user_id must resolve to another Telegram user"
        );
        params["user_id"] = peer;
    }
    let response = api.secret_call(name, params).await;
    if name == "accept_secret_chat" {
        return Ok(
            response.unwrap_or_else(|_| json!({"status":"refused","code":"backend_unavailable"}))
        );
    }
    response
}

fn payload(name: &str, args: &Value) -> Result<Value> {
    let params = match name {
        "secret_chat_status" if args["chat_id"].is_null() => json!({}),
        "secret_chat_status" => json!({"chat_id": chat_id(args, "chat_id")?}),
        "accept_secret_chat" => {
            ensure!(
                args["chat_id"].as_i64().is_some(),
                "chat_id must be an integer"
            );
            if !args["account"].is_null() {
                nonempty_text(args, "account")?;
            }
            json!({"chat_id":chat_id(args,"chat_id")?})
        }
        "create_secret_chat" => {
            let user = args::required(args, "user_id")?;
            ensure!(
                user.as_i64().is_some() || user.as_str().is_some_and(|s| !s.trim().is_empty()),
                "user_id must be an integer or a non-empty username or alias"
            );
            json!({"user_id": user})
        }
        "list_secret_chats" => json!({}),
        "set_secret_chat_timer" => {
            let chat_id = chat_id(args, "chat_id")?;
            let seconds = required_integer(args, "seconds")?;
            ensure!(
                (0..=604_800).contains(&seconds),
                "seconds must be between 0 and 604800"
            );
            json!({"chat_id": chat_id, "seconds": seconds})
        }
        "close_secret_chat" | "forget_secret_chat" => {
            json!({"secret_chat_id": chat_id(args, "secret_chat_id")?})
        }
        _ => bail!("Unknown secret-chat lifecycle tool"),
    };
    with_account(params, args)
}

pub(super) fn required_integer(args: &Value, key: &str) -> Result<i64> {
    args::required(args, key)?;
    args::integer(args, key, 0)
}

pub(super) fn optional_integer(args: &Value, key: &str) -> Result<Option<i64>> {
    (!args[key].is_null())
        .then(|| required_integer(args, key))
        .transpose()
}

pub(super) fn chat_id(args: &Value, key: &str) -> Result<i64> {
    let id = required_integer(args, key)?;
    ensure!(
        crate::secret::wire_chat_id(id).is_some(),
        "{key} must identify a non-zero signed 32-bit secret chat"
    );
    Ok(id)
}

pub(super) fn optional_text<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    (!args[key].is_null())
        .then(|| args::text(args, key))
        .transpose()
}

pub(super) fn nonempty_text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    let text = args::text(args, key)?;
    ensure!(!text.trim().is_empty(), "{key} must not be empty");
    Ok(text)
}

pub(super) fn history_limit(args: &Value) -> Result<i64> {
    let limit = args::integer(args, "limit", 30)?;
    ensure!(
        (1..=500).contains(&limit),
        "limit must be between 1 and 500"
    );
    Ok(limit)
}

pub(super) fn with_account(mut params: Value, args: &Value) -> Result<Value> {
    if let Some(account) = optional_text(args, "account")? {
        params["account"] = json!(account);
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::payload;
    use serde_json::json;

    #[test]
    fn validates_both_chat_id_forms_without_changing_them() {
        for id in [
            17_i64,
            -1_999_999_999_983,
            2_000_000_000_017,
            -698_877_786,
            -2_000_698_877_786,
            1_999_301_122_214,
            i64::from(i32::MIN),
        ] {
            let result = payload("secret_chat_status", &json!({"chat_id":id}));
            assert_eq!(result.ok(), Some(json!({"chat_id":id})));
        }
        for id in [
            0,
            -2_000_000_000_000_i64,
            2_000_000_000_000,
            i64::from(i32::MIN) - 1,
        ] {
            assert!(payload("secret_chat_status", &json!({"chat_id":id})).is_err());
        }
        assert!(payload("close_secret_chat", &json!({"chat_id":17})).is_err());
    }
}
