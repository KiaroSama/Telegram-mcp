use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

pub struct Settings {
    pub(super) engine: String,
    pub(super) disabled: bool,
    pub(super) model: String,
    pub(super) language: Option<String>,
    pub(super) endpoint: Option<url::Url>,
    pub(super) key: Option<String>,
    pub(super) timeout: Duration,
    pub(super) whisper_cli: Option<PathBuf>,
    pub(super) whisper_model: Option<PathBuf>,
}
impl Settings {
    pub fn from_values(values: &BTreeMap<String, String>, args: &Value) -> Result<Self> {
        let get = |key: &str| values.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
        let mode = get("TELEGRAM_TRANSCRIBE")
            .unwrap_or("on-demand")
            .to_ascii_lowercase();
        ensure!(
            matches!(mode.as_str(), "off" | "on-demand" | "auto"),
            "TELEGRAM_TRANSCRIBE must be off, on-demand or auto"
        );
        let engine = args
            .get("engine")
            .filter(|v| !v.is_null())
            .map(|_| crate::args::text(args, "engine"))
            .transpose()?
            .or(get("TELEGRAM_TRANSCRIBE_ENGINE"))
            .unwrap_or("telegram")
            .trim()
            .to_ascii_lowercase();
        ensure!(
            ["telegram", "groq", "openai", "whisper"].contains(&engine.as_str()),
            "engine must be telegram, groq, openai or whisper"
        );
        let mut endpoint = match engine.as_str() {
            "groq" => Some(url::Url::parse(
                "https://api.groq.com/openai/v1/audio/transcriptions",
            )?),
            "openai" => get("TELEGRAM_TRANSCRIBE_OPENAI_URL")
                .map(url::Url::parse)
                .transpose()
                .context("Configured transcription endpoint is invalid")?,
            _ => None,
        };
        if let Some(endpoint) = &mut endpoint {
            ensure!(
                matches!(endpoint.scheme(), "http" | "https")
                    && endpoint.host_str().is_some()
                    && endpoint.username().is_empty()
                    && endpoint.password().is_none()
                    && endpoint.fragment().is_none(),
                "Transcription endpoint must be an HTTP(S) URL without embedded credentials"
            );
            let path = endpoint.path().trim_end_matches('/');
            if !path.ends_with("/audio/transcriptions") {
                endpoint.set_path(&format!("{path}/audio/transcriptions"));
            }
        }
        let key = match engine.as_str() {
            "groq" => get("GROQ_API_KEY").map(str::to_owned),
            "openai" => get("TELEGRAM_TRANSCRIBE_OPENAI_API_KEY")
                .or_else(|| get("OPENAI_API_KEY"))
                .map(str::to_owned),
            _ => None,
        };
        if let Some(key) = &key {
            ensure!(
                key.len() <= 8192 && !key.chars().any(char::is_control),
                "Configured transcription key contains invalid characters"
            );
        }
        if let Some(endpoint) = &endpoint {
            let loopback = match endpoint.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            ensure!(
                endpoint.scheme() == "https" || (key.is_none() && loopback),
                "Transcription endpoints require HTTPS; credential-free HTTP is allowed only on a literal loopback address"
            );
        }
        let model = match engine.as_str() {
            "groq" => get("TELEGRAM_TRANSCRIBE_GROQ_MODEL").unwrap_or("whisper-large-v3-turbo"),
            "openai" => get("TELEGRAM_TRANSCRIBE_OPENAI_MODEL").unwrap_or("whisper-1"),
            "whisper" => get("TELEGRAM_TRANSCRIBE_WHISPER_MODEL_PATH")
                .or_else(|| get("TELEGRAM_TRANSCRIBE_WHISPER_MODEL"))
                .unwrap_or(""),
            _ => "telegram",
        }
        .to_owned();
        ensure!(
            model.len() <= 4096 && !model.chars().any(char::is_control),
            "Configured transcription model is invalid"
        );
        let language = get("TELEGRAM_TRANSCRIBE_LANGUAGE")
            .filter(|language| *language != "auto")
            .map(str::to_owned);
        if let Some(language) = &language {
            ensure!(
                language.len() <= 16
                    && language
                        .bytes()
                        .all(|b| b.is_ascii_alphabetic() || b == b'-'),
                "TELEGRAM_TRANSCRIBE_LANGUAGE must be an ISO language code"
            );
        }
        let seconds = get("TELEGRAM_TRANSCRIBE_TIMEOUT")
            .map(|value| value.parse::<u64>())
            .transpose()
            .context("TELEGRAM_TRANSCRIBE_TIMEOUT must be an integer")?
            .unwrap_or(40);
        ensure!(
            (1..=45).contains(&seconds),
            "Transcription timeout must be between 1 and 45 seconds"
        );
        Ok(Self {
            engine: engine.clone(),
            disabled: mode == "off",
            whisper_cli: get("TELEGRAM_TRANSCRIBE_WHISPER_CLI").map(PathBuf::from),
            whisper_model: (engine == "whisper" && !model.is_empty())
                .then(|| PathBuf::from(&model)),
            model,
            language,
            endpoint,
            key,
            timeout: Duration::from_secs(seconds),
        })
    }
    pub(super) fn preflight(&self) -> Result<()> {
        match self.engine.as_str() {
            "groq" => ensure!(
                self.key.is_some(),
                "GROQ_API_KEY is not configured; select telegram or configure the hosted engine"
            ),
            "openai" => ensure!(
                self.endpoint.is_some(),
                "TELEGRAM_TRANSCRIBE_OPENAI_URL is not configured"
            ),
            "whisper" => {
                let command=self.whisper_cli.as_ref().context("Set TELEGRAM_TRANSCRIBE_WHISPER_CLI to an installed native whisper-cli executable")?;
                ensure!(
                    command.is_file(),
                    "The configured native whisper-cli executable does not exist"
                );
                let model=self.whisper_model.as_ref().context("Set TELEGRAM_TRANSCRIBE_WHISPER_MODEL_PATH to an existing local model; no model is downloaded automatically")?;
                let meta = std::fs::metadata(model)
                    .context("The configured local Whisper model does not exist")?;
                ensure!(
                    meta.is_file() && meta.len() > 0 && meta.len() <= 4 * 1024 * 1024 * 1024,
                    "Whisper model must be an existing regular file up to 4 GiB"
                );
            }
            "telegram" => {}
            _ => bail!("Unsupported transcription engine"),
        }
        Ok(())
    }
    pub(super) fn display_model(&self) -> String {
        if self.engine == "whisper" {
            self.whisper_model
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            self.model.clone()
        }
    }
    pub(super) fn cache_model(&self) -> String {
        let modified = self
            .whisper_model
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        format!("{}:{modified:?}", self.model)
    }
}
