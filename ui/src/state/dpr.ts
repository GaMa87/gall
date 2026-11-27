// The display scale (window.devicePixelRatio) as React state: it changes when the window moves to
// another monitor, the system scale changes or the webview is zoomed.
// Shared across all consumers and guarded against same-DPR resize bursts so window resizing
// never tears down and recreates `matchMedia` listeners when the scale has not changed.
import { useSyncExternalStore } from "react";

export interface ResolutionQuery {
  addEventListener: (type: "change", fn: () => void) => void;
  removeEventListener: (type: "change", fn: () => void) => void;
}

export interface DprEnvironment {
  getDpr: () => number;
  matchResolution: (dpr: number) => ResolutionQuery;
  addResizeListener: (fn: () => void) => void;
  removeResizeListener: (fn: () => void) => void;
}

const browserEnv: DprEnvironment = {
  getDpr: () => (typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1),
  matchResolution: (dpr) => window.matchMedia(`(resolution: ${dpr}dppx)`),
  addResizeListener: (fn) => window.addEventListener("resize", fn),
  removeResizeListener: (fn) => window.removeEventListener("resize", fn),
};

export function createDprStore(env: DprEnvironment = browserEnv) {
  let current = env.getDpr();
  let mq: ResolutionQuery | null = null;
  const listeners = new Set<() => void>();

  const rearm = (dpr: number) => {
    mq?.removeEventListener("change", update);
    mq = env.matchResolution(dpr);
    mq.addEventListener("change", update);
  };

  const update = () => {
    const next = env.getDpr();
    if (next === current) return;
    current = next;
    rearm(next);
    listeners.forEach((listener) => listener());
  };

  const subscribe = (listener: () => void) => {
    if (listeners.size === 0) {
      current = env.getDpr();
      rearm(current);
      env.addResizeListener(update);
    }
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) {
        mq?.removeEventListener("change", update);
        mq = null;
        env.removeResizeListener(update);
      }
    };
  };

  return {
    subscribe,
    getSnapshot: () => current,
  };
}

const dprStore = createDprStore();

export function useDpr(): number {
  return useSyncExternalStore(dprStore.subscribe, dprStore.getSnapshot);
}
