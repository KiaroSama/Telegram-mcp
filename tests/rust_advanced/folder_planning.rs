use super::common::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
struct Planner {
    filters: Mutex<Value>,
    calls: Mutex<Vec<(String, Value)>>,
    config: Value,
    premium: Option<bool>,
    race: AtomicBool,
    wrong: bool,
    reject: bool,
}
fn definition() -> Value {
    json!({"_":"dialogFilter","id":2,"title":{"_":"textWithEntities","text":"Original","entities":[]},"emoticon":"📁","color":3,"pinned_peers":[{"_":"inputPeerUser","user_id":"1","access_hash":"101"}],"include_peers":[{"_":"inputPeerUser","user_id":"2","access_hash":"202"}],"exclude_peers":[{"_":"inputPeerChannel","channel_id":"3","access_hash":"303"}],"contacts":true,"non_contacts":false,"groups":true,"broadcasts":false,"bots":true,"exclude_muted":true,"exclude_read":true,"exclude_archived":true,"title_noanimate":true})
}
fn config() -> Value {
    let mut rows = Vec::new();
    for (key, value) in [
        ("dialog_filters_limit_default", 7),
        ("dialog_filters_limit_premium", 23),
        ("dialog_filters_chats_limit_default", 3),
        ("dialog_filters_chats_limit_premium", 5),
        ("dialogs_folder_pinned_limit_default", 2),
        ("dialogs_folder_pinned_limit_premium", 4),
    ] {
        rows.push(json!({"_":"jsonObjectValue","key":key,"value":{"_":"jsonNumber","value":value as f64}}));
    }
    json!({"_":"help.appConfig","config":{"_":"jsonObject","value":rows}})
}
impl Planner {
    fn new() -> Self {
        Self {
            filters: Mutex::new(
                json!({"filters":[{"_":"dialogFilterDefault"},definition()],"tags_enabled":true}),
            ),
            calls: Mutex::new(vec![]),
            config: config(),
            premium: Some(false),
            race: AtomicBool::new(false),
            wrong: false,
            reject: false,
        }
    }
    fn writes(&self) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == "messages.updateDialogFilter")
            .map(|(_, v)| v.clone())
            .collect()
    }
}
#[async_trait]
impl Telegram for Planner {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.into(), args.clone()));
        match name {
            "messages.getDialogFilters" => Ok(self.filters.lock().unwrap().clone()),
            "users.getUsers" => {
                let mut user = json!({"id":"99"});
                if let Some(p) = self.premium {
                    user["premium"] = json!(p);
                }
                Ok(json!([user]))
            }
            "help.getAppConfig" => Ok(self.config.clone()),
            "messages.updateDialogFilter" => {
                if self.reject {
                    bail!("Synthetic server refusal");
                }
                self.filters.lock().unwrap()["filters"][1] = args["filter"].clone();
                Ok(json!(true))
            }
            _ => bail!("Unexpected request: {name}"),
        }
    }
    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        if self.race.swap(false, Ordering::SeqCst) {
            self.filters.lock().unwrap()["filters"][1]["exclude_archived"] = json!(false);
        }
        let n = value
            .as_i64()
            .or_else(|| value.as_str()?.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Bad test peer ID"))?;
        if self.wrong {
            return Ok(json!({"_":"inputPeerChat","chat_id":"9"}));
        }
        Ok(if n == 99 {
            json!({"_":"inputPeerSelf"})
        } else if n < -1_000_000_000_000i64 {
            json!({"_":"inputPeerChannel","channel_id":(-1_000_000_000_000i64-n).to_string(),"access_hash":"999"})
        } else if n < 0 {
            json!({"_":"inputPeerChat","chat_id":(-n).to_string()})
        } else {
            json!({"_":"inputPeerUser","user_id":n.to_string(),"access_hash":"999"})
        })
    }
}
async fn call(api: &Planner, name: &str, args: Value) -> Result<Value> {
    advanced::call(api, name, &args).await.unwrap()
}
#[tokio::test]
async fn folder_limits_select_free_and_premium_tiers() {
    for (premium, folder, chats) in [(false, 7, 3), (true, 23, 5)] {
        let mut p = Planner::new();
        p.premium = Some(premium);
        let r = call(&p, "get_folder_limits", json!({})).await.unwrap();
        assert_eq!(r["premium"], premium);
        assert_eq!(r["limits"]["folders"], folder);
        assert_eq!(r["limits"]["chats_per_folder"], chats);
        assert_eq!(r["title_limit_utf16"], 12);
        assert!(p.writes().is_empty());
    }
}
#[tokio::test]
async fn unknown_premium_status_does_not_invent_effective_limits() {
    let mut p = Planner::new();
    p.premium = None;
    let r = call(&p, "get_folder_limits", json!({})).await.unwrap();
    assert!(r["limits"]["folders"].is_null());
    assert!(r["config_keys"]["folders"].is_null());
}
#[tokio::test]
async fn unavailable_config_leaves_limits_unknown() {
    let mut p = Planner::new();
    p.config = json!({"_":"help.appConfigNotModified"});
    let r = call(&p, "get_folder_limits", json!({})).await.unwrap();
    assert_eq!(r["config_available"], false);
    assert!(r["limits"]["folders"].is_null());
}
#[tokio::test]
async fn fractional_and_boolean_config_limits_are_not_truncated() {
    let mut p = Planner::new();
    p.config["config"]["value"][0]["value"]["value"] = json!(7.5);
    p.config["config"]["value"][2]["value"] = json!({"_":"jsonBool","value":true});
    let r = call(&p, "get_folder_limits", json!({})).await.unwrap();
    assert!(r["limits"]["folders"].is_null());
    assert!(r["limits"]["chats_per_folder"].is_null());
}
#[tokio::test]
async fn complete_folder_snapshot_contains_order_flags_and_no_access_hashes() {
    let p = Planner::new();
    p.filters.lock().unwrap()["filters"].as_array_mut().unwrap().push(json!({"_":"dialogFilterChatlist","id":5,"title":{"_":"textWithEntities","text":"Shared","entities":[]},"pinned_peers":[],"include_peers":[],"has_my_invites":true}));
    let r = call(&p, "get_folder_snapshot", json!({})).await.unwrap();
    assert_eq!(r["folder_order"], json!([0, 2, 5]));
    assert_eq!(r["tags_enabled"], true);
    let d = &r["folders"][1]["definition"];
    assert_eq!(d["pinned_chat_ids"], json!(["1"]));
    assert_eq!(d["exclude_chat_ids"], json!(["-1000000000003"]));
    assert_eq!(d["exclude_archived"], true);
    assert_eq!(r["folders"][2]["editable"], false);
    assert!(!r.to_string().contains("access_hash"));
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn snapshot_saved_messages_uses_stable_account_id() {
    let p = Planner::new();
    p.filters.lock().unwrap()["filters"][1]["include_peers"] = json!([{"_":"inputPeerSelf"}]);
    let r = call(&p, "get_folder_snapshot", json!({})).await.unwrap();
    assert_eq!(
        r["folders"][1]["definition"]["include_chat_ids"],
        json!(["99"])
    );
}
#[tokio::test]
async fn folder_patch_preserves_every_omitted_field() {
    let p = Planner::new();
    let before = p.filters.lock().unwrap()["filters"][1].clone();
    let r = call(
        &p,
        "update_folder",
        json!({"folder_id":2,"patch":{"title":"New"}}),
    )
    .await
    .unwrap();
    assert_eq!(r["changed"], true);
    let mut expected = before;
    expected["title"] = json!({"_":"textWithEntities","text":"New","entities":[]});
    assert_eq!(p.writes()[0]["filter"], expected);
    assert_eq!(p.writes()[0]["id"], 2);
}
#[tokio::test]
async fn folder_archived_rule_patch_never_unarchives_chats() {
    let p = Planner::new();
    call(
        &p,
        "update_folder",
        json!({"folder_id":2,"patch":{"exclude_archived":false}}),
    )
    .await
    .unwrap();
    assert_eq!(p.writes()[0]["filter"]["exclude_archived"], false);
    assert!(p.calls.lock().unwrap().iter().all(|(n, _)| {
        [
            "messages.getDialogFilters",
            "users.getUsers",
            "help.getAppConfig",
            "messages.updateDialogFilter",
        ]
        .contains(&n.as_str())
    }));
}
#[tokio::test]
async fn no_op_folder_patch_does_not_write() {
    let p = Planner::new();
    let r = call(&p, "update_folder", json!({"folder_id":2,"patch":{}}))
        .await
        .unwrap();
    assert_eq!(r["changed"], false);
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn snapshot_definition_can_restore_a_private_folder() {
    let p = Planner::new();
    let snapshot = call(&p, "get_folder_snapshot", json!({})).await.unwrap();
    let original = snapshot["folders"][1]["definition"].clone();
    call(&p,"update_folder",json!({"folder_id":2,"patch":{"title":"New","exclude_archived":false,"include_chat_ids":[9]}})).await.unwrap();
    call(&p, "update_folder", json!({"folder_id":2,"patch":original}))
        .await
        .unwrap();
    let restored = call(&p, "get_folder_snapshot", json!({})).await.unwrap();
    assert_eq!(restored["folders"][1]["definition"], original);
}
#[tokio::test]
async fn stale_folder_revision_refuses_before_write() {
    let p = Planner::new();
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"title":"New"},"expected_revision":"0".repeat(64)})
        )
        .await
        .is_err()
    );
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn folder_refetch_detects_external_change_during_peer_resolution() {
    let p = Planner::new();
    p.race.store(true, Ordering::SeqCst);
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"include_chat_ids":[9]}})
        )
        .await
        .is_err()
    );
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn folder_pinned_union_obeys_premium_specific_peer_limit() {
    for (premium, allowed) in [(false, false), (true, true)] {
        let mut p = Planner::new();
        p.premium = Some(premium);
        let r = call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"include_chat_ids":[10,11,12]}}),
        )
        .await;
        assert_eq!(r.is_ok(), allowed);
        assert_eq!(!p.writes().is_empty(), allowed);
    }
}
#[tokio::test]
async fn included_and_excluded_folder_peers_cannot_overlap() {
    let p = Planner::new();
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"exclude_chat_ids":[1]}})
        )
        .await
        .is_err()
    );
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn empty_folder_inclusion_is_refused() {
    let p = Planner::new();
    assert!(call(&p,"update_folder",json!({"folder_id":2,"patch":{"include_chat_ids":[],"pinned_chat_ids":[],"contacts":false,"non_contacts":false,"groups":false,"broadcasts":false,"bots":false}})).await.is_err());
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn folder_peer_resolution_must_match_requested_marked_id() {
    let mut p = Planner::new();
    p.wrong = true;
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"include_chat_ids":[9]}})
        )
        .await
        .is_err()
    );
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn invalid_folder_patch_is_refused_before_any_rpc() {
    for args in [
        json!({"folder_id":1,"patch":{"title":"New"}}),
        json!({"folder_id":"2","patch":{"title":"New"}}),
        json!({"folder_id":2,"patch":{"include_peers":[]}}),
        json!({"folder_id":2,"patch":{"title":"😀".repeat(7)}}),
        json!({"folder_id":2,"patch":{"include_chat_ids":[1,1]}}),
        json!({"folder_id":2,"patch":{"color":8}}),
        json!({"folder_id":2,"patch":{"exclude_archived":"false"}}),
    ] {
        let p = Planner::new();
        assert!(call(&p, "update_folder", args).await.is_err());
        assert!(p.calls.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn folder_title_entities_accept_upstream_constructor_names_and_utf16_bounds() {
    let p = Planner::new();
    call(&p,"update_folder",json!({"folder_id":2,"patch":{"title":"🙂","title_entities":[{"_":"MessageEntityCustomEmoji","offset":0,"length":2,"document_id":"123"}],"color":null,"emoticon":null}})).await.unwrap();
    let f = &p.writes()[0]["filter"];
    assert_eq!(f["title"]["entities"][0]["_"], "messageEntityCustomEmoji");
    assert_eq!(f["title"]["entities"][0]["document_id"], "123");
    assert!(f["color"].is_null());
    assert!(f["emoticon"].is_null());
}
#[tokio::test]
async fn invalid_folder_title_entities_are_refused_before_rpc() {
    let p = Planner::new();
    assert!(call(&p,"update_folder",json!({"folder_id":2,"patch":{"title":"🙂","title_entities":[{"_":"MessageEntityCustomEmoji","offset":1,"length":2,"document_id":"123"}]}})).await.is_err());
    assert!(p.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn shared_folders_are_read_only_for_private_patches() {
    let p = Planner::new();
    p.filters.lock().unwrap()["filters"][1]["_"] = json!("dialogFilterChatlist");
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"title":"New"}})
        )
        .await
        .is_err()
    );
    assert!(p.writes().is_empty());
}
#[tokio::test]
async fn server_folder_refusal_is_not_reported_as_success() {
    let mut p = Planner::new();
    p.reject = true;
    assert!(
        call(
            &p,
            "update_folder",
            json!({"folder_id":2,"patch":{"title":"New"}})
        )
        .await
        .is_err()
    );
    assert_eq!(
        p.filters.lock().unwrap()["filters"][1]["title"]["text"],
        "Original"
    );
}
