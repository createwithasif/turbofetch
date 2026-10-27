// Talks to the ADM app over its local bridge (see src-tauri/src/bridge.rs).
const APP = "http://127.0.0.1:47321";
const MEDIA_EXT = /\.(mp4|webm|mkv|mov|m4v|flv|mp3|m4a|aac|ogg|wav|flac)(\?|#|$)/i;
// Formats that only make sense as pieces of a stream, never as a whole file.
const FRAGMENT_EXT = /\.(m4s|ts|m3u8|mpd)(\?|#|$)/i;
const MIN_MEDIA_BYTES = 300 * 1024;

async function settings() {
  const { enabled = true } = await chrome.storage.local.get("enabled");
  return { enabled };
}

// Never let a stuck request hold up a download.
const withTimeout = (ms) => AbortSignal.timeout(ms);

async function ping() {
  try {
    const res = await fetch(`${APP}/ping`, { headers: { "X-ADM": "1" }, signal: withTimeout(1500) });
    return res.ok ? await res.json() : null;
  } catch {
    return null;
  }
}

async function cookiesFor(url, referrer) {
  try {
    const set = new Map();
    if (referrer && /^https?:\/\//i.test(referrer)) {
      const refList = await chrome.cookies.getAll({ url: referrer });
      for (const c of refList) set.set(c.name, c.value);
    }
    const list = await chrome.cookies.getAll({ url });
    for (const c of list) set.set(c.name, c.value);
    return Array.from(set.entries()).map(([k, v]) => `${k}=${v}`).join("; ") || null;
  } catch {
    return null;
  }
}

async function post(path, { url, fileName, referrer }) {
  return fetch(`${APP}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-ADM": "1" },
    signal: withTimeout(8000),
    body: JSON.stringify({
      url,
      fileName: fileName || null,
      referrer: referrer || null,
      cookies: await cookiesFor(url, referrer),
      userAgent: navigator.userAgent,
    }),
  });
}

async function sendToApp(item) {
  // A second try covers ADM still starting up or waking from sleep.
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const res = await post("/add", item);
      const body = await res.json().catch(() => ({}));
      return res.ok ? { ok: true } : { ok: false, error: body.error || `HTTP ${res.status}` };
    } catch {
      await new Promise((r) => setTimeout(r, 700));
    }
  }
  return { ok: false, error: "ADM is not running" };
}

/* ---- Take over browser downloads ---- */

// Downloads handed back to the browser because ADM couldn't take them.
const returned = new Set();

chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url) || item.state !== "in_progress") return;
  if (returned.delete(url)) return;
  if (!(await settings()).enabled) return;

  const fileName = item.filename ? item.filename.split(/[\\/]/).pop() : null;
  // Stop the browser first: its placeholder file holds the name, and ADM
  // would otherwise save the download as "name (1)".
  try {
    await chrome.downloads.cancel(item.id);
    await chrome.downloads.erase({ id: item.id });
  } catch {}
  const result = await sendToApp({ url, fileName, referrer: item.referrer });
  // ADM is closed or refused it, so nothing gets lost: the browser downloads it after all.
  if (!result.ok) {
    console.debug("ADM: returning download to the browser:", result.error);
    returned.add(url);
    chrome.downloads.download({ url }).catch(() => returned.delete(url));
  }
});

/* ---- Right-click menu ---- */

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "adm-link",
    title: "Download with ADM",
    contexts: ["link", "video", "audio", "image"],
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  const url = info.srcUrl || info.linkUrl;
  if (url) sendToApp({ url, referrer: tab?.url });
});

/* ---- Detect videos and audio playing on a page ---- */

const mediaKey = (tabId) => `media:${tabId}`;
const hlsKey = (tabId) => `hls:${tabId}`;
const isPlaylist = (url, type) =>
  /\.(m3u8|mpd)(\?|#|$)/i.test(url) || type.includes("mpegurl") || type.includes("dash+xml");

async function getList(key) {
  return (await chrome.storage.session.get(key))[key] || [];
}

/** Direct files the page asked for, minus the pieces a stream player fetched. */
async function directMedia(tabId) {
  const direct = await getList(mediaKey(tabId));
  if (!(await getList(hlsKey(tabId))).length) return direct;
  // With a manifest on the tab, files a player pulled in by script are just
  // parts of that stream (e.g. DASH `.m4v` pieces), not videos of their own.
  return direct.filter((m) => !m.byScript);
}

/** Direct files plus the qualities of every HLS stream seen on the tab. */
async function getMedia(tabId, referrer) {
  const direct = await directMedia(tabId);
  const playlists = await getList(hlsKey(tabId));
  const results = await Promise.all(
    playlists.map(async (url) => {
      // Without the app's quality list, still offer the stream itself; the
      // download then fails with the app's real error instead of vanishing.
      const whole = { url, variants: [{ url, label: "Stream" }] };
      try {
        const res = await post("/hls-info", { url, referrer });
        return res.ok ? { url, variants: (await res.json()).variants } : whole;
      } catch {
        console.debug("ADM: no quality list for", url);
        return whole;
      }
    }),
  );
  // Quality playlists already listed under a master playlist are skipped.
  const listed = new Set();
  for (const r of results) if (r && r.variants.length > 1) r.variants.forEach((v) => listed.add(v.url));
  const streams = [];
  for (const r of results) {
    if (!r || (r.variants.length === 1 && listed.has(r.url))) continue;
    for (const v of r.variants) {
      if (streams.some((s) => s.url === v.url)) continue;
      const note = /\.mpd(\?|#|$)/i.test(v.url) ? "DASH" : "HLS";
      streams.push({ url: v.url, label: v.label, size: 0, note, hls: true });
    }
  }
  return [...streams, ...direct.sort((a, b) => (b.size || 0) - (a.size || 0))];
}

async function mediaCount(tabId) {
  return (await directMedia(tabId)).length + (await getList(hlsKey(tabId))).length;
}

async function noteFound(tabId) {
  const count = await mediaCount(tabId);
  // No listener means the tab predates this extension (re)load, so it has no
  // content script yet; inject it and it will show the button itself.
  chrome.tabs
    .sendMessage(tabId, { type: "media-found", count })
    .catch(() => chrome.scripting.executeScript({ target: { tabId }, files: ["content.js"] }).catch(() => {}));
  chrome.action.setBadgeText({ tabId, text: String(count) });
  chrome.action.setBadgeBackgroundColor({ color: "#2e86de" });
}

function qualityLabel(url, contentType) {
  const m = url.match(/(?:^|[^\d])(2160|1440|1080|720|480|360|240|144)p?(?:[^\d]|$)/);
  if (m) return `${m[1]}p`;
  if (contentType.startsWith("audio/") || /\.(mp3|m4a|aac|ogg|wav|flac)/i.test(url)) return "Audio";
  return "Video";
}

function totalSize(headers) {
  const get = (name) => headers.find((h) => h.name.toLowerCase() === name)?.value;
  const range = get("content-range");
  if (range && range.includes("/")) return Number(range.split("/").pop()) || 0;
  return Number(get("content-length")) || 0;
}

// Responses arrive in bursts; one at a time so list updates don't overwrite each other.
let queue = Promise.resolve();
const serially = (fn) => (queue = queue.then(fn, fn));

chrome.webRequest.onHeadersReceived.addListener(
  (details) => serially(() => record(details)),
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other", "object"] },
  ["responseHeaders"],
);

async function record(details) {
  if (details.tabId < 0) return;
  const headers = details.responseHeaders || [];
  const type = (headers.find((h) => h.name.toLowerCase() === "content-type")?.value || "").toLowerCase();
  const url = details.url;
  if (isPlaylist(url, type)) {
    const list = await getList(hlsKey(details.tabId));
    if (list.includes(url)) return;
    list.push(url);
    await chrome.storage.session.set({ [hlsKey(details.tabId)]: list.slice(-10) });
    await noteFound(details.tabId);
    return;
  }
  const isMedia = type.startsWith("video/") || type.startsWith("audio/") || MEDIA_EXT.test(url);
  if (!isMedia || FRAGMENT_EXT.test(url)) return;

  const size = totalSize(headers);
  if (size && size < MIN_MEDIA_BYTES) return;

  // Strip range-style query params so the same file isn't listed many times.
  const clean = url.replace(/([?&])(range|rn|rbuf|bytestart|byteend)=[^&]*/gi, "$1");
  const list = await getList(mediaKey(details.tabId));
  if (list.some((m) => m.url === clean)) return;
  list.push({ url: clean, size, label: qualityLabel(url, type), byScript: details.type === "xmlhttprequest" });
  await chrome.storage.session.set({ [mediaKey(details.tabId)]: list.slice(-20) });
  await noteFound(details.tabId);
}

const forget = (tabId) => chrome.storage.session.remove([mediaKey(tabId), hlsKey(tabId)]);
// Page addresses without the #fragment, so jumping within a page keeps its media.
const pages = new Map();
chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (!change.url) return;
  const page = change.url.split("#")[0];
  if (pages.get(tabId) === page) return;
  pages.set(tabId, page);
  serially(async () => {
    await forget(tabId);
    // Single-page sites keep the same document, so tell its button the list is empty now.
    chrome.tabs.sendMessage(tabId, { type: "media-found", count: 0 }).catch(() => {});
    chrome.action.setBadgeText({ tabId, text: "" });
  });
});
chrome.tabs.onRemoved.addListener((tabId) => {
  pages.delete(tabId);
  forget(tabId);
});

/* ---- Messages from the page button and the popup ---- */

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  (async () => {
    const tabId = msg.tabId ?? sender.tab?.id;
    switch (msg.type) {
      case "media-count":
        reply(await mediaCount(tabId));
        break;
      case "list-media":
        reply(await getMedia(tabId, sender.tab?.url || msg.referrer));
        break;
      case "download":
        reply(await sendToApp({ url: msg.url, fileName: msg.fileName, referrer: sender.tab?.url || msg.referrer }));
        break;
      case "status":
        reply({ app: await ping(), ...(await settings()) });
        break;
      case "set-enabled":
        await chrome.storage.local.set({ enabled: msg.enabled });
        reply({ ok: true });
        break;
    }
  })();
  return true; // keep the channel open for the async reply
});
