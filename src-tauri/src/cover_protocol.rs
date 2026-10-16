//! `cover://` protocol: `http://cover.localhost/{key}/{size}` serves a pre-cut JPEG
//! thumbnail. Responses are immutable, so the webview caches them for good.

use crate::state::AppState;
use mixtape_artwork::CoverStore;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let store = ctx.app_handle().state::<AppState>().covers.clone();
    let path = request.uri().path().to_owned();
    std::thread::spawn(move || responder.respond(serve(&store, &path)));
}

fn serve(store: &CoverStore, path: &str) -> Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(key), Some(size)) = (parts.next(), parts.next()) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok(size) = size.parse::<u32>() else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok(file) = store.path(key, size) else {
        return status(StatusCode::BAD_REQUEST);
    };
    match std::fs::read(&file) {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .expect("static response parts are valid"),
        Err(_) => status(StatusCode::NOT_FOUND),
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).expect("static response parts are valid")
}
