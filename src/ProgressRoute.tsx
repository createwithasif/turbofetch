import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ProgressWindow } from "./components/ProgressWindow";
import { Download, percent } from "./lib/model";

const HISTORY = 60;

/** Content of a standalone progress window: follows one download by id. */
export default function ProgressRoute({ id }: { id: string }) {
  const [d, setD] = useState<Download | null>(null);
  const [history, setHistory] = useState<number[]>([]);
  const seen = useRef(false);

  useEffect(() => {
    const win = getCurrentWindow();
    const update = (list: Download[]) => {
      const found = list.find((x) => x.id === id) ?? null;
      if (found) seen.current = true;
      // The download was removed from the main window: nothing left to show.
      else if (seen.current) win.close();
      setD(found);
      if (found) setHistory((h) => [...h.slice(-(HISTORY - 1)), found.speed]);
    };
    invoke<Download[]>("list_downloads").then(update);
    const unlisten = listen<Download[]>("downloads", (e) => update(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, [id]);

  // Title shows live progress, like "42% movie.mp4".
  useEffect(() => {
    if (!d) return;
    const pct = percent(d);
    const title =
      d.status === "completed" ? `Download complete · ${d.fileName}` : `${pct !== null ? `${pct.toFixed(0)}% ` : ""}${d.fileName}`;
    getCurrentWindow().setTitle(title);
  }, [d?.status, d?.fileName, d && Math.floor(percent(d) ?? 0)]);

  if (!d) return <div className="standalone-loading">Connecting…</div>;
  return <ProgressWindow d={d} history={history} standalone onClose={() => getCurrentWindow().close()} />;
}
