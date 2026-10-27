import { FormEvent, ReactNode, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { APP_NAME, APP_VERSION, DEVELOPER, Settings } from "../lib/model";
import { Icon, Logo } from "./icons";

function Window({
  title,
  onClose,
  children,
  width = 480,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="overlay" onMouseDown={onClose}>
      <div className="window" style={{ width }} onMouseDown={(e) => e.stopPropagation()}>
        <div className="win-title">
          <span>{title}</span>
          <button className="icon-btn" onClick={onClose} aria-label="Close">
            {Icon.close}
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

export function AddUrlDialog({
  initialUrl,
  onClose,
  onAdded,
}: {
  initialUrl: string;
  onClose: () => void;
  onAdded: (id: string) => void;
}) {
  const [url, setUrl] = useState(initialUrl);
  const [dir, setDir] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    // Pre-fill from the clipboard if it holds a link.
    if (initialUrl) return;
    readText()
      .then((t) => /^(https?:\/\/|magnet:\?)/i.test(t.trim()) && setUrl(t.trim()))
      .catch(() => {});
  }, [initialUrl]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!url.trim()) return;
    setBusy(true);
    setError("");
    try {
      onAdded(await invoke<string>("add_download", { url, dir: dir || null }));
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <Window title="Add new download" onClose={onClose} width={520}>
      <form onSubmit={submit}>
        <div className="win-body">
          <label className="field">
            <span>Address</span>
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://…  ·  magnet:?…  ·  .m3u8 video link"
              autoFocus
              spellCheck={false}
            />
          </label>
          <label className="field">
            <span>Save to</span>
            <input
              value={dir}
              onChange={(e) => setDir(e.target.value)}
              placeholder="Default download folder"
              spellCheck={false}
            />
          </label>
          <p className="hint">Files, videos (HLS), magnet links and .torrent files are all supported.</p>
          {error && <p className="error">{error}</p>}
        </div>
        <div className="win-buttons">
          <span className="spacer" />
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={busy}>
            {busy ? "Connecting…" : <>{Icon.download} Start download</>}
          </button>
        </div>
      </form>
    </Window>
  );
}

export function OptionsDialog({ onClose, onBrowserSetup }: { onClose: () => void; onBrowserSetup: () => void }) {
  const [s, setS] = useState<Settings | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    invoke<Settings>("get_settings").then(setS);
  }, []);

  if (!s) return null;
  const set = (patch: Partial<Settings>) => setS({ ...s, ...patch });

  async function save(e: FormEvent) {
    e.preventDefault();
    try {
      await invoke("set_settings", { settings: s });
      onClose();
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <Window title="Options" onClose={onClose} width={520}>
      <form onSubmit={save}>
        <div className="win-body">
          <section className="group">
            <h4>Connection</h4>
            <label className="row-field">
              <span>Connections per download</span>
              <select value={s.connections} onChange={(e) => set({ connections: +e.target.value })}>
                {[1, 2, 4, 8, 16, 24, 32].map((n) => (
                  <option key={n}>{n}</option>
                ))}
              </select>
            </label>
            <label className="row-field">
              <span>Downloads at the same time</span>
              <input
                type="number"
                min={1}
                max={20}
                value={s.maxActive}
                onChange={(e) => set({ maxActive: +e.target.value })}
              />
            </label>
            <label className="row-field">
              <span>Speed limit (KB/s, 0 = unlimited)</span>
              <input
                type="number"
                min={0}
                value={s.speedLimitKb}
                onChange={(e) => set({ speedLimitKb: Math.max(0, +e.target.value) })}
              />
            </label>
          </section>
          <section className="group">
            <h4>Save to</h4>
            <input
              className="wide"
              value={s.downloadDir ?? ""}
              onChange={(e) => set({ downloadDir: e.target.value || null })}
              spellCheck={false}
            />
          </section>
          <section className="group">
            <h4>Integration</h4>
            <label className="check">
              <input
                type="checkbox"
                checked={s.browserIntegration}
                onChange={(e) => set({ browserIntegration: e.target.checked })}
              />
              Take over downloads from the browser
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={s.clipboardMonitor}
                onChange={(e) => set({ clipboardMonitor: e.target.checked })}
              />
              Offer to download links I copy
            </label>
            <button type="button" className="ghost left" onClick={onBrowserSetup}>
              {Icon.globe} Browser extension setup…
            </button>
          </section>
          {error && <p className="error">{error}</p>}
        </div>
        <div className="win-buttons">
          <span className="spacer" />
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="primary">
            Save
          </button>
        </div>
      </form>
    </Window>
  );
}

export function BrowserSetupDialog({ onClose }: { onClose: () => void }) {
  const [dir, setDir] = useState("");
  useEffect(() => {
    invoke<string>("extension_dir").then(setDir).catch(() => {});
  }, []);

  return (
    <Window title={`Enable ${APP_NAME} in your browser`} onClose={onClose} width={540}>
      <div className="win-body">
        <p className="lead">
          Add the {APP_NAME} extension so your browser sends every download here and shows a download
          button on videos.
        </p>
        <ol className="steps">
          <li>
            Open <code>chrome://extensions</code> in Chrome or Brave, or <code>edge://extensions</code> in
            Edge.
          </li>
          <li>
            Turn on <b>Developer mode</b>.
          </li>
          <li>
            Click <b>Load unpacked</b> and choose this folder:
            <div className="path">{dir || "…"}</div>
          </li>
          <li>Pin the {APP_NAME} icon. A green dot in its popup means it's connected.</li>
        </ol>
      </div>
      <div className="win-buttons">
        <span className="spacer" />
        <button onClick={onClose}>Later</button>
        <button className="primary" disabled={!dir} onClick={() => revealItemInDir(dir)}>
          {Icon.folder} Show folder
        </button>
      </div>
    </Window>
  );
}

export function AboutDialog({ onClose }: { onClose: () => void }) {
  return (
    <Window title={`About ${APP_NAME}`} onClose={onClose} width={400}>
      <div className="win-body about">
        <Logo size={64} />
        <h2>{APP_NAME}</h2>
        <p className="muted">Advanced download manager · version {APP_VERSION}</p>
        <ul className="about-list">
          <li>Up to 32 connections per file, with dynamic segmentation</li>
          <li>Downloads never break: automatic reconnect and resume</li>
          <li>Video streams (HLS), magnet links and torrents</li>
          <li>Browser integration with a video download button</li>
        </ul>
        <p className="credit">
          Developed by <strong>{DEVELOPER}</strong>
        </p>
      </div>
      <div className="win-buttons">
        <span className="spacer" />
        <button className="primary" onClick={onClose}>
          OK
        </button>
      </div>
    </Window>
  );
}
