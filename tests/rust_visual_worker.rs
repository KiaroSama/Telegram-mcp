//! Native child integration: a real fixture window on Windows, real animated
//! pixels through the same bounded worker protocol on other platforms.

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

fn run_worker(request: &Value) -> Value {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
            command.arg("__capture-worker").env_clear();
            for key in [
                "PATH",
                "SystemRoot",
                "WINDIR",
                "TEMP",
                "TMP",
                "LANG",
                "LLVM_PROFILE_FILE",
            ] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            let mut child = command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(request).unwrap())
                .await
                .unwrap();
            let output =
                tokio::time::timeout(std::time::Duration::from_secs(25), child.wait_with_output())
                    .await
                    .expect(
                        "Native visual worker did not finish within its integration-test deadline",
                    )
                    .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["ok"], true, "{value}");
            value["result"].clone()
        })
}

#[test]
fn native_visual_worker_drains_responses_larger_than_pipe_capacity() {
    use base64::Engine as _;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("noise.gif");
    let image = image::RgbaImage::from_fn(512, 512, |x, y| {
        let mut n = x + y * 512;
        n = (n ^ (n >> 16)).wrapping_mul(0x7feb_352d);
        n = (n ^ (n >> 15)).wrapping_mul(0x846c_a68b);
        n ^= n >> 16;
        image::Rgba([(n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, 255])
    });
    let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
    encoder
        .encode_frame(image::Frame::new(image.clone()))
        .unwrap();
    encoder.encode_frame(image::Frame::new(image)).unwrap();
    drop(encoder);
    let expected = image::open(&path).unwrap().to_rgba8();
    let result =
        run_worker(&json!({"tool":"__decode_media","path":path,"count":1,"max_dimension":512}));
    let data = base64::engine::general_purpose::STANDARD
        .decode(result[0]["data"].as_str().unwrap())
        .unwrap();
    assert!(data.len() > 64 * 1024);
    assert_eq!(
        telegram_mcp::tools::visual::images::decode_image(&data)
            .unwrap()
            .to_rgba8(),
        expected
    );
}

#[test]
fn native_visual_worker_returns_actual_fixture_pixels_without_stdout_noise() {
    use base64::Engine as _;
    #[cfg(windows)]
    {
        let fixture = window::Fixture::create();
        let process = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let result = run_worker(
            &json!({"tool":"get_telegram_screen","process_name":process,"hwnd":fixture.handle.to_string(),
            "count":1,"interval_ms":50,"method":"window","client_only":true,"image_format":"png","max_dimension":256}),
        );
        let data = base64::engine::general_purpose::STANDARD
            .decode(result["content"][1]["data"].as_str().unwrap())
            .unwrap();
        let image = telegram_mcp::tools::visual::images::decode_image(&data).unwrap();
        assert!(image.width() > 0 && image.width() <= 256);
        assert!(image.height() > 0 && image.height() <= 256);
        assert_eq!(
            result["metadata"]["window"]["hwnd"],
            fixture.handle.to_string()
        );
        assert!(
            result["metadata"]["window"]["title"]
                .as_str()
                .unwrap()
                .contains("fixture")
        );
        let windows = run_worker(&json!({"tool":"list_telegram_windows","process_name":process}));
        assert!(
            windows["windows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|window| window["hwnd"] == fixture.handle.to_string())
        );
        let region = run_worker(
            &json!({"tool":"get_telegram_region","process_name":process,"hwnd":fixture.handle.to_string(),
            "count":1,"method":"window","left":0,"top":0,"right":100,"bottom":100,"image_format":"png","max_dimension":256}),
        );
        assert_eq!(region["metadata"]["frames"][0]["image"]["width"], 100);
        assert_eq!(region["metadata"]["frames"][0]["image"]["height"], 100);
        let frames = run_worker(
            &json!({"tool":"get_telegram_frames","process_name":process,"hwnd":fixture.handle.to_string(),
            "count":2,"interval_ms":50,"method":"window","image_format":"png","max_dimension":128}),
        );
        assert_eq!(frames["content"].as_array().unwrap().len(), 3);
        assert_eq!(frames["metadata"]["frames"].as_array().unwrap().len(), 2);
        assert!(
            frames["metadata"]["frames"][1]["elapsed_ms"]
                .as_u64()
                .unwrap()
                >= 50
        );
    }
    #[cfg(not(windows))]
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("animation.gif");
        {
            let mut encoder =
                image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        16,
                        16,
                        image::Rgba(color),
                    )))
                    .unwrap();
            }
        }
        let result =
            run_worker(&json!({"tool":"__decode_media","path":path,"count":2,"max_dimension":64}));
        let frames = result.as_array().unwrap();
        assert_eq!(frames.len(), 2);
        let first = base64::engine::general_purpose::STANDARD
            .decode(frames[0]["data"].as_str().unwrap())
            .unwrap();
        let last = base64::engine::general_purpose::STANDARD
            .decode(frames[1]["data"].as_str().unwrap())
            .unwrap();
        let first = telegram_mcp::tools::visual::images::decode_image(&first)
            .unwrap()
            .to_rgba8();
        let last = telegram_mcp::tools::visual::images::decode_image(&last)
            .unwrap()
            .to_rgba8();
        assert_eq!(first.dimensions(), (16, 16));
        assert_ne!(first, last);
        assert_eq!(frames[0]["metadata"]["source"], "rust-image");
    }
}

#[cfg(windows)]
mod window {
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, MSG, PostThreadMessageW,
            TranslateMessage, WM_QUIT, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    };
    pub struct Fixture {
        pub handle: isize,
        thread_id: u32,
        join: Option<std::thread::JoinHandle<()>>,
    }
    impl Fixture {
        pub fn create() -> Self {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let join = std::thread::spawn(move || {
                let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
                let title: Vec<u16> = "Native Rust visual fixture سلام\0".encode_utf16().collect();
                let window = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        title.as_ptr(),
                        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                        30,
                        30,
                        320,
                        200,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    )
                };
                if window.is_null() {
                    return;
                }
                sender
                    .send((window as isize, unsafe { GetCurrentThreadId() }))
                    .unwrap();
                let mut message: MSG = unsafe { std::mem::zeroed() };
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                unsafe { DestroyWindow(window) };
            });
            let (handle, thread_id) = receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("Windows runner could not create the native fixture window");
            Self {
                handle,
                thread_id,
                join: Some(join),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}
