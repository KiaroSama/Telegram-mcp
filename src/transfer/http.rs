use super::*;
use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, Request, State as AxumState},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// This router owns its registry; only these two opaque capability paths bypass
/// the service's ordinary owner-token authentication.
pub fn router(registry: Arc<Transfers>) -> Router {
    Router::new()
        .route("/upload/{token}", axum::routing::put(upload))
        .route("/media/{token}", axum::routing::get(download))
        .with_state(registry)
}
pub async fn upload(
    AxumState(registry): AxumState<Arc<Transfers>>,
    AxumPath(token): AxumPath<String>,
    request: Request,
) -> Response {
    match upload_inner(&registry, &token, request).await {
        Ok(value) => (StatusCode::CREATED, axum::Json(value)).into_response(),
        Err(status) => status.into_response(),
    }
}
async fn upload_inner(
    registry: &Transfers,
    token: &str,
    request: Request,
) -> std::result::Result<Value, StatusCode> {
    let Ticket::Upload { path, max_bytes } = registry
        .take_upload(token)
        .map_err(|_| StatusCode::NOT_FOUND)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };
    let current = registry
        .state
        .resolve_file(&path, FileAccess::Write)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if current != path {
        return Err(StatusCode::FORBIDDEN);
    }
    if path.exists() {
        return Err(StatusCode::CONFLICT);
    }
    if let Some(length) = request.headers().get(header::CONTENT_LENGTH) {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if length > max_bytes {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }
    let temporary =
        files::Upload::new(&registry.state, &path).map_err(|_| StatusCode::FORBIDDEN)?;
    let mut output = tokio::fs::File::from_std(
        temporary
            .writer()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    let mut body = request.into_body().into_data_stream();
    let mut size = 0u64;
    let transfer = async {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| StatusCode::BAD_REQUEST)?;
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or(StatusCode::PAYLOAD_TOO_LARGE)?;
            if size > max_bytes {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            output
                .write_all(&chunk)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        output
            .flush()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        output
            .sync_all()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok::<(), StatusCode>(())
    };
    tokio::time::timeout(Duration::from_secs(3600), transfer)
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)??;
    drop(output);
    temporary
        .publish(&registry.state)
        .map_err(|_| StatusCode::CONFLICT)?;
    Ok(json!({"file_path":path,"size":size,"uploaded":true}))
}
pub async fn download(
    AxumState(registry): AxumState<Arc<Transfers>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    match download_inner(&registry, &token).await {
        Ok(response) => response,
        Err(status) => status.into_response(),
    }
}
async fn download_inner(
    registry: &Transfers,
    token: &str,
) -> std::result::Result<Response, StatusCode> {
    let Ticket::Download {
        path,
        length,
        modified,
        identity,
    } = registry
        .find_download(token)
        .map_err(|_| StatusCode::NOT_FOUND)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };
    let current = registry
        .state
        .resolve_file(&path, FileAccess::Read)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if current != path {
        return Err(StatusCode::FORBIDDEN);
    }
    let file = files::open_download(&registry.state, &path).map_err(|_| StatusCode::FORBIDDEN)?;
    let meta = file.metadata().map_err(|_| StatusCode::NOT_FOUND)?;
    if !meta.is_file()
        || meta.len() != length
        || meta.modified().ok() != Some(modified)
        || files::identity(&file).map_err(|_| StatusCode::CONFLICT)? != identity
    {
        return Err(StatusCode::CONFLICT);
    }
    let file = tokio::fs::File::from_std(file);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    let encoded = filename
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect::<String>();
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, length)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename*=UTF-8''{encoded}"),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(
            file.take(length),
        )))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
