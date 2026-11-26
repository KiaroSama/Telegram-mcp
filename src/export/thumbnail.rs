//! Telegram Desktop thumbnail pipeline: ICC -> sRGB conversion before Qt resampling and JPEG output.

use super::files::{clean_relative_path, write_new_export_file};
use super::icc::{
    DEFAULT_DOTS_PER_METER, ExportIccTransform, ExtractedIcc, IccOutcome, MAX_ICC_PROFILE_BYTES,
    PixelLayout, convert_to_srgb, encode_desktop_jpeg, jpeg_source_metadata, smooth_scale,
};
use image::codecs::png::PngEncoder;
use image::{
    ColorType, DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, ImageReader, Limits,
};
use serde_json::{Value, json};
use std::io::Cursor;
use std::path::Path;

pub const MAX_IMAGE_DIMENSION: u32 = 10_000;
pub const PHOTO_MAX_WIDTH: u32 = 520;
pub const PHOTO_MAX_HEIGHT: u32 = 520;
pub const PHOTO_MIN_WIDTH: u32 = 80;
pub const PHOTO_MIN_HEIGHT: u32 = 80;
pub const STICKER_MAX_WIDTH: u32 = 384;
pub const STICKER_MAX_HEIGHT: u32 = 384;
pub const STICKER_MIN_WIDTH: u32 = 80;
pub const STICKER_MIN_HEIGHT: u32 = 80;
pub const ENTRY_USERPIC_SIZE: u32 = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailArtifact {
    pub bytes: Vec<u8>,
    pub scaled_pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub output_layout: PixelLayout,
    pub dpm: (u32, u32),
    pub transform: ExportIccTransform,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenThumbnail {
    pub relative_path: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
}

struct DecodedImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    layout: PixelLayout,
    format: Option<ImageFormat>,
}

fn scaled_keep_aspect(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_w, max_h);
    }
    let rw = (u64::from(max_h) * u64::from(width)) / u64::from(height);
    if rw <= u64::from(max_w) {
        (rw as u32, max_h)
    } else {
        let rh = (u64::from(max_w) * u64::from(height)) / u64::from(width);
        (max_w, rh as u32)
    }
}

/// Desktop `CalculateThumbSize`: optional 2x retina check, aspect-preserving cap, even rounding, min bound.
pub fn calculate_thumb_size(
    width: u32,
    height: u32,
    max_w: u32,
    max_h: u32,
    min_w: u32,
    min_h: u32,
    expand_for_retina: bool,
) -> (u32, u32) {
    let mult = if expand_for_retina { 2_u64 } else { 1_u64 };
    let check_w = u64::from(width) * mult;
    let check_h = u64::from(height) * mult;
    let (sw, sh) = if check_w > u64::from(max_w) || check_h > u64::from(max_h) {
        scaled_keep_aspect(width, height, max_w, max_h)
    } else {
        (width, height)
    };
    let (rw, rh) = (sw & !1, sh & !1);
    if rw < min_w || rh < min_h {
        (0, 0)
    } else {
        (rw, rh)
    }
}

fn decode_bounded_image(source: &[u8]) -> Option<DecodedImage> {
    let mut reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .ok()?;
    let format = reader.format();
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 || width >= MAX_IMAGE_DIMENSION || height >= MAX_IMAGE_DIMENSION {
        return None;
    }
    let (pixels, layout) = match decoded {
        DynamicImage::ImageLuma8(buf) => (buf.into_raw(), PixelLayout::Gray8),
        DynamicImage::ImageRgb8(buf) => (buf.into_raw(), PixelLayout::Rgb8),
        DynamicImage::ImageRgba8(buf) => (buf.into_raw(), PixelLayout::Rgba8),
        other => match other.color() {
            ColorType::L8 | ColorType::L16 => (other.into_luma8().into_raw(), PixelLayout::Gray8),
            c if c.has_alpha() => (other.into_rgba8().into_raw(), PixelLayout::Rgba8),
            _ => (other.into_rgb8().into_raw(), PixelLayout::Rgb8),
        },
    };
    Some(DecodedImage {
        pixels,
        width,
        height,
        layout,
        format,
    })
}

fn apply_source_icc(
    px: &[u8],
    width: u32,
    height: u32,
    layout: PixelLayout,
    source: &[u8],
) -> Option<(ExportIccTransform, (u32, u32), Vec<u8>)> {
    if source.starts_with(b"\xff\xd8") {
        let meta = jpeg_source_metadata(source);
        let transform = match meta.icc {
            ExtractedIcc::Unprofiled => convert_to_srgb(px, width, height, layout, None).ok()?,
            ExtractedIcc::Present(ref icc) => {
                convert_to_srgb(px, width, height, layout, Some(icc)).ok()?
            }
            ExtractedIcc::Malformed => {
                ExportIccTransform::apply_from_container(px, width, height, layout, source).ok()?
            }
        };
        Some((transform, (meta.dpm_x, meta.dpm_y), meta.comments))
    } else {
        let transform =
            ExportIccTransform::apply_from_container(px, width, height, layout, source).ok()?;
        Some((
            transform,
            (DEFAULT_DOTS_PER_METER, DEFAULT_DOTS_PER_METER),
            Vec::new(),
        ))
    }
}

/// Never tags fallback pixels as sRGB: emits sRGB profile only on `Converted` or `AlreadySrgb`.
pub fn emitted_jpeg_icc(transform: &ExportIccTransform) -> &[u8] {
    match transform.outcome {
        IccOutcome::Converted | IccOutcome::AlreadySrgb => &transform.emitted_profile,
        IccOutcome::Unprofiled | IccOutcome::MalformedFallback => &[],
        IccOutcome::UnsupportedFallback => {
            if transform.emitted_profile.len() <= MAX_ICC_PROFILE_BYTES {
                &transform.emitted_profile
            } else {
                &[]
            }
        }
    }
}

pub fn icc_summary_json(transform: &ExportIccTransform) -> Value {
    json!({
        "target": transform.target,
        "source_layout": transform.source_layout,
        "destination_layout": transform.destination_layout,
        "outcome": transform.outcome.summary_kind(),
        "detail": transform.outcome,
        "has_source_profile": transform.source_profile.is_some(),
    })
}

/// Decodes a source image, converts supported ICC profiles to sRGB before scaling,
/// resamples via Qt `smooth_scale`, and encodes a Desktop JPEG thumbnail.
pub fn build_desktop_jpeg_thumbnail(
    source: &[u8],
    target_size: Option<(u32, u32)>,
    quality: Option<u8>,
) -> Option<ThumbnailArtifact> {
    let img = decode_bounded_image(source)?;
    let (transform, dpm, comments) =
        apply_source_icc(&img.pixels, img.width, img.height, img.layout, source)?;
    let (dst_w, dst_h) = target_size.unwrap_or_else(|| {
        calculate_thumb_size(
            img.width,
            img.height,
            PHOTO_MAX_WIDTH,
            PHOTO_MAX_HEIGHT,
            PHOTO_MIN_WIDTH,
            PHOTO_MIN_HEIGHT,
            false,
        )
    });
    if dst_w == 0 || dst_h == 0 {
        return None;
    }
    let (scaled_pixels, scaled_layout) = smooth_scale(
        &transform.pixels,
        img.width,
        img.height,
        transform.destination_layout,
        dst_w,
        dst_h,
    )
    .ok()?;
    let bytes = encode_desktop_jpeg(
        &scaled_pixels,
        dst_w,
        dst_h,
        scaled_layout,
        quality,
        dpm,
        emitted_jpeg_icc(&transform),
        &comments,
    )
    .ok()?;
    Some(ThumbnailArtifact {
        bytes,
        scaled_pixels,
        width: dst_w,
        height: dst_h,
        output_layout: scaled_layout,
        dpm,
        transform,
    })
}

fn encode_png_bytes(px: &[u8], width: u32, height: u32, layout: PixelLayout) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let ct = match layout {
        PixelLayout::Gray8 => ExtendedColorType::L8,
        PixelLayout::Rgb8 => ExtendedColorType::Rgb8,
        PixelLayout::Rgba8 => ExtendedColorType::Rgba8,
    };
    PngEncoder::new(&mut out)
        .write_image(px, width, height, ct)
        .ok()?;
    Some(out)
}

fn thumb_suggested_path(
    relative_large_path: &str,
    postfix: &str,
    ext_override: Option<&str>,
) -> Option<String> {
    let clean = clean_relative_path(relative_large_path)?;
    let after_slash = clean.rfind('/').map_or(0, |idx| idx + 1);
    let candidate = match clean[after_slash..].find('.') {
        Some(dot) => {
            let split_at = after_slash + dot;
            let ext = ext_override.unwrap_or(&clean[split_at..]);
            format!("{}{postfix}{ext}", &clean[..split_at])
        }
        None => {
            let ext = ext_override.unwrap_or(".jpg");
            format!("{clean}{postfix}{ext}")
        }
    };
    clean_relative_path(&candidate)
}

/// Inspects ICC transform and writes a sibling `_thumb` image when dimensions meet Desktop bounds.
pub fn write_media_thumbnail(
    export_dir: &Path,
    relative_large_path: &str,
    source_bytes: &[u8],
    is_sticker: bool,
) -> (Option<WrittenThumbnail>, Option<ExportIccTransform>) {
    let Some(img) = decode_bounded_image(source_bytes) else {
        return (None, None);
    };
    let Some((transform, dpm, comments)) =
        apply_source_icc(&img.pixels, img.width, img.height, img.layout, source_bytes)
    else {
        return (None, None);
    };
    let (dst_w, dst_h) = if is_sticker {
        calculate_thumb_size(
            img.width,
            img.height,
            STICKER_MAX_WIDTH,
            STICKER_MAX_HEIGHT,
            STICKER_MIN_WIDTH,
            STICKER_MIN_HEIGHT,
            false,
        )
    } else {
        calculate_thumb_size(
            img.width,
            img.height,
            PHOTO_MAX_WIDTH,
            PHOTO_MAX_HEIGHT,
            PHOTO_MIN_WIDTH,
            PHOTO_MIN_HEIGHT,
            false,
        )
    };
    if dst_w == 0 || dst_h == 0 {
        return (None, Some(transform));
    }
    let Ok((scaled_px, scaled_layout)) = smooth_scale(
        &transform.pixels,
        img.width,
        img.height,
        transform.destination_layout,
        dst_w,
        dst_h,
    ) else {
        return (None, Some(transform));
    };
    let use_png = is_sticker
        || (img.format != Some(ImageFormat::Jpeg) && scaled_layout == PixelLayout::Rgba8);
    let (encoded, ext_override) = if use_png {
        (
            encode_png_bytes(&scaled_px, dst_w, dst_h, scaled_layout),
            Some(".png"),
        )
    } else {
        (
            encode_desktop_jpeg(
                &scaled_px,
                dst_w,
                dst_h,
                scaled_layout,
                None,
                dpm,
                emitted_jpeg_icc(&transform),
                &comments,
            )
            .ok(),
            if img.format == Some(ImageFormat::Jpeg) {
                None
            } else {
                Some(".jpg")
            },
        )
    };
    let Some(bytes) = encoded else {
        return (None, Some(transform));
    };
    let Some(suggested) = thumb_suggested_path(relative_large_path, "_thumb", ext_override) else {
        return (None, Some(transform));
    };
    let written = write_new_export_file(export_dir, &suggested, &bytes)
        .ok()
        .map(|relative_path| WrittenThumbnail {
            relative_path,
            width: dst_w,
            height: dst_h,
            byte_size: bytes.len() as u64,
        });
    (written, Some(transform))
}

/// Normalizes a downloaded document thumbnail (`_thumb.jpg`) through ICC -> sRGB when decodable.
pub fn normalize_downloaded_thumb(source_bytes: &[u8]) -> (Vec<u8>, Option<ExportIccTransform>) {
    let Some(img) = decode_bounded_image(source_bytes) else {
        return (source_bytes.to_vec(), None);
    };
    let Some((transform, dpm, comments)) =
        apply_source_icc(&img.pixels, img.width, img.height, img.layout, source_bytes)
    else {
        return (source_bytes.to_vec(), None);
    };
    if transform.outcome == IccOutcome::Converted
        && let Ok(reencoded) = encode_desktop_jpeg(
            &transform.pixels,
            img.width,
            img.height,
            transform.destination_layout,
            Some(87),
            dpm,
            emitted_jpeg_icc(&transform),
            &comments,
        )
    {
        return (reencoded, Some(transform));
    }
    (source_bytes.to_vec(), Some(transform))
}
