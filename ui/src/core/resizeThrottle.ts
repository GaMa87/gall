// Lightweight frame-coalescing and trailing-debounce helpers for live window resizing.

export interface RafScheduler {
  requestFrame: (cb: () => void) => number;
  cancelFrame: (id: number) => void;
}

export interface TimerScheduler {
  setTimer: (cb: () => void, ms: number) => ReturnType<typeof setTimeout>;
  clearTimer: (id: ReturnType<typeof setTimeout>) => void;
}

const defaultRaf: RafScheduler = {
  requestFrame: (cb) => requestAnimationFrame(cb),
  cancelFrame: (id) => cancelAnimationFrame(id),
};

const defaultTimers: TimerScheduler = {
  setTimer: (cb, ms) => setTimeout(cb, ms),
  clearTimer: (id) => clearTimeout(id),
};

/**
 * Coalesces rapid calls (e.g. ResizeObserver or window resize bursts) so `run` executes
 * at most once per animation frame.
 */
export function createRafCoalescer(run: () => void, raf: RafScheduler = defaultRaf) {
  let frameId: number | null = null;
  const invoke = () => {
    frameId = null;
    run();
  };
  return {
    schedule() {
      if (frameId !== null) return;
      frameId = raf.requestFrame(invoke);
    },
    flush() {
      if (frameId === null) return;
      raf.cancelFrame(frameId);
      frameId = null;
      run();
    },
    cancel() {
      if (frameId === null) return;
      raf.cancelFrame(frameId);
      frameId = null;
    },
    pending: () => frameId !== null,
  };
}

/**
 * Runs `run` once after `delayMs` elapse with no further triggers. Used for discrete
 * post-resize checks such as querying native maximized state over IPC.
 */
export function createTrailingDebounce(
  run: () => void,
  delayMs: number,
  timers: TimerScheduler = defaultTimers,
) {
  let timerId: ReturnType<typeof setTimeout> | null = null;
  return {
    trigger() {
      if (timerId !== null) timers.clearTimer(timerId);
      timerId = timers.setTimer(() => {
        timerId = null;
        run();
      }, delayMs);
    },
    cancel() {
      if (timerId === null) return;
      timers.clearTimer(timerId);
      timerId = null;
    },
    pending: () => timerId !== null,
  };
}

/**
 * Tracks whether a resize gesture is currently active so CSS layout transitions
 * (such as Miller column width transitions) pause while dragging and resume after `settleMs`.
 */
export function createResizeActivityTracker(
  onChange: (active: boolean) => void,
  settleMs = 150,
  timers: TimerScheduler = defaultTimers,
) {
  let active = false;
  const settle = createTrailingDebounce(() => {
    if (!active) return;
    active = false;
    onChange(false);
  }, settleMs, timers);

  return {
    bump() {
      if (!active) {
        active = true;
        onChange(true);
      }
      settle.trigger();
    },
    cancel() {
      settle.cancel();
      if (active) {
        active = false;
        onChange(false);
      }
    },
    isActive: () => active,
  };
}

/**
 * CSS `flex` value for the primary split pane in `Workspace` so window resizes
 * proportion both browsers natively in CSS without React re-renders.
 */
export function workspaceSplitFlex(ratio: number): string {
  const clamped = Math.max(0, Math.min(1000, Number.isFinite(ratio) ? ratio : 500));
  const pct = (clamped / 10).toFixed(4).replace(/\.?0+$/, "");
  return `0 0 clamp(40px, ${pct}%, calc(100% - 40px))`;
}

/**
 * Bitmask of the column-visibility width thresholds in `FolderPane` list/details view.
 * Identical masks mean width changes do not add or hide any column.
 */
export function folderPaneColumnMask(w: number, fontSize: number, compact: boolean): number {
  const charW = Math.max(1, fontSize * 0.6);
  const chars = w / charW;
  let mask = 0;
  if (chars > (compact ? 30 : 26)) mask |= 1 << 0;
  if (chars > (compact ? 40 : 36)) mask |= 1 << 1;
  if (chars > (compact ? 45 : 41)) mask |= 1 << 2;
  if (chars > (compact ? 52 : 48)) mask |= 1 << 3;
  if (chars > 58) mask |= 1 << 4;
  if (chars > (compact ? 66 : 62)) mask |= 1 << 5;
  if (chars > 66) mask |= 1 << 6;
  if (chars > 76) mask |= 1 << 7;
  return mask;
}

/**
 * Rounds list-view height up to 2-row steps so `windowOf` (which carries a 4-row overscan)
 * always covers the visible viewport while skipping React re-renders on small vertical drags.
 */
export function quantizeListHeight(h: number, itemH: number): number {
  if (h <= 0) return 0;
  const step = Math.max(1, Math.round(itemH * 2));
  return Math.ceil(h / step) * step;
}

export function folderPaneSizeChanged(
  prev: { w: number; h: number; scrollbarW: number },
  next: { w: number; h: number; scrollbarW: number },
  opts: { isGrid: boolean; fontSize: number; compact: boolean; itemH: number },
): boolean {
  if (prev.scrollbarW !== next.scrollbarW) return true;
  if (opts.isGrid) return prev.w !== next.w || prev.h !== next.h;
  return (
    folderPaneColumnMask(prev.w, opts.fontSize, opts.compact)
      !== folderPaneColumnMask(next.w, opts.fontSize, opts.compact)
    || quantizeListHeight(prev.h, opts.itemH) !== quantizeListHeight(next.h, opts.itemH)
  );
}
