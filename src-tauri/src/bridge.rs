//! Tiny HTTP server on 127.0.0.1 that the browser extension talks to.
//!
//! Web pages can't use it: every request must carry the custom
//! `X-ADM` header, which forces a CORS preflight we never approve.
//! Extensions with host permission for 127.0.0.1 skip CORS, so only they get through.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager as _};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::engine::Manager;
use crate::torrent::Torrents;

pub const PORT: u16 = 47321;
const MAX_REQUEST: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddRequest {
    url: String,
    file_name: Option<String>,
    referrer: Option<String>,
    cookies: Option<String>,
    user_agent: Option<String>,
}

pub async fn serve(app: AppHandle, manager: Arc<Manager>, torrents: Arc<Torrents>) {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("browser bridge disabled: {e}");
            return;
        }
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (app, manager, torrents) = (app.clone(), manager.clone(), torrents.clone());
        tauri::async_runtime::spawn(async move {
            let _ = handle(stream, app, manager, torrents).await;
        });
    }
}

async fn handle(
    mut stream: TcpStream,
    app: AppHandle,
    manager: Arc<Manager>,
    torrents: Arc<Torrents>,
) -> std::io::Result<()> {
    let Some((method, path, headers, body)) = read_request(&mut stream).await? else {
        return respond(&mut stream, 400, json!({ "error": "bad request" })).await;
    };
    let has_header = headers.iter().any(|(k, _)| k == "x-adm");

    let (status, reply) = match (method.as_str(), path.as_str()) {
        ("GET", "/ping") if has_header => (
            200,
            json!({
                "app": "ADM",
                "version": env!("CARGO_PKG_VERSION"),
                "enabled": manager.settings().browser_integration,
            }),
        ),
        ("POST", "/add") if has_header => add(&app, &manager, &torrents, &body).await,
        ("POST", "/hls-info") if has_header => hls_info(&manager, &body).await,
        _ => (404, json!({ "error": "not found" })),
    };
    respond(&mut stream, status, reply).await
}

async fn add(
    app: &AppHandle,
    manager: &Arc<Manager>,
    torrents: &Torrents,
    body: &[u8],
) -> (u16, serde_json::Value) {
    if !manager.settings().browser_integration {
        // Tells the extension to let the browser handle the download itself.
        return (503, json!({ "error": "browser integration is turned off" }));
    }
    let req: AddRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return (400, json!({ "error": e.to_string() })),
    };
    if !["http://", "https://", "magnet:"].iter().any(|p| req.url.starts_with(p)) {
        return (400, json!({ "error": "only http(s) and magnet links are supported" }));
    }

    let headers = browser_headers(&req);
    let dir = match crate::download_dir(app, manager, None) {
        Ok(d) => d,
        Err(e) => return (500, json!({ "error": e })),
    };
    match crate::add_any(manager, torrents, req.url.clone(), dir, headers, req.file_name.clone()).await {
        Ok(id) => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            let _ = app.emit("external-add", &id);
            (200, json!({ "id": id }))
        }
        Err(e) => (502, json!({ "error": e })),
    }
}

/// Lists the qualities of an HLS playlist the extension spotted on a page.
async fn hls_info(manager: &Arc<Manager>, body: &[u8]) -> (u16, serde_json::Value) {
    let req: AddRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return (400, json!({ "error": e.to_string() })),
    };
    match manager.hls_variants(&req.url, &browser_headers(&req)).await {
        Ok(variants) => (200, json!({ "variants": variants })),
        Err(e) => (502, json!({ "error": e })),
    }
}

/// The browser's cookies, referrer and user agent, so protected links still work.
fn browser_headers(req: &AddRequest) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, value) in [
        ("Referer", &req.referrer),
        ("Cookie", &req.cookies),
        ("User-Agent", &req.user_agent),
    ] {
        if let Some(v) = value.as_ref().filter(|v| !v.is_empty() && !v.contains(['\r', '\n'])) {
            headers.push((name.to_string(), v.clone()));
        }
    }
    headers
}

type Request = (String, String, Vec<(String, String)>, Vec<u8>);

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_REQUEST {
            return Ok(None);
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();

    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_REQUEST {
        return Ok(None);
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Some((method, path, headers, body)))
}

async fn respond(stream: &mut TcpStream, status: u16, body: serde_json::Value) -> std::io::Result<()> {
    let body = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}
