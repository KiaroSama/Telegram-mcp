//! Telegram Desktop / Qt JPEG APP2 ICC extraction, JFIF density, COM comments, and encoding.

use super::{ExtractedIcc, IccError, MAX_ICC_PROFILE_BYTES, PixelLayout, be16, validate_buf};
use image::ExtendedColorType;
use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
use std::collections::BTreeMap;

pub const DEFAULT_DOTS_PER_METER: u32 = 3_780;
const MAX_JPEG_ICC_CHUNK: usize = 65_533 - 14;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegSourceMetadata {
    pub dpm_x: u32,
    pub dpm_y: u32,
    pub icc: ExtractedIcc,
    pub comments: Vec<u8>,
}

pub fn extract_jpeg_icc(data: &[u8]) -> ExtractedIcc {
    jpeg_source_metadata(data).icc
}

pub fn jpeg_source_metadata(data: &[u8]) -> JpegSourceMetadata {
    let mut dpm = [DEFAULT_DOTS_PER_METER, DEFAULT_DOTS_PER_METER];
    let (mut segs, mut exp_tot, mut bad) = (BTreeMap::<u8, Vec<u8>>::new(), None, false);
    let mut texts: BTreeMap<Vec<u16>, (String, String)> = BTreeMap::new();
    let mut i = if data.starts_with(b"\xff\xd8") {
        2
    } else {
        data.len()
    };
    while i + 4 <= data.len() && data[i] == 0xFF {
        let m = data[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if matches!(m, 0xD9 | 0xDA) {
            break;
        }
        if matches!(m, 0x01 | 0xD0..=0xD8) {
            i += 2;
            continue;
        }
        let len = be16(&data[i + 2..i + 4]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            break;
        }
        let p = &data[i + 4..i + 2 + len];
        if m == 0xE0 && p.len() >= 12 && &p[..5] == b"JFIF\0" {
            for (ax, raw) in [be16(&p[8..10]), be16(&p[10..12])].into_iter().enumerate() {
                let v = match p[7] {
                    1 => (100.0 * raw as f64 / 2.54) as u32,
                    2 => 100 * raw as u32,
                    _ => 0,
                };
                if v > 0 {
                    dpm[ax] = v;
                }
            }
        } else if m == 0xE2 && p.len() >= 14 && &p[..12] == b"ICC_PROFILE\0" {
            let (seq, tot, ch) = (p[12], p[13], &p[14..]);
            let clash =
                exp_tot.is_some_and(|t| t != tot) || segs.insert(seq, ch.to_vec()).is_some();
            if seq == 0 || tot == 0 || seq > tot || ch.is_empty() || clash {
                bad = true;
            } else {
                exp_tot = Some(tot);
            }
        } else if m == 0xFE {
            let c = String::from_utf8_lossy(p);
            let (k, v) = match c.find(": ") {
                Some(idx) if c.find(' ').is_none_or(|sp| sp >= idx) => (&c[..idx], &c[idx + 2..]),
                _ => ("Description", c.as_ref()),
            };
            if !k.is_empty() {
                texts.insert(k.encode_utf16().collect(), (k.to_owned(), v.to_owned()));
            }
        }
        i += 2 + len;
    }
    let icc = assemble_jpeg_icc(bad, exp_tot, &segs);
    let mut comments = Vec::new();
    for (_, (k, v)) in texts {
        let s = format!("{k}: {v}");
        let b = &s.as_bytes()[..s.len().min(65_533)];
        comments.extend_from_slice(&[0xFF, 0xFE]);
        comments.extend_from_slice(&((b.len() + 2) as u16).to_be_bytes());
        comments.extend_from_slice(b);
    }
    JpegSourceMetadata {
        dpm_x: dpm[0],
        dpm_y: dpm[1],
        icc,
        comments,
    }
}

fn assemble_jpeg_icc(bad: bool, exp_tot: Option<u8>, segs: &BTreeMap<u8, Vec<u8>>) -> ExtractedIcc {
    let Some(tot) = (!bad).then_some(exp_tot).flatten() else {
        return if bad {
            ExtractedIcc::Malformed
        } else {
            ExtractedIcc::Unprofiled
        };
    };
    if segs.len() != tot as usize {
        return ExtractedIcc::Malformed;
    }
    let mut out = Vec::new();
    for s in 1..=tot {
        let Some(ch) = segs.get(&s) else {
            return ExtractedIcc::Malformed;
        };
        if out.len().saturating_add(ch.len()) > MAX_ICC_PROFILE_BYTES {
            return ExtractedIcc::Malformed;
        }
        out.extend_from_slice(ch);
    }
    if out.is_empty() {
        ExtractedIcc::Malformed
    } else {
        ExtractedIcc::Present(out)
    }
}

pub fn density_fields(dpm_x: u32, dpm_y: u32) -> (u8, u16, u16) {
    let qr = |v: f64| (v + 0.5) as u32;
    let (ix, iy) = (dpm_x as f64 * 2.54 / 100.0, dpm_y as f64 * 2.54 / 100.0);
    let (cx, cy) = (dpm_x as f64 / 100.0, dpm_y as f64 / 100.0);
    let diff_in = (ix - qr(ix) as f64).abs() + (iy - qr(iy) as f64).abs();
    let diff_cm = ((cx - qr(cx) as f64).abs() + (cy - qr(cy) as f64).abs()) * 2.54;
    if diff_in < diff_cm {
        (1, qr(ix).min(65_535) as u16, qr(iy).min(65_535) as u16)
    } else {
        (
            2,
            ((dpm_x + 50) / 100).min(65_535) as u16,
            ((dpm_y + 50) / 100).min(65_535) as u16,
        )
    }
}

pub fn splice_jpeg_metadata(
    enc: &[u8],
    dpm: (u32, u32),
    icc: &[u8],
    comments: &[u8],
) -> Option<Vec<u8>> {
    if !enc.starts_with(b"\xff\xd8") {
        return None;
    }
    let (unit, dx, dy) = density_fields(dpm.0, dpm.1);
    let (app0, rest) = if enc.len() >= 18 && &enc[2..4] == b"\xff\xe0" && &enc[6..11] == b"JFIF\0" {
        let len = be16(&enc[4..6]) as usize;
        let end = 4_usize
            .checked_add(len)
            .filter(|&e| len >= 16 && e <= enc.len())?;
        let mut a = enc[2..end].to_vec();
        a[11] = unit;
        a[12..14].copy_from_slice(&dx.to_be_bytes());
        a[14..16].copy_from_slice(&dy.to_be_bytes());
        (a, end)
    } else {
        let ([xh, xl], [yh, yl]) = (dx.to_be_bytes(), dy.to_be_bytes());
        (
            vec![
                0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, unit, xh, xl, yh, yl, 0, 0,
            ],
            2,
        )
    };
    let chunks: Vec<&[u8]> = if icc.is_empty() {
        Vec::new()
    } else {
        icc.chunks(MAX_JPEG_ICC_CHUNK).collect()
    };
    let total = u8::try_from(chunks.len()).ok()?;
    let mut out = Vec::with_capacity(enc.len() + app0.len() + comments.len() + icc.len() + 32);
    out.extend_from_slice(b"\xff\xd8");
    out.extend_from_slice(&app0);
    out.extend_from_slice(comments);
    for (idx, chunk) in chunks.into_iter().enumerate() {
        let blen = u16::try_from(16 + chunk.len()).ok()?;
        out.extend_from_slice(b"\xff\xe2");
        out.extend_from_slice(&blen.to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        out.extend_from_slice(&[(idx + 1) as u8, total]);
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&enc[rest..]);
    Some(out)
}

#[allow(clippy::too_many_arguments)]
pub fn encode_desktop_jpeg(
    px: &[u8],
    width: u32,
    height: u32,
    layout: PixelLayout,
    quality: Option<u8>,
    dpm: (u32, u32),
    icc: &[u8],
    comments: &[u8],
) -> Result<Vec<u8>, IccError> {
    validate_buf(px, width, height, layout)?;
    let (unit, dx, dy) = density_fields(dpm.0, dpm.1);
    let mut raw = Vec::new();
    let q = quality.map_or(75, |v| v.min(100)).max(1);
    let mut enc = JpegEncoder::new_with_quality(&mut raw, q);
    let punit = if unit == 2 {
        PixelDensityUnit::Centimeters
    } else {
        PixelDensityUnit::Inches
    };
    enc.set_pixel_density(PixelDensity {
        density: (dx, dy),
        unit: punit,
    });
    let (buf, ct) = match layout {
        PixelLayout::Rgb8 => (px.to_vec(), ExtendedColorType::Rgb8),
        PixelLayout::Gray8 => (px.to_vec(), ExtendedColorType::L8),
        PixelLayout::Rgba8 => (
            px.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
            ExtendedColorType::Rgb8,
        ),
    };
    enc.encode(&buf, width, height, ct)
        .map_err(|e| IccError::EncodeFailed(e.to_string()))?;
    let icc_out = if layout == PixelLayout::Gray8 {
        &[]
    } else {
        icc
    };
    splice_jpeg_metadata(&raw, dpm, icc_out, comments)
        .ok_or_else(|| IccError::EncodeFailed("failed to splice JPEG metadata".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::icc::srgb_profile_bytes;

    #[test]
    fn reassembles_multisegment_app2_and_splices_encoded_jpeg() {
        let srgb = srgb_profile_bytes();
        let mid = srgb.len() / 2;
        let mut jpg =
            b"\xff\xd8\xff\xe0\x00\x10JFIF\0\x01\x01\x01\x00\x48\x00\x48\x00\x00".to_vec();
        for (seq, part) in [(2_u8, &srgb[mid..]), (1_u8, &srgb[..mid])] {
            jpg.extend_from_slice(b"\xff\xe2");
            jpg.extend_from_slice(&((16 + part.len()) as u16).to_be_bytes());
            jpg.extend_from_slice(b"ICC_PROFILE\0");
            jpg.extend_from_slice(&[seq, 2]);
            jpg.extend_from_slice(part);
        }
        jpg.extend_from_slice(b"\xff\xda\x00\x02");
        let meta = jpeg_source_metadata(&jpg);
        assert_eq!(
            (meta.dpm_x, meta.dpm_y, meta.icc),
            (2834, 2834, ExtractedIcc::Present(srgb.to_vec()))
        );

        let enc = encode_desktop_jpeg(
            &[120, 80, 40],
            1,
            1,
            PixelLayout::Rgb8,
            Some(75),
            (3780, 3780),
            srgb,
            b"",
        )
        .expect("jpg");
        assert_eq!(extract_jpeg_icc(&enc), ExtractedIcc::Present(srgb.to_vec()));
    }
}
