use super::*;
use rmcp::{
    ServerHandler, ServiceExt,
    model::*,
    service::{RequestContext, RoleServer},
};
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;
struct Fixture {
    calls: Calls,
}

fn png(colour: [u8; 4]) -> Vec<u8> {
    let image =
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba(colour)));
    crate::tools::visual::images::encode_image(&image, "png", 64, 90, false)
        .unwrap()
        .0
}

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        let args = Value::Object(request.arguments.unwrap_or_default());
        self.calls
            .lock()
            .unwrap()
            .push((request.name.to_string(), args.clone()));
        if request.name.as_ref() == "refuse" {
            return Ok(CallToolResult::error(vec![ContentBlock::text("fixture refused")]).into());
        }
        let mut content = Vec::new();
        let payload = if request.name.as_ref() == "get_custom_emoji" {
            let mut rows = Vec::new();
            for id in args["document_ids"].as_array().unwrap() {
                let count = args["count"].as_u64().unwrap_or(1);
                let mut metadata = Vec::new();
                for frame in 0..count {
                    let data = png(if frame == 0 && count > 1 {
                        [0, 0, 0, 0]
                    } else {
                        [255, 0, 0, 255]
                    });
                    metadata.push(json!({"blank":frame==0&&count>1,"bytes":data.len()}));
                    content.push(json!({"type":"image","mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(data)}));
                }
                rows.push(json!({"document_id":id,"placeholder":"✅","preview":metadata}));
            }
            json!({"metadata":{"emoji":rows}})
        } else if request.name.as_ref() == "inspect_sticker_set" {
            json!({"documents":[{"id":"9007199254740993"},{"id":"9007199254740994"}]})
        } else if request.name.as_ref() == "edit_message" {
            json!({"approval_required":true,"performed":false,"human_action":"Approve the exact operation in a genuine human terminal"})
        } else {
            json!({"ok":true})
        };
        content.push(json!({"type":"text","text":payload.to_string()}));
        let result: CallToolResult = serde_json::from_value(json!({"content":content})).unwrap();
        Ok(result.into())
    }
}

async fn connect() -> (Studio, Calls, tokio::task::JoinHandle<()>) {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let calls = Calls::default();
    let fixture = Fixture {
        calls: Arc::clone(&calls),
    };
    let server = tokio::spawn(async move {
        let service = fixture.serve(server_io).await.unwrap();
        let _ = service.waiting().await;
    });
    let remote = ().serve(client_io).await.unwrap();
    (
        Studio {
            remote,
            account: Some("work".into()),
        },
        calls,
        server,
    )
}

#[tokio::test]
async fn studio_reuses_one_mcp_session_and_keeps_all_batch_tail_documents() {
    let (studio, calls, server) = connect().await;
    let ids: Vec<String> = (1..=12).map(|id| id.to_string()).collect();
    let (images, facts) = studio.frames(&ids, 96, 3).await.unwrap();
    assert_eq!(images.len(), 12);
    assert_eq!(facts.len(), 12);
    assert_eq!(images["12"].len(), 3);
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(calls.lock().unwrap()[0].1["account"], "work");
    assert_eq!(
        calls.lock().unwrap()[1].1["document_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    studio.remote.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn studio_fingerprints_a_lit_frame_instead_of_the_blank_first_frame() {
    let (studio, _, server) = connect().await;
    let images = studio.images(&["1".into()], 96, 3).await.unwrap();
    let image = images["1"].to_rgba8();
    assert!(
        image
            .pixels()
            .all(|pixel| pixel[0] == 255 && pixel[3] == 255)
    );
    studio.remote.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn studio_metadata_preserves_placeholder_and_exact_long_document_ids() {
    let (studio, _, server) = connect().await;
    let facts = studio.meta(&["9007199254740993".into()]).await.unwrap();
    assert_eq!(facts["9007199254740993"]["placeholder"], "✅");
    let ids = studio.pack_ids("pack").await.unwrap();
    assert_eq!(ids, vec!["9007199254740993", "9007199254740994"]);
    studio.remote.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn studio_propagates_human_approval_without_retrying_a_write() {
    let (studio, calls, server) = connect().await;
    let reply = studio
        .call(
            "edit_message",
            json!({"chat_id":"1","message_id":2,"new_text":"x"}),
        )
        .await
        .unwrap();
    assert_eq!(reply.payload["approval_required"], true);
    assert_eq!(reply.payload["performed"], false);
    assert_eq!(calls.lock().unwrap().len(), 1);
    studio.remote.cancel().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn studio_surfaces_mcp_tool_errors_without_invented_images() {
    let (studio, _, server) = connect().await;
    assert!(
        studio
            .call("refuse", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("refused")
    );
    studio.remote.cancel().await.unwrap();
    server.await.unwrap();
}
