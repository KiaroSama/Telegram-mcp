//! Bounded image decoding and encoding shared by screenshots and media previews.

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader};
use serde_json::{Value, json};
use std::io::Cursor;

pub const MAX_IMAGE_DIMENSION: u32 = 1568;
pub const MAX_NATIVE_DIMENSION: u32 = 4096;
pub const MAX_DECODED_PIXELS: u64 = 50_000_000;
pub const MAXIMUM_TILES: usize = 24;

pub fn bounded_dimension(value: i64) -> u32 {
    if value == 0 {
        MAX_IMAGE_DIMENSION
    } else {
        value.clamp(64, i64::from(MAX_IMAGE_DIMENSION)) as u32
    }
}

pub fn decode_image(data: &[u8]) -> Result<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .context("Image format could not be identified")?;
    let format = reader.format().context("Unsupported image format")?;
    let (width, height) = reader
        .into_dimensions()
        .context("Image header is invalid")?;
    if u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS {
        bail!("Image declares more than {MAX_DECODED_PIXELS} pixels; refusing to decode it");
    }
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_PIXELS * 8);
    reader.limits(limits);
    reader.decode().context("Image data could not be decoded")
}

pub fn encode_image(
    image: &DynamicImage,
    format: &str,
    max_dimension: i64,
    quality: u8,
    native: bool,
) -> Result<(Vec<u8>, Value)> {
    let key = format.trim_start_matches('.').to_ascii_lowercase();
    let (format, mime, name) = match key.as_str() {
        "png" | "" => (ImageFormat::Png, "image/png", "png"),
        "jpeg" | "jpg" => (ImageFormat::Jpeg, "image/jpeg", "jpeg"),
        "webp" => (ImageFormat::WebP, "image/webp", "webp"),
        _ => bail!("Unsupported image format; expected png, jpeg, jpg or webp"),
    };
    let (original_width, original_height) = image.dimensions();
    if u64::from(original_width) * u64::from(original_height) > MAX_DECODED_PIXELS {
        bail!("Image dimensions exceed the decoded-pixel limit");
    }
    let edge = if native {
        MAX_NATIVE_DIMENSION
    } else {
        bounded_dimension(max_dimension)
    };
    let resized = original_width.max(original_height) > edge;
    let image = if resized {
        image.resize(edge, edge, image::imageops::FilterType::Lanczos3)
    } else {
        image.clone()
    };
    let mut output = Cursor::new(Vec::new());
    if format == ImageFormat::Jpeg {
        let rgb = image.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality.clamp(1, 100))
            .encode_image(&rgb)
            .context("Image could not be encoded as JPEG")?;
    } else {
        image
            .write_to(&mut output, format)
            .context("Image could not be encoded")?;
    }
    let data = output.into_inner();
    let mut meta = json!({"format":name,"mime_type":mime,"width":image.width(),
        "height":image.height(),"bytes":data.len()});
    if resized {
        meta["original_width"] = original_width.into();
        meta["original_height"] = original_height.into();
        meta["downscaled"] = true.into();
    }
    if native {
        meta["native_resolution"] = true.into();
    }
    if format == ImageFormat::WebP {
        // The Rust image encoder is lossless, so a requested JPEG quality must
        // never be represented as a WebP quality that was actually applied.
        meta["lossless"] = true.into();
    }
    Ok((data, meta))
}

/// Photo IDs and timestamps use a tiny built-in digit font; full Unicode labels
/// stay in metadata, without depending on an OS-specific font installation.
pub fn compose_contact_sheet(
    tiles: &[(Vec<u8>, String)],
    columns: Option<usize>,
) -> Result<(Vec<u8>, Value)> {
    if tiles.is_empty() {
        bail!("There are no images to compose into a contact sheet");
    }
    if tiles.len() > MAXIMUM_TILES {
        bail!("A contact sheet holds at most {MAXIMUM_TILES} images");
    }
    let cols = columns
        .filter(|n| *n > 0)
        .map(|n| n.min(tiles.len()))
        .unwrap_or_else(|| ((tiles.len() as f64).sqrt().ceil() as usize).clamp(1, 4));
    let rows = tiles.len().div_ceil(cols);
    let mut sheet = image::RgbImage::from_pixel(
        (cols * 256) as u32,
        (rows * 282) as u32,
        image::Rgb([24, 24, 24]),
    );
    let mut cells = Vec::with_capacity(tiles.len());
    for (index, (data, label)) in tiles.iter().enumerate() {
        let (left, top) = (((index % cols) * 256) as u32, ((index / cols) * 282) as u32);
        let mut decoded = false;
        if let Ok(image) = decode_image(data) {
            let tile = if image.width().max(image.height()) > 256 {
                image.resize(256, 256, image::imageops::FilterType::Lanczos3)
            } else {
                image
            };
            image::imageops::replace(
                &mut sheet,
                &tile.to_rgb8(),
                i64::from(left + (256 - tile.width()) / 2),
                i64::from(top + (256 - tile.height()) / 2),
            );
            decoded = true;
        }
        for y in top + 256..top + 282 {
            for x in left..left + 256 {
                sheet.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
        paint_number(&mut sheet, label, left + 8, top + 261);
        cells.push(json!({"index":index+1,"label":label,"column":index%cols,"row":index/cols,"decoded":decoded}));
    }
    let (data, mut meta) = encode_image(
        &DynamicImage::ImageRgb8(sheet),
        "png",
        i64::from(MAX_IMAGE_DIMENSION),
        85,
        false,
    )?;
    meta["sheet"] = true.into();
    meta["columns"] = cols.into();
    meta["rows"] = rows.into();
    meta["cells"] = cells.into();
    meta["cell_pixels"] = 256.into();
    meta["label_rendering"] = "numeric photo ID or timestamp; full labels in cells metadata".into();
    Ok((data, meta))
}

fn paint_number(image: &mut image::RgbImage, label: &str, left: u32, top: u32) {
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    for (offset, digit) in label.bytes().take(20).enumerate() {
        let pattern = match digit {
            b'0'..=b'9' => DIGITS[(digit - b'0') as usize],
            b'-' => [0, 0, 7, 0, 0],
            b':' => [0, 2, 0, 2, 0],
            _ => continue,
        };
        for (row, bits) in pattern.iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) != 0 {
                    for dy in 0..3 {
                        for dx in 0..3 {
                            image.put_pixel(
                                left + offset as u32 * 12 + col * 3 + dx,
                                top + row as u32 * 3 + dy,
                                image::Rgb([255, 255, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
}
