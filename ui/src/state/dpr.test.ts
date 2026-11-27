import { describe, expect, it, vi } from "vitest";
import { createDprStore } from "./dpr";

describe("shared devicePixelRatio store", () => {
  it("ignores same-DPR resize bursts without recreating matchMedia or notifying subscribers", () => {
    let dpr = 1.5;
    const resizeListeners = new Set<() => void>();
    const mqListeners = new Set<() => void>();
    const matchCalls: number[] = [];

    const store = createDprStore({
      getDpr: () => dpr,
      matchResolution: (value) => {
        matchCalls.push(value);
        return {
          addEventListener: (_type, fn) => { mqListeners.add(fn as () => void); },
          removeEventListener: (_type, fn) => { mqListeners.delete(fn as () => void); },
        };
      },
      addResizeListener: (fn) => { resizeListeners.add(fn); },
      removeResizeListener: (fn) => { resizeListeners.delete(fn); },
    });

    const subA = vi.fn();
    const subB = vi.fn();
    const unA = store.subscribe(subA);
    const unB = store.subscribe(subB);

    // One shared listener and one initial matchMedia query regardless of subscriber count.
    expect(resizeListeners.size).toBe(1);
    expect(matchCalls).toEqual([1.5]);
    expect(store.getSnapshot()).toBe(1.5);

    // A burst of 40 window resizes at the same scale causes zero work or notifications.
    for (let i = 0; i < 40; i++) {
      resizeListeners.forEach((fn) => fn());
    }
    expect(matchCalls).toEqual([1.5]);
    expect(subA).not.toHaveBeenCalled();
    expect(subB).not.toHaveBeenCalled();

    // Moving to a 2x monitor updates the snapshot, re-arms matchMedia once, and notifies all subscribers.
    dpr = 2;
    mqListeners.forEach((fn) => fn());
    expect(store.getSnapshot()).toBe(2);
    expect(matchCalls).toEqual([1.5, 2]);
    expect(subA).toHaveBeenCalledTimes(1);
    expect(subB).toHaveBeenCalledTimes(1);

    // Unsubscribing cleans up listeners only once the last subscriber leaves.
    unA();
    expect(resizeListeners.size).toBe(1);
    unB();
    expect(resizeListeners.size).toBe(0);
    expect(mqListeners.size).toBe(0);
  });
});
