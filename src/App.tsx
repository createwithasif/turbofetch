import { MouseEvent as ReactMouseEvent, ReactNode, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { AboutDialog, AddUrlDialog, BrowserSetupDialog, OptionsDialog } from "./components/dialogs";
import { categoryColor, FileBadge, Icon, Logo } from "./components/icons";
import {
  APP_NAME,
  APP_VERSION,
  categoryOf,
  DEVELOPER,
  Download,
  expectedTotal,
  formatBytes,
  formatDate,
  isReconnecting,
  looksDownloadable,
  matchesView,
  percent,
  Settings,
  Status,
  statusLabel,
  timeLeft,
  View,
} from "./lib/model";
import { openProgressWindow } from "./lib/windows";

/** A public 100 MB speed-test file: handy to see the engine working right away. */
const TEST_URL = "https://proof.ovh.net/files/100Mb.dat";

const NAV: { view: View; label: string; icon: ReactNode; color?: string; indent?: boolean }[] = [
  { view: "all", label: "All downloads", icon: Icon.layers },
  { view: "Compressed", label: "Compressed", icon: null, color: categoryColor("Compressed"), indent: true },
  { view: "Documents", label: "Documents", icon: null, color: categoryColor("Documents"), indent: true },
  { view: "Music", label: "Music", icon: null, color: categoryColor("Music"), indent: true },
  { view: "Programs", label: "Programs", icon: null, color: categoryColor("Programs"), indent: true },
  { view: "Video", label: "Video", icon: null, color: categoryColor("Video"), indent: true },
  { view: "torrents", label: "Torrents", icon: null, color: categoryColor("Torrent"), indent: true },
  { view: "active", label: "Downloading", icon: Icon.bolt },
  { view: "unfinished", label: "Unfinished", icon: Icon.clock },
  { view: "finished", label: "Finished", icon: Icon.check },
];

function App() {
  const [downloads, setDownloads] = useState<Download[]>([]);
  const [view, setView] = useState<View>("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [addUrl, setAddUrl] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"options" | "browser" | "about" | null>(() => {
    // The browser setup guide is shown once, on first launch.
    try {
      if (localStorage.getItem("browserSetupShown")) return null;
      localStorage.setItem("browserSetupShown", "1");
    } catch {}
    return "browser";
  });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [error, setError] = useState("");
  const lastStatus = useRef(new Map<string, Status>());
  const lastClip = useRef<string | null>(null);

  const refresh = () => invoke<Download[]>("list_downloads").then(setDownloads);

  const showProgress = (id: string, list = downloads) => {
    const d = list.find((x) => x.id === id);
    openProgressWindow(id, d?.fileName ?? "Download");
  };

  async function added(id: string) {
    setAddUrl(null);
    const list = await invoke<Download[]>("list_downloads");
    setDownloads(list);
    setSelected(new Set([id]));
    showProgress(id, list);
  }

  useEffect(() => {
    refresh();
    const unlisten = listen<Download[]>("downloads", (e) => setDownloads(e.payload));
    const unlistenExternal = listen<string>("external-add", (e) => added(e.payload));
    const closeMenu = () => setMenu(null);
    window.addEventListener("click", closeMenu);
    isPermissionGranted().then((ok) => {
      if (!ok) requestPermission();
    });
    return () => {
      unlisten.then((f) => f());
      unlistenExternal.then((f) => f());
      window.removeEventListener("click", closeMenu);
    };
  }, []);

  // Notify when a download finishes or fails.
  useEffect(() => {
    const seen = lastStatus.current;
    for (const d of downloads) {
      const before = seen.get(d.id);
      if (before && before !== "completed" && d.status === "completed") {
        sendNotification({ title: "Download complete", body: d.fileName });
      } else if (before && before !== "error" && d.status === "error") {
        sendNotification({ title: "Download failed", body: `${d.fileName}: ${d.error ?? ""}` });
      }
      seen.set(d.id, d.status);
    }
  }, [downloads]);

  // Clipboard monitor: offer to download file links as soon as they're copied.
  useEffect(() => {
    const timer = setInterval(async () => {
      let text: string;
      try {
        text = (await readText()).trim();
      } catch {
        return;
      }
      const first = lastClip.current === null;
      if (text === lastClip.current) return;
      lastClip.current = text;
      // Ignore whatever was already on the clipboard when the app started.
      if (first || !looksDownloadable(text)) return;
      const settings = await invoke<Settings>("get_settings");
      if (settings.clipboardMonitor) setAddUrl((cur) => cur ?? text);
    }, 1500);
    return () => clearInterval(timer);
  }, []);

  const call = (cmd: string, args: Record<string, unknown>) =>
    invoke(cmd, args).catch((err) => setError(String(err)));

  const q = query.trim().toLowerCase();
  const shown = downloads.filter(
    (d) => matchesView(d, view) && (!q || d.fileName.toLowerCase().includes(q) || d.url.toLowerCase().includes(q)),
  );
  const chosen = downloads.filter((d) => selected.has(d.id));
  const canResume = (d: Download) => d.status === "paused" || d.status === "error";
  const canStop = (d: Download) => d.status === "downloading" || d.status === "queued";

  const resume = (list = chosen) => list.filter(canResume).forEach((d) => call("resume_download", { id: d.id }));
  const stop = (list = chosen) => list.filter(canStop).forEach((d) => call("pause_download", { id: d.id }));
  const remove = async (list: Download[]) => {
    if (list.length === 0) return;
    for (const d of list) await call("remove_download", { id: d.id, deleteFile: false });
    setSelected(new Set());
    refresh();
  };

  async function testDownload() {
    setError("");
    try {
      added(await invoke<string>("add_download", { url: TEST_URL }));
    } catch (err) {
      setError(String(err));
    }
  }

  function selectRow(e: ReactMouseEvent, id: string) {
    if (e.metaKey || e.ctrlKey) {
      const next = new Set(selected);
      next.has(id) ? next.delete(id) : next.add(id);
      setSelected(next);
    } else {
      setSelected(new Set([id]));
    }
  }

  function openRow(d: Download) {
    if (d.status === "completed") openPath(d.path).catch(() => revealItemInDir(d.path));
    else showProgress(d.id);
  }

  const tools: { icon: ReactNode; label: string; onClick: () => void; disabled?: boolean; tone?: string }[] = [
    { icon: Icon.add, label: "Add URL", onClick: () => setAddUrl(""), tone: "blue" },
    { icon: Icon.play, label: "Resume", onClick: () => resume(), disabled: !chosen.some(canResume), tone: "green" },
    { icon: Icon.pause, label: "Stop", onClick: () => stop(), disabled: !chosen.some(canStop), tone: "amber" },
    {
      icon: Icon.stopAll,
      label: "Stop all",
      onClick: () => stop(downloads),
      disabled: !downloads.some(canStop),
      tone: "amber",
    },
    { icon: Icon.trash, label: "Delete", onClick: () => remove(chosen), disabled: chosen.length === 0, tone: "red" },
    {
      icon: Icon.sweep,
      label: "Clear done",
      onClick: () => remove(downloads.filter((d) => d.status === "completed")),
      disabled: !downloads.some((d) => d.status === "completed"),
      tone: "red",
    },
    { icon: Icon.gear, label: "Options", onClick: () => setDialog("options"), tone: "slate" },
    { icon: Icon.globe, label: "Browser", onClick: () => setDialog("browser"), tone: "slate" },
  ];

  const totalSpeed = downloads.reduce((sum, d) => sum + d.speed, 0);
  const running = downloads.filter((d) => d.status === "downloading").length;
  const queued = downloads.filter((d) => d.status === "queued").length;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <Logo size={34} />
          <div>
            <div className="brand-name">{APP_NAME}</div>
            <div className="brand-sub">Advanced Download Manager</div>
          </div>
        </div>
        <nav className="toolbar">
          {tools.map((t) => (
            <button key={t.label} className={`tool ${t.tone}`} onClick={t.onClick} disabled={t.disabled}>
              <span className="tool-icon">{t.icon}</span>
              <span>{t.label}</span>
            </button>
          ))}
        </nav>
        <div className="speed-chip" title="Total download speed">
          {Icon.download}
          <span>{formatBytes(totalSpeed)}/s</span>
        </div>
      </header>

      <div className="main">
        <aside className="sidebar">
          <div className="side-title">Categories</div>
          {NAV.map((n) => {
            const count = downloads.filter((d) => matchesView(d, n.view)).length;
            return (
              <button
                key={n.view}
                className={`nav ${view === n.view ? "on" : ""} ${n.indent ? "indent" : ""}`}
                onClick={() => setView(n.view)}
              >
                {n.icon ? (
                  <span className="nav-icon">{n.icon}</span>
                ) : (
                  <span className="dot" style={{ background: n.color }} />
                )}
                <span>{n.label}</span>
                {count > 0 && <span className="count">{count}</span>}
              </button>
            );
          })}
          <div className="side-foot">
            <button className="nav" onClick={() => setDialog("about")}>
              <span className="nav-icon">{Icon.info}</span>
              <span>About {APP_NAME}</span>
            </button>
          </div>
        </aside>

        <section className="content">
          <div className="content-head">
            <h1>{NAV.find((n) => n.view === view)?.label}</h1>
            <label className="search">
              {Icon.search}
              <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search downloads" />
            </label>
          </div>

          {shown.length === 0 ? (
            <div className="empty">
              <div className="empty-art">{Icon.download}</div>
              <h2>{downloads.length === 0 ? "No downloads yet" : "Nothing here"}</h2>
              <p>
                Paste a link, or install the browser extension so downloads from Chrome, Edge and Brave open in{" "}
                {APP_NAME} automatically.
              </p>
              <div className="empty-actions">
                <button className="primary" onClick={() => setAddUrl("")}>
                  {Icon.add} Add URL
                </button>
                <button onClick={testDownload}>{Icon.bolt} Try a test download</button>
                <button onClick={() => setDialog("browser")}>{Icon.globe} Set up browser</button>
              </div>
            </div>
          ) : (
            <div className="table-wrap" onClick={() => setSelected(new Set())}>
              <table className="grid">
                <thead>
                  <tr>
                    <th className="c-name">File name</th>
                    <th className="c-size">Size</th>
                    <th className="c-status">Status</th>
                    <th className="c-time">Time left</th>
                    <th className="c-rate">Transfer rate</th>
                    <th className="c-date">Last try</th>
                    <th>Description</th>
                  </tr>
                </thead>
                <tbody>
                  {shown.map((d) => {
                    const pct = percent(d);
                    const total = expectedTotal(d);
                    return (
                      <tr
                        key={d.id}
                        className={selected.has(d.id) ? "sel" : ""}
                        onClick={(e) => {
                          e.stopPropagation();
                          selectRow(e, d.id);
                        }}
                        onDoubleClick={() => openRow(d)}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          if (!selected.has(d.id)) setSelected(new Set([d.id]));
                          setMenu({ x: e.clientX, y: e.clientY });
                        }}
                      >
                        <td className="c-name">
                          <div className="fname">
                            <FileBadge cat={d.torrent ? "Torrent" : categoryOf(d)} />
                            <span title={d.fileName}>{d.fileName}</span>
                          </div>
                        </td>
                        <td>{total ? `${d.total ? "" : "~"}${formatBytes(total)}` : ""}</td>
                        <td>
                          {d.status === "completed" || d.status === "error" || d.status === "queued" ? (
                            <span className={`pill ${d.status}`}>{statusLabel(d)}</span>
                          ) : (
                            <div className={`rowbar ${d.status} ${isReconnecting(d) ? "warn" : ""}`}>
                              <div style={{ width: `${pct ?? 0}%` }} />
                              <span>{statusLabel(d)}</span>
                            </div>
                          )}
                        </td>
                        <td className="num">{timeLeft(d)}</td>
                        <td className="num">{d.status === "downloading" ? `${formatBytes(d.speed)}/s` : ""}</td>
                        <td className="muted">{formatDate(d.lastTry)}</td>
                        <td className="muted desc" title={d.error ?? d.url}>
                          {d.error ?? (d.torrent ? `Torrent · ${d.peers} peers` : d.url)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>
      </div>

      <footer className="statusbar">
        <span>{downloads.length} items</span>
        <span>{running} downloading</span>
        {queued > 0 && <span>{queued} queued</span>}
        {error && (
          <span className="error" onClick={() => setError("")}>
            {error}
          </span>
        )}
        <span className="spacer" />
        <span className="credit">
          {APP_NAME} {APP_VERSION} · Developed by {DEVELOPER}
        </span>
      </footer>

      {menu && chosen.length > 0 && (
        <div className="context" style={{ left: menu.x, top: menu.y }}>
          {chosen.length === 1 && <div onClick={() => openRow(chosen[0])}>Open</div>}
          {chosen.length === 1 && <div onClick={() => revealItemInDir(chosen[0].path)}>Show in folder</div>}
          {chosen.length === 1 && chosen[0].status !== "completed" && (
            <div onClick={() => showProgress(chosen[0].id)}>Show progress window</div>
          )}
          <hr />
          <div onClick={() => resume()}>Resume</div>
          <div onClick={() => stop()}>Stop</div>
          <hr />
          <div onClick={() => navigator.clipboard?.writeText(chosen.map((d) => d.url).join("\n"))}>
            Copy address
          </div>
          <div className="danger" onClick={() => remove(chosen)}>
            Delete
          </div>
        </div>
      )}

      {addUrl !== null && <AddUrlDialog initialUrl={addUrl} onClose={() => setAddUrl(null)} onAdded={added} />}
      {dialog === "options" && (
        <OptionsDialog onClose={() => setDialog(null)} onBrowserSetup={() => setDialog("browser")} />
      )}
      {dialog === "browser" && <BrowserSetupDialog onClose={() => setDialog(null)} />}
      {dialog === "about" && <AboutDialog onClose={() => setDialog(null)} />}
    </div>
  );
}

export default App;
