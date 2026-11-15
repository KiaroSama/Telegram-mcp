//! Animation decoders run in a child, including the first frame's native work.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use image::{AnimationDecoder, ImageDecoder};
use serde_json::{Value, json};
use std::io::Cursor;
use std::path::Path;
#[cfg(test)]
#[path = "../../../tests/unit/visual_frames.rs"]
mod tests;

pub const MAX_ANIMATION_FRAMES: usize = 3000;
pub const MAX_FRAMES: usize = 10;
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

pub async fn extract_frames(
    path: &Path,
    count: i64,
    max_dimension: i64,
) -> Result<Vec<(Vec<u8>, Value)>> {
    let state = crate::state::State::from_environment()?;
    let path = state.resolve_file(path, crate::state::FileAccess::Read)?;
    let metadata = std::fs::metadata(&path)?;
    if metadata.len() > MAX_INPUT_BYTES {
        bail!("Animation exceeds the 64 MiB input limit");
    }
    let response = super::capture::run_child(
        json!({"tool":"__decode_media","path":path,"count":count.clamp(1,MAX_FRAMES as i64),
        "max_dimension":super::images::bounded_dimension(max_dimension)}),
    )
    .await?;
    response
        .as_array()
        .context("Animation decoder returned an invalid frame list")?
        .iter()
        .map(|frame| {
            let bytes = base64::engine::general_purpose::STANDARD.decode(
                frame["data"]
                    .as_str()
                    .context("Animation decoder omitted frame bytes")?,
            )?;
            Ok((bytes, frame["metadata"].clone()))
        })
        .collect()
}

pub(crate) fn worker(args: &Value) -> Result<Value> {
    let path = Path::new(args["path"].as_str().context("Animation path is missing")?);
    let suffix = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["gif", "webp", "png", "apng", "tgs"].contains(&suffix.as_str()) {
        bail!(
            "Native animation decoding supports GIF, WebP, APNG and TGS; use the video decoder for other media"
        );
    }
    let file = std::fs::File::open(path).context("Animation file could not be opened")?;
    use std::io::Read;
    let mut data = Vec::new();
    file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_INPUT_BYTES {
        bail!("Animation exceeds the 64 MiB input limit");
    }
    let frames = if suffix == "tgs" {
        super::lottie::render_tgs(
            &data,
            args["count"].as_u64().unwrap_or(4) as usize,
            args["max_dimension"].as_i64().unwrap_or(1568),
        )?
    } else {
        decode_animation(
            &data,
            &suffix,
            args["count"].as_u64().unwrap_or(4) as usize,
            args["max_dimension"].as_i64().unwrap_or(1568),
        )?
    };
    Ok(frames.into_iter().map(|(data,metadata)|json!({"data":base64::engine::general_purpose::STANDARD.encode(data),"metadata":metadata})).collect())
}

fn limit_decoder<'a, D: ImageDecoder + AnimationDecoder<'a> + 'a>(
    mut decoder: D,
) -> Result<image::Frames<'a>> {
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > super::images::MAX_DECODED_PIXELS {
        bail!("Animation declares too many decoded pixels");
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(super::images::MAX_DECODED_PIXELS * 8);
    decoder.set_limits(limits)?;
    Ok(decoder.into_frames())
}

fn frame_iterator<'a>(data: &'a [u8], suffix: &str) -> Result<image::Frames<'a>> {
    match suffix.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "gif" => limit_decoder(
            image::codecs::gif::GifDecoder::new(Cursor::new(data))
                .context("Could not decode this GIF")?,
        ),
        "webp" => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(data))
                .context("Could not decode this WebP")?;
            if !decoder.has_animation() {
                bail!("This WebP is not animated");
            }
            // image-webp leaves disposal disabled until a background is chosen.
            // Telegram sprites use a transparent canvas, matching libwebp's
            // composited frames instead of retaining a disposed opaque frame.
            decoder.set_background_color(image::Rgba([0, 0, 0, 0]))?;
            limit_decoder(decoder)
        }
        "png" | "apng" => {
            let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(data))
                .context("Could not decode this PNG")?;
            let (width, height) = decoder.dimensions();
            if u64::from(width) * u64::from(height) > super::images::MAX_DECODED_PIXELS {
                bail!("Animation declares too many decoded pixels");
            }
            if !decoder.is_apng()? {
                bail!("This PNG is not animated");
            }
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(super::images::MAX_DECODED_PIXELS * 8);
            decoder.set_limits(limits)?;
            Ok(decoder.apng()?.into_frames())
        }
        _ => bail!("Unsupported animation suffix"),
    }
}

/// Two streaming passes determine the sampling positions without retaining
/// thousands of decoded source images in memory at the same time.
pub fn decode_animation(
    data: &[u8],
    suffix: &str,
    count: usize,
    max_dimension: i64,
) -> Result<Vec<(Vec<u8>, Value)>> {
    if data.len() as u64 > MAX_INPUT_BYTES {
        bail!("Animation exceeds the input limit");
    }
    let mut total = 0;
    for frame in frame_iterator(data, suffix)? {
        let _ = frame.context("Animation contains a truncated or invalid frame")?;
        total += 1;
        if total > MAX_ANIMATION_FRAMES {
            bail!("Animation exceeds the {MAX_ANIMATION_FRAMES}-frame decode limit");
        }
    }
    if total < 2 {
        bail!("This image is not animated");
    }
    let wanted = count.clamp(1, MAX_FRAMES).min(total);
    let indexes: Vec<usize> = if wanted == 1 {
        vec![0]
    } else {
        (0..wanted)
            .map(|index| index * (total - 1) / (wanted - 1))
            .collect()
    };
    let mut output = Vec::with_capacity(wanted);
    let mut size = 0;
    for (index, frame) in frame_iterator(data, suffix)?.enumerate() {
        let frame = frame.context("Animation contains a truncated or invalid frame")?;
        if !indexes.contains(&index) {
            continue;
        }
        let image = image::DynamicImage::ImageRgba8(frame.into_buffer());
        let transparent = image
            .as_rgba8()
            .is_some_and(|pixels| pixels.pixels().all(|pixel| pixel[3] == 0));
        let (data, mut meta) =
            super::images::encode_image(&image, "png", max_dimension, 85, false)?;
        size += data.len();
        if size > MAX_OUTPUT_BYTES {
            bail!("Animation frames exceed the aggregate decoder output limit");
        }
        meta["frame_index"] = index.into();
        meta["frame_count"] = total.into();
        meta["source"] = "rust-image".into();
        meta["transparent"] = transparent.into();
        output.push((data, meta));
        if output.len() == wanted {
            break;
        }
    }
    if output.len() != wanted {
        bail!("Animation decoder did not return every requested frame");
    }
    Ok(output)
}
