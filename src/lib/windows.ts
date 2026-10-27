import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

/** Opens (or focuses) the separate per-download progress window, IDM style. */
export async function openProgressWindow(id: string, title: string) {
  const label = `progress-${id.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.unminimize();
    await existing.setFocus();
    return;
  }
  new WebviewWindow(label, {
    url: `index.html#progress=${encodeURIComponent(id)}`,
    title,
    width: 600,
    height: 560,
    minWidth: 480,
    minHeight: 420,
    center: true,
    focus: true,
  });
}
