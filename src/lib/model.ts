export const APP_NAME = "ADM";
export const APP_VERSION = "0.1.0";
export const DEVELOPER = "Asif Khan";

export type Status = "queued" | "downloading" | "paused" | "completed" | "error";

export interface Segment {
  start: number;
  pos: number;
  end: number;
}

export interface Download {
  id: string;
  url: string;
  fileName: string;
  path: string;
  total: number | null;
  downloaded: number;
  speed: number;
  status: Status;
  error: string | null;
  resumable: boolean;
  segments: Segment[];
  lastTry: number;
  reconnecting: boolean;
  hls: boolean;
  partsTotal: number;
  partsDone: number;
  torrent: boolean;
  peers: number;
}

export interface Settings {
  connections: number;
  maxActive: number;
  speedLimitKb: number;
  downloadDir: string | null;
  clipboardMonitor: boolean;
  browserIntegration: boolean;
}

export type Category = "Compressed" | "Documents" | "Music" | "Programs" | "Video" | "General";
export type View = "all" | Category | "torrents" | "active" | "unfinished" | "finished";

const EXTENSIONS: Record<Exclude<Category, "General">, string[]> = {
  Compressed: ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", "iso", "img"],
  Documents: ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "epub", "csv"],
  Music: ["mp3", "wav", "flac", "aac", "m4a", "ogg", "wma", "opus"],
  Programs: ["exe", "msi", "dmg", "pkg", "apk", "deb", "rpm", "appimage", "torrent"],
  Video: ["mp4", "mkv", "avi", "mov", "wmv", "webm", "flv", "m4v", "ts"],
};

export function categoryOf(d: Pick<Download, "fileName" | "hls">): Category {
  if (d.hls) return "Video";
  const ext = d.fileName.split(".").pop()?.toLowerCase() ?? "";
  for (const [cat, exts] of Object.entries(EXTENSIONS)) {
    if (exts.includes(ext)) return cat as Category;
  }
  return "General";
}

export function matchesView(d: Download, view: View): boolean {
  switch (view) {
    case "all":
      return true;
    case "torrents":
      return d.torrent;
    case "active":
      return d.status === "downloading" || d.status === "queued";
    case "unfinished":
      return d.status !== "completed";
    case "finished":
      return d.status === "completed";
    default:
      return !d.torrent && categoryOf(d) === view;
  }
}

/** Links worth offering to download when they show up on the clipboard. */
export function looksDownloadable(text: string): boolean {
  if (/^magnet:\?/i.test(text)) return true;
  if (!/^https?:\/\/\S+$/i.test(text)) return false;
  const path = text.split(/[?#]/)[0].toLowerCase();
  if (path.endsWith(".m3u8")) return true;
  const ext = path.split(".").pop() ?? "";
  return Object.values(EXTENSIONS).some((exts) => exts.includes(ext));
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${Math.round(n)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 2 : v < 100 ? 1 : 0)} ${units[i]}`;
}

export function formatSecs(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function formatDate(secs: number): string {
  if (!secs) return "";
  return new Date(secs * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Real size, or for a stream an estimate from the pieces fetched so far. */
export function expectedTotal(d: Download): number | null {
  if (d.total) return d.total;
  if (d.hls && d.partsDone > 0) return (d.downloaded / d.partsDone) * d.partsTotal;
  return null;
}

export function percent(d: Download): number | null {
  if (d.status === "completed") return 100;
  if (d.hls) return d.partsTotal ? (d.partsDone / d.partsTotal) * 100 : null;
  return d.total ? (d.downloaded / d.total) * 100 : null;
}

export function timeLeft(d: Download): string {
  const total = expectedTotal(d);
  if (d.status !== "downloading" || !total || d.speed <= 0) return "";
  return formatSecs(Math.round(Math.max(0, total - d.downloaded) / d.speed));
}

export function isReconnecting(d: Download): boolean {
  return d.status === "downloading" && d.reconnecting && d.speed === 0;
}

export function statusLabel(d: Download): string {
  const pct = percent(d);
  switch (d.status) {
    case "completed":
      return "Complete";
    case "error":
      return "Failed";
    case "queued":
      return "Queued";
    case "paused":
      return pct !== null ? `Paused · ${pct.toFixed(1)}%` : "Paused";
    default:
      if (isReconnecting(d)) return "Reconnecting…";
      if (d.torrent && d.total === null) return "Finding peers…";
      return pct !== null ? `${pct.toFixed(1)}%` : "Downloading…";
  }
}

export function kindLabel(d: Download): string {
  if (d.torrent) return "Torrent";
  if (d.hls) return "HLS stream";
  return "HTTP";
}
