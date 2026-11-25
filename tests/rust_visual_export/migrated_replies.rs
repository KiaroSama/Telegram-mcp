use super::*;

struct ReplyFixture(Fixture);

#[async_trait::async_trait]
impl Telegram for ReplyFixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        let mut response = self.0.invoke(method, params.clone()).await?;
        let mut query = &params;
        while let Some(nested) = query.get("query") {
            query = nested;
        }
        if query["_"] == "messages.getHistory" && query["peer"]["_"] == "inputPeerChat" {
            for message in response["messages"].as_array_mut().unwrap() {
                let id = message["id"].as_i64().unwrap();
                let mut raw = json!({"_":"message","id":id,"peer_id":{"_":"peerChat","chat_id":"42"},"date":1700000000,"message":"legacy reply","reply_to":{"_":"messageReplyHeader","reply_to_msg_id":1}});
                if id == 1 {
                    raw["reply_to"]["reply_to_msg_id"] = json!(99);
                    raw["reply_to"]["reply_to_peer_id"] = json!({"_":"peerUser","user_id":"7"});
                }
                let schema = telegram_mcp::tl::Schema::global();
                *message = schema.decode("Message", &schema.encode_object(&raw)?)?;
            }
        }
        Ok(response)
    }

    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.0.resolve_peer(input).await
    }
}

#[test]
fn migrated_export_remaps_decoded_local_replies_without_changing_cross_chat_replies() {
    if isolated(
        "migrated_replies::migrated_export_remaps_decoded_local_replies_without_changing_cross_chat_replies",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = ReplyFixture(Fixture {
            complex: true,
            ..Default::default()
        });
        let settings = settings("migrated-replies");
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        export::run(
            &api,
            "chat",
            &json!({"chat_id":"@test"}),
            &settings,
            receiver,
            "migrated-replies",
        )
        .await
        .unwrap();
        let output: Value =
            serde_json::from_slice(&std::fs::read(settings.path.join("result.json")).unwrap())
                .unwrap();
        let records = output["messages"].as_array().unwrap();
        let local = records
            .iter()
            .find(|message| message["id"] == -999999998)
            .unwrap();
        assert_eq!(
            local["reply_to"].get("reply_to_peer_id"),
            Some(&Value::Null)
        );
        assert_eq!(local["reply_to"]["reply_to_msg_id"], -999999999);
        let external = records
            .iter()
            .find(|message| message["id"] == -999999999)
            .unwrap();
        assert_eq!(external["reply_to"]["reply_to_peer_id"]["user_id"], "7");
        assert_eq!(external["reply_to"]["reply_to_msg_id"], 99);
    });
}
