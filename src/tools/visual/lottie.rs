//! Small cross-platform wrapper of rlottie's published stable C ABI.
//! The optional renderer lives in the killable media child, never an MCP thread.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::ffi::{CString, c_char, c_void};
use std::io::Read;
use std::sync::Mutex;

const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
static RENDERER_LOCK: Mutex<()> = Mutex::new(());
type Create = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type GetSize = unsafe extern "C" fn(*const c_void, *mut usize, *mut usize);
type GetFrames = unsafe extern "C" fn(*const c_void) -> usize;
type Render = unsafe extern "C" fn(*mut c_void, usize, *mut u32, usize, usize, usize);
type Lifecycle = unsafe extern "C" fn();

struct Renderer {
    _library: libloading::Library,
    create: Create,
    destroy: Destroy,
    size: GetSize,
    frames: GetFrames,
    render: Render,
    shutdown: Option<Lifecycle>,
    animation: *mut c_void,
}

impl Renderer {
    fn load() -> Result<Self> {
        let supplied = std::env::var_os("TELEGRAM_RLOTTIE_LIBRARY")
            .or_else(|| std::env::var_os("RLOTTIE_LIBRARY"));
        let names: Vec<std::ffi::OsString> = match supplied {
            Some(path) => vec![path],
            None => {
                if cfg!(windows) {
                    vec!["rlottie.dll".into()]
                } else if cfg!(target_os = "macos") {
                    vec!["librlottie.dylib".into()]
                } else {
                    vec!["librlottie.so".into(), "librlottie.so.0".into()]
                }
            }
        };
        let mut loaded = None;
        for name in names {
            // Library paths come only from owner configuration or the system
            // loader, never from a tool argument or an animation's metadata.
            if let Ok(library) = unsafe { libloading::Library::new(name) } {
                loaded = Some(library);
                break;
            }
        }
        let library=loaded.context("The optional rlottie renderer is unavailable; set TELEGRAM_RLOTTIE_LIBRARY to its library path, use get_media_thumbnail, or capture get_telegram_frames")?;
        // Symbols use the signatures in Samsung/rlottie inc/rlottie_capi.h.
        // The Library stays alive until all animation calls and cleanup finish.
        let (create, destroy, size, frames, render, init, shutdown) = unsafe {
            (
                *library.get::<Create>(b"lottie_animation_from_data\0")?,
                *library.get::<Destroy>(b"lottie_animation_destroy\0")?,
                *library.get::<GetSize>(b"lottie_animation_get_size\0")?,
                *library.get::<GetFrames>(b"lottie_animation_get_totalframe\0")?,
                *library.get::<Render>(b"lottie_animation_render\0")?,
                library.get::<Lifecycle>(b"lottie_init\0").ok().map(|s| *s),
                library
                    .get::<Lifecycle>(b"lottie_shutdown\0")
                    .ok()
                    .map(|s| *s),
            )
        };
        if let Some(init) = init {
            unsafe { init() };
        }
        Ok(Self {
            _library: library,
            create,
            destroy,
            size,
            frames,
            render,
            shutdown,
            animation: std::ptr::null_mut(),
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if !self.animation.is_null() {
            unsafe { (self.destroy)(self.animation) };
        }
        if let Some(shutdown) = self.shutdown {
            unsafe { shutdown() };
        }
    }
}

pub fn available() -> bool {
    let Ok(_lock) = RENDERER_LOCK.lock() else {
        return false;
    };
    Renderer::load().is_ok()
}

pub fn parse_tgs(data: &[u8]) -> Result<Value> {
    let decoder = flate2::read::GzDecoder::new(data);
    let mut bytes = Vec::new();
    decoder
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Could not decompress this TGS animation")?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        bail!("TGS expands beyond the 16 MiB JSON limit");
    }
    let value: Value =
        serde_json::from_slice(&bytes).context("TGS does not contain valid Lottie JSON")?;
    let width = value["w"].as_u64().context("TGS width is invalid")?;
    let height = value["h"].as_u64().context("TGS height is invalid")?;
    if width == 0
        || height == 0
        || width
            .checked_mul(height)
            .is_none_or(|pixels| pixels > super::images::MAX_DECODED_PIXELS)
    {
        bail!("TGS canvas exceeds the decoded-pixel limit");
    }
    let start = value["ip"].as_f64().context("TGS start frame is invalid")?;
    let end = value["op"].as_f64().context("TGS end frame is invalid")?;
    let rate = value["fr"].as_f64().context("TGS frame rate is invalid")?;
    if !start.is_finite()
        || !end.is_finite()
        || !rate.is_finite()
        || rate <= 0.0
        || rate > 240.0
        || end <= start
        || end - start > super::frames::MAX_ANIMATION_FRAMES as f64
    {
        bail!("TGS frame count or rate exceeds the animation bounds");
    }
    for asset in value["assets"].as_array().into_iter().flatten() {
        if asset
            .get("p")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
            || asset
                .get("u")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        {
            bail!("TGS may not load external image resources");
        }
    }
    if value["fonts"]["list"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|font| font["fPath"].as_str().is_some_and(|path| !path.is_empty()))
    {
        bail!("TGS may not load external font resources");
    }
    Ok(value)
}

pub fn render_tgs(data: &[u8], count: usize, max_dimension: i64) -> Result<Vec<(Vec<u8>, Value)>> {
    let value = parse_tgs(data)?;
    let _lock = RENDERER_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Lottie renderer lock is unavailable"))?;
    let mut renderer = Renderer::load()?;
    let data = CString::new(serde_json::to_string(&value)?)?;
    let key = CString::new(format!(
        "telegram-mcp-{}",
        crate::lowercase_hex(&Sha256::digest(data.as_bytes()))
    ))?;
    let path = CString::new("")?;
    renderer.animation = unsafe { (renderer.create)(data.as_ptr(), key.as_ptr(), path.as_ptr()) };
    if renderer.animation.is_null() {
        bail!("The native renderer could not open this TGS animation");
    }
    let (mut width, mut height) = (0, 0);
    unsafe { (renderer.size)(renderer.animation, &mut width, &mut height) };
    let total = unsafe { (renderer.frames)(renderer.animation) };
    if width == 0
        || height == 0
        || width
            .checked_mul(height)
            .is_none_or(|pixels| pixels > super::images::MAX_DECODED_PIXELS as usize)
        || total == 0
        || total > super::frames::MAX_ANIMATION_FRAMES
    {
        bail!("The native renderer reported out-of-bounds animation dimensions");
    }
    let longest = width.max(height);
    let side = super::images::bounded_dimension(max_dimension) as usize;
    if longest > side {
        width = ((width as f64 * side as f64 / longest as f64).round() as usize).max(1);
        height = ((height as f64 * side as f64 / longest as f64).round() as usize).max(1);
    }
    let wanted = count.clamp(1, super::frames::MAX_FRAMES).min(total);
    let indexes: Vec<usize> = if wanted == 1 {
        vec![0]
    } else {
        (0..wanted)
            .map(|index| index * (total - 1) / (wanted - 1))
            .collect()
    };
    let mut output = Vec::with_capacity(wanted);
    let mut size = 0;
    for index in indexes {
        let mut pixels = vec![0_u32; width * height];
        unsafe {
            (renderer.render)(
                renderer.animation,
                index,
                pixels.as_mut_ptr(),
                width,
                height,
                width * 4,
            )
        };
        let mut rgba = Vec::with_capacity(width * height * 4);
        let mut transparent = true;
        for pixel in pixels {
            let alpha = (pixel >> 24) as u8;
            transparent &= alpha == 0;
            let unpremultiply = |channel: u32| -> u8 {
                if alpha == 0 {
                    0
                } else {
                    ((channel * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
                }
            };
            rgba.extend_from_slice(&[
                unpremultiply((pixel >> 16) & 255),
                unpremultiply((pixel >> 8) & 255),
                unpremultiply(pixel & 255),
                alpha,
            ]);
        }
        let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
            .context("Rendered animation dimensions are invalid")?;
        let (data, mut meta) = super::images::encode_image(
            &image::DynamicImage::ImageRgba8(image),
            "png",
            max_dimension,
            85,
            false,
        )?;
        size += data.len();
        if size > 32 * 1024 * 1024 {
            bail!("Rendered TGS frames exceed the aggregate response limit");
        }
        meta["source"] = "rlottie".into();
        meta["frame_index"] = index.into();
        meta["frame_count"] = total.into();
        meta["transparent"] = transparent.into();
        meta["frame_rate"] = value["fr"].clone();
        output.push((data, meta));
    }
    Ok(output)
}
