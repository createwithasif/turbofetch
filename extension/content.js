// The background injects this file into tabs opened before the extension was
// (re)loaded, so make sure a tab only ever runs it once.
if (!window.__admContentLoaded) {
  window.__admContentLoaded = true;

  // Floating "Download this video" button shown over the main video on a page.
  (() => {
    let host = null;
    let menuOpen = false;
    let found = 0; // files and streams the background has seen on this page
    let dismissed = null; // page address where the user closed the button

    function biggestVideo() {
      let best = null;
      let bestArea = 0;
      for (const v of document.querySelectorAll("video")) {
        const r = v.getBoundingClientRect();
        const area = r.width * r.height;
        if (r.width >= 200 && r.height >= 120 && area > bestArea) {
          best = v;
          bestArea = area;
        }
      }
      return best;
    }

    function formatSize(n) {
      if (!n) return "";
      const mb = n / 1024 / 1024;
      return mb >= 1024 ? `${(mb / 1024).toFixed(2)} GB` : `${mb.toFixed(1)} MB`;
    }

    function create() {
      host = document.createElement("div");
      host.style.cssText = "position:fixed;z-index:2147483647;top:0;left:0;display:none;";
      const root = host.attachShadow({ mode: "closed" });
      root.innerHTML = `
        <style>
          * { box-sizing: border-box; font: 12px -apple-system, "Segoe UI", sans-serif; }
          .btn { display:flex; align-items:center; gap:6px; padding:5px 10px; border:0; border-radius:4px;
                 background:linear-gradient(#4a97e8,#2e78cc); color:#fff; cursor:pointer;
                 box-shadow:0 2px 6px rgba(0,0,0,.35); opacity:.92; }
          .btn:hover { opacity:1; }
          .menu { margin-top:4px; min-width:240px; max-width:320px; background:#fff; color:#1a1a1a; border:1px solid #c9d0d9;
                  border-radius:4px; box-shadow:0 4px 14px rgba(0,0,0,.3); overflow:hidden; }
          .item { display:flex; justify-content:space-between; gap:16px; padding:6px 10px; cursor:pointer; }
          .item:hover { background:#cce4ff; }
          .size { color:#5d6673; }
          .msg { padding:8px 10px; color:#5d6673; line-height:1.45; }
          .msg a { color:#2e78cc; text-decoration:underline; font-weight:500; }
          .x { margin-left:4px; opacity:.7; }
          .x:hover { opacity:1; }
        </style>
        <button class="btn"><span>&#11015; Download this video</span><span class="x" title="Hide">&times;</span></button>
        <div class="menu" hidden></div>`;
      const btn = root.querySelector(".btn");
      const menu = root.querySelector(".menu");

      root.querySelector(".x").addEventListener("click", (e) => {
        e.stopPropagation();
        dismissed = location.href;
        hide();
      });

      btn.addEventListener("click", async () => {
        menuOpen = !menuOpen;
        menu.hidden = !menuOpen;
        if (!menuOpen) return;
        const media = await chrome.runtime.sendMessage({ type: "list-media" });
        menu.innerHTML = "";
        if (!media.length) {
          // A blob: player builds the video itself, so there is no file to hand over.
          if (isYouTubeWatch()) {
            menu.innerHTML = `<div class="msg">YouTube streams video in its own protected SABR/UMP format. No downloadable file was found.<br><br>Channel owners can download their own videos from <a href="https://studio.youtube.com" target="_blank" rel="noreferrer">YouTube Studio</a>.</div>`;
            menu.querySelectorAll("a").forEach((a) => a.addEventListener("click", (e) => e.stopPropagation()));
          } else if (biggestVideo()?.currentSrc.startsWith("blob:")) {
            menu.innerHTML = `<div class="msg">This player streams video in its own protected format.<br>No downloadable file was found.</div>`;
          } else {
            menu.innerHTML = `<div class="msg">Play the video for a moment, then try again.</div>`;
          }
          return;
        }
        for (const m of media) {
          const item = document.createElement("div");
          item.className = "item";
          item.innerHTML = `<span></span><span class="size"></span>`;
          item.children[0].textContent = m.label;
          item.children[1].textContent = m.note || formatSize(m.size);
          item.addEventListener("click", async () => {
            menu.innerHTML = `<div class="msg">Sending to ADM…</div>`;
            const title = document.title.replace(/[\\/:*?"<>|]/g, "_").slice(0, 120);
            const ext = (m.url.split(/[?#]/)[0].match(/\.(\w{2,4})$/) || [, "mp4"])[1];
            const res = await chrome.runtime.sendMessage({
              type: "download",
              url: m.url,
              fileName: title ? `${title} (${m.label}).${m.hls ? "mp4" : ext}` : null,
            });
            menu.textContent = "";
            const msg = document.createElement("div");
            msg.className = "msg";
            msg.textContent = res.ok ? "Added to ADM ✓" : `Error: ${res.error}`;
            menu.appendChild(msg);
            setTimeout(() => {
              menuOpen = false;
              menu.hidden = true;
            }, 1800);
          });
          menu.appendChild(item);
        }
      });
      document.documentElement.appendChild(host);
    }

    function position() {
      // After the extension is reloaded this copy can no longer reach it; the
      // fresh copy injected by the background takes over the button.
      if (!chrome.runtime?.id) {
        hide();
        clearInterval(timer);
        return;
      }
      // YouTube never exposes a file, but its watch pages still get the button;
      // the menu then says what is (not) available.
      const wanted = found > 0 || isYouTubeWatch();
      if (wanted && !host && dismissed !== location.href) show();
      if (!wanted && host) hide();
      if (!host) return;
      const v = biggestVideo();
      if (!v) {
        host.style.display = "none";
        return;
      }
      // A fullscreen player hides everything outside it, so the button moves in too.
      const parent = document.fullscreenElement || document.documentElement;
      if (host.parentNode !== parent) parent.appendChild(host);
      const r = v.getBoundingClientRect();
      host.style.display = "block";
      host.style.top = `${Math.max(0, r.top) + 8}px`;
      host.style.left = `${Math.max(0, Math.min(r.right, innerWidth) - 200)}px`;
    }

    const isYouTubeWatch = () => /(^|\.)youtube\.com$/.test(location.hostname) && location.pathname === "/watch";

    function show() {
      if (host) return;
      create();
      position();
    }

    // Nothing left to offer (e.g. the site moved to another page without reloading).
    function hide() {
      host?.remove();
      host = null;
      menuOpen = false;
    }

    chrome.runtime.onMessage.addListener((msg) => {
      if (msg.type !== "media-found") return;
      found = msg.count;
      found > 0 ? show() : hide();
    });
    // Players often fetch their stream before this script loads, so catch up.
    chrome.runtime
      .sendMessage({ type: "media-count" })
      .then((count) => {
        found = count;
        if (count > 0) show();
      })
      .catch(() => {});
    const timer = setInterval(position, 700);
    addEventListener("scroll", position, { passive: true });
  })();

  // Magnet links open in ADM instead of asking for another torrent app.
  document.addEventListener(
    "click",
    (e) => {
      const a = e.target instanceof Element ? e.target.closest('a[href^="magnet:"]') : null;
      if (!a) return;
      e.preventDefault();
      e.stopPropagation();
      chrome.runtime.sendMessage({ type: "download", url: a.href });
    },
    true,
  );
}
