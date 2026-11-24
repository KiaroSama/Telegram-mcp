use serde_json::{Value, json};
use std::io::{Cursor, Write};
use telegram_mcp::tools::visual::{frames, images, lottie};
#[path = "rust_visual_frames/extra.rs"]
mod extra;

fn gif(frame_count: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for index in 0..frame_count {
            let color = match index % 3 {
                0 => [255, 0, 0, 255],
                1 => [0, 255, 0, 255],
                _ => [0, 0, 255, 255],
            };
            let image = image::RgbaImage::from_pixel(16, 16, image::Rgba(color));
            encoder
                .encode_frame(image::Frame::from_parts(
                    image,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(40, 1),
                ))
                .unwrap();
        }
    }
    bytes
}

fn gzip(value: &Value) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    encoder.finish().unwrap()
}

fn animation() -> Value {
    json!({"v":"5.5.7","fr":30,"ip":0,"op":30,"w":64,"h":64,"nm":"test","ddd":0,"assets":[],
        "layers":[{"ddd":0,"ind":1,"ty":1,"nm":"solid","sr":1,"sc":"#ff0000","sw":64,"sh":64,
        "ks":{"o":{"a":1,"k":[{"t":0,"s":[100],"i":{"x":[1],"y":[1]},"o":{"x":[0],"y":[0]}},{"t":29,"s":[0]}]},
        "r":{"a":0,"k":0},"p":{"a":0,"k":[32,32,0]},"a":{"a":0,"k":[32,32,0]},"s":{"a":0,"k":[100,100,100]}},
        "ao":0,"ip":0,"op":30,"st":0,"bm":0}]})
}

#[test]
fn gif_frames_are_actual_distinct_sampled_frames() {
    let result = frames::decode_animation(&gif(3), "gif", 2, 1568).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].1["frame_index"], 0);
    assert_eq!(result[1].1["frame_index"], 2);
    assert_eq!(result[0].1["frame_count"], 3);
    assert_ne!(
        images::decode_image(&result[0].0).unwrap().to_rgba8(),
        images::decode_image(&result[1].0).unwrap().to_rgba8()
    );
}
#[test]
fn gif_frame_count_is_clamped_to_available_source_frames() {
    assert_eq!(
        frames::decode_animation(&gif(3), "gif", 100, 1568)
            .unwrap()
            .len(),
        3
    );
}
#[test]
fn gif_frame_samples_respect_requested_pixel_budget() {
    let frames = frames::decode_animation(&gif(3), "gif", 2, 64).unwrap();
    assert_eq!(frames[0].1["width"], 16);
    assert_eq!(frames[0].1["height"], 16);
}
#[test]
fn gif_zero_requested_frames_is_bounded_to_one() {
    assert_eq!(
        frames::decode_animation(&gif(3), "gif", 0, 1568)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn gif_excessive_source_frames_are_refused_without_a_fallback_decoder() {
    assert!(
        frames::decode_animation(&gif(3001), "gif", 2, 1568)
            .unwrap_err()
            .to_string()
            .contains("3000-frame")
    );
}
#[test]
fn static_gif_is_reported_as_not_animated() {
    assert!(
        frames::decode_animation(&gif(1), "gif", 2, 1568)
            .unwrap_err()
            .to_string()
            .contains("not animated")
    );
}
#[test]
fn damaged_gif_is_refused_as_decode_error() {
    assert!(frames::decode_animation(b"GIF89a broken", "gif", 2, 1568).is_err());
}
#[test]
fn unknown_animation_suffix_is_refused_before_decode() {
    assert!(frames::decode_animation(&gif(3), "hta", 2, 1568).is_err());
}
#[test]
fn static_png_never_masquerades_as_an_animation() {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    assert!(
        frames::decode_animation(&bytes.into_inner(), "png", 2, 1568)
            .unwrap_err()
            .to_string()
            .contains("not animated")
    );
}
#[test]
fn tgs_invalid_compression_is_refused_before_native_renderer() {
    assert!(
        lottie::parse_tgs(b"not gzip")
            .unwrap_err()
            .to_string()
            .contains("decompress")
    );
}
#[test]
fn tgs_valid_gzip_requires_lottie_dimensions() {
    assert!(
        lottie::parse_tgs(&gzip(&json!({})))
            .unwrap_err()
            .to_string()
            .contains("width")
    );
}
#[test]
fn tgs_canvas_limit_is_enforced_before_native_renderer() {
    let mut value = animation();
    value["w"] = 100000.into();
    value["h"] = 100000.into();
    assert!(
        lottie::parse_tgs(&gzip(&value))
            .unwrap_err()
            .to_string()
            .contains("pixel")
    );
}
#[test]
fn tgs_frame_limit_is_enforced_before_native_renderer() {
    let mut value = animation();
    value["op"] = 100000.into();
    assert!(
        lottie::parse_tgs(&gzip(&value))
            .unwrap_err()
            .to_string()
            .contains("bounds")
    );
}
#[test]
fn tgs_frame_rate_limit_is_enforced_before_native_renderer() {
    let mut value = animation();
    value["fr"] = 1000.into();
    assert!(lottie::parse_tgs(&gzip(&value)).is_err());
}
#[test]
fn tgs_external_image_resource_cannot_read_owner_files() {
    let mut value = animation();
    value["assets"] = json!([{"id":"image","p":"../../private.png","u":"/"}]);
    assert!(
        lottie::parse_tgs(&gzip(&value))
            .unwrap_err()
            .to_string()
            .contains("external")
    );
}
#[test]
fn tgs_structural_preflight_preserves_unicode() {
    let mut value = animation();
    value["nm"] = "انیمیشن 👨‍👩‍👧".into();
    assert_eq!(
        lottie::parse_tgs(&gzip(&value)).unwrap()["nm"],
        "انیمیشن 👨‍👩‍👧"
    );
}
#[test]
fn tgs_renderer_renders_distinct_frames_when_required() {
    let required = std::env::var("TELEGRAM_REQUIRE_LOTTIE").as_deref() == Ok("1");
    if !lottie::available() {
        assert!(
            !required,
            "Renderer CI requires a real rlottie library; configure TELEGRAM_RLOTTIE_LIBRARY"
        );
        let error = lottie::render_tgs(&gzip(&animation()), 2, 1568)
            .unwrap_err()
            .to_string();
        assert!(error.contains("TELEGRAM_RLOTTIE_LIBRARY"));
        assert!(error.contains("get_media_thumbnail"));
        return;
    }
    let result = lottie::render_tgs(&gzip(&animation()), 3, 64).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].1["source"], "rlottie");
    // The pinned current rlottie model reports the inclusive endpoint:
    // totalFrame() = endFrame - startFrame + 1 (lottiemodel.h).
    assert_eq!(result[0].1["frame_count"], 31);
    let first = images::decode_image(&result[0].0).unwrap().to_rgba8();
    let last = images::decode_image(&result[2].0).unwrap().to_rgba8();
    assert_ne!(first, last);
    assert_eq!(first.dimensions(), (64, 64));
    assert!(first.pixels().any(|pixel| pixel[0] > 0 && pixel[3] > 0));
    assert_eq!(result[2].1["transparent"], true);
}
