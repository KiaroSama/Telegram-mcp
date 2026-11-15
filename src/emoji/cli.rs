use super::commands::{gif, index, measure_accuracy, nearest};
use super::{
    compose, packs, sheet,
    studio::{Pictures, Studio},
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Parser)]
#[command(
    about = "Choose custom emoji from their actual images and compose UTF-16-safe messages",
    version
)]
pub struct Cli {
    #[arg(long, global = true, env = "TELEGRAM_MCP_URL")]
    pub url: Option<String>,
    #[arg(long, global = true)]
    pub account: Option<String>,
    #[arg(long, global = true)]
    pub cache: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args)]
pub struct PackSource {
    #[arg(long)]
    pub packs_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Render a labelled PNG grid, preferring local pack thumbnails.
    Sheet {
        #[command(flatten)]
        source: PackSource,
        #[arg(long)]
        ids: String,
        #[arg(long, default_value = "sheet.png")]
        out: PathBuf,
        #[arg(long, default_value_t = 96)]
        size: u32,
        #[arg(long, default_value_t = 8)]
        columns: usize,
    },
    /// Export actual sampled animation frames as a looping GIF.
    Gif {
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "emoji.gif")]
        out: PathBuf,
        #[arg(long, default_value_t = 128)]
        size: u32,
        #[arg(long, default_value_t = 10)]
        frames: usize,
        #[arg(long, default_value_t = 80)]
        duration: u32,
    },
    /// Build/update visual fingerprints from local exports or named Telegram sets.
    Index {
        #[command(flatten)]
        source: PackSource,
        #[arg(long)]
        packs: Option<String>,
        #[arg(long, default_value_t = 64)]
        size: u32,
        #[arg(long, default_value_t = 3)]
        frames: usize,
        #[arg(long)]
        rebuild: bool,
    },
    /// Refresh changed pack thumbnails and remove vanished pack-owned entries.
    Refresh {
        #[command(flatten)]
        source: PackSource,
        #[arg(long)]
        rebuild: bool,
    },
    /// Rank by silhouette, colour and structure, and write a comparison sheet.
    Nearest {
        #[command(flatten)]
        source: PackSource,
        #[arg(long)]
        id: String,
        #[arg(long, default_value_t = 8)]
        k: usize,
        #[arg(long, default_value_t = 96)]
        size: u32,
        #[arg(long, default_value = "nearest.png")]
        out: PathBuf,
    },
    /// Number message lines and identify custom emoji already present.
    Lines {
        #[arg(long)]
        chat: String,
        #[arg(long)]
        message: i64,
    },
    /// Insert an emoji while preserving entity boundaries; edits use MCP approval.
    Insert {
        #[command(flatten)]
        source: PackSource,
        #[arg(long)]
        chat: String,
        #[arg(long)]
        message: i64,
        #[arg(long)]
        line: usize,
        #[arg(long)]
        id: String,
        #[arg(long)]
        glyph: Option<String>,
        #[arg(long = "where", default_value = "end")]
        where_: String,
        #[arg(long)]
        preview: bool,
    },
    /// Score true original/republished pairs from a read-only SQLite catalog.
    Accuracy {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long, default_value_t = 40)]
        sample: usize,
        #[arg(long, default_value_t = 64)]
        size: u32,
    },
}

pub(super) fn ids(source: &str) -> Result<Vec<String>> {
    let values: Vec<String> = source
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    ensure!(
        !values.is_empty() && values.len() <= 512,
        "Provide 1 to 512 comma-separated ids"
    );
    ensure!(
        values
            .iter()
            .all(|id| id.parse::<i64>().is_ok_and(|id| id > 0)),
        "Emoji ids must be positive 64-bit decimal strings"
    );
    Ok(values)
}

pub(super) fn cache(cli: &Cli) -> Result<PathBuf> {
    match &cli.cache {
        Some(path) => Ok(path.clone()),
        None => Ok(crate::state::State::global()?
            .root()
            .join("emoji-index.json")),
    }
}
pub(super) fn source(cli: &Cli, source: &PackSource) -> Result<Pictures> {
    let root = if source.packs_dir.is_some() || std::env::var_os(packs::ENV_VAR).is_some() {
        Some(packs::find_root(source.packs_dir.as_deref())?)
    } else {
        None
    };
    Pictures::new(root, cli.account.clone(), cli.url.clone())
}
pub(super) fn tail(id: &str) -> String {
    id.chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    crate::logging::initialize()?;
    match &cli.command {
        Command::Sheet {
            source: options,
            ids: raw,
            out,
            size,
            columns,
        } => {
            let mut pictures = source(&cli, options)?;
            let ids = ids(raw)?;
            let images = pictures.get(&ids, *size).await?;
            let unknown: Vec<String> = ids
                .iter()
                .filter(|id| !pictures.entries.contains_key(*id))
                .cloned()
                .collect();
            let facts = if unknown.is_empty() {
                BTreeMap::new()
            } else {
                pictures.studio().await?.meta(&unknown).await?
            };
            let labels = ids
                .iter()
                .map(|id| {
                    let glyph = pictures
                        .glyph(id)
                        .or_else(|| facts.get(id).and_then(|v| v["placeholder"].as_str()))
                        .unwrap_or("?");
                    (id.clone(), format!("{glyph} {}", tail(id)))
                })
                .collect();
            sheet::contact_sheet(&images, &ids, &labels, *columns, 84, out)?;
            println!("{}", out.display());
        }
        Command::Gif {
            id,
            out,
            size,
            frames,
            duration,
        } => gif(&cli, id, out, *size, *frames, *duration).await?,
        Command::Index {
            source,
            packs: remote,
            size,
            frames,
            rebuild,
        } => index(&cli, source, remote.as_deref(), *size, *frames, *rebuild).await?,
        Command::Refresh { source, rebuild } => index(&cli, source, None, 64, 3, *rebuild).await?,
        Command::Nearest {
            source: options,
            id,
            k,
            size,
            out,
        } => nearest(&cli, options, id, *k, *size, out).await?,
        Command::Lines { chat, message } => {
            let studio = Studio::connect(cli.account.clone(), cli.url.as_deref()).await?;
            let response = studio
                .call(
                    "inspect_message",
                    json!({"chat_id":chat,"message_id":message}),
                )
                .await?;
            let record = message_record(&response.payload)?;
            let rows = compose::describe_lines(
                message_text(record),
                record["entities"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?;
            for row in rows {
                println!(
                    "{:>3} | {}{}",
                    row["line"],
                    row["text"].as_str().unwrap_or(""),
                    if row["emoji"].as_array().is_some_and(|v| !v.is_empty()) {
                        format!("  <- {}", row["emoji"])
                    } else {
                        String::new()
                    }
                );
            }
        }
        Command::Insert {
            source: options,
            chat,
            message,
            line,
            id,
            glyph,
            where_,
            preview,
        } => {
            ids(id)?;
            let mut pictures = source(&cli, options)?;
            let glyph = match glyph
                .clone()
                .or_else(|| pictures.glyph(id).map(str::to_owned))
            {
                Some(glyph) => glyph,
                None => pictures
                    .studio()
                    .await?
                    .meta(std::slice::from_ref(id))
                    .await?
                    .get(id)
                    .and_then(|v| v["placeholder"].as_str())
                    .context("No fallback glyph was returned; pass --glyph explicitly")?
                    .to_owned(),
            };
            let response = pictures
                .studio()
                .await?
                .call(
                    "inspect_message",
                    json!({"chat_id":chat,"message_id":message}),
                )
                .await?;
            let record = message_record(&response.payload)?;
            let entities = record["entities"].as_array().cloned().unwrap_or_default();
            let (text, entities) =
                compose::insert_emoji(message_text(record), &entities, *line, &glyph, id, where_)?;
            let arguments = json!({"chat_id":chat,"message_id":message,"new_text":text,"entities":entities,"parse_mode":"none"});
            if *preview {
                println!("{}", serde_json::to_string_pretty(&arguments)?);
            } else {
                let result = pictures
                    .studio()
                    .await?
                    .call("edit_message", arguments)
                    .await?;
                println!("{}", serde_json::to_string_pretty(&result.payload)?);
            }
        }
        Command::Accuracy {
            catalog,
            sample,
            size,
        } => measure_accuracy(&cli, catalog, *sample, *size).await?,
    }
    Ok(())
}

fn message_record(payload: &Value) -> Result<&Value> {
    payload
        .get("message")
        .filter(|value| value.is_object())
        .or_else(|| payload["results"].as_array().and_then(|rows| rows.first()))
        .or_else(|| payload.is_object().then_some(payload))
        .context("Message inspection returned no record")
}
fn message_text(record: &Value) -> &str {
    record["text"]
        .as_str()
        .or_else(|| record["message"].as_str())
        .unwrap_or("")
}
