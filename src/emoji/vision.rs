use anyhow::{Result, ensure};
use image::{DynamicImage, GenericImageView, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const CANON: u32 = 64;
const GRID: u32 = 12;
const COLOUR_GRID: u32 = 6;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Fingerprint {
    #[serde(default)]
    pub silhouette: Vec<f64>,
    #[serde(default)]
    pub colour: Vec<f64>,
    #[serde(default)]
    pub structure: Vec<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Distance {
    pub total: f64,
    pub silhouette: f64,
    pub colour: f64,
    pub structure: f64,
}

pub fn normalise(picture: &DynamicImage) -> Result<RgbaImage> {
    let (width, height) = picture.dimensions();
    ensure!(
        width > 0
            && height > 0
            && u64::from(width) * u64::from(height)
                <= crate::tools::visual::images::MAX_DECODED_PIXELS,
        "Emoji image dimensions exceed the pixel bound"
    );
    let source = picture.to_rgba8();
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (x, y, pixel) in source.enumerate_pixels() {
        if pixel[3] > 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    if right == 0 || bottom == 0 {
        return Ok(RgbaImage::new(CANON, CANON));
    }
    let mut crop =
        image::imageops::crop_imm(&source, left, top, right - left, bottom - top).to_image();
    // Premultiplied filtering prevents invisible RGB from creating dark fringes
    // when a transparent sprite is scaled to the canonical square.
    for pixel in crop.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = ((u32::from(pixel[channel]) * u32::from(pixel[3]) + 127) / 255) as u8;
        }
    }
    let ratio = f64::from(CANON) / f64::from(crop.width().max(crop.height()));
    let (w, h) = (
        (f64::from(crop.width()) * ratio).round().max(1.0) as u32,
        (f64::from(crop.height()) * ratio).round().max(1.0) as u32,
    );
    let mut fitted = image::imageops::resize(&crop, w, h, image::imageops::FilterType::Lanczos3);
    for pixel in fitted.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = if pixel[3] == 0 {
                0
            } else {
                ((u32::from(pixel[channel]) * 255 + u32::from(pixel[3]) / 2) / u32::from(pixel[3]))
                    .min(255) as u8
            };
        }
    }
    let mut canvas = RgbaImage::new(CANON, CANON);
    image::imageops::replace(
        &mut canvas,
        &fitted,
        i64::from((CANON - w) / 2),
        i64::from((CANON - h) / 2),
    );
    Ok(canvas)
}

pub fn fingerprint(picture: &DynamicImage) -> Result<Fingerprint> {
    let canon = normalise(picture)?;
    let alpha =
        image::GrayImage::from_fn(CANON, CANON, |x, y| image::Luma([canon.get_pixel(x, y)[3]]));
    let small =
        image::imageops::resize(&alpha, GRID, GRID, image::imageops::FilterType::CatmullRom);
    let silhouette = small
        .as_raw()
        .iter()
        .map(|v| f64::from(*v) / 255.0)
        .collect();
    let flat = image::RgbImage::from_fn(CANON, CANON, |x, y| {
        let pixel = canon.get_pixel(x, y);
        let a = u32::from(pixel[3]);
        image::Rgb(std::array::from_fn(|c| {
            ((u32::from(pixel[c]) * a + 128 * (255 - a) + 127) / 255) as u8
        }))
    });
    let small = image::imageops::resize(
        &flat,
        COLOUR_GRID,
        COLOUR_GRID,
        image::imageops::FilterType::CatmullRom,
    );
    let colour = small
        .as_raw()
        .iter()
        .map(|v| f64::from(*v) / 255.0)
        .collect();
    let grey = image::GrayImage::from_fn(CANON, CANON, |x, y| {
        let p = flat.get_pixel(x, y);
        image::Luma([
            ((299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]) + 500) / 1000)
                as u8,
        ])
    });
    let edges = image::GrayImage::from_fn(CANON, CANON, |x, y| {
        if x == 0 || y == 0 || x == CANON - 1 || y == CANON - 1 {
            return *grey.get_pixel(x, y);
        }
        let mut energy = 8 * i32::from(grey.get_pixel(x, y)[0]);
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dy != 0 {
                    energy -= i32::from(
                        grey.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0],
                    );
                }
            }
        }
        image::Luma([energy.clamp(0, 255) as u8])
    });
    let small =
        image::imageops::resize(&edges, GRID, GRID, image::imageops::FilterType::CatmullRom);
    let structure = small
        .as_raw()
        .iter()
        .map(|v| f64::from(*v) / 255.0)
        .collect();
    Ok(Fingerprint {
        silhouette,
        colour,
        structure,
    })
}

fn mean_abs(left: &[f64], right: &[f64]) -> f64 {
    if left.is_empty()
        || left.len() != right.len()
        || left.iter().chain(right).any(|v| !v.is_finite())
    {
        return 1.0;
    }
    (left
        .iter()
        .zip(right)
        .map(|(a, b)| (a - b).abs())
        .sum::<f64>()
        / left.len() as f64)
        .clamp(0.0, 1.0)
}

pub fn distance(left: &Fingerprint, right: &Fingerprint) -> Distance {
    let silhouette = mean_abs(&left.silhouette, &right.silhouette);
    let colour = mean_abs(&left.colour, &right.colour);
    let structure = mean_abs(&left.structure, &right.structure);
    Distance {
        total: (silhouette * 0.4 + colour * 0.4 + structure * 0.2).clamp(0.0, 1.0),
        silhouette,
        colour,
        structure,
    }
}

pub fn rank(
    target: &Fingerprint,
    index: &BTreeMap<String, Value>,
    k: usize,
    exclude: &BTreeSet<String>,
) -> Vec<(f64, String)> {
    let mut scored: Vec<_> = index
        .iter()
        .filter(|(id, _)| !exclude.contains(*id))
        .map(|(id, value)| {
            let value = serde_json::from_value::<Fingerprint>(value.clone()).unwrap_or_default();
            (distance(target, &value).total, id.clone())
        })
        .collect();
    scored.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    scored.truncate(k);
    scored
}
