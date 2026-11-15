use anyhow::{Result, ensure};
use image::DynamicImage;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, path::Path};

pub fn contact_sheet(
    images: &BTreeMap<String, DynamicImage>,
    order: &[String],
    labels: &BTreeMap<String, String>,
    columns: usize,
    cell: u32,
    out: &Path,
) -> Result<Value> {
    ensure!(
        !order.is_empty() && order.len() <= 512,
        "A studio sheet requires 1 to 512 emoji ids"
    );
    ensure!(
        (1..=32).contains(&columns) && (16..=512).contains(&cell),
        "Sheet columns must be 1-32 and cell size 16-512"
    );
    let (gap, foot) = (12_u32, 15_u32);
    let rows = order.len().div_ceil(columns);
    let (width, height) = (
        columns as u32 * (cell + gap) + gap,
        rows as u32 * (cell + gap + foot) + gap,
    );
    ensure!(
        u64::from(width) * u64::from(height) <= crate::tools::visual::images::MAX_DECODED_PIXELS,
        "Studio contact sheet exceeds the pixel budget"
    );
    let mut sheet = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));
    let mut cells = Vec::new();
    for (index, id) in order.iter().enumerate() {
        let (left, top) = (
            gap + index as u32 % columns as u32 * (cell + gap),
            gap + index as u32 / columns as u32 * (cell + gap + foot),
        );
        if let Some(picture) = images.get(id) {
            let picture = picture.resize(cell, cell, image::imageops::FilterType::Lanczos3);
            image::imageops::overlay(
                &mut sheet,
                &picture.to_rgba8(),
                i64::from(left + (cell - picture.width()) / 2),
                i64::from(top + (cell - picture.height()) / 2),
            );
        }
        for x in left.saturating_sub(2)..=left + cell + 1 {
            for y in [top - 2, top + cell + 1] {
                sheet.put_pixel(x, y, image::Rgba([205, 205, 205, 255]));
            }
        }
        for y in top.saturating_sub(2)..=top + cell + 1 {
            for x in [left - 2, left + cell + 1] {
                sheet.put_pixel(x, y, image::Rgba([205, 205, 205, 255]));
            }
        }
        let label = labels.get(id).cloned().unwrap_or_else(|| {
            id.chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        });
        paint(&mut sheet, &label, left, top + cell + 3, cell);
        cells.push(json!({"id":id,"label":label,"column":index%columns,"row":index/columns,"available":images.contains_key(id)}));
    }
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    DynamicImage::ImageRgba8(sheet).write_to(temporary.as_file_mut(), image::ImageFormat::Png)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(out)?;
    let metadata = json!({"width":width,"height":height,"columns":columns,"rows":rows,"cells":cells,"label_font":"ASCII bitmap; complete Unicode labels are preserved in metadata"});
    let sidecar = out.with_file_name(format!(
        "{}.json",
        out.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(serde_json::to_string_pretty(&metadata)?.as_bytes())?;
    file.as_file_mut().sync_all()?;
    file.persist(sidecar)?;
    Ok(metadata)
}

fn paint(image: &mut image::RgbaImage, text: &str, left: u32, top: u32, width: u32) {
    for (index, ch) in text.chars().take((width / 6) as usize).enumerate() {
        let rows = match ch {
            '0' => [14, 17, 19, 21, 25, 17, 14],
            '1' => [4, 12, 4, 4, 4, 4, 14],
            '2' => [14, 17, 1, 2, 4, 8, 31],
            '3' => [30, 1, 1, 14, 1, 1, 30],
            '4' => [2, 6, 10, 18, 31, 2, 2],
            '5' => [31, 16, 16, 30, 1, 1, 30],
            '6' => [14, 16, 16, 30, 17, 17, 14],
            '7' => [31, 1, 2, 4, 8, 8, 8],
            '8' => [14, 17, 17, 14, 17, 17, 14],
            '9' => [14, 17, 17, 15, 1, 1, 14],
            'Q' => [14, 17, 17, 17, 21, 18, 13],
            'U' => [17, 17, 17, 17, 17, 17, 14],
            'E' => [31, 16, 16, 30, 16, 16, 31],
            'R' => [30, 17, 17, 30, 20, 18, 17],
            'Y' => [17, 17, 10, 4, 4, 4, 4],
            '.' => [0, 0, 0, 0, 0, 12, 12],
            '-' => [0, 0, 0, 31, 0, 0, 0],
            ' ' => [0; 7],
            _ => [14, 17, 1, 2, 4, 0, 4],
        };
        for (y, row) in rows.iter().enumerate() {
            for x in 0..5 {
                if row & (1 << (4 - x)) != 0 {
                    image.put_pixel(
                        left + index as u32 * 6 + x,
                        top + y as u32,
                        image::Rgba([70, 70, 70, 255]),
                    );
                }
            }
        }
    }
}
