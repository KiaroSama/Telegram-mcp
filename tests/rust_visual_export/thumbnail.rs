use image::ImageReader;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::PathBuf;
use telegram_mcp::export::icc::{
    ExtractedIcc, IccOutcome, PixelLayout, convert_to_srgb, jpeg_source_metadata, smooth_scale,
    srgb_profile_bytes,
};
use telegram_mcp::export::thumbnail::{
    build_desktop_jpeg_thumbnail, calculate_thumb_size, write_media_thumbnail,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tdexport_icc")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn read_hex_pixels(stem: &str) -> Vec<u8> {
    let raw = std::fs::read_to_string(fixture_dir().join(format!("{stem}.hex"))).unwrap();
    let clean: Vec<u8> = raw.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    clean
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn provenance_icc_profiles_match_exact_oracle_rgb_and_grayscale_hashes() {
    let prov: Value =
        serde_json::from_slice(&std::fs::read(fixture_dir().join("provenance.json")).unwrap())
            .unwrap();
    let srgb = std::fs::read(fixture_dir().join("srgb.icc")).unwrap();
    assert_eq!(srgb.as_slice(), srgb_profile_bytes());
    let srgb_sha = sha256_hex(&srgb);

    for entry in prov["cases"].as_array().unwrap() {
        let case = entry["case"].as_str().unwrap();
        let mode = entry["input_mode"].as_str().unwrap_or("RGB");
        let icc = std::fs::read(fixture_dir().join(format!("{case}.icc"))).unwrap();
        assert_eq!(
            sha256_hex(&icc),
            entry["icc_sha256"].as_str().unwrap(),
            "ICC hash mismatch for {case}"
        );

        let (src_px, layout, expected_px, expected_sha_key) = if mode == "L" {
            let src = read_hex_pixels(&format!("{case}.source.gray"));
            let exp = read_hex_pixels(&format!("{case}.gray"));
            (src, PixelLayout::Gray8, exp, "expected_gray_sha256")
        } else {
            let src_name = if case == "adobe-rgb" {
                "source.rgb".to_owned()
            } else {
                format!("{case}.source.rgb")
            };
            let src = read_hex_pixels(&src_name);
            let exp = read_hex_pixels(&format!("{case}.rgb"));
            (src, PixelLayout::Rgb8, exp, "expected_rgb_sha256")
        };

        assert_eq!(
            sha256_hex(&src_px),
            entry["source_sha256"].as_str().unwrap(),
            "Source pixel hash mismatch for {case}"
        );
        let transform = convert_to_srgb(&src_px, 16, 16, layout, Some(&icc)).unwrap();
        assert_eq!(transform.source_layout, layout);
        assert_eq!(transform.destination_layout, layout);
        assert_eq!(transform.target, "sRGB");
        assert_eq!(transform.pixels, expected_px, "Pixel mismatch for {case}");
        assert_eq!(
            sha256_hex(&transform.pixels),
            entry[expected_sha_key].as_str().unwrap(),
            "Converted SHA-256 mismatch for {case}"
        );
        if case == "srgb" {
            assert_eq!(transform.outcome, IccOutcome::AlreadySrgb);
            assert_eq!(transform.outcome.summary_kind(), "unchanged");
        } else {
            assert_eq!(transform.outcome, IccOutcome::Converted);
            assert_eq!(transform.outcome.summary_kind(), "converted");
            assert_eq!(sha256_hex(&transform.emitted_profile), srgb_sha);
        }
    }
}

#[test]
fn adobe_thumbnail_pipeline_converts_before_smooth_scale_and_preserves_desktop_jpeg_metadata() {
    let prov: Value =
        serde_json::from_slice(&std::fs::read(fixture_dir().join("provenance.json")).unwrap())
            .unwrap();
    let thumb_meta = &prov["thumbnail"];
    let src_jpg =
        std::fs::read(fixture_dir().join(thumb_meta["source"].as_str().unwrap())).unwrap();
    let exp_jpg =
        std::fs::read(fixture_dir().join(thumb_meta["expected"].as_str().unwrap())).unwrap();
    assert_eq!(
        sha256_hex(&src_jpg),
        thumb_meta["source_sha256"].as_str().unwrap()
    );
    assert_eq!(
        sha256_hex(&exp_jpg),
        thumb_meta["expected_sha256"].as_str().unwrap()
    );

    let artifact = build_desktop_jpeg_thumbnail(&src_jpg, Some((8, 8)), None)
        .expect("Adobe RGB source JPEG must produce 8x8 Desktop thumbnail");
    assert_eq!((artifact.width, artifact.height), (8, 8));
    assert_eq!(artifact.output_layout, PixelLayout::Rgb8);
    assert_eq!(artifact.transform.outcome, IccOutcome::Converted);
    assert_eq!(artifact.transform.outcome.summary_kind(), "converted");

    let out_meta = jpeg_source_metadata(&artifact.bytes);
    let exp_meta = jpeg_source_metadata(&exp_jpg);
    assert_eq!(out_meta.dpm_x, exp_meta.dpm_x);
    assert_eq!(out_meta.dpm_y, exp_meta.dpm_y);
    assert_eq!(
        out_meta.icc,
        ExtractedIcc::Present(srgb_profile_bytes().to_vec())
    );
    assert_eq!(out_meta.icc, exp_meta.icc);

    let adobe_icc = std::fs::read(fixture_dir().join("adobe-rgb.icc")).unwrap();
    let exact_conv = convert_to_srgb(
        &read_hex_pixels("source.rgb"),
        16,
        16,
        PixelLayout::Rgb8,
        Some(&adobe_icc),
    )
    .unwrap();
    let (exact_scaled, _) =
        smooth_scale(&exact_conv.pixels, 16, 16, PixelLayout::Rgb8, 8, 8).unwrap();
    let (oracle_scaled, _) = smooth_scale(
        &read_hex_pixels("adobe-rgb.rgb"),
        16,
        16,
        PixelLayout::Rgb8,
        8,
        8,
    )
    .unwrap();
    assert_eq!(exact_scaled, oracle_scaled);
    let pre_jpeg_diff = artifact
        .scaled_pixels
        .iter()
        .zip(oracle_scaled.iter())
        .map(|(&a, &b)| a.abs_diff(b))
        .max()
        .unwrap_or(0);
    assert!(
        pre_jpeg_diff <= 4,
        "Pre-JPEG scaled sRGB pixels drifted from Qt oracle by {pre_jpeg_diff}"
    );

    let decoded_out = ImageReader::new(Cursor::new(&artifact.bytes))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap()
        .into_rgb8();
    let decoded_exp = ImageReader::new(Cursor::new(&exp_jpg))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap()
        .into_rgb8();
    assert_eq!(decoded_out.dimensions(), (8, 8));
    assert_eq!(decoded_exp.dimensions(), (8, 8));
    let total_diff: u64 = decoded_out
        .as_raw()
        .iter()
        .zip(decoded_exp.as_raw().iter())
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum();
    let mean_diff = total_diff as f64 / decoded_exp.as_raw().len() as f64;
    let (raw_unconverted_scaled, _) = smooth_scale(
        &read_hex_pixels("source.rgb"),
        16,
        16,
        PixelLayout::Rgb8,
        8,
        8,
    )
    .unwrap();
    let unconverted_diff: u64 = raw_unconverted_scaled
        .iter()
        .zip(decoded_exp.as_raw().iter())
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum();
    assert!(
        mean_diff <= 8.0 && total_diff < unconverted_diff,
        "Decoded Q=75 8x8 sRGB thumbnail mean diff {mean_diff:.2} exceeded bound"
    );
}

#[test]
fn unprofiled_gray_and_fallback_profiles_never_claim_false_srgb_conversion() {
    let gray_icc = std::fs::read(fixture_dir().join("gray-adobe.icc")).unwrap();
    let gray_src = read_hex_pixels("gray-adobe.source.gray");
    let gray_conv =
        convert_to_srgb(&gray_src, 16, 16, PixelLayout::Gray8, Some(&gray_icc)).unwrap();
    let (scaled_gray, scaled_layout) =
        smooth_scale(&gray_conv.pixels, 16, 16, PixelLayout::Gray8, 8, 8).unwrap();
    assert_eq!(scaled_layout, PixelLayout::Rgb8);
    assert_eq!(scaled_gray.len(), 8 * 8 * 3);

    let mut raw_unprofiled = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut raw_unprofiled, 90)
        .encode(
            &read_hex_pixels("source.rgb"),
            16,
            16,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    let unprof_thumb = build_desktop_jpeg_thumbnail(&raw_unprofiled, Some((8, 8)), None).unwrap();
    assert_eq!(unprof_thumb.transform.outcome, IccOutcome::Unprofiled);
    assert_eq!(unprof_thumb.transform.outcome.summary_kind(), "unchanged");
    assert_eq!(
        jpeg_source_metadata(&unprof_thumb.bytes).icc,
        ExtractedIcc::Unprofiled
    );

    let mut malformed = raw_unprofiled.clone();
    let broken_app2 = b"\xff\xe2\x00\x12ICC_PROFILE\x00\x01\x02\xaa\xbb";
    malformed.splice(2..2, broken_app2.iter().copied());
    let mal_thumb = build_desktop_jpeg_thumbnail(&malformed, Some((8, 8)), None).unwrap();
    assert_eq!(mal_thumb.transform.outcome, IccOutcome::MalformedFallback);
    assert_eq!(mal_thumb.transform.outcome.summary_kind(), "fallback");
    assert_eq!(
        jpeg_source_metadata(&mal_thumb.bytes).icc,
        ExtractedIcc::Unprofiled,
        "Malformed ICC fallback must never tag output with sRGB or corrupt APP2"
    );

    let mut cmyk_icc = std::fs::read(fixture_dir().join("adobe-rgb.icc")).unwrap();
    cmyk_icc[16..20].copy_from_slice(b"CMYK");
    let cmyk_spliced = telegram_mcp::export::icc::splice_jpeg_metadata(
        &raw_unprofiled,
        (3780, 3780),
        &cmyk_icc,
        b"",
    )
    .unwrap();
    let unsup_thumb = build_desktop_jpeg_thumbnail(&cmyk_spliced, Some((8, 8)), None).unwrap();
    assert_eq!(
        unsup_thumb.transform.outcome,
        IccOutcome::UnsupportedFallback
    );
    assert_eq!(unsup_thumb.transform.outcome.summary_kind(), "fallback");
    assert_ne!(
        jpeg_source_metadata(&unsup_thumb.bytes).icc,
        ExtractedIcc::Present(srgb_profile_bytes().to_vec()),
        "Unsupported ICC fallback must never claim sRGB profile conversion"
    );
}

#[test]
fn write_media_thumbnail_creates_even_desktop_thumb_in_allowed_export_folder() {
    if super::isolated(
        "thumbnail::write_media_thumbnail_creates_even_desktop_thumb_in_allowed_export_folder",
    ) {
        return;
    }
    let dir = PathBuf::from(std::env::var("TEST_EXPORT_ROOT").unwrap()).join("thumbs-e2e");
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    let adobe = std::fs::read(fixture_dir().join("adobe-rgb.icc")).unwrap();
    let px = vec![180_u8; 200 * 121 * 3];
    let mut raw = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut raw, 85)
        .encode(&px, 200, 121, image::ExtendedColorType::Rgb8)
        .unwrap();
    let profiled =
        telegram_mcp::export::icc::splice_jpeg_metadata(&raw, (3780, 3780), &adobe, b"").unwrap();
    assert_eq!(
        calculate_thumb_size(200, 121, 520, 520, 80, 80, false),
        (200, 120)
    );
    let (written, icc) = write_media_thumbnail(&dir, "photos/1_2_photo_2.jpg", &profiled, false);
    let wt = written.expect("200x121 photo must emit _thumb.jpg");
    assert_eq!(wt.relative_path, "photos/1_2_photo_2_thumb.jpg");
    assert_eq!((wt.width, wt.height), (200, 120));
    assert!(wt.byte_size > 0);
    assert_eq!(icc.unwrap().outcome, IccOutcome::Converted);
    let saved = std::fs::read(dir.join(&wt.relative_path)).unwrap();
    assert_eq!(
        jpeg_source_metadata(&saved).icc,
        ExtractedIcc::Present(srgb_profile_bytes().to_vec())
    );
}
