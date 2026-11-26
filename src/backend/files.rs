use super::*;
use crate::state::FileAccess;
use grammers_client::media::Downloadable;
use tl::Deserializable;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Location {
    raw: tl::enums::InputFileLocation,
    dc: Option<i32>,
}
impl Downloadable for Location {
    fn to_raw_input_location(&self) -> Option<tl::enums::InputFileLocation> {
        Some(self.raw.clone())
    }
    fn dc_id(&self) -> Option<i32> {
        self.dc
    }
}
impl Native {
    pub(super) async fn download_in_memory(
        &self,
        mut location: Value,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        if max_bytes == 0 || max_bytes > 512 * 1024 * 1024 {
            bail!("In-memory download byte budget must be within 1 byte and 512 MiB");
        }
        let dc = location
            .as_object_mut()
            .and_then(|object| object.remove("_dc_id"))
            .and_then(|value| value.as_i64())
            .map(i32::try_from)
            .transpose()?;
        let raw =
            tl::enums::InputFileLocation::from_bytes(&Schema::global().encode_object(&location)?)?;
        let mut download = self.0.client.iter_download(&Location { raw, dc });
        let mut bytes = Vec::new();
        while let Some(chunk) = download
            .next()
            .await
            .context("Could not download media into memory")?
        {
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > max_bytes {
                bail!("Media exceeded its in-memory byte budget");
            }
            bytes.extend(chunk);
        }
        Ok(bytes)
    }
    pub(super) async fn upload_file(&self, path: &Path) -> Result<Value> {
        let path = self.0.state.resolve_file(path, FileAccess::Read)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).context("Cannot open upload source")?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > 4 * 1024 * 1024 * 1024 {
            bail!("Upload source must be a nonempty regular file up to 4 GiB");
        }
        let size = meta.len();
        let big = size > 10 * 1024 * 1024;
        let chunk = 512 * 1024usize;
        let parts = size.div_ceil(chunk as u64);
        let id = crate::args::random_id();
        let mut source = tokio::fs::File::from_std(file);
        let mut digest = md5::Context::new();
        let mut total = 0u64;
        for part in 0..parts {
            let want = ((size - total) as usize).min(chunk);
            let mut buffer = vec![0; want];
            source.read_exact(&mut buffer).await?;
            total += buffer.len() as u64;
            digest.consume(&buffer);
            let mut params = json!({"file_id":id,"file_part":part,"bytes":buffer});
            let method = if big {
                params["file_total_parts"] = json!(parts);
                "upload.saveBigFilePart"
            } else {
                "upload.saveFilePart"
            };
            if self.invoke_raw(method, params).await? != json!(true) {
                bail!("Telegram refused an upload part; no message was sent");
            }
        }
        let mut tail = [0u8; 1];
        if source.read(&mut tail).await? != 0 {
            bail!("Upload source changed size while being read; no message was sent");
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("Upload filename must be valid UTF-8")?;
        Ok(if big {
            json!({"_":"inputFileBig","id":id,"parts":parts,"name":name})
        } else {
            json!({"_":"inputFile","id":id,"parts":parts,"name":name,"md5_checksum":format!("{:x}",digest.finalize())})
        })
    }
    pub(super) async fn download_file(
        &self,
        mut location: Value,
        destination: &Path,
        max_bytes: u64,
    ) -> Result<Value> {
        if max_bytes == 0 || max_bytes > 4 * 1024 * 1024 * 1024 {
            bail!("Download byte budget must be within 1 byte and 4 GiB");
        }
        let path = self.0.state.resolve_download_file(destination)?;
        if path.exists() {
            bail!("Download destination already exists; it was not overwritten");
        }
        let dc = location
            .as_object_mut()
            .and_then(|o| o.remove("_dc_id"))
            .and_then(|v| v.as_i64())
            .map(i32::try_from)
            .transpose()?;
        let bytes = Schema::global().encode_object(&location)?;
        let raw =
            tl::enums::InputFileLocation::from_bytes(&bytes).context("Invalid media location")?;
        let location = Location { raw, dc };
        let parent = path
            .parent()
            .context("Download destination has no parent")?;
        std::fs::create_dir_all(parent)?;
        // A private temporary file is published without replacement only after a
        // complete successful download. Cancellation drops the temporary file.
        let temp = tempfile::NamedTempFile::new_in(parent)?;
        let mut output = tokio::fs::File::from_std(temp.reopen()?);
        let mut download = self.0.client.iter_download(&location);
        let mut total = 0u64;
        while let Some(chunk) = download.next().await.context(
            "Media download failed; refresh the source message if its file reference expired",
        )? {
            total = total
                .checked_add(chunk.len() as u64)
                .context("Download size overflow")?;
            if total > max_bytes {
                bail!("Media exceeded download byte budget; partial file was discarded");
            }
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        output.sync_all().await?;
        drop(output);
        temp.persist_noclobber(&path).map_err(|_| {
            anyhow::anyhow!("Could not publish download without replacing an existing file")
        })?;
        Ok(json!({"path":path,"bytes":total}))
    }
}
