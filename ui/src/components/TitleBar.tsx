// Custom window title bar (the native one is disabled in tauri.conf.json): brand mark,
// wordmark, the active folder, and window controls. Empty areas drag the window.
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { bindTitleBarGestures } from "../core/titleBarDrag";
import { createTrailingDebounce } from "../core/resizeThrottle";
import { api } from "../api/ipc";
import type { Layout } from "../api/types";
import { windowButtonsShown } from "../core/adaptiveLayout";
import { t, tt } from "../i18n";
import { useStore } from "../state/store";
import { copyPaths, openOverlay, setLayout } from "../state/actions";
import { useKeyLabels } from "../state/keyLabels";
import { tildify } from "../core/path";
import { IS_MAC } from "../core/keymap";
import { Icon } from "./Icon";
import { PanelMap } from "./PanelMap";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

const LAYOUTS: { id: Layout; icon: string; label: string }[] = [
  { id: "standard", icon: "layoutStd", label: "layout.standard" },
  { id: "columns", icon: "layoutCols", label: "layout.columns" },
  { id: "classic", icon: "layoutClassic", label: "layout.classic" },
];

/** The three layouts as one click each, next to the window controls. */
function LayoutSwitch() {
  const layout = useStore((s) => s.session?.layout ?? "standard");
  const adaptive = useStore((s) => s.adaptiveMode);
  const next = useKeyLabels().action("layoutNext");
  const adaptiveNote = adaptive
    ? ` ${t("layout.adaptive", { mode: t(`layout.mode.${adaptive}`) })}`
    : "";
  return (
    <div className="tb-layouts" role="radiogroup" aria-label={tt("Layout")}>
      {LAYOUTS.map((l) => (
        <button key={l.id} role="radio" aria-checked={layout === l.id} className={`tb-layout ${layout === l.id ? "on" : ""}`}
          title={`${t(l.label)}${l.id === "standard" ? adaptiveNote : ""}${next ? ` ${t("layout.cycles", { keys: next })}` : ""}`} onClick={() => setLayout(l.id)}>
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
  const root = useRef<HTMLElement>(null);
  const session = useStore((s) => s.session);
  const home = useStore((s) => s.info?.home ?? "");
  const [maximized, setMaximized] = useState(false);
  const settingsKeys = useKeyLabels().action("settings");
  const showButtons = useStore((s) =>
    windowButtonsShown(s.settings?.windowButtons, s.omarchy?.tilingWm ?? false, s.omarchy?.floatingWorkspace ?? false),
  );

  const br = session?.browsers[session.activeBrowser];
  const path = br?.tabs[br.active]?.path ?? "";
  const shown = tildify(path, home);

  const floatingWorkspace = useStore((s) => s.omarchy?.floatingWorkspace ?? false);

  useEffect(() => {
    if (!win) return;
    let un: (() => void) | undefined;
    let disposed = false;
    let inFlight = false;
    const sync = () => {
      if (disposed || inFlight) return;
      inFlight = true;
      api.windowIsMaximized()
        .catch(() => win.isMaximized())
        .then((next) => { if (!disposed) setMaximized(next); })
        .catch(() => {})
        .finally(() => { inFlight = false; });
    };
    const debouncedSync = createTrailingDebounce(sync, 120);
    sync();
    win.onResized(() => debouncedSync.trigger()).then((f) => {
      if (disposed) f();
      else un = f;
    }).catch(() => {});
    return () => {
      disposed = true;
      debouncedSync.cancel();
      un?.();
    };
  }, [floatingWorkspace]);

  const toggleMaximize = () =>
    api.windowToggleMaximize()
      .then((next) => setMaximized(next))
      .catch(() => win?.toggleMaximize());

  useEffect(() => {
    const element = root.current;
    if (!element || !win) return;
    return bindTitleBarGestures(element, {
      startDragging: () => win.startDragging(),
      toggleMaximize,
    });
  }, []);

  useEffect(() => {
    document.title = shown ? `${shown} — Gall` : "Gall";
  }, [shown]);

  return (
    <header ref={root} className={`titlebar${IS_MAC ? " mac" : ""}`} data-tauri-drag-region="false">
      <div className="tb-left">
        {IS_MAC && showButtons && (
          // macOS: the window has no native frame, so the traffic lights are drawn on the left.
          <div className="tb-lights">
            <button className="tb-light close" title={t("window.close")} aria-label={t("window.close")} onClick={() => win?.close()} />
            <button className="tb-light min" title={t("window.minimize")} aria-label={t("window.minimize")} onClick={() => win?.minimize()} />
            <button className="tb-light zoom" title={maximized ? t("window.restore") : t("window.zoom")} aria-label={t("window.zoom")} onClick={() => win?.toggleMaximize()} />
          </div>
        )}
        <div className="tb-brand">
          <button className="tb-logo" title={settingsKeys ? t("sidebar.settings", { keys: settingsKeys }) : t("titlebar.settings")} onClick={() => openOverlay({ type: "settings" })}>
            <Logo />
          </button>
          <span className="wordmark">{tt("Gall")}</span>
        </div>
        <WorkspaceSwitcher />
        <div className="tb-path-wrap">
          {path && <button className="tb-path" title={`${path}\nClick to copy`} onClick={() => copyPaths([], path)}>{shown}</button>}
        </div>
      </div>
      <PanelMap />
      <LayoutSwitch />
      {!IS_MAC && showButtons && <div className="tb-controls">
        <button className="tb-btn" title={t("window.minimize")} onClick={() => { void api.windowMinimize().catch(() => win?.minimize()); }}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1 5h8" /></svg>
        </button>
        <button className="tb-btn" title={maximized ? t("window.restore") : t("window.maximize")} onClick={() => { void toggleMaximize(); }}>
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 3.5h5v5h-5zM3.5 3.5V1.5h5v5h-2" /></svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 1.5h7v7h-7z" /></svg>
          )}
        </button>
        <button className="tb-btn close" title={t("window.close")} onClick={() => win?.close()}>
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 1.5l7 7M8.5 1.5l-7 7" /></svg>
        </button>
      </div>}
    </header>
  );
}
