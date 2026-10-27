import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  categoryOf,
  Download,
  expectedTotal,
  formatBytes,
  isReconnecting,
  kindLabel,
  percent,
  statusLabel,
  timeLeft,
} from "../lib/model";
import { FileBadge, Icon } from "./icons";

/** Live speed graph from the last minute of samples. */
function Sparkline({ samples }: { samples: number[] }) {
  const w = 480;
  const h = 56;
  const max = Math.max(...samples, 1);
  const pts = samples.map((v, i) => [
    (i / Math.max(samples.length - 1, 1)) * w,
    h - 4 - (v / max) * (h - 10),
  ]);
  const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join("");
  return (
    <svg className="spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id="spark-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--accent)" stopOpacity="0.35" />
          <stop offset="1" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>
      {pts.length > 1 && (
        <>
          <path d={`${line}L${w},${h}L0,${h}Z`} fill="url(#spark-fill)" />
          <path d={line} fill="none" stroke="var(--accent)" strokeWidth="2" vectorEffect="non-scaling-stroke" />
        </>
      )}
    </svg>
  );
}

/** Which byte ranges each connection has filled, like a disk-defrag map. */
export function SegmentMap({ d }: { d: Download }) {
  const total = d.hls || d.torrent ? null : d.total;
  return (
    <div className="segmap">
      {d.status === "completed" || !total ? (
        <div className="segfill" style={{ left: 0, width: `${percent(d) ?? 0}%` }} />
      ) : (
        d.segments.map((s, i) => (
          <div
            key={i}
            className={`segfill ${s.pos < s.end && d.status === "downloading" ? "live" : ""}`}
            style={{
              left: `${(s.start / total) * 100}%`,
              width: `${((Math.min(s.pos, s.end) - s.start) / total) * 100}%`,
            }}
          />
        ))
      )}
    </div>
  );
}

export function ProgressWindow({
  d,
  history,
  onClose,
  standalone = false,
}: {
  d: Download;
  history: number[];
  onClose: () => void;
  /** Rendered as its own OS window (like IDM) instead of an in-app overlay. */
  standalone?: boolean;
}) {
  const [tab, setTab] = useState<"status" | "connections">("status");
  const [details, setDetails] = useState(true);
  const pct = percent(d);
  const total = expectedTotal(d);
  const done = d.status === "completed";
  const active = d.segments.filter((s) => s.pos < s.end);
  const cat = d.torrent ? "Torrent" : categoryOf(d);

  const statusLine = isReconnecting(d)
    ? `Connection lost — reconnecting automatically${d.error ? ` (${d.error})` : ""}`
    : d.status === "downloading"
      ? d.torrent && !d.total
        ? "Fetching torrent info from peers…"
        : "Receiving data…"
      : d.status === "error"
        ? (d.error ?? "Failed")
        : statusLabel(d);

  const facts: [string, string][] = [
    ["File size", total ? `${d.total ? "" : "≈ "}${formatBytes(total)}` : "Unknown"],
    ["Downloaded", `${formatBytes(d.downloaded)}${pct !== null ? `  (${pct.toFixed(2)}%)` : ""}`],
    ["Transfer rate", d.status === "downloading" ? `${formatBytes(d.speed)}/s` : "—"],
    ["Time left", timeLeft(d) || "—"],
    ["Resume capability", d.resumable || d.hls || d.torrent ? "Yes" : "No"],
    d.torrent
      ? ["Peers", String(d.peers)]
      : d.hls
        ? ["Stream pieces", `${d.partsDone} of ${d.partsTotal || "?"}`]
        : ["Connections", d.status === "downloading" ? `${active.length} active` : "—"],
  ];

  const content = (
      <div
        className={`window progress-window ${standalone ? "standalone" : ""}`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {!standalone && (
          <div className="win-title">
            <span>{done ? "Download complete" : `${pct !== null ? `${pct.toFixed(0)}% ` : ""}${d.fileName}`}</span>
            <button className="icon-btn" onClick={onClose} aria-label="Close">
              {Icon.close}
            </button>
          </div>
        )}

        <div className="tabs">
          <button className={tab === "status" ? "on" : ""} onClick={() => setTab("status")}>
            Download status
          </button>
          {!d.hls && !d.torrent && (
            <button className={tab === "connections" ? "on" : ""} onClick={() => setTab("connections")}>
              Connections
            </button>
          )}
        </div>

        {tab === "status" ? (
          <div className="pw-body">
            <div className="pw-head">
              <FileBadge cat={cat} size={42} />
              <div className="pw-name">
                <strong title={d.fileName}>{d.fileName}</strong>
                <span title={d.url}>{d.url}</span>
              </div>
              <span className="kind">{kindLabel(d)}</span>
            </div>

            <div className={`pw-status ${d.status} ${isReconnecting(d) ? "warn" : ""}`}>{statusLine}</div>

            <div className="pw-progress">
              <div className="pw-pct">{pct !== null ? `${pct.toFixed(1)}%` : "…"}</div>
              <div className={`bigbar ${d.status}`}>
                <div style={{ width: `${pct ?? 0}%` }} />
              </div>
            </div>

            <div className="facts">
              {facts.map(([k, v]) => (
                <div key={k} className="fact">
                  <span>{k}</span>
                  <strong>{v}</strong>
                </div>
              ))}
            </div>

            {details && d.status === "downloading" && (
              <div className="pw-graph">
                <div className="caption">Speed · last minute</div>
                <Sparkline samples={history} />
              </div>
            )}
            {details && !d.hls && !d.torrent && (
              <div>
                <div className="caption">Start positions and download progress by connections</div>
                <SegmentMap d={d} />
              </div>
            )}
          </div>
        ) : (
          <div className="pw-body">
            <div className="conn-table">
              <table>
                <thead>
                  <tr>
                    <th>N.</th>
                    <th>Range</th>
                    <th>Downloaded</th>
                    <th>Info</th>
                  </tr>
                </thead>
                <tbody>
                  {d.segments.map((s, i) => (
                    <tr key={i}>
                      <td>{i + 1}</td>
                      <td>{formatBytes(s.start)}</td>
                      <td>{formatBytes(Math.min(s.pos, s.end) - s.start)}</td>
                      <td className={s.pos >= s.end ? "ok" : ""}>
                        {s.pos >= s.end ? "Finished" : d.status === "downloading" ? "Receiving data…" : "Stopped"}
                      </td>
                    </tr>
                  ))}
                  {d.segments.length === 0 && (
                    <tr>
                      <td colSpan={4} className="muted">
                        No connections yet.
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          </div>
        )}

        <div className="win-buttons">
          {tab === "status" && (
            <button className="ghost" onClick={() => setDetails(!details)}>
              {details ? "« Hide details" : "Show details »"}
            </button>
          )}
          <span className="spacer" />
          {d.status === "downloading" && (
            <button onClick={() => invoke("pause_download", { id: d.id })}>{Icon.pause} Pause</button>
          )}
          {(d.status === "paused" || d.status === "error") && (
            <button onClick={() => invoke("resume_download", { id: d.id })}>{Icon.play} Resume</button>
          )}
          {done ? (
            <>
              <button onClick={() => revealItemInDir(d.path)}>{Icon.folder} Open folder</button>
              <button className="primary" onClick={() => openPath(d.path).catch(() => revealItemInDir(d.path))}>
                Open
              </button>
            </>
          ) : (
            <button
              className="danger"
              onClick={() => {
                invoke("remove_download", { id: d.id, deleteFile: true });
                onClose();
              }}
            >
              Cancel
            </button>
          )}
        </div>
      </div>
  );

  if (standalone) return content;
  return (
    <div className="overlay" onMouseDown={onClose}>
      {content}
    </div>
  );
}
