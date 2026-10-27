//! HLS (`.m3u8`) streams: fetch every media piece in parallel, decrypt
//! AES-128 if needed, and join them into one video file.

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;

use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

use super::*;

#[derive(Clone, Serialize)]
pub struct Variant {
    pub url: String,
    pub label: String,
    pub bandwidth: u64,
}

pub(super) struct Piece {
    pub(super) url: String,
    pub(super) key: Option<KeyRef>,
    pub(super) seq: u64,
}

#[derive(Clone)]
pub(super) struct KeyRef {
    url: String,
    iv: Option<[u8; 16]>,
}

struct MediaPlaylist {
    init: Option<String>,
    pieces: Vec<Piece>,
    ended: bool,
}

pub(super) fn looks_like(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.to_ascii_lowercase().ends_with(".m3u8")
}

/// Media players need a container that macOS can open; ffmpeg turns `.ts` into `.mp4`.
pub(super) fn ffmpeg() -> Option<PathBuf> {
    // GUI apps on macOS get a minimal PATH, so check the usual install spots first.
    let known = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"];
    known
        .iter()
        .map(PathBuf::from)
        .chain(
            std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).collect::<Vec<_>>())
                .unwrap_or_default(),
        )
        .find(|p| p.is_file())
}

pub(super) fn resolve(base: &str, uri: &str) -> Result<String, String> {
    reqwest::Url::parse(base)
        .and_then(|b| b.join(uri.trim()))
        .map(|u| u.to_string())
        .map_err(|e| format!("bad playlist link: {e}"))
}

/// Reads `KEY=value` pairs from a tag like `#EXT-X-KEY:METHOD=AES-128,URI="k.bin"`.
fn attrs(line: &str) -> HashMap<String, String> {
    let list = line.split_once(':').map_or("", |(_, rest)| rest);
    let mut out = HashMap::new();
    let mut key = String::new();
    let mut value = String::new();
    let (mut in_value, mut quoted) = (false, false);
    for c in list.chars().chain(std::iter::once(',')) {
        match c {
            '"' if in_value => quoted = !quoted,
            '=' if !in_value => in_value = true,
            ',' if !quoted => {
                if !key.is_empty() {
                    out.insert(key.trim().to_ascii_uppercase(), std::mem::take(&mut value));
                }
                key.clear();
                in_value = false;
            }
            c if in_value => value.push(c),
            c => key.push(c),
        }
    }
    out
}

fn parse_master(base: &str, text: &str) -> Result<Vec<Variant>, String> {
    let mut variants: Vec<Variant> = Vec::new();
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if !line.starts_with("#EXT-X-STREAM-INF") {
            continue;
        }
        let a = attrs(line);
        let Some(uri) = lines.by_ref().find(|l| !l.is_empty() && !l.starts_with('#')) else {
            break;
        };
        let bandwidth = a.get("BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0);
        let height = a
            .get("RESOLUTION")
            .and_then(|r| r.split('x').nth(1))
            .and_then(|h| h.parse::<u32>().ok());
        let label = match height {
            Some(h) => format!("{h}p"),
            None => format!("{} kbps", bandwidth / 1000),
        };
        let url = resolve(base, uri)?;
        if !variants.iter().any(|v| v.url == url) {
            variants.push(Variant { url, label, bandwidth });
        }
    }
    variants.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
    Ok(variants)
}

fn parse_media(base: &str, text: &str) -> Result<MediaPlaylist, String> {
    let mut playlist = MediaPlaylist {
        init: None,
        pieces: Vec::new(),
        ended: false,
    };
    let mut seq = 0u64;
    let mut key: Option<KeyRef> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = rest.trim().parse().unwrap_or(0);
        } else if line.starts_with("#EXT-X-KEY") {
            let a = attrs(line);
            key = match a.get("METHOD").map(String::as_str) {
                None | Some("NONE") => None,
                Some("AES-128") => {
                    let uri = a.get("URI").ok_or("encryption key link is missing")?;
                    let iv = a.get("IV").map(|iv| parse_iv(iv)).transpose()?;
                    Some(KeyRef {
                        url: resolve(base, uri)?,
                        iv,
                    })
                }
                Some(_) => return Err("this stream is DRM-protected and can't be downloaded".into()),
            };
        } else if line.starts_with("#EXT-X-MAP") {
            let uri = attrs(line).remove("URI").ok_or("stream header link is missing")?;
            playlist.init = Some(resolve(base, &uri)?);
        } else if line.starts_with("#EXT-X-BYTERANGE") {
            return Err("this stream layout (byte ranges) isn't supported yet".into());
        } else if line.starts_with("#EXT-X-ENDLIST") {
            playlist.ended = true;
        } else if !line.starts_with('#') {
            playlist.pieces.push(Piece {
                url: resolve(base, line)?,
                key: key.clone(),
                seq,
            });
            seq += 1;
        }
    }
    if playlist.pieces.is_empty() {
        return Err("the playlist has no video pieces".into());
    }
    Ok(playlist)
}

fn parse_iv(hex: &str) -> Result<[u8; 16], String> {
    let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
    let value = u128::from_str_radix(hex, 16).map_err(|_| "bad encryption IV")?;
    Ok(value.to_be_bytes())
}

async fn fetch_text(client: &reqwest::Client, url: &str, headers: &[(String, String)]) -> Result<String, String> {
    let resp = with_headers(client.get(url), headers)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    Ok(text)
}

/// All qualities a playlist offers, best first.
pub(super) async fn variants(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Vec<Variant>, String> {
    let text = fetch_text(client, url, headers).await?;
    if text.contains("#EXT-X-STREAM-INF") {
        return parse_master(url, &text);
    }
    let playlist = parse_media(url, &text)?;
    Ok(vec![Variant {
        url: url.to_string(),
        label: format!("Stream ({} parts)", playlist.pieces.len()),
        bandwidth: 0,
    }])
}

/// Turns any playlist link into the media playlist to download (best quality
/// for a master playlist). Returns that link and whether it's fragmented MP4.
pub(super) async fn pick_media(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<(String, bool), String> {
    let mut url = url.to_string();
    let mut text = fetch_text(client, &url, headers).await?;
    if text.contains("#EXT-X-STREAM-INF") {
        let best = parse_master(&url, &text)?
            .into_iter()
            .next()
            .ok_or("the playlist lists no qualities")?;
        url = best.url;
        text = fetch_text(client, &url, headers).await?;
    }
    let playlist = parse_media(&url, &text)?;
    if !playlist.ended {
        return Err("live streams can't be downloaded yet".into());
    }
    Ok((url, playlist.init.is_some()))
}

/// GET with the same never-give-up policy as normal downloads.
/// Returns `None` if the user paused.
async fn get_bytes(
    client: &reqwest::Client,
    d: &Download,
    url: &str,
    token: &CancellationToken,
) -> Result<Option<Vec<u8>>, String> {
    let mut backoff_step = 0u32;
    let mut fatal_tries = 0u32;
    loop {
        let attempt = async {
            let resp = with_headers(client.get(url), &d.headers)
                .send()
                .await
                .map_err(|e| Failure::Network(e.to_string()))?;
            let status = resp.status();
            if !status.is_success() {
                let msg = format!("server returned {status}");
                return Err(match status.as_u16() {
                    408 | 425 | 429 | 500..=599 => Failure::Network(msg),
                    _ => Failure::Fatal(msg),
                });
            }
            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| Failure::Network(e.to_string()))
        };
        let result = tokio::select! {
            r = attempt => r,
            _ = token.cancelled() => return Ok(None),
        };
        match result {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(Failure::Network(e)) => *d.error.lock().unwrap() = Some(e),
            Err(Failure::Fatal(e)) => {
                fatal_tries += 1;
                if fatal_tries >= MAX_FATAL_RETRIES {
                    return Err(e);
                }
            }
        }
        backoff_step += 1;
        let backoff = Duration::from_millis(500 * 2u64.pow(backoff_step.min(5))).min(MAX_BACKOFF);
        d.reconnecting.fetch_add(1, Ordering::SeqCst);
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(backoff) => false,
            _ = token.cancelled() => true,
        };
        d.reconnecting.fetch_sub(1, Ordering::SeqCst);
        if cancelled {
            return Ok(None);
        }
    }
}

pub(super) fn piece_path(work: &Path, index: usize) -> PathBuf {
    work.join(format!("{index:06}"))
}

pub(super) async fn run(m: &Manager, d: &Arc<Download>, token: &CancellationToken) -> Result<(), String> {
    let text = loop {
        match fetch_text(&m.client, &d.url, &d.headers).await {
            Ok(t) => break t,
            Err(e) if e.starts_with("server returned 4") || e.contains("playlist") => return Err(e),
            Err(e) => {
                *d.error.lock().unwrap() = Some(e);
                tokio::select! {
                    _ = tokio::time::sleep(MAX_BACKOFF) => {}
                    _ = token.cancelled() => return Ok(()),
                }
            }
        }
    };
    let playlist = parse_media(&d.url, &text)?;

    // Piece 0 is the fMP4 header when there is one; it's never encrypted.
    let mut pieces: Vec<Piece> = Vec::new();
    if let Some(init) = &playlist.init {
        pieces.push(Piece {
            url: init.clone(),
            key: None,
            seq: 0,
        });
    }
    let fmp4 = playlist.init.is_some();
    pieces.extend(playlist.pieces);
    let count = pieces.len();

    let work = d.part_path();
    if !fetch_pieces(m, d, token, pieces, &work).await? {
        return Ok(());
    }

    // Join the pieces in order.
    let merged = work.join(if fmp4 { "merged.mp4" } else { "merged.ts" });
    {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(File::create(&merged).map_err(|e| e.to_string())?);
        for i in 0..count {
            let data = std::fs::read(piece_path(&work, i))
                .map_err(|_| "a downloaded piece went missing; resume to fetch it again".to_string())?;
            out.write_all(&data).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
    }

    let target = d.final_path();
    let wants_mp4 = target.extension().is_some_and(|e| e == "mp4");
    if !fmp4 && wants_mp4 {
        let ffmpeg = ffmpeg().ok_or("ffmpeg is needed to make an .mp4 from this stream")?;
        let status = tokio::process::Command::new(ffmpeg)
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&merged)
            .args(["-c", "copy", "-bsf:a", "aac_adtstoasc"])
            .arg(&target)
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if !status.success() {
            // Don't leave a broken video behind; the pieces stay for a retry.
            let _ = std::fs::remove_file(&target);
            return Err("ffmpeg couldn't convert the video".into());
        }
    } else {
        std::fs::rename(&merged, &target).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

/// Downloads every piece into `work` in parallel, skipping ones finished earlier.
/// Returns `false` if the user paused before it finished.
pub(super) async fn fetch_pieces(
    m: &Manager,
    d: &Arc<Download>,
    token: &CancellationToken,
    pieces: Vec<Piece>,
    work: &Path,
) -> Result<bool, String> {
    let pieces = Arc::new(pieces);
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;

    // Count what earlier sessions already finished.
    let (mut done, mut bytes) = (0u64, 0u64);
    for i in 0..pieces.len() {
        if let Ok(meta) = std::fs::metadata(piece_path(&work, i)) {
            done += 1;
            bytes += meta.len();
        }
    }
    d.parts_total.store(pieces.len() as u64, Ordering::SeqCst);
    d.parts_done.store(done, Ordering::SeqCst);
    d.hls_bytes.store(bytes, Ordering::SeqCst);

    let next = Arc::new(AtomicUsize::new(0));
    let keys: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
    let workers = m.settings.lock().unwrap().connections.clamp(1, 16);
    let mut handles = Vec::new();
    for _ in 0..workers {
        let (d, token, pieces, next, keys, work) =
            (d.clone(), token.clone(), pieces.clone(), next.clone(), keys.clone(), work.to_path_buf());
        let (client, limiter) = (m.client.clone(), m.limiter.clone());
        handles.push(tauri::async_runtime::spawn(async move {
            loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(piece) = pieces.get(i) else {
                    return Ok(());
                };
                let path = piece_path(&work, i);
                if path.exists() {
                    continue;
                }
                let Some(mut data) = get_bytes(&client, &d, &piece.url, &token).await? else {
                    return Ok(());
                };
                if let Some(key) = &piece.key {
                    let cached = keys.lock().unwrap().get(&key.url).cloned();
                    let key_bytes = match cached {
                        Some(k) => k,
                        None => {
                            let Some(k) = get_bytes(&client, &d, &key.url, &token).await? else {
                                return Ok(());
                            };
                            keys.lock().unwrap().insert(key.url.clone(), k.clone());
                            k
                        }
                    };
                    let iv = key.iv.unwrap_or((piece.seq as u128).to_be_bytes());
                    data = cbc::Decryptor::<Aes128>::new_from_slices(&key_bytes, &iv)
                        .map_err(|_| "the stream's encryption key is invalid".to_string())?
                        .decrypt_padded_vec_mut::<Pkcs7>(&data)
                        .map_err(|_| "couldn't decrypt the stream".to_string())?;
                }
                tokio::select! {
                    _ = limiter.consume(data.len() as u64) => {}
                    _ = token.cancelled() => return Ok(()),
                }
                // Write then rename, so a half-written piece never counts as finished.
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
                std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
                d.parts_done.fetch_add(1, Ordering::SeqCst);
                d.hls_bytes.fetch_add(data.len() as u64, Ordering::SeqCst);
                if d.reconnecting.load(Ordering::SeqCst) == 0 {
                    d.error.lock().unwrap().take();
                }
            }
        }));
    }

    let mut first_error = None;
    for h in handles {
        if let Err(e) = h.await.map_err(|e| e.to_string()).and_then(|r: Result<(), String>| r) {
            first_error.get_or_insert(e);
            token.cancel();
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(!token.is_cancelled())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_master_playlist() {
        let text = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n\
            low/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\n\
            https://cdn.example.com/hd/index.m3u8\n";
        let v = parse_master("https://example.com/video/master.m3u8", text).unwrap();
        assert_eq!(v[0].label, "1080p");
        assert_eq!(v[0].url, "https://cdn.example.com/hd/index.m3u8");
        assert_eq!(v[1].label, "360p");
        assert_eq!(v[1].url, "https://example.com/video/low/index.m3u8");
    }

    #[test]
    fn parses_media_playlist_with_key() {
        let text = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x0000000000000000000000000000000A\n\
            #EXTINF:4.0,\nseg7.ts\n#EXTINF:4.0,\nseg8.ts\n#EXT-X-ENDLIST\n";
        let p = parse_media("https://example.com/v/index.m3u8", text).unwrap();
        assert!(p.ended);
        assert_eq!(p.pieces.len(), 2);
        assert_eq!(p.pieces[1].seq, 8);
        assert_eq!(p.pieces[0].url, "https://example.com/v/seg7.ts");
        let key = p.pieces[0].key.as_ref().unwrap();
        assert_eq!(key.url, "https://example.com/v/key.bin");
        assert_eq!(key.iv.unwrap()[15], 10);
    }

    #[test]
    fn rejects_drm() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\nseg.ts\n";
        assert!(parse_media("https://example.com/a.m3u8", text).is_err());
    }
}
