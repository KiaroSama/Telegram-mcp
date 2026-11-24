use super::*;
use std::io::Write;

#[test]
fn native_worker_serializes_real_sampled_gif_pixels_and_frame_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.gif");
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for colour in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    4,
                    4,
                    image::Rgba(colour),
                )))
                .unwrap();
        }
    }
    std::fs::write(&path, bytes).unwrap();
    let result = worker(&json!({"path":path,"count":2,"max_dimension":64})).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 2);
    assert_eq!(result[0]["metadata"]["frame_count"], 2);
    let first = base64::engine::general_purpose::STANDARD
        .decode(result[0]["data"].as_str().unwrap())
        .unwrap();
    let image = crate::tools::visual::images::decode_image(&first)
        .unwrap()
        .to_rgba8();
    assert_eq!(image.get_pixel(1, 1).0, [255, 0, 0, 255]);
}

#[test]
fn native_worker_refuses_missing_paths_unknown_formats_and_unreadable_files() {
    assert!(
        worker(&json!({}))
            .unwrap_err()
            .to_string()
            .contains("path is missing")
    );
    assert!(
        worker(&json!({"path":"untrusted.exe"}))
            .unwrap_err()
            .to_string()
            .contains("supports GIF")
    );
    let temp = tempfile::tempdir().unwrap();
    assert!(
        worker(&json!({"path":temp.path().join("missing.gif")}))
            .unwrap_err()
            .to_string()
            .contains("opened")
    );
}

#[test]
fn native_worker_refuses_oversized_sparse_input_before_codec_work() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oversized.gif");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert!(
        worker(&json!({"path":path}))
            .unwrap_err()
            .to_string()
            .contains("64 MiB")
    );
}

#[test]
fn native_worker_routes_tgs_to_native_lottie_validation_without_image_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("input.tgs");
    let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed.write_all(b"{}").unwrap();
    std::fs::write(&path, compressed.finish().unwrap()).unwrap();
    assert!(
        worker(&json!({"path":path}))
            .unwrap_err()
            .to_string()
            .contains("width")
    );
}
