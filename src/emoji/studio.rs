use super::{compose, packs};
use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use image::DynamicImage;
use rmcp::{
    model::{CallToolRequestParams, CallToolResponse},
    service::{RoleClient, RunningService},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[cfg(test)]
#[path = "../../tests/unit/emoji_studio.rs"]
mod tests;

#[derive(Debug)]
pub struct ToolReply {
    pub content: Vec<Value>,
    pub payload: Value,
}

impl ToolReply {
    pub fn parse(value: Value) -> Result<Self> {
        ensure!(
            value["isError"] != true,
            "The shared MCP tool refused the request: {}",
            value["content"]
        );
        let content = value["content"].as_array().cloned().unwrap_or_default();
        let payload = value
            .get("structuredContent")
            .filter(|value| !value.is_null())
            .cloned()
            .or_else(|| {
                content.iter().rev().find_map(|block| {
                    block["text"]
                        .as_str()
                        .and_then(|text| serde_json::from_str(text).ok())
                })
            })
            .unwrap_or(Value::Null);
        Ok(Self { content, payload })
    }
    pub fn records(&self) -> Vec<Value> {
        self.payload["metadata"]["emoji"]
            .as_array()
            .or_else(|| self.payload["results"].as_array())
            .cloned()
            .unwrap_or_default()
    }
    pub fn image_blocks(&self) -> Vec<&Value> {
        self.content
            .iter()
            .filter(|block| block["type"] == "image")
            .collect()
    }
}

pub type Frames = BTreeMap<String, Vec<(Vec<u8>, Value)>>;
pub type Metadata = BTreeMap<String, Value>;

pub fn group_frames(reply: &ToolReply) -> Result<(Frames, Metadata)> {
    let mut images = BTreeMap::new();
    let mut facts = BTreeMap::new();
    let blocks = reply.image_blocks();
    let mut cursor = 0;
    for record in reply.records() {
        let id = packs::decimal_id(&record["document_id"])
            .context("Custom emoji response omitted a valid document id")?;
        let metadata = record["preview"]
            .as_array()
            .cloned()
            .or_else(|| record["preview"]["frame_metadata"].as_array().cloned())
            .unwrap_or_else(|| vec![record["preview"].clone()]);
        let available = metadata.len().max(1);
        ensure!(
            cursor + available <= blocks.len(),
            "Custom emoji response has fewer images than its frame metadata declares"
        );
        let mut frames = Vec::with_capacity(available);
        for (index, block) in blocks[cursor..cursor + available].iter().enumerate() {
            let encoded = block["data"]
                .as_str()
                .context("Custom emoji image data is missing")?;
            ensure!(
                encoded.len() <= 48 * 1024 * 1024,
                "Custom emoji frame exceeds the response byte bound"
            );
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
            let meta = metadata.get(index).cloned().unwrap_or(Value::Null);
            frames.push((bytes, meta));
        }
        cursor += available;
        images.insert(id.clone(), frames);
        facts.insert(id, record);
    }
    ensure!(
        cursor == blocks.len(),
        "Custom emoji response contains images without a document association"
    );
    Ok((images, facts))
}

pub struct Studio {
    remote: RunningService<RoleClient, ()>,
    account: Option<String>,
}

impl Studio {
    pub async fn connect(account: Option<String>, url: Option<&str>) -> Result<Self> {
        Ok(Self {
            remote: crate::server::bridge::connect_shared_at(url).await?,
            account,
        })
    }

    pub async fn call(&self, name: &str, mut args: Value) -> Result<ToolReply> {
        if let Some(account) = &self.account
            && args.get("account").is_none()
        {
            args["account"] = account.clone().into();
        }
        let request: CallToolRequestParams =
            serde_json::from_value(json!({"name":name,"arguments":args}))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            self.remote.call_tool_once(request),
        )
        .await
        .context("Shared MCP request timed out; inspect state before retrying a mutation")??;
        match result {
            CallToolResponse::Complete(result) => ToolReply::parse(serde_json::to_value(result)?),
            CallToolResponse::InputRequired(result) => Ok(ToolReply {
                content: Vec::new(),
                payload: serde_json::to_value(result)?,
            }),
            CallToolResponse::Task(result) => Ok(ToolReply {
                content: Vec::new(),
                payload: serde_json::to_value(result)?,
            }),
            _ => anyhow::bail!(
                "Shared MCP returned an unsupported response; no request was repeated"
            ),
        }
    }

    pub async fn frames(
        &self,
        ids: &[String],
        size: u32,
        count: usize,
    ) -> Result<(Frames, Metadata)> {
        let mut images = BTreeMap::new();
        let mut facts = BTreeMap::new();
        for chunk in ids.chunks(10) {
            let reply = self
                .call(
                    "get_custom_emoji",
                    json!({"document_ids":chunk,"max_dimension":size,"count":count.clamp(1,10)}),
                )
                .await?;
            let (frames, metadata) = group_frames(&reply)?;
            images.extend(frames);
            facts.extend(metadata);
        }
        Ok((images, facts))
    }

    pub async fn images(
        &self,
        ids: &[String],
        size: u32,
        count: usize,
    ) -> Result<BTreeMap<String, DynamicImage>> {
        let (frames, _) = self.frames(ids, size, count).await?;
        frames
            .into_iter()
            .map(|(id, frames)| {
                let metadata: Vec<Value> = frames.iter().map(|(_, meta)| meta.clone()).collect();
                let selected = compose::best_frame(&metadata);
                let frame = frames
                    .get(selected)
                    .context("Emoji returned no preview frames")?;
                Ok((id, crate::tools::visual::images::decode_image(&frame.0)?))
            })
            .collect()
    }

    pub async fn meta(&self, ids: &[String]) -> Result<BTreeMap<String, Value>> {
        Ok(self.frames(ids, 64, 1).await?.1)
    }

    pub async fn pack_ids(&self, short_name: &str) -> Result<Vec<String>> {
        let reply = self
            .call("inspect_sticker_set", json!({"short_name":short_name}))
            .await?;
        let record = reply.payload["results"]
            .as_array()
            .and_then(|values| values.first())
            .unwrap_or(&reply.payload);
        let stickers = record
            .get("stickers")
            .or_else(|| record.get("documents"))
            .and_then(Value::as_array)
            .context("Sticker-set response has no document list")?;
        Ok(stickers
            .iter()
            .filter_map(|item| packs::decimal_id(item.get("document_id").unwrap_or(&item["id"])))
            .collect())
    }
}

pub struct Pictures {
    pub entries: BTreeMap<String, packs::Entry>,
    pub from_source: BTreeMap<String, String>,
    studio: Option<Studio>,
    account: Option<String>,
    url: Option<String>,
}

impl Pictures {
    pub fn new(
        root: Option<PathBuf>,
        account: Option<String>,
        url: Option<String>,
    ) -> Result<Self> {
        let (entries, from_source) = if let Some(root) = root {
            let (entries, problems) = packs::read_catalogue(&root)?;
            for problem in problems {
                eprintln!("packs: {problem}");
            }
            (entries, packs::source_map(&root)?)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };
        Ok(Self {
            entries,
            from_source,
            studio: None,
            account,
            url,
        })
    }
    pub async fn studio(&mut self) -> Result<&Studio> {
        if self.studio.is_none() {
            self.studio = Some(Studio::connect(self.account.clone(), self.url.as_deref()).await?);
        }
        self.studio
            .as_ref()
            .context("Shared MCP session is unavailable")
    }
    pub async fn get(
        &mut self,
        ids: &[String],
        size: u32,
    ) -> Result<BTreeMap<String, DynamicImage>> {
        let mut images = BTreeMap::new();
        let mut remote = Vec::new();
        for id in ids {
            if let Some(path) = self.entries.get(id).and_then(|entry| entry.thumb.as_ref()) {
                match packs::open_picture(path).await {
                    Ok(image) => {
                        images.insert(id.clone(), image);
                        continue;
                    }
                    Err(error) => eprintln!("packs: {id}: {error}; trying Telegram"),
                }
            }
            remote.push(id.clone());
        }
        if !remote.is_empty() {
            images.extend(self.studio().await?.images(&remote, size, 6).await?);
        }
        Ok(images)
    }
    pub fn glyph(&self, id: &str) -> Option<&str> {
        self.entries.get(id).and_then(|entry| entry.glyph.as_str())
    }
}
