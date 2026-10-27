const $ = (id) => document.getElementById(id);

(async () => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const status = await chrome.runtime.sendMessage({ type: "status" });

  $("dot").classList.toggle("on", !!status.app);
  $("status").textContent = status.app
    ? `Connected to ADM ${status.app.version}`
    : "ADM is not running";
  $("enabled").checked = status.enabled;
  $("enabled").addEventListener("change", (e) =>
    chrome.runtime.sendMessage({ type: "set-enabled", enabled: e.target.checked }),
  );

  const media = await chrome.runtime.sendMessage({ type: "list-media", tabId: tab.id, referrer: tab.url });
  if (!media.length) {
    $("media").innerHTML = `<div class="muted" style="padding-top:6px">Nothing found yet. Start playing a video.</div>`;
    return;
  }
  for (const m of media) {
    const row = document.createElement("div");
    row.className = "item";
    row.innerHTML = "<span></span><span class='muted'></span>";
    row.children[0].textContent = m.label;
    row.children[1].textContent = m.note || (m.size ? `${(m.size / 1048576).toFixed(1)} MB` : "");
    row.title = m.url;
    row.addEventListener("click", async () => {
      const res = await chrome.runtime.sendMessage({ type: "download", url: m.url, referrer: tab.url });
      row.children[1].textContent = res.ok ? "Added ✓" : res.error;
    });
    $("media").appendChild(row);
  }
})();
