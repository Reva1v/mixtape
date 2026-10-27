//! `cover://` protocol: `http://cover.localhost/{key}/{size}` serves a pre-cut JPEG
//! thumbnail. Responses are immutable, so the webview caches them for good — except a
//! smaller stand-in for a size not stored (a browsed item's cover asked for at 1024), which
//! is revalidated so the full size shows once the item is in the library.

use crate::state::Session;
use mixtape_artwork::CoverStore;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let store = ctx.app_handle().state::<std::sync::Arc<Session>>().covers.clone();
    let path = request.uri().path().to_owned();
    // A pooled blocking thread instead of a new OS thread per image.
    tauri::async_runtime::spawn_blocking(move || responder.respond(serve(&store, &path)));
}

fn serve(store: &CoverStore, path: &str) -> Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(key), Some(size)) = (parts.next(), parts.next()) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok(size) = size.parse::<u32>() else {
        return status(StatusCode::BAD_REQUEST);
    };
    if !CoverStore::valid_key(key) {
        return status(StatusCode::BAD_REQUEST);
    }
    let Some((file, stand_in)) = store.best_path(key, size) else {
        return status(StatusCode::NOT_FOUND);
    };
    match std::fs::read(&file) {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, if stand_in { "no-cache" } else { "public, max-age=31536000, immutable" })
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .expect("static response parts are valid"),
        Err(_) => status(StatusCode::NOT_FOUND),
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).expect("static response parts are valid")
}
