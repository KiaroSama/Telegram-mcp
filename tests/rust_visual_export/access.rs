use super::*;

async fn migrated_export(folder: &str) -> (Value, Fixture) {
    let api = Fixture {
        complex: true,
        ..Default::default()
    };
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let result = export::run(
        &api,
        "chat",
        &json!({"chat_id":"@test"}),
        &settings(folder),
        receiver,
        folder,
    )
    .await
    .unwrap();
    (result, api)
}

fn legacy_history_reads(api: &Fixture) -> usize {
    api.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, request)| {
            let mut query = request;
            while query.get("query").is_some() {
                query = &query["query"];
            }
            query["_"] == "messages.getHistory" && query["peer"]["_"] == "inputPeerChat"
        })
        .count()
}

#[test]
fn supergroup_permission_does_not_export_its_denied_legacy_group() {
    if isolated_with_allowlist(
        "access::supergroup_permission_does_not_export_its_denied_legacy_group",
        Some("-1000000000012"),
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (result, api) = migrated_export("legacy-denied").await;
        assert_eq!(result["messages"], 2);
        assert_eq!(legacy_history_reads(&api), 0);
    });
}

#[test]
fn separately_allowed_legacy_group_retains_migrated_history() {
    if isolated_with_allowlist(
        "access::separately_allowed_legacy_group_retains_migrated_history",
        Some("-1000000000012,-42"),
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (result, api) = migrated_export("legacy-allowed").await;
        assert_eq!(result["messages"], 4);
        assert_eq!(legacy_history_reads(&api), 2);
    });
}

#[test]
fn topic_only_legacy_permission_cannot_authorize_a_whole_chat_export() {
    if isolated_with_allowlist(
        "access::topic_only_legacy_permission_cannot_authorize_a_whole_chat_export",
        Some("-1000000000012,-42:9"),
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (result, api) = migrated_export("legacy-topic-denied").await;
        assert_eq!(result["messages"], 2);
        assert_eq!(legacy_history_reads(&api), 0);
    });
}
