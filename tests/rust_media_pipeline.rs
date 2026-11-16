//! Media transactions against isolated roots, real codecs, and exact TL calls.
#[path = "local_support/mod.rs"]
mod local_support;
#[path = "common/media.rs"]
mod support;
mod rust_media_pipeline {
    mod downloads;
    mod photos;
    mod previews;
    mod uploads;
}
