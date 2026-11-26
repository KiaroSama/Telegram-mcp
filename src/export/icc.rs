//! Bounded pure-Rust ICC conversion to sRGB, container ICC extraction, scaling, and JPEG helpers.

pub mod jpeg;
mod qt_matrix;
pub mod scale;

pub use jpeg::{
    DEFAULT_DOTS_PER_METER, JpegSourceMetadata, density_fields, encode_desktop_jpeg,
    extract_jpeg_icc, jpeg_source_metadata, splice_jpeg_metadata,
};
pub use scale::{ScaleTaps, down_taps, smooth_scale, up_taps};

use crc32fast::Hasher as Crc32Hasher;
use flate2::read::ZlibDecoder;
use moxcms::{
    ColorProfile, DataColorSpace, Layout, ParsingOptions, ProfileClass, TransformOptions,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, io::Read};

pub const MAX_ICC_PROFILE_BYTES: usize = 4 * 1024 * 1024;

#[rustfmt::skip]
pub(crate) const SRGB_ICC_PROFILE: [u8; 459] = [
    0x00,0x00,0x01,0xcb,0x00,0x00,0x00,0x00,0x02,0x40,0x00,0x00,0x6d,0x6e,0x74,0x72,0x52,0x47,0x42,0x20,0x58,0x59,0x5a,0x20,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x61,0x63,0x73,0x70,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x01,0x00,0x00,0xf6,0xd6,
    0x00,0x01,0x00,0x00,0x00,0x00,0xd3,0x2d,0x51,0x74,0x05,0x0f,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x09,0x72,0x58,0x59,0x5a,0x00,0x00,0x00,0xf0,0x00,0x00,0x00,0x14,
    0x67,0x58,0x59,0x5a,0x00,0x00,0x01,0x04,0x00,0x00,0x00,0x14,0x62,0x58,0x59,0x5a,0x00,0x00,0x01,0x18,0x00,0x00,0x00,0x14,
    0x77,0x74,0x70,0x74,0x00,0x00,0x01,0x2c,0x00,0x00,0x00,0x14,0x63,0x70,0x72,0x74,0x00,0x00,0x01,0x40,0x00,0x00,0x00,0x0c,
    0x72,0x54,0x52,0x43,0x00,0x00,0x01,0x4c,0x00,0x00,0x00,0x20,0x67,0x54,0x52,0x43,0x00,0x00,0x01,0x4c,0x00,0x00,0x00,0x20,
    0x62,0x54,0x52,0x43,0x00,0x00,0x01,0x4c,0x00,0x00,0x00,0x20,0x64,0x65,0x73,0x63,0x00,0x00,0x01,0x6c,0x00,0x00,0x00,0x5f,
    0x58,0x59,0x5a,0x20,0x00,0x00,0x00,0x00,0x00,0x00,0x6f,0x9f,0x00,0x00,0x38,0xf4,0x00,0x00,0x03,0x91,0x58,0x59,0x5a,0x20,
    0x00,0x00,0x00,0x00,0x00,0x00,0x62,0x96,0x00,0x00,0xb7,0x87,0x00,0x00,0x18,0xdc,0x58,0x59,0x5a,0x20,0x00,0x00,0x00,0x00,
    0x00,0x00,0x24,0xa1,0x00,0x00,0x0f,0x85,0x00,0x00,0xb6,0xd3,0x58,0x59,0x5a,0x20,0x00,0x00,0x00,0x00,0x00,0x00,0xf3,0x4f,
    0x00,0x01,0x00,0x00,0x00,0x01,0x16,0xc2,0x74,0x65,0x78,0x74,0x00,0x00,0x00,0x00,0x4e,0x2f,0x41,0x00,0x70,0x61,0x72,0x61,
    0x00,0x00,0x00,0x00,0x00,0x03,0x00,0x00,0x00,0x02,0x66,0x66,0x00,0x00,0xf2,0xa7,0x00,0x00,0x0d,0x59,0x00,0x00,0x13,0xd0,
    0x00,0x00,0x0a,0x5b,0x64,0x65,0x73,0x63,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x05,0x73,0x52,0x47,0x42,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
    0x00,0x00,0x00,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelLayout {
    Rgb8,
    Rgba8,
    Gray8,
}

impl PixelLayout {
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
            Self::Gray8 => 1,
        }
    }

    pub fn expected_len(self, width: u32, height: u32) -> Option<usize> {
        let px = (width != 0 && height != 0).then_some(width as usize)?;
        px.checked_mul(height as usize)?
            .checked_mul(self.channels())
    }

    pub const fn to_moxcms(self) -> Layout {
        match self {
            Self::Rgb8 => Layout::Rgb,
            Self::Rgba8 => Layout::Rgba,
            Self::Gray8 => Layout::Gray,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IccOutcome {
    Converted,
    Unprofiled,
    AlreadySrgb,
    UnsupportedFallback,
    MalformedFallback,
}

impl IccOutcome {
    pub const fn is_converted(self) -> bool {
        matches!(self, Self::Converted)
    }

    pub const fn is_unchanged(self) -> bool {
        matches!(self, Self::Unprofiled | Self::AlreadySrgb)
    }

    pub const fn is_fallback(self) -> bool {
        matches!(self, Self::UnsupportedFallback | Self::MalformedFallback)
    }

    pub const fn summary_kind(self) -> &'static str {
        match self {
            Self::Converted => "converted",
            Self::Unprofiled | Self::AlreadySrgb => "unchanged",
            Self::UnsupportedFallback | Self::MalformedFallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IccError {
    InvalidDimensions {
        width: u32,
        height: u32,
    },
    BufferLengthMismatch {
        expected: usize,
        actual: usize,
        layout: PixelLayout,
    },
    EncodeFailed(String),
}

impl fmt::Display for IccError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => write!(f, "invalid size {width}x{height}"),
            Self::BufferLengthMismatch {
                expected,
                actual,
                layout,
            } => {
                write!(f, "buffer length {actual} != {expected} for {layout:?}")
            }
            Self::EncodeFailed(msg) => write!(f, "JPEG thumbnail encode failed: {msg}"),
        }
    }
}
impl std::error::Error for IccError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractedIcc {
    Unprofiled,
    Present(Vec<u8>),
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportIccTransform {
    pub source_profile: Option<Vec<u8>>,
    pub target: &'static str,
    pub source_layout: PixelLayout,
    pub destination_layout: PixelLayout,
    pub outcome: IccOutcome,
    pub pixels: Vec<u8>,
    pub emitted_profile: Vec<u8>,
}

impl ExportIccTransform {
    pub fn apply(
        px: &[u8],
        width: u32,
        height: u32,
        layout: PixelLayout,
        profile: Option<&[u8]>,
    ) -> Result<Self, IccError> {
        validate_buf(px, width, height, layout)?;
        let Some(raw) = profile.filter(|b| !b.is_empty()) else {
            return Ok(Self::keep(px, layout, None, IccOutcome::Unprofiled));
        };
        if raw.len() > MAX_ICC_PROFILE_BYTES {
            return Ok(Self::keep(
                px,
                layout,
                Some(raw.to_vec()),
                IccOutcome::MalformedFallback,
            ));
        }
        match convert_profile_bytes(px, layout, raw) {
            Ok((IccOutcome::Converted, pixels)) => Ok(Self {
                source_profile: Some(raw.to_vec()),
                target: "sRGB",
                source_layout: layout,
                destination_layout: layout,
                outcome: IccOutcome::Converted,
                pixels,
                emitted_profile: SRGB_ICC_PROFILE.to_vec(),
            }),
            Ok((out, _)) | Err(out) => Ok(Self::keep(px, layout, Some(raw.to_vec()), out)),
        }
    }

    pub fn apply_from_container(
        px: &[u8],
        width: u32,
        height: u32,
        layout: PixelLayout,
        container: &[u8],
    ) -> Result<Self, IccError> {
        validate_buf(px, width, height, layout)?;
        match extract_embedded_icc(container) {
            ExtractedIcc::Unprofiled => Self::apply(px, width, height, layout, None),
            ExtractedIcc::Present(icc) => Self::apply(px, width, height, layout, Some(&icc)),
            ExtractedIcc::Malformed => {
                Ok(Self::keep(px, layout, None, IccOutcome::MalformedFallback))
            }
        }
    }

    fn keep(
        px: &[u8],
        layout: PixelLayout,
        source_profile: Option<Vec<u8>>,
        outcome: IccOutcome,
    ) -> Self {
        let emitted_profile = source_profile.clone().unwrap_or_default();
        Self {
            source_profile,
            target: "sRGB",
            source_layout: layout,
            destination_layout: layout,
            outcome,
            pixels: px.to_vec(),
            emitted_profile,
        }
    }
}

pub fn srgb_profile_bytes() -> &'static [u8] {
    &SRGB_ICC_PROFILE
}

pub fn convert_to_srgb(
    px: &[u8],
    width: u32,
    height: u32,
    layout: PixelLayout,
    profile: Option<&[u8]>,
) -> Result<ExportIccTransform, IccError> {
    ExportIccTransform::apply(px, width, height, layout, profile)
}

pub(crate) fn validate_buf(
    px: &[u8],
    width: u32,
    height: u32,
    layout: PixelLayout,
) -> Result<usize, IccError> {
    let expected = layout
        .expected_len(width, height)
        .ok_or(IccError::InvalidDimensions { width, height })?;
    (px.len() == expected)
        .then_some(expected)
        .ok_or(IccError::BufferLengthMismatch {
            expected,
            actual: px.len(),
            layout,
        })
}

pub(crate) fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
pub(crate) fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
pub(crate) fn bei32(b: &[u8]) -> i32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
pub(crate) fn qgray([r, g, b]: [u8; 3]) -> u8 {
    ((r as u16 * 11 + g as u16 * 16 + b as u16 * 5) >> 5) as u8
}

pub fn extract_embedded_icc(bytes: &[u8]) -> ExtractedIcc {
    if bytes.starts_with(b"\xff\xd8") {
        extract_jpeg_icc(bytes)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        extract_png_icc(bytes)
    } else {
        ExtractedIcc::Unprofiled
    }
}

pub fn extract_png_icc(data: &[u8]) -> ExtractedIcc {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return ExtractedIcc::Unprofiled;
    }
    let mut off = 8_usize;
    while let Some(hdr) = data.get(off..off + 8) {
        let (len, kind) = (be32(hdr) as usize, &hdr[4..8]);
        let Some(end) = off.checked_add(8).and_then(|v| v.checked_add(len)) else {
            return ExtractedIcc::Malformed;
        };
        let Some(crc_b) = data.get(end..end + 4) else {
            return ExtractedIcc::Malformed;
        };
        if kind == b"iCCP" {
            let p = &data[off + 8..end];
            let mut crc = Crc32Hasher::new();
            crc.update(kind);
            crc.update(p);
            let Some(nul) = p.iter().take(80).position(|&b| b == 0) else {
                return ExtractedIcc::Malformed;
            };
            let bad = crc.finalize() != be32(crc_b) || nul == 0 || p.get(nul + 1) != Some(&0);
            if bad || p.len() <= nul + 2 {
                return ExtractedIcc::Malformed;
            }
            let mut dec = ZlibDecoder::new(&p[nul + 2..]).take(MAX_ICC_PROFILE_BYTES as u64 + 1);
            let mut out = Vec::new();
            let ok = dec.read_to_end(&mut out).is_ok();
            return if ok && (1..=MAX_ICC_PROFILE_BYTES).contains(&out.len()) {
                ExtractedIcc::Present(out)
            } else {
                ExtractedIcc::Malformed
            };
        }
        if kind == b"sRGB" {
            return if len == 1 && data[off + 8] <= 3 {
                ExtractedIcc::Present(SRGB_ICC_PROFILE.to_vec())
            } else {
                ExtractedIcc::Malformed
            };
        }
        if kind == b"IEND" {
            break;
        }
        off = end + 4;
    }
    ExtractedIcc::Unprofiled
}

fn convert_profile_bytes(
    px: &[u8],
    layout: PixelLayout,
    p: &[u8],
) -> Result<(IccOutcome, Vec<u8>), IccOutcome> {
    if p.len() < 132 || &p[36..40] != b"acsp" {
        return Err(IccOutcome::MalformedFallback);
    }
    let (size, count) = (be32(&p[0..4]) as usize, be32(&p[128..132]) as usize);
    let tend = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(132))
        .ok_or(IccOutcome::MalformedFallback)?;
    if size < tend || size > p.len() {
        return Err(IccOutcome::MalformedFallback);
    }
    let mut tags: BTreeMap<[u8; 4], &[u8]> = BTreeMap::new();
    for i in 0..count {
        let b = 132 + i * 12;
        let (off, len) = (
            be32(&p[b + 4..b + 8]) as usize,
            be32(&p[b + 8..b + 12]) as usize,
        );
        let end = off.checked_add(len).ok_or(IccOutcome::MalformedFallback)?;
        if off < tend || off % 4 != 0 || len < 12 || end > size {
            return Err(IccOutcome::MalformedFallback);
        }
        tags.insert([p[b], p[b + 1], p[b + 2], p[b + 3]], &p[off..end]);
    }
    if !qt_matrix::valid_pcs_illuminant(&p[68..80]) {
        return Err(IccOutcome::MalformedFallback);
    }
    let opts = ParsingOptions {
        max_profile_size: MAX_ICC_PROFILE_BYTES,
        max_allowed_clut_size: 10_000_000,
        max_allowed_trc_size: 65_536,
    };
    let mox = ColorProfile::new_from_slice_with_options(p, opts)
        .map_err(|_| IccOutcome::MalformedFallback)?;
    let bad_cls = matches!(
        mox.profile_class,
        ProfileClass::DeviceLink | ProfileClass::Abstract | ProfileClass::Named
    );
    if !matches!(mox.color_space, DataColorSpace::Rgb | DataColorSpace::Gray) || bad_cls {
        return Err(IccOutcome::UnsupportedFallback);
    }
    let has_a2b = [b"A2B0", b"A2B1", b"A2B2"]
        .iter()
        .any(|k| tags.contains_key(*k));
    if &p[20..24] == b"XYZ " && !has_a2b {
        qt_matrix::convert_qt_matrix_trc(px, layout, p, &tags)
    } else {
        convert_via_moxcms(px, layout, &mox)
    }
}

fn convert_via_moxcms(
    px: &[u8],
    layout: PixelLayout,
    src: &ColorProfile,
) -> Result<(IccOutcome, Vec<u8>), IccOutcome> {
    let srgb = ColorProfile::new_srgb();
    let xf = |il: Layout, ol: Layout| {
        src.create_transform_8bit(il, &srgb, ol, TransformOptions::default())
            .map_err(|_| IccOutcome::UnsupportedFallback)
    };
    let mut out = vec![0_u8; px.len()];
    match (src.color_space, layout) {
        (DataColorSpace::Rgb, PixelLayout::Rgb8) => {
            xf(Layout::Rgb, Layout::Rgb)?
                .transform(px, &mut out)
                .map_err(|_| IccOutcome::MalformedFallback)?;
        }
        (DataColorSpace::Rgb, PixelLayout::Rgba8) => {
            xf(Layout::Rgba, Layout::Rgba)?
                .transform(px, &mut out)
                .map_err(|_| IccOutcome::MalformedFallback)?;
        }
        (DataColorSpace::Rgb, PixelLayout::Gray8) => {
            let rgb: Vec<u8> = px.iter().flat_map(|&g| [g, g, g]).collect();
            let mut tmp = vec![0_u8; rgb.len()];
            xf(Layout::Rgb, Layout::Rgb)?
                .transform(&rgb, &mut tmp)
                .map_err(|_| IccOutcome::MalformedFallback)?;
            out = tmp.as_chunks::<3>().0.iter().copied().map(qgray).collect();
        }
        (DataColorSpace::Gray, _) => {
            let mut tmp = vec![0_u8; px.len() * 3];
            xf(Layout::Gray, Layout::Rgb)?
                .transform(px, &mut tmp)
                .map_err(|_| IccOutcome::MalformedFallback)?;
            let chs = tmp.as_chunks::<3>().0;
            out = match layout {
                PixelLayout::Gray8 => chs.iter().copied().map(qgray).collect(),
                PixelLayout::Rgb8 => chs.iter().map(|c| c[0]).collect(),
                PixelLayout::Rgba8 => px
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .flat_map(|(i, s)| [chs[i * 4][0], chs[i * 4 + 1][0], chs[i * 4 + 2][0], s[3]])
                    .collect(),
            };
        }
        _ => return Err(IccOutcome::UnsupportedFallback),
    }
    Ok((IccOutcome::Converted, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::{io::Write, path::PathBuf};

    fn fix() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tdexport_icc")
    }

    fn hex_px(name: &str) -> Vec<u8> {
        let raw = std::fs::read_to_string(fix().join(format!("{name}.hex"))).expect("fixture");
        let clean: Vec<u8> = raw.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        clean
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("00"), 16).unwrap_or(0))
            .collect()
    }

    #[test]
    fn converts_all_qt_oracle_fixtures_and_rgba_alpha() {
        let srgb = std::fs::read(fix().join("srgb.icc")).expect("srgb");
        assert_eq!(srgb.as_slice(), srgb_profile_bytes());
        let adobe = std::fs::read(fix().join("adobe-rgb.icc")).expect("adobe");
        let r = convert_to_srgb(
            &hex_px("source.rgb"),
            16,
            16,
            PixelLayout::Rgb8,
            Some(&adobe),
        )
        .expect("ok");
        assert_eq!(
            (r.outcome, r.pixels),
            (IccOutcome::Converted, hex_px("adobe-rgb.rgb"))
        );
        #[rustfmt::skip]
        let rgb = [
            "parametric-0", "parametric-1", "parametric-2", "parametric-3", "parametric-4",
            "table", "table-offset", "linear-table", "linear", "gamma88", "channels",
            "apple", "custom-matrix", "gray", "srgb",
        ];
        for c in rgb {
            let icc = std::fs::read(fix().join(format!("{c}.icc"))).expect("icc");
            let out = convert_to_srgb(
                &hex_px(&format!("{c}.source.rgb")),
                16,
                16,
                PixelLayout::Rgb8,
                Some(&icc),
            )
            .expect("ok");
            let exp = if c == "srgb" {
                IccOutcome::AlreadySrgb
            } else {
                IccOutcome::Converted
            };
            assert_eq!(
                (out.outcome, out.pixels),
                (exp, hex_px(&format!("{c}.rgb"))),
                "{c}"
            );
        }
        for c in ["gray-jpeg", "gray-adobe"] {
            let icc = std::fs::read(fix().join(format!("{c}.icc"))).expect("icc");
            let out = convert_to_srgb(
                &hex_px(&format!("{c}.source.gray")),
                16,
                16,
                PixelLayout::Gray8,
                Some(&icc),
            )
            .expect("ok");
            assert_eq!(
                (out.outcome, out.pixels),
                (IccOutcome::Converted, hex_px(&format!("{c}.gray")))
            );
        }
        let rgba = convert_to_srgb(&[200, 100, 50, 77], 1, 1, PixelLayout::Rgba8, Some(&adobe))
            .expect("ok");
        assert_eq!((rgba.outcome, rgba.pixels[3]), (IccOutcome::Converted, 77));
    }

    #[test]
    fn rejects_bad_buffers_and_preserves_fallback_and_png_icc() {
        let px = [10_u8, 20, 30, 255];
        assert!(convert_to_srgb(&px[..3], 2, 2, PixelLayout::Rgb8, None).is_err());
        assert!(convert_to_srgb(&[], 0, 2, PixelLayout::Rgb8, None).is_err());
        assert_eq!(
            convert_to_srgb(&px, 1, 1, PixelLayout::Rgba8, None)
                .expect("ok")
                .outcome,
            IccOutcome::Unprofiled
        );
        let bad = convert_to_srgb(&px, 1, 1, PixelLayout::Rgba8, Some(b"bad")).expect("ok");
        assert_eq!(
            (bad.outcome, bad.pixels.as_slice()),
            (IccOutcome::MalformedFallback, px.as_slice())
        );
        let mut cmyk = std::fs::read(fix().join("parametric-1.icc")).expect("icc");
        cmyk[16..20].copy_from_slice(b"CMYK");
        let unsup = convert_to_srgb(&px, 1, 1, PixelLayout::Rgba8, Some(&cmyk)).expect("ok");
        assert_eq!(
            (unsup.outcome, unsup.pixels.as_slice()),
            (IccOutcome::UnsupportedFallback, px.as_slice())
        );

        let adobe = std::fs::read(fix().join("adobe-rgb.icc")).expect("adobe");
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&adobe).expect("z");
        let mut iccp = b"p\0\x00".to_vec();
        iccp.extend_from_slice(&z.finish().expect("fin"));
        let mut crc = Crc32Hasher::new();
        crc.update(b"iCCP");
        crc.update(&iccp);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&(iccp.len() as u32).to_be_bytes());
        png.extend_from_slice(b"iCCP");
        png.extend_from_slice(&iccp);
        png.extend_from_slice(&crc.finalize().to_be_bytes());
        assert_eq!(extract_png_icc(&png), ExtractedIcc::Present(adobe));
    }
}
