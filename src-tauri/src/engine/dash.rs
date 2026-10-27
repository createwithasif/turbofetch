//! MPEG-DASH (`.mpd`) streams: read the manifest, fetch the chosen video and
//! the best audio piece by piece, then let ffmpeg put them in one `.mp4`.
//!
//! The chosen quality travels in the link itself (`…/manifest.mpd#adm-video=ID`),
//! so a saved download resumes with the same quality.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::hls::{fetch_pieces, piece_path, resolve, Piece, Variant};
use super::*;

const PICK: &str = "#adm-video=";
/// Marks an audio-only DASH link, which has no quality to pin.
const AUDIO_ONLY: &str = "#adm-dash";

/// A `.mpd` link, or one we already confirmed and marked as DASH.
pub(super) fn looks_like(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.to_ascii_lowercase().ends_with(".mpd") || url.contains(PICK) || url.ends_with(AUDIO_ONLY)
}

/// Manifests served without a `.mpd` name are recognised by their type.
pub(super) fn is_dash_type(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("dash+xml")
}

/// The manifest link without our quality marker.
fn manifest_url(url: &str) -> &str {
    let url = url.strip_suffix(AUDIO_ONLY).unwrap_or(url);
    url.split_once(PICK).map_or(url, |(base, _)| base)
}

/* ---- A tiny XML tree, enough for a manifest ---- */

#[derive(Default)]
struct Node {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn local(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    name.rsplit(':').next().unwrap_or_default().to_string()
}

fn element(e: &BytesStart) -> Node {
    let mut node = Node {
        name: local(e.name().as_ref()),
        ..Default::default()
    };
    for a in e.attributes().flatten() {
        node.attrs
            .insert(local(a.key.as_ref()), unescape(&String::from_utf8_lossy(&a.value)));
    }
    node
}

fn parse_xml(text: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(text);
    let mut stack = vec![Node::default()];
    loop {
        match reader.read_event().map_err(|e| format!("bad DASH manifest: {e}"))? {
            Event::Start(e) => stack.push(element(&e)),
            Event::Empty(e) => {
                let node = element(&e);
                stack.last_mut().unwrap().children.push(node);
            }
            Event::End(_) => {
                let node = stack.pop().unwrap();
                stack.last_mut().ok_or("bad DASH manifest")?.children.push(node);
            }
            Event::Text(t) => {
                let text = unescape(&String::from_utf8_lossy(&t));
                stack.last_mut().unwrap().text.push_str(text.trim());
            }
            Event::CData(t) => stack.last_mut().unwrap().text.push_str(String::from_utf8_lossy(&t).trim()),
            // `&amp;` and friends inside text arrive as their own event.
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().ok().flatten() {
                    Some(c) => c.to_string(),
                    None => unescape(&format!("&{};", String::from_utf8_lossy(&r))),
                };
                stack.last_mut().unwrap().text.push_str(&c);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    stack
        .pop()
        .and_then(|root| root.children.into_iter().find(|c| c.name == "MPD"))
        .ok_or_else(|| "not a DASH manifest".into())
}

/* ---- Manifest model ---- */

/// One video or audio quality and where its pieces live.
struct Rep {
    id: String,
    video: bool,
    bandwidth: u64,
    height: Option<u32>,
    protected: bool,
    init: Option<String>,
    segments: Vec<String>,
}

/// `PT1H2M3.5S` → seconds.
fn duration(s: &str) -> Option<f64> {
    let s = s.strip_prefix('P')?;
    let (days, time) = s.split_once('T').unwrap_or((s, ""));
    let mut total = 0.0;
    if let Some(d) = days.strip_suffix('D') {
        total += d.parse::<f64>().ok()? * 86400.0;
    }
    let mut num = String::new();
    for c in time.chars() {
        match c {
            'H' => total += num.parse::<f64>().ok()? * 3600.0,
            'M' => total += num.parse::<f64>().ok()? * 60.0,
            'S' => total += num.parse::<f64>().ok()?,
            _ => {
                num.push(c);
                continue;
            }
        }
        num.clear();
    }
    Some(total)
}

/// Fills `$RepresentationID$`, `$Number%05d$`, `$Time$` and friends.
fn fill(template: &str, id: &str, bandwidth: u64, number: u64, time: u64) -> String {
    let mut out = String::new();
    let mut parts = template.split('$');
    out.push_str(parts.next().unwrap_or_default());
    let mut inside = true;
    for part in parts {
        if !inside {
            out.push_str(part);
            inside = true;
            continue;
        }
        inside = false;
        let (name, format) = part.split_once('%').unwrap_or((part, ""));
        let value = match name {
            "" => "$".to_string(),
            "RepresentationID" => id.to_string(),
            "Bandwidth" => bandwidth.to_string(),
            "Number" => number.to_string(),
            "Time" => time.to_string(),
            _ => format!("${part}$"),
        };
        let width = format
            .trim_start_matches('0')
            .trim_end_matches('d')
            .parse::<usize>()
            .unwrap_or(0);
        out.push_str(&format!("{value:0>width$}"));
    }
    out
}

/// Attributes set on outer levels apply to inner ones unless overridden.
fn merged_template<'a>(levels: &[&'a Node]) -> Option<(HashMap<String, String>, Option<&'a Node>)> {
    let mut attrs = HashMap::new();
    let mut timeline = None;
    let mut found = false;
    for level in levels {
        if let Some(t) = level.child("SegmentTemplate") {
            found = true;
            attrs.extend(t.attrs.clone());
            if let Some(tl) = t.child("SegmentTimeline") {
                timeline = Some(tl);
            }
        }
    }
    found.then_some((attrs, timeline))
}

fn template_segments(
    base: &str,
    rep_id: &str,
    bandwidth: u64,
    attrs: &HashMap<String, String>,
    timeline: Option<&Node>,
    period_secs: Option<f64>,
) -> Result<(Option<String>, Vec<String>), String> {
    let num = |k: &str, default: u64| attrs.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(default);
    let timescale = num("timescale", 1).max(1);
    let mut number = num("startNumber", 1);
    let init = attrs
        .get("initialization")
        .map(|t| resolve(base, &fill(t, rep_id, bandwidth, 0, 0)))
        .transpose()?;
    let media = attrs.get("media").ok_or("the DASH manifest has no segment links")?;
    let end = period_secs.map(|s| (s * timescale as f64) as u64);

    let mut segments = Vec::new();
    if let Some(timeline) = timeline {
        let mut time = 0u64;
        for s in timeline.all("S") {
            let d = s.attr("d").and_then(|v| v.parse::<u64>().ok()).ok_or("bad DASH timeline")?;
            if let Some(t) = s.attr("t").and_then(|v| v.parse().ok()) {
                time = t;
            }
            let repeat: i64 = s.attr("r").and_then(|v| v.parse().ok()).unwrap_or(0);
            let count = if repeat < 0 {
                // Repeat until the end of the period.
                let end = end.ok_or("the DASH manifest doesn't say how long the video is")?;
                end.saturating_sub(time).div_ceil(d.max(1))
            } else {
                repeat as u64 + 1
            };
            for _ in 0..count {
                segments.push(resolve(base, &fill(media, rep_id, bandwidth, number, time))?);
                time += d;
                number += 1;
            }
        }
    } else {
        let d = num("duration", 0);
        let end = end.ok_or("the DASH manifest doesn't say how long the video is")?;
        if d == 0 {
            return Err("the DASH manifest has no segment length".into());
        }
        for i in 0..end.div_ceil(d) {
            segments.push(resolve(base, &fill(media, rep_id, bandwidth, number, i * d))?);
            number += 1;
        }
    }
    Ok((init, segments))
}

fn list_segments(base: &str, list: &Node) -> Result<(Option<String>, Vec<String>), String> {
    if list.all("SegmentURL").any(|s| s.attr("mediaRange").is_some()) {
        return Err("this stream layout (byte ranges) isn't supported yet".into());
    }
    let init = list
        .child("Initialization")
        .and_then(|i| i.attr("sourceURL"))
        .map(|u| resolve(base, u))
        .transpose()?;
    let segments = list
        .all("SegmentURL")
        .filter_map(|s| s.attr("media"))
        .map(|u| resolve(base, u))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((init, segments))
}

fn with_base(base: &str, node: &Node) -> Result<String, String> {
    match node.child("BaseURL").filter(|b| !b.text.is_empty()) {
        Some(b) => resolve(base, &b.text),
        None => Ok(base.to_string()),
    }
}

fn parse_manifest(url: &str, text: &str) -> Result<Vec<Rep>, String> {
    let mpd = parse_xml(text)?;
    if mpd.attr("type") == Some("dynamic") {
        return Err("live streams can't be downloaded yet".into());
    }
    let periods: Vec<&Node> = mpd.all("Period").collect();
    let [period] = periods.as_slice() else {
        return Err("DASH videos made of several parts aren't supported yet".into());
    };
    let period_secs = period
        .attr("duration")
        .or(mpd.attr("mediaPresentationDuration"))
        .and_then(duration);
    let base = with_base(&with_base(url, &mpd)?, period)?;

    let mut reps = Vec::new();
    for set in period.all("AdaptationSet") {
        let set_base = with_base(&base, set)?;
        let set_protected = set.child("ContentProtection").is_some();
        for rep in set.all("Representation") {
            let kind = [rep.attr("contentType"), rep.attr("mimeType"), set.attr("contentType"), set.attr("mimeType")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let video = kind.contains("video");
            if !video && !kind.contains("audio") {
                continue; // subtitles, thumbnails
            }
            let id = rep.attr("id").unwrap_or_default().to_string();
            let bandwidth = rep.attr("bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0);
            let rep_base = with_base(&set_base, rep)?;
            let (init, segments) = if let Some(list) = rep.child("SegmentList").or(set.child("SegmentList")) {
                list_segments(&rep_base, list)?
            } else if let Some((attrs, timeline)) = merged_template(&[period, set, rep]) {
                template_segments(&rep_base, &id, bandwidth, &attrs, timeline, period_secs)?
            } else {
                // A plain file per quality (SegmentBase or nothing at all).
                (None, vec![rep_base])
            };
            reps.push(Rep {
                id,
                video,
                bandwidth,
                height: rep.attr("height").or(set.attr("height")).and_then(|h| h.parse().ok()),
                protected: set_protected || rep.child("ContentProtection").is_some(),
                init,
                segments,
            });
        }
    }
    reps.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
    Ok(reps)
}

/// Video quality to download (or `None` for audio-only) plus the best audio.
fn choose<'a>(reps: &'a [Rep], url: &str) -> Result<(Option<&'a Rep>, Option<&'a Rep>), String> {
    let usable = |r: &&Rep| !r.protected;
    let has_video = reps.iter().any(|r| r.video);
    let video = match url.split_once(PICK) {
        Some((_, id)) => Some(reps.iter().find(|r| r.video && r.id == id).ok_or("that video quality is no longer offered")?),
        None => reps.iter().filter(|r| r.video).find(usable),
    };
    let audio = reps.iter().filter(|r| !r.video).find(usable);
    if video.is_some_and(|v| v.protected) || (has_video && video.is_none()) || (video.is_none() && audio.is_none()) {
        return Err("this stream is DRM-protected and can't be downloaded".into());
    }
    Ok((video, audio))
}

async fn fetch_text(client: &reqwest::Client, url: &str, headers: &[(String, String)]) -> Result<String, String> {
    let resp = with_headers(client.get(manifest_url(url)), headers)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !text.contains("<MPD") {
        return Err("not a DASH manifest".into());
    }
    Ok(text)
}

/// All video qualities, best first; one "Audio" entry for audio-only streams.
pub(super) async fn variants(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Vec<Variant>, String> {
    let url = manifest_url(url);
    let reps = parse_manifest(url, &fetch_text(client, url, headers).await?)?;
    choose(&reps, url)?;
    let mut out: Vec<Variant> = Vec::new();
    for r in reps.iter().filter(|r| r.video && !r.protected) {
        let label = match r.height {
            Some(h) => format!("{h}p"),
            None => format!("{} kbps", r.bandwidth / 1000),
        };
        // The same height at several bitrates: keep only the best.
        if out.iter().any(|v| v.label == label) {
            continue;
        }
        out.push(Variant {
            url: format!("{url}{PICK}{}", r.id),
            label,
            bandwidth: r.bandwidth,
        });
    }
    if out.is_empty() {
        out.push(Variant {
            url: format!("{url}{AUDIO_ONLY}"),
            label: "Audio".into(),
            bandwidth: 0,
        });
    }
    Ok(out)
}

/// Checks the stream can be downloaded and returns the link to store
/// (with the quality pinned).
pub(super) async fn pick(client: &reqwest::Client, url: &str, headers: &[(String, String)]) -> Result<String, String> {
    let reps = parse_manifest(manifest_url(url), &fetch_text(client, url, headers).await?)?;
    let (video, audio) = choose(&reps, url)?;
    if video.is_some() && audio.is_some() && super::hls::ffmpeg().is_none() {
        return Err("ffmpeg is needed to join this video's picture and sound".into());
    }
    Ok(match video {
        Some(v) => format!("{}{PICK}{}", manifest_url(url), v.id),
        None => format!("{}{AUDIO_ONLY}", manifest_url(url)),
    })
}

fn pieces_of(rep: &Rep) -> Vec<Piece> {
    rep.init
        .iter()
        .chain(&rep.segments)
        .map(|url| Piece {
            url: url.clone(),
            key: None,
            seq: 0,
        })
        .collect()
}

fn join(work: &Path, range: std::ops::Range<usize>, out: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(File::create(out).map_err(|e| e.to_string())?);
    for i in range {
        let data = std::fs::read(piece_path(work, i))
            .map_err(|_| "a downloaded piece went missing; resume to fetch it again".to_string())?;
        file.write_all(&data).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())
}

pub(super) async fn run(m: &Manager, d: &Arc<Download>, token: &CancellationToken) -> Result<(), String> {
    let text = loop {
        match fetch_text(&m.client, &d.url, &d.headers).await {
            Ok(t) => break t,
            Err(e) if e.starts_with("server returned 4") || e.contains("manifest") => return Err(e),
            Err(e) => {
                *d.error.lock().unwrap() = Some(e);
                tokio::select! {
                    _ = tokio::time::sleep(MAX_BACKOFF) => {}
                    _ = token.cancelled() => return Ok(()),
                }
            }
        }
    };
    let reps = parse_manifest(manifest_url(&d.url), &text)?;
    let (video, audio) = choose(&reps, &d.url)?;

    // Video pieces first, then audio, all in one numbered folder.
    let mut pieces = video.map(pieces_of).unwrap_or_default();
    let split = pieces.len();
    pieces.extend(audio.map(pieces_of).unwrap_or_default());
    let count = pieces.len();

    let work = d.part_path();
    if !fetch_pieces(m, d, token, pieces, &work).await? {
        return Ok(());
    }

    let target = d.final_path();
    if split > 0 && split < count {
        let (v, a) = (work.join("video.mp4"), work.join("audio.mp4"));
        join(&work, 0..split, &v)?;
        join(&work, split..count, &a)?;
        let ffmpeg = super::hls::ffmpeg().ok_or("ffmpeg is needed to join this video's picture and sound")?;
        let status = tokio::process::Command::new(ffmpeg)
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&v)
            .arg("-i")
            .arg(&a)
            .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy"])
            .arg(&target)
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if !status.success() {
            // Don't leave a broken video behind; the pieces stay for a retry.
            let _ = std::fs::remove_file(&target);
            return Err("ffmpeg couldn't join the video and audio".into());
        }
    } else {
        join(&work, 0..count, &target)?;
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT0M10S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate timescale="1000" duration="4000" startNumber="1"
        initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/seg-$Number%03d$.m4s"/>
      <Representation id="v720" bandwidth="3000000" height="720"/>
      <Representation id="v360" bandwidth="800000" height="360"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="a1" bandwidth="128000">
        <SegmentTemplate timescale="48000" initialization="a/init.mp4" media="a/$Time$.m4s">
          <SegmentTimeline><S t="0" d="96000" r="2"/><S d="48000"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn parses_templates_and_timelines() {
        let reps = parse_manifest("https://example.com/v/manifest.mpd", MANIFEST).unwrap();
        let v = reps.iter().find(|r| r.id == "v720").unwrap();
        assert_eq!(v.init.as_deref(), Some("https://example.com/v/v720/init.mp4"));
        assert_eq!(v.segments.len(), 3);
        assert_eq!(v.segments[2], "https://example.com/v/v720/seg-003.m4s");
        let a = reps.iter().find(|r| r.id == "a1").unwrap();
        assert_eq!(a.segments.len(), 4);
        assert_eq!(a.segments[3], "https://example.com/v/a/288000.m4s");
    }

    #[test]
    fn picks_quality_from_link() {
        let reps = parse_manifest("https://example.com/m.mpd", MANIFEST).unwrap();
        let (v, a) = choose(&reps, "https://example.com/m.mpd").unwrap();
        assert_eq!(v.unwrap().id, "v720");
        assert_eq!(a.unwrap().id, "a1");
        let (v, _) = choose(&reps, "https://example.com/m.mpd#adm-video=v360").unwrap();
        assert_eq!(v.unwrap().id, "v360");
    }

    #[test]
    fn rejects_drm_and_live() {
        let drm = MANIFEST.replace(
            r#"<AdaptationSet mimeType="video/mp4">"#,
            r#"<AdaptationSet mimeType="video/mp4"><ContentProtection schemeIdUri="urn:uuid:edef8ba9"/>"#,
        );
        let reps = parse_manifest("https://example.com/m.mpd", &drm).unwrap();
        assert!(choose(&reps, "https://example.com/m.mpd").is_err());
        let live = MANIFEST.replace("type=\"static\"", "type=\"dynamic\"");
        assert!(parse_manifest("https://example.com/m.mpd", &live).is_err());
    }

    #[test]
    fn keeps_escaped_characters_in_links() {
        let text = r#"<MPD type="static" mediaPresentationDuration="PT4S"><Period>
            <BaseURL>https://cdn.example.com/v/?sig=a&amp;exp=1</BaseURL>
            <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
              <BaseURL>file.mp4?t=1&amp;u=2</BaseURL></Representation></AdaptationSet>
          </Period></MPD>"#;
        let reps = parse_manifest("https://example.com/m.mpd", text).unwrap();
        assert_eq!(reps[0].segments, ["https://cdn.example.com/v/file.mp4?t=1&u=2"]);
    }

    #[test]
    fn recognises_dash_links_and_types() {
        assert!(looks_like("https://e.com/a/manifest.MPD?sig=1"));
        assert!(looks_like("https://e.com/play?id=7#adm-video=v720"));
        assert!(looks_like("https://e.com/play?id=7#adm-dash"));
        assert!(!looks_like("https://e.com/play?id=7"));
        assert!(!looks_like("https://e.com/v/index.m3u8"));
        assert_eq!(manifest_url("https://e.com/play?id=7#adm-video=v1"), "https://e.com/play?id=7");
        assert_eq!(manifest_url("https://e.com/play?id=7#adm-dash"), "https://e.com/play?id=7");
        assert!(is_dash_type("application/dash+xml; charset=utf-8"));
        assert!(!is_dash_type("application/vnd.apple.mpegurl"));
        assert!(!is_dash_type("video/mp4"));
    }

    #[test]
    fn audio_only_link_is_pinned() {
        let audio = MANIFEST.split("<AdaptationSet mimeType=\"video/mp4\">").next().unwrap().to_string()
            + MANIFEST.split("</AdaptationSet>").nth(1).unwrap()
            + "</AdaptationSet></Period></MPD>";
        let reps = parse_manifest("https://e.com/play?id=7", &audio).unwrap();
        let (v, a) = choose(&reps, "https://e.com/play?id=7#adm-dash").unwrap();
        assert!(v.is_none());
        assert_eq!(a.unwrap().id, "a1");
    }

    /// Real public test stream: every video and audio piece downloads and
    /// the joined tracks are valid MP4 (this part needs no ffmpeg).
    #[test]
    #[ignore]
    fn downloads_real_dash_pieces() {
        let client = reqwest::Client::new();
        // One file per quality, and numbered segment templates.
        for url in [
            "https://dash.akamaized.net/dash264/TestCases/1a/netflix/exMPD_BIP_TC1.mpd",
            "https://dash.akamaized.net/akamai/bbb_30fps/bbb_30fps.mpd",
        ] {
        tauri::async_runtime::block_on(async {
            let reps = parse_manifest(url, &fetch_text(&client, url, &[]).await.unwrap()).unwrap();
            let lowest = reps.iter().filter(|r| r.video).last().unwrap();
            let (v, a) = choose(&reps, &format!("{url}{PICK}{}", lowest.id)).unwrap();
            for rep in [v, a].into_iter().flatten() {
                let mut track = Vec::new();
                // The header plus the first few pieces is enough to prove the links work.
                for p in pieces_of(rep).into_iter().take(6) {
                    let resp = client.get(&p.url).send().await.unwrap();
                    assert!(resp.status().is_success(), "{} -> {}", p.url, resp.status());
                    track.extend(resp.bytes().await.unwrap());
                }
                println!("{} ({}): {} pieces, {} bytes", rep.id, if rep.video { "video" } else { "audio" }, pieces_of(rep).len(), track.len());
                assert_eq!(&track[4..8], b"ftyp", "joined track should start with an MP4 header");
            }
        });
        }
    }

    #[test]
    fn reads_durations() {
        assert_eq!(duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(duration("P1DT0S"), Some(86400.0));
    }
}
