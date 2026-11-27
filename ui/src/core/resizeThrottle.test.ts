import { describe, expect, it, vi } from "vitest";
import {
  createRafCoalescer,
  createResizeActivityTracker,
  createTrailingDebounce,
  folderPaneColumnMask,
  folderPaneSizeChanged,
  quantizeListHeight,
  workspaceSplitFlex,
} from "./resizeThrottle";

describe("resize throttling and scheduling helpers", () => {
  it("coalesces a burst of schedule calls into one execution per animation frame", () => {
    const queue = new Map<number, () => void>();
    let nextId = 1;
    const raf = {
      requestFrame: (cb: () => void) => {
        const id = nextId++;
        queue.set(id, cb);
        return id;
      },
      cancelFrame: (id: number) => {
        queue.delete(id);
      },
    };
    const run = vi.fn();
    const coalescer = createRafCoalescer(run, raf);

    for (let i = 0; i < 25; i++) coalescer.schedule();
    expect(queue.size).toBe(1);
    expect(run).not.toHaveBeenCalled();

    const [cb] = [...queue.values()];
    queue.clear();
    cb();
    expect(run).toHaveBeenCalledTimes(1);
    expect(coalescer.pending()).toBe(false);

    // Scheduling and cancelling before the frame fires never runs the callback.
    coalescer.schedule();
    expect(coalescer.pending()).toBe(true);
    coalescer.cancel();
    expect(coalescer.pending()).toBe(false);
    expect(queue.size).toBe(0);
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("debounces rapid resize triggers to one trailing callback after quiet", () => {
    vi.useFakeTimers();
    try {
      const run = vi.fn();
      const debounced = createTrailingDebounce(run, 120);

      for (let i = 0; i < 50; i++) {
        debounced.trigger();
        vi.advanceTimersByTime(10);
      }
      expect(run).not.toHaveBeenCalled();
      expect(debounced.pending()).toBe(true);

      // 10 ms already elapsed after the final trigger in the loop (10 + 109 = 119 ms).
      vi.advanceTimersByTime(109);
      expect(run).not.toHaveBeenCalled();

      vi.advanceTimersByTime(1);
      expect(run).toHaveBeenCalledTimes(1);
      expect(debounced.pending()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("tracks active resize gestures without duplicate transitions and resets on cancel", () => {
    vi.useFakeTimers();
    try {
      const states: boolean[] = [];
      const tracker = createResizeActivityTracker((active) => states.push(active), 150);

      tracker.bump();
      tracker.bump();
      vi.advanceTimersByTime(100);
      tracker.bump();
      expect(states).toEqual([true]);
      expect(tracker.isActive()).toBe(true);

      vi.advanceTimersByTime(150);
      expect(states).toEqual([true, false]);
      expect(tracker.isActive()).toBe(false);

      tracker.bump();
      expect(states).toEqual([true, false, true]);
      tracker.cancel();
      expect(states).toEqual([true, false, true, false]);
      expect(tracker.isActive()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("formats workspace split ratios as clamped CSS percentages", () => {
    expect(workspaceSplitFlex(500)).toBe("0 0 clamp(40px, 50%, calc(100% - 40px))");
    expect(workspaceSplitFlex(333.33)).toBe("0 0 clamp(40px, 33.333%, calc(100% - 40px))");
    expect(workspaceSplitFlex(-20)).toBe("0 0 clamp(40px, 0%, calc(100% - 40px))");
    expect(workspaceSplitFlex(1400)).toBe("0 0 clamp(40px, 100%, calc(100% - 40px))");
    expect(workspaceSplitFlex(Number.NaN)).toBe("0 0 clamp(40px, 50%, calc(100% - 40px))");
  });

  it("ignores sub-threshold width/height jitter in FolderPane list view while preserving grid and column thresholds", () => {
    const listOpts = { isGrid: false, fontSize: 13, compact: false, itemH: 24 };
    // Within the same column band (>76 chars at 13px = >592.8px) and same 48px height bucket (481..528): no re-render.
    expect(folderPaneColumnMask(700, 13, false)).toBe(folderPaneColumnMask(620, 13, false));
    expect(quantizeListHeight(490, 24)).toBe(528);
    expect(quantizeListHeight(520, 24)).toBe(528);
    expect(
      folderPaneSizeChanged({ w: 700, h: 490, scrollbarW: 8 }, { w: 620, h: 520, scrollbarW: 8 }, listOpts),
    ).toBe(false);

    // Crossing a column visibility threshold triggers an immediate update.
    expect(
      folderPaneSizeChanged({ w: 620, h: 500, scrollbarW: 8 }, { w: 500, h: 500, scrollbarW: 8 }, listOpts),
    ).toBe(true);

    // Crossing a 2-row height bucket or scrollbar appearance triggers an update.
    expect(
      folderPaneSizeChanged({ w: 700, h: 500, scrollbarW: 8 }, { w: 700, h: 540, scrollbarW: 8 }, listOpts),
    ).toBe(true);
    expect(
      folderPaneSizeChanged({ w: 700, h: 500, scrollbarW: 0 }, { w: 700, h: 500, scrollbarW: 8 }, listOpts),
    ).toBe(true);

    // Grid view reacts to every pixel change so cell widths stay fluid.
    expect(
      folderPaneSizeChanged({ w: 700, h: 500, scrollbarW: 8 }, { w: 699, h: 500, scrollbarW: 8 }, { ...listOpts, isGrid: true }),
    ).toBe(true);
  });
});
