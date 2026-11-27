// Desktop integration state: Omarchy (live theme / rounding / font / Glassy / tiling WM) and the
// window size that drives the adaptive layout and the narrow-window sidebar auto-hide.
import { api, events } from "../api/ipc";
import { resolveAdaptiveLayout, sidebarAutoHidden, workspaceWidth } from "../core/adaptiveLayout";
import { alignedRowHeight, devicePx, paneFrameDevicePx, snapCss } from "../core/pixelGrid";
import { createRafCoalescer, createResizeActivityTracker } from "../core/resizeThrottle";
import { getState, setState, setViewportSilent, sidebarShown } from "./store";

/** Self-test only: a virtual window size for compositors that ignore client resizes (tiling WMs). */
let viewportOverride: { w: number; h: number } | null = null;
export function setViewportOverride(v: { w: number; h: number } | null) {
  viewportOverride = v;
  syncViewport();
}

/** The sidebar's width in px: the saved splitter size (see `useStoredSize("sidebar")`). */
export function sidebarWidth(): number {
  try {
    const raw = typeof localStorage !== "undefined" ? localStorage.getItem("size:sidebar") : null;
    const n = Number(raw);
    return Number.isFinite(n) && n > 0 ? n : 230;
  } catch {
    return 230;
  }
}

/** Recomputes the viewport and the sidebar auto-hide from the window size. */
export function syncViewport() {
  const s = getState();
  const w = viewportOverride?.w ?? window.innerWidth, h = viewportOverride?.h ?? window.innerHeight;
  const enabled = s.settings?.adaptiveLayout ?? true;
  const auto = enabled && sidebarAutoHidden(w, s.sidebarAuto);
  const patch: Partial<typeof s> = {};
  setViewportSilent(w, h);
  // The sidebar may have just auto-hidden: judge the panes by the width they will really get.
  const shown = sidebarShown({ sidebarVisible: s.sidebarVisible, sidebarAuto: auto, sidebarOverride: auto !== s.sidebarAuto ? null : s.sidebarOverride });
  const workspaceW = workspaceWidth(w, shown, sidebarWidth());
  const adaptiveMode = enabled ? resolveAdaptiveLayout(w, h, s.adaptiveMode, { workspaceW }) : null;
  if (adaptiveMode !== s.adaptiveMode) patch.adaptiveMode = adaptiveMode;
  if (auto !== s.sidebarAuto) {
    patch.sidebarAuto = auto;
    // A manual toggle only lasts for the narrow episode it was made in.
    patch.sidebarOverride = null;
  }
  if (Object.keys(patch).length) setState(patch);
}

/**
 * Starts the window-size sync and the Omarchy state feed. Returns the cleanup that removes
 * the listeners (also correct under React StrictMode's double effect).
 */
export function startDesktopSync(): () => void {
  let disposed = false;
  let unlisten: (() => void) | null = null;
  syncViewport();
  const coalesce = createRafCoalescer(syncViewport);
  const activity = createResizeActivityTracker((active) => {
    document.documentElement.classList.toggle("resizing", active);
  }, 150);
  const onResize = () => {
    activity.bump();
    coalesce.schedule();
  };
  window.addEventListener("resize", onResize);
  // A change that arrives while the initial read is in flight wins over that (older) read.
  let live = false;
  events.omarchyStateChanged((o) => { live = true; setState({ omarchy: o }); })
    .then((u) => { if (disposed) u(); else unlisten = u; })
    .catch(() => {});
  api.omarchyStateGet()
    .then((o) => { if (!live && !disposed) setState({ omarchy: o }); })
    .catch(() => {});
  return () => {
    disposed = true;
    window.removeEventListener("resize", onResize);
    coalesce.cancel();
    activity.cancel();
    unlisten?.();
  };
}

/** CSS px spacings of the sidebar, title bar, tab bar and pane header that follow the device-pixel grid (`--sp-N`). */
const SIDEBAR_SPACINGS = [1, 2, 3, 4, 5, 6, 8, 10, 14, 15, 26, 34, 38, 54];

/**
 * Sets `--line-h`, the line height of all text, to the font's natural line height rounded to a
 * line whose box (plus the 4 px padding above and below in sidebar rows) lets a 16 px icon sit on whole device
 * pixels. `line-height: normal` is fractional (e.g. 17.5 px), which put every icon on a
 * half pixel and blurred its 1 px strokes.
 */
export function syncLineHeight(dpr: number) {
  const probe = document.createElement("span");
  probe.textContent = "Mg";
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap;line-height:normal;";
  document.body.appendChild(probe);
  const natural = probe.getBoundingClientRect().height;
  probe.remove();
  if (!natural) return;
  // Sidebar spacing (`--sp-N`, used by the sidebar rules in app.css) is snapped to whole device
  // pixels, so at 125 % / 150 % the rows stack on the grid instead of on 0.25 / 0.5 px offsets.
  // At whole scales the values are the plain N px.
  const root = document.documentElement.style;
  for (const n of SIDEBAR_SPACINGS) root.setProperty(`--sp-${n}`, `${snapCss(n, dpr)}px`);
  // One device pixel for hairline rings (box-shadow rings are not snapped by the browser) and the
  // frame of the active pane, whose rounded corners need to be as heavy as its straight edges.
  root.setProperty("--hair", `${devicePx(1, dpr)}px`);
  root.setProperty("--pane-frame", `${devicePx(paneFrameDevicePx(dpr), dpr)}px`);
  // Sidebar rows: 4 px (snapped) padding top and bottom around the line, no border.
  const pad = 2 * snapCss(4, dpr);
  const row = alignedRowHeight(Math.ceil(natural * dpr) / dpr + pad, 16, dpr, 0);
  root.setProperty("--line-h", `${row - pad}px`);
  // The pane header's content (title, pill buttons) has em-based heights: at a fractional scale its
  // natural height is rounded up to whole device pixels so everything below it stays on the grid.
  root.removeProperty("--fp-head-h");
  const head = document.querySelector(".fp-head");
  if (head && !Number.isInteger(dpr)) {
    root.setProperty("--fp-head-h", `${Math.ceil(head.getBoundingClientRect().height * dpr - 1e-6) / dpr}px`);
  }
}
