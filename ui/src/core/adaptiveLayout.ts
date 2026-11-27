// Adaptive pane layout: how many panes fit a window and how they are arranged. Pure and
// deterministic so the window-size thresholds (with hysteresis against flicker while a tiling
// window manager resizes) can be unit tested.

export type AdaptiveMode = "horizontal" | "vertical" | "single";

/** Width from which two panes sit side by side. */
export const WIDE_MIN = 920;
/** Below this width a single pane is always used. */
export const COMPACT_W = 480;
/** Height from which a narrow window stacks two panes. */
export const TALL_MIN = 620;
/** A narrow window is "tall" when its height is at least this fraction of its width. */
export const TALL_RATIO = 0.85;
/** Dead band (px) around every threshold: the previous mode wins inside it. */
export const HYSTERESIS = 36;
/** The sidebar hides by itself below this window width. */
export const SIDEBAR_AUTO_HIDE_W = 820;

export interface AdaptiveOpts {
  /** Dead band in px (default {@link HYSTERESIS}). */
  hysteresis?: number;
  /**
   * Width left for the panes: the window width minus the visible sidebar. The side-by-side
   * threshold is checked against it, so two panes never get crushed beside an open sidebar.
   * Defaults to the window width.
   */
  workspaceW?: number;
}

/** The width available to the panes: the window minus the sidebar when it is shown. */
export function workspaceWidth(w: number, sidebarShown: boolean, sidebarW: number): number {
  return sidebarShown ? Math.max(0, w - sidebarW) : w;
}

/** The mode with no hysteresis: the pure thresholds. */
export function rawAdaptiveMode(w: number, h: number, workspaceW: number = w): AdaptiveMode {
  if (w < COMPACT_W) return "single";
  if (workspaceW >= WIDE_MIN) return "horizontal";
  if (h < TALL_MIN) return "single";
  return h >= w * TALL_RATIO ? "vertical" : "single";
}

/**
 * Picks the layout for a `w` × `h` window. `prev` is the mode in use so far (null at start).
 * Each mode keeps its window while the size is within `hysteresis` px of its own thresholds, so a
 * window dragged across 920 px (or tiled by a window manager that resizes in steps) does not flip
 * back and forth. Outside that band the plain thresholds decide.
 */
export function resolveAdaptiveLayout(w: number, h: number, prev: AdaptiveMode | null, opts: AdaptiveOpts = {}): AdaptiveMode {
  const hy = opts.hysteresis ?? HYSTERESIS;
  const ww = opts.workspaceW ?? w;
  if (prev === "horizontal" && ww >= WIDE_MIN - hy && w >= COMPACT_W) return "horizontal";
  if (prev === "vertical" && w >= COMPACT_W - hy && ww < WIDE_MIN + hy && h >= TALL_MIN - hy && h >= w * TALL_RATIO - hy) return "vertical";
  if (prev === "single" && (w < COMPACT_W + hy || (ww < WIDE_MIN + hy && (h < TALL_MIN + hy || h < w * TALL_RATIO + hy)))) return "single";
  return rawAdaptiveMode(w, h, ww);
}

/** The sidebar is hidden automatically: the window is narrower than the threshold (with hysteresis). */
export function sidebarAutoHidden(w: number, prevHidden: boolean, hysteresis = HYSTERESIS): boolean {
  return prevHidden ? w < SIDEBAR_AUTO_HIDE_W + hysteresis : w < SIDEBAR_AUTO_HIDE_W - hysteresis;
}

/**
 * Minimize / maximize / close buttons: "auto" hides them on tiled workspaces of a tiling WM,
 * but shows them on classic desktops and on Omarchy floating workspaces (where Gall suppresses
 * Hyprland's extra `hyprbars` row so controls stay in Gall's single title bar).
 */
export function windowButtonsShown(
  mode: "auto" | "always" | "never" | undefined,
  tilingWm: boolean,
  floatingWorkspace = false,
): boolean {
  if (mode === "always") return true;
  if (mode === "never") return false;
  return !tilingWm || floatingWorkspace;
}
