//! Serving rasterised pages to the webview.
//!
//! Page bitmaps are far too large for the command channel: one page at retina
//! scale is around thirteen megabytes of RGBA. They travel over a custom URI
//! scheme instead, as raw samples with the dimensions in headers, which avoids
//! a PNG encode in Rust and a decode in the webview — both of which would cost
//! more than drawing the page did.

use std::path::PathBuf;

use tauri::{
    Manager, Runtime, UriSchemeContext, UriSchemeResponder,
    http::{Request, Response, StatusCode},
};

use crate::{
    AppState,
    error::{AppError, AppResult},
};

pub const SCHEME: &str = "press";

pub fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let response = match serve(&app, &request).await {
            Ok(response) => response,
            Err(error) => {
                let status = match error {
                    AppError::NotFound(_) => StatusCode::NOT_FOUND,
                    AppError::InvalidInput(_) => StatusCode::BAD_REQUEST,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                Response::builder()
                    .status(status)
                    .header("Content-Type", "text/plain; charset=utf-8")
                    // Also needed on the error path, or the webview reports a
                    // CORS violation instead of the actual problem.
                    .header("Access-Control-Allow-Origin", "*")
                    .body(error.to_string().into_bytes())
                    .unwrap_or_else(|_| Response::new(Vec::new()))
            }
        };
        responder.respond(response);
    });
}

async fn serve<R: Runtime>(
    app: &tauri::AppHandle<R>,
    request: &Request<Vec<u8>>,
) -> AppResult<Response<Vec<u8>>> {
    let uri = request.uri();
    let segments = uri
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    match segments.as_slice() {
        // /page/{artifact}/{revision}/{index}?scale=2.6
        // The revision is in the path so a rebuilt document cannot be served
        // from a stale cache anywhere along the way.
        ["page", artifact, _revision, index] => {
            let artifact_id = artifact
                .parse::<i64>()
                .map_err(|_| AppError::InvalidInput("malformed artifact id".into()))?;
            let page = index
                .parse::<usize>()
                .map_err(|_| AppError::InvalidInput("malformed page number".into()))?;
            let scale = query_value(uri.query(), "scale")
                .and_then(|value| value.parse::<f32>().ok())
                .filter(|scale| scale.is_finite() && *scale > 0.0 && *scale <= 12.0)
                .ok_or_else(|| AppError::InvalidInput("missing or unusable scale".into()))?;
            render(app, artifact_id, page, scale).await
        }
        _ => Err(AppError::NotFound(format!("no route for {}", uri.path()))),
    }
}

async fn render<R: Runtime>(
    app: &tauri::AppHandle<R>,
    artifact_id: i64,
    page: usize,
    scale: f32,
) -> AppResult<Response<Vec<u8>>> {
    let path = resolve(app, artifact_id).await?;
    let state = app.state::<AppState>();
    let rendered = state.renderer.render(path, page, scale).await?;

    ok(frame(&rendered))
}

/// Prefixes the samples with their dimensions as two little-endian `u32`s.
///
/// The obvious place for these is response headers, but the webview reads this
/// cross-origin, and custom headers are invisible to a cross-origin reader
/// unless the server also lists them in `Access-Control-Expose-Headers`. Putting
/// them in the body removes that dependency entirely: if the bytes arrive, the
/// dimensions arrived with them.
fn frame(rendered: &crate::render::RenderedPage) -> Vec<u8> {
    let mut body = Vec::with_capacity(PAGE_HEADER_BYTES + rendered.samples.len());
    body.extend_from_slice(&rendered.width.to_le_bytes());
    body.extend_from_slice(&rendered.height.to_le_bytes());
    body.extend_from_slice(&rendered.samples);
    body
}

pub const PAGE_HEADER_BYTES: usize = 8;

fn ok(body: Vec<u8>) -> AppResult<Response<Vec<u8>>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        // The page is fetched from a different origin than the document: the
        // dev server is http://localhost:5173 and the bundled app is
        // tauri://localhost, while this is press://localhost. Without this the
        // fetch is refused before it reaches any of the code above.
        .header("Access-Control-Allow-Origin", "*")
        // The viewer keeps its own drawn pages; a second copy in the webview's
        // cache would only double the memory.
        .header("Cache-Control", "no-store")
        .body(body)
        .map_err(|error| AppError::Task(format!("could not build the page response: {error}")))
}

/// Looks up an artifact's file and refuses anything outside Press-managed
/// storage, the same check the command path makes.
pub async fn resolve<R: Runtime>(app: &tauri::AppHandle<R>, artifact_id: i64) -> AppResult<PathBuf> {
    let state = app.state::<AppState>();
    let repository = std::sync::Arc::clone(&state.repository);
    let stored = tauri::async_runtime::spawn_blocking(move || repository.artifact(artifact_id))
        .await
        .map_err(|error| AppError::Task(error.to_string()))??;

    let canonical = tokio::fs::canonicalize(&stored.pdf_path).await?;
    let artifact_root = tokio::fs::canonicalize(&state.artifact_root).await?;
    if !canonical.starts_with(&artifact_root) {
        return Err(AppError::InvalidInput(
            "that PDF is outside Press-managed storage".into(),
        ));
    }
    Ok(canonical)
}

fn query_value<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_query_parameters() {
        assert_eq!(query_value(Some("scale=2.6"), "scale"), Some("2.6"));
        assert_eq!(query_value(Some("a=1&scale=3&b=2"), "scale"), Some("3"));
        assert_eq!(query_value(Some("a=1"), "scale"), None);
        assert_eq!(query_value(None, "scale"), None);
        assert_eq!(query_value(Some("broken"), "scale"), None);
    }
}
