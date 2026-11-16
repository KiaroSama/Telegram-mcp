use super::*;

fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    output.extend_from_slice(&crc.finalize().to_be_bytes());
}
fn apng() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&4u32.to_be_bytes());
    header.extend_from_slice(&4u32.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut data, b"IHDR", &header);
    let mut animation = Vec::new();
    animation.extend_from_slice(&3u32.to_be_bytes());
    animation.extend_from_slice(&0u32.to_be_bytes());
    chunk(&mut data, b"acTL", &animation);
    let mut sequence = 0u32;
    for (index, colour) in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
        .into_iter()
        .enumerate()
    {
        let mut control = Vec::new();
        for value in [sequence, 4, 4, 0, 0] {
            control.extend_from_slice(&value.to_be_bytes());
        }
        sequence += 1;
        control.extend_from_slice(&1u16.to_be_bytes());
        control.extend_from_slice(&10u16.to_be_bytes());
        control.extend_from_slice(&[0, 0]);
        chunk(&mut data, b"fcTL", &control);
        let mut row = vec![0];
        row.extend(colour.repeat(4));
        let raw = row.repeat(4);
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&raw).unwrap();
        let compressed = encoder.finish().unwrap();
        if index == 0 {
            chunk(&mut data, b"IDAT", &compressed);
        } else {
            let mut payload = sequence.to_be_bytes().to_vec();
            sequence += 1;
            payload.extend_from_slice(&compressed);
            chunk(&mut data, b"fdAT", &payload);
        }
    }
    chunk(&mut data, b"IEND", &[]);
    data
}

#[test]
fn native_apng_decoding_preserves_actual_colour_frames_and_sampling_endpoints() {
    let decoded = frames::decode_animation(&apng(), ".APNG", 2, 64).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].1["frame_count"], 3);
    assert_eq!(decoded[1].1["frame_index"], 2);
    let first = images::decode_image(&decoded[0].0).unwrap().to_rgba8();
    let last = images::decode_image(&decoded[1].0).unwrap().to_rgba8();
    assert_eq!(first.get_pixel(1, 1).0, [255, 0, 0, 255]);
    assert_eq!(last.get_pixel(1, 1).0, [0, 0, 255, 255]);
}

#[test]
fn native_animated_webp_keeps_blank_endpoints_and_visible_middle_frame() {
    let bytes = include_bytes!("../fixtures/emoji/transparent-red-transparent.webp");
    let decoded = frames::decode_animation(bytes, "webp", 3, 64).unwrap();
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded[0].1["transparent"], true);
    assert_eq!(decoded[1].1["transparent"], false);
    assert_eq!(decoded[2].1["transparent"], true);
    // image-webp's integer alpha blend can round 255 over transparent to 254;
    // require every pixel to remain opaque red within that one-channel unit.
    assert!(
        images::decode_image(&decoded[1].0)
            .unwrap()
            .to_rgba8()
            .pixels()
            .all(|pixel| pixel[0] >= 254 && pixel[1] == 0 && pixel[2] == 0 && pixel[3] == 255)
    );
}

#[test]
fn static_webp_does_not_claim_an_animation_even_when_frames_are_requested() {
    let picture = image::DynamicImage::new_rgba8(4, 4);
    let bytes = images::encode_image(&picture, "webp", 64, 90, false)
        .unwrap()
        .0;
    assert!(
        frames::decode_animation(&bytes, "webp", 3, 64)
            .unwrap_err()
            .to_string()
            .contains("not animated")
    );
}

#[test]
fn animation_decoders_refuse_pixel_bombs_from_gif_and_png_headers() {
    let mut gif = gif(3);
    gif[6..8].copy_from_slice(&60000u16.to_le_bytes());
    gif[8..10].copy_from_slice(&60000u16.to_le_bytes());
    assert!(
        frames::decode_animation(&gif, "gif", 2, 64)
            .unwrap_err()
            .to_string()
            .contains("pixels")
    );
    let picture = image::DynamicImage::new_rgba8(4, 4);
    let mut png = images::encode_image(&picture, "png", 64, 90, false)
        .unwrap()
        .0;
    png[16..20].copy_from_slice(&60000u32.to_be_bytes());
    png[20..24].copy_from_slice(&60000u32.to_be_bytes());
    let crc = crc32fast::hash(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(
        frames::decode_animation(&png, "png", 2, 64)
            .unwrap_err()
            .to_string()
            .contains("pixels")
    );
}

#[test]
fn damaged_animation_frame_never_returns_a_partial_successful_preview() {
    let mut bytes = gif(3);
    bytes.truncate(bytes.len() / 2);
    assert!(
        frames::decode_animation(&bytes, "gif", 3, 64)
            .unwrap_err()
            .to_string()
            .contains("invalid frame")
    );
}

#[test]
fn oversized_encoded_animation_is_rejected_before_trying_its_codec() {
    let data = vec![0; 64 * 1024 * 1024 + 1];
    assert!(
        frames::decode_animation(&data, "gif", 3, 64)
            .unwrap_err()
            .to_string()
            .contains("input limit")
    );
}
