import { afterEach, describe, expect, it, vi } from "vitest";
import { setViewportOverride, startDesktopSync, syncViewport } from "./desktop";
import { getState, setState, subscribe } from "./store";

vi.mock("../api/ipc", () => ({
  api: { omarchyStateGet: () => Promise.resolve(null) },
  events: { omarchyStateChanged: () => Promise.resolve(() => {}) },
}));

describe("viewport resize sync", () => {
  afterEach(() => {
    setViewportOverride(null);
    vi.useRealTimers();
    vi.unstubAllGlobals();
    setState({ sidebarVisible: true, sidebarAuto: false, sidebarOverride: null, adaptiveMode: null });
  });

  it("updates viewport silently without waking store subscribers while layout mode is unchanged", () => {
    vi.stubGlobal("window", { innerWidth: 1400, innerHeight: 860 });
    setState({ sidebarVisible: true, sidebarAuto: false, sidebarOverride: null, adaptiveMode: "horizontal" });
    setViewportOverride({ w: 1400, h: 860 });

    const notified = vi.fn();
    const unsubscribe = subscribe(notified);
    try {
      // Simulate a live drag resizing smoothly within the wide horizontal band.
      for (let w = 1395; w >= 1220; w -= 5) {
        setViewportOverride({ w, h: 860 - (1400 - w) / 4 });
      }
      expect(notified).not.toHaveBeenCalled();
      expect(getState().viewport).toEqual({ w: 1220, h: 815 });

      // Crossing into narrow single-pane width triggers a single real store update.
      setViewportOverride({ w: 460, h: 700 });
      syncViewport();
      expect(notified).toHaveBeenCalledTimes(1);
      expect(getState().adaptiveMode).toBe("single");
      expect(getState().sidebarAuto).toBe(true);
    } finally {
      unsubscribe();
    }
  });

  it("coalesces resize bursts per frame and toggles the resizing class until settled", () => {
    vi.useFakeTimers();
    const classes = new Set<string>();
    const resizeListeners = new Set<() => void>();
    const rafQueue = new Map<number, () => void>();
    let rafId = 1;
    const win = {
      innerWidth: 1400,
      innerHeight: 860,
      addEventListener: (type: string, fn: () => void) => {
        if (type === "resize") resizeListeners.add(fn);
      },
      removeEventListener: (type: string, fn: () => void) => {
        if (type === "resize") resizeListeners.delete(fn);
      },
    };
    vi.stubGlobal("window", win);
    vi.stubGlobal("document", {
      documentElement: {
        classList: {
          toggle: (name: string, force?: boolean) => {
            if (force) classes.add(name);
            else classes.delete(name);
          },
        },
      },
    });
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
      const id = rafId++;
      rafQueue.set(id, cb);
      return id;
    });
    vi.stubGlobal("cancelAnimationFrame", (id: number) => {
      rafQueue.delete(id);
    });

    const stop = startDesktopSync();
    try {
      expect(classes.has("resizing")).toBe(false);

      // Fire 30 resize events before the next animation frame: only 1 rAF is queued.
      for (let i = 1; i <= 30; i++) {
        win.innerWidth = 1400 - i * 4;
        resizeListeners.forEach((fn) => fn());
      }
      expect(classes.has("resizing")).toBe(true);
      expect(rafQueue.size).toBe(1);

      const [frame] = [...rafQueue.values()];
      rafQueue.clear();
      frame();
      expect(getState().viewport.w).toBe(1280);

      // Still active before settle window expires, then cleared after 150 ms of quiet.
      vi.advanceTimersByTime(149);
      expect(classes.has("resizing")).toBe(true);
      vi.advanceTimersByTime(1);
      expect(classes.has("resizing")).toBe(false);
    } finally {
      stop();
    }
    expect(resizeListeners.size).toBe(0);
    expect(classes.has("resizing")).toBe(false);
  });
});
