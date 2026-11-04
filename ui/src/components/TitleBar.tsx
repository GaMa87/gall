// Custom window title bar (the native one is disabled in tauri.conf.json): brand mark,
// wordmark, the active folder, and window controls. Empty areas drag the window.
import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Layout } from "../api/types";
import { useStore } from "../state/store";
import { copyPaths, openOverlay, setLayout } from "../state/actions";
import { useKeyLabels } from "../state/keyLabels";
import { tildify } from "../core/path";
import { Icon } from "./Icon";
import { PanelMap } from "./PanelMap";

const LAYOUTS: { id: Layout; icon: string; label: string }[] = [
  { id: "standard", icon: "layoutStd", label: "Side by side: two browsers, Miller columns on top" },
  { id: "columns", icon: "layoutCols", label: "Stacked: two browsers, the list as the last column" },
  { id: "classic", icon: "layoutClassic", label: "Single: one browser, more room" },
];

/** The three layouts as one click each, next to the window controls. */
function LayoutSwitch() {
  const layout = useStore((s) => s.session?.layout ?? "standard");
  const next = useKeyLabels().action("layoutNext");
  return (
    <div className="tb-layouts" role="radiogroup" aria-label="Layout">
      {LAYOUTS.map((l) => (
        <button key={l.id} role="radio" aria-checked={layout === l.id} className={`tb-layout ${layout === l.id ? "on" : ""}`}
          title={`${l.label}${next ? ` (${next} cycles)` : ""}`} onClick={() => setLayout(l.id)}>
          <Icon name={l.icon} size={15} />
        </button>
      ))}
    </div>
  );
}

const win = (() => {
  try {
    return getCurrentWindow();
  } catch {
    return null; // plain browser (vite dev without Tauri)
  }
})();

/** The app icon (assets/icon.svg) as inline SVG without its background: a lowercase "g". */
export function Logo({ size = 18 }: { size?: number }) {
  return (
    <svg className="logo" width={size} height={size} viewBox="0 0 24 24" aria-hidden>
      <defs>
        <linearGradient id="lg-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#7aa7f5" />
          <stop offset="1" stopColor="#4f7fd9" />
        </linearGradient>
        <linearGradient id="lg-b" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#ffd06b" />
          <stop offset="1" stopColor="#f0a92e" />
        </linearGradient>
      </defs>
      <circle cx="12" cy="8.6" r="6" fill="none" stroke="url(#lg-a)" strokeWidth="3.4" />
      <path d="M18 2.7V15A6.35 6.35 0 0 1 11.65 21.3H8.5" fill="none" stroke="url(#lg-b)" strokeWidth="3.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function TitleBar() {
  const session = useStore((s) => s.session);
  const home = useStore((s) => s.info?.home ?? "");
  const [maximized, setMaximized] = useState(false);
  const settingsKeys = useKeyLabels().action("settings");

  const br = session?.browsers[session.activeBrowser];
  const path = br?.tabs[br.active]?.path ?? "";
  const shown = tildify(path, home);

  useEffect(() => {
    if (!win) return;
    let un: (() => void) | undefined;
    const sync = () => win.isMaximized().then(setMaximized).catch(() => {});
    sync();
    win.onResized(sync).then((f) => (un = f)).catch(() => {});
    return () => un?.();
  }, []);

  useEffect(() => {
    document.title = shown ? `${shown} — Gall` : "Gall";
  }, [shown]);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="tb-brand" data-tauri-drag-region>
        <button className="tb-logo" title={`Settings${settingsKeys ? ` (${settingsKeys})` : ""}`} onClick={() => openOverlay({ type: "settings" })}>
          <Logo />
        </button>
        <span className="wordmark" data-tauri-drag-region>Gall</span>
      </div>
      <div className="tb-path-wrap" data-tauri-drag-region>
        {path && <button className="tb-path" title={`${path}\nClick to copy`} onClick={() => copyPaths([], path)}>{shown}</button>}
      </div>
      <PanelMap />
      <LayoutSwitch />
      <div className="tb-controls">
        <button className="tb-btn" title="Minimize" onClick={() => win?.minimize()}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1 5h8" /></svg>
        </button>
        <button className="tb-btn" title={maximized ? "Restore" : "Maximize"} onClick={() => win?.toggleMaximize()}>
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 3.5h5v5h-5zM3.5 3.5V1.5h5v5h-2" /></svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 1.5h7v7h-7z" /></svg>
          )}
        </button>
        <button className="tb-btn close" title="Close" onClick={() => win?.close()}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 1.5l7 7M8.5 1.5l-7 7" /></svg>
        </button>
      </div>
    </header>
  );
}
