use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "list_peer_stories"
            | "get_stories"
            | "react_to_story"
            | "post_story"
            | "delete_story"
            | "preview_story"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
fn describe(story: &Value, max: i64) -> Value {
    let mut out = story.clone();
    out["seen_by_you"] = json!(id(&story["id"]).is_ok_and(|n| n <= max));
    out["state"] = json!(match story["_"].as_str() {
        Some("storyItemDeleted") => "deleted",
        Some("storyItemSkipped") => "not_fetched",
        _ => "available",
    });
    out
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    if name == "post_story" {
        let privacy = a::text(args, "privacy")?;
        let rule = match privacy {
            "everyone" => "inputPrivacyValueAllowAll",
            "contacts" => "inputPrivacyValueAllowContacts",
            "close_friends" => "inputPrivacyValueAllowCloseFriends",
            _ => bail!("privacy must be everyone, contacts or close_friends"),
        };
        let hours = a::integer(args, "hours", 24)?;
        ensure!(
            [6, 12, 24, 48].contains(&hours),
            "hours must be 6, 12, 24 or 48"
        );
        let path = a::text(args, "file_path")?;
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(
                extension.as_str(),
                "jpg" | "jpeg" | "png" | "webp" | "mp4" | "webm" | "mov"
            ),
            "A story must be a photo or video"
        );
        let p = api
            .resolve_peer(
                args.get("chat_id")
                    .filter(|v| !v.is_null())
                    .unwrap_or(&json!("me")),
            )
            .await?;
        let state = crate::state::State::global()?;
        let absolute =
            state.resolve_file(std::path::Path::new(path), crate::state::FileAccess::Read)?;
        let media = upload_media(
            api,
            absolute
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Story file path must be UTF-8"))?,
        )
        .await?;
        let mut request = json!({"peer":p,"media":media,"privacy_rules":[{"_":rule}],"random_id":a::random_id(),"period":hours*3600,"pinned":a::boolean(args,"pin_to_profile",false)?});
        copy_optional(args, &mut request, &["caption"]);
        return api.invoke("stories.sendStory", request).await;
    }
    let p = api
        .resolve_peer(
            args.get("chat_id")
                .filter(|v| !v.is_null())
                .unwrap_or(&json!("me")),
        )
        .await?;
    match name{
        "list_peer_stories"=>{
            let pinned=a::boolean(args,"pinned",false)?;
            let result=if pinned{api.invoke("stories.getPinnedStories",json!({"peer":p,"offset_id":0,"limit":100})).await?}else{api.invoke("stories.getPeerStories",json!({"peer":p})).await?};
            let peer=if pinned{&result}else{&result["stories"]};let max=peer["max_read_id"].as_i64().unwrap_or(0);
            let rows:Vec<_>=peer["stories"].as_array().into_iter().flatten().map(|v|describe(v,max)).collect();
            Ok(json!({"records":rows,"max_read_id":max,"marked_read":false,"pinned":pinned}))
        }
        "get_stories"|"preview_story"=>{
            let wanted=if name=="preview_story"{vec![positive(a::required(args,"story_id")?)?]}else{ids(a::required(args,"story_ids")?,100)?};unique(&wanted)?;
            let result=api.invoke("stories.getStoriesByID",json!({"peer":p,"id":wanted})).await?;
            let rows=result["stories"].as_array().cloned().unwrap_or_default();let returned:Vec<_>=rows.iter().filter_map(|s|id(&s["id"]).ok()).collect();
            let missing:Vec<_>=wanted.iter().filter(|s|!returned.contains(s)).collect();
            if name=="preview_story"{
                let story=rows.iter().find(|s|s["_"]=="storyItem").ok_or_else(||anyhow::anyhow!("Story is expired, deleted or not visible to this account"))?;
                let max=a::integer(args,"max_bytes",50*1024*1024)?;ensure!((1..=50*1024*1024).contains(&max),"max_bytes must be 1..=50 MiB");
                let count=a::limit(args,"count",4,12)?;let dimension=a::integer(args,"max_dimension",900)?;ensure!((64..=2048).contains(&dimension),"max_dimension must be 64..=2048");
                let location=crate::tools::messaging::media::download_location(&story["media"])?;
                let bytes=crate::tools::messaging::media::download_bytes(api,location,max as u64).await?;
                let mut rendered=crate::tools::messaging::media::previews::render(&bytes,count,dimension).await?;
                rendered["story"]=story.clone();rendered["marked_read"]=json!(false);return Ok(rendered);
            }
            Ok(json!({"records":rows.iter().map(|v|describe(v,0)).collect::<Vec<_>>(),"not_returned":missing,"not_returned_reason":"Expired, deleted or never visible; Telegram does not distinguish these cases.","marked_read":false}))
        }
        "react_to_story"=>api.invoke("stories.sendReaction",json!({"peer":p,"story_id":positive(a::required(args,"story_id")?)?,"reaction":reaction(args,true)?})).await,
        "delete_story"=>api.invoke("stories.deleteStories",json!({"peer":p,"id":ids(a::required(args,"story_id")?,100)?})).await,
        _=>bail!("Unknown story operation"),
    }
}
