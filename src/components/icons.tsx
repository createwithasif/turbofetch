import { ReactNode } from "react";
import { Category } from "../lib/model";

/* Original line-icon set, drawn on a 24px grid and coloured with currentColor. */

const Line = ({ children, size = 20 }: { children: ReactNode; size?: number }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.9"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    {children}
  </svg>
);

export const Icon = {
  add: (
    <Line>
      <path d="M12 5v14M5 12h14" />
    </Line>
  ),
  play: (
    <Line>
      <path d="M8 5.5v13l10.5-6.5z" fill="currentColor" stroke="none" />
    </Line>
  ),
  pause: (
    <Line>
      <rect x="6.5" y="5.5" width="3.8" height="13" rx="1" fill="currentColor" stroke="none" />
      <rect x="13.7" y="5.5" width="3.8" height="13" rx="1" fill="currentColor" stroke="none" />
    </Line>
  ),
  stopAll: (
    <Line>
      <rect x="5" y="5" width="14" height="14" rx="3" />
      <path d="M9.5 9.5h5v5h-5z" fill="currentColor" stroke="none" />
    </Line>
  ),
  trash: (
    <Line>
      <path d="M4 7h16M9.5 7V4.5h5V7M6.5 7l.9 12.5h9.2L17.5 7M10 11v5M14 11v5" />
    </Line>
  ),
  sweep: (
    <Line>
      <path d="M4 7h11M8.5 7V4.5h4V7M6 7l.8 12.5h6M15.5 15.5l2 2 3.5-4" />
    </Line>
  ),
  gear: (
    <Line>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
    </Line>
  ),
  globe: (
    <Line>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </Line>
  ),
  info: (
    <Line>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.5M12 7.5h.01" />
    </Line>
  ),
  folder: (
    <Line>
      <path d="M3.5 7.5A2 2 0 0 1 5.5 5.5h4l2 2.2h7a2 2 0 0 1 2 2v7.8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />
    </Line>
  ),
  layers: (
    <Line>
      <path d="M12 3 3 8l9 5 9-5zM3 13l9 5 9-5" />
    </Line>
  ),
  bolt: (
    <Line>
      <path d="M13 3 5 13.5h6L10 21l8-10.5h-6z" fill="currentColor" stroke="none" />
    </Line>
  ),
  clock: (
    <Line>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7.5V12l3 2" />
    </Line>
  ),
  check: (
    <Line>
      <circle cx="12" cy="12" r="9" />
      <path d="m8 12.3 2.6 2.6L16 9.5" />
    </Line>
  ),
  download: (
    <Line>
      <path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19.5h14" />
    </Line>
  ),
  magnet: (
    <Line>
      <path d="M6 4h4v7a2 2 0 0 0 4 0V4h4v7a6 6 0 0 1-12 0z" />
      <path d="M6 8h4M14 8h4" />
    </Line>
  ),
  close: (
    <Line size={16}>
      <path d="M6 6l12 12M18 6 6 18" />
    </Line>
  ),
  search: (
    <Line size={16}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20 20-4.2-4.2" />
    </Line>
  ),
};

const CATEGORY_STYLE: Record<Category | "Torrent", { color: string; glyph: ReactNode }> = {
  Compressed: { color: "#d99a2b", glyph: <path d="M11 5h2M11 8h2M11 11h2M10.5 14h3v3h-3z" /> },
  Documents: { color: "#3b82f6", glyph: <path d="M8.5 11h7M8.5 14h7M8.5 17h4.5" /> },
  Music: { color: "#e2557f", glyph: <path d="M10 17.5V9l6-1.5v8M10 17.5a1.6 1.6 0 1 1-1.6-1.6A1.6 1.6 0 0 1 10 17.5zM16 15.5a1.6 1.6 0 1 1-1.6-1.6 1.6 1.6 0 0 1 1.6 1.6z" /> },
  Programs: { color: "#64748b", glyph: <path d="M8 10.5h8v6H8zM10 18.5h4" /> },
  Video: { color: "#8b5cf6", glyph: <path d="M10 10v6.5l5.5-3.25z" /> },
  General: { color: "#94a3b8", glyph: <path d="M9 13h6" /> },
  Torrent: { color: "#10b981", glyph: <path d="M12 9v6.5M9.5 13l2.5 2.5 2.5-2.5" /> },
};

/** Document-shaped badge coloured by file type. */
export function FileBadge({ cat, size = 22 }: { cat: Category | "Torrent"; size?: number }) {
  const { color, glyph } = CATEGORY_STYLE[cat];
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M6 2.5h8.5L19 7v13.5a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-17a1 1 0 0 1 1-1z" fill={color} />
      <path d="M14.5 2.5V7H19" fill="rgba(255,255,255,.35)" />
      <g stroke="#fff" strokeWidth="1.6" fill="none" strokeLinecap="round" strokeLinejoin="round">
        {glyph}
      </g>
    </svg>
  );
}

export function categoryColor(cat: Category | "Torrent"): string {
  return CATEGORY_STYLE[cat].color;
}

/** The ADM mark: an "A" with a download arrow for its crossbar (see branding/adm-logo.svg). */
export function Logo({ size = 30 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="80 80 864 864" aria-hidden="true">
      <defs>
        <linearGradient id="adm-tile" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#4f8cff" />
          <stop offset="0.55" stopColor="#5b5cf6" />
          <stop offset="1" stopColor="#8b3dff" />
        </linearGradient>
        <linearGradient id="adm-arrow" gradientUnits="userSpaceOnUse" x1="0" y1="452" x2="0" y2="700">
          <stop offset="0" stopColor="#7cf3ff" />
          <stop offset="1" stopColor="#2ee6a6" />
        </linearGradient>
      </defs>
      <rect x="80" y="80" width="864" height="864" rx="200" fill="url(#adm-tile)" />
      <g fill="none" strokeLinecap="round" strokeLinejoin="round">
        <path d="M292 790 L512 236 L732 790" stroke="#fff" strokeWidth="112" />
        <path d="M512 452 V690M418 600 L512 694 L606 600" stroke="url(#adm-arrow)" strokeWidth="78" />
      </g>
    </svg>
  );
}
