import { describe, expect, it } from "vitest";
import { etaSeconds, formatEta, updateRate, type Rate } from "./eta";

/** Feeds a steady `perSec` for `ms`, one reading every 100 ms like the backend. */
function feed(perSec: number, ms: number, start?: Rate, t = 0, done = 0) {
  let r = start;
  for (let s = 0; s <= ms; s += 100) r = updateRate(r, t + s, done + (perSec * s) / 1000);
  return r!;
}

describe("ETA", () => {
  it("no estimate during warm-up", () => {
    const r = feed(100, 1000);
    expect(etaSeconds(r, 10_000, 1000)).toBeNull();
  });

  it("steady speed gives the exact time left", () => {
    const r = feed(1000, 3000); // 3000 of 10000 done at 1000/s
    expect(etaSeconds(r, 10_000, 3000)).toBeCloseTo(7, 0);
  });

  it("follows a speed change smoothly", () => {
    const fast = feed(1000, 3000);
    const slow = feed(250, 3000, fast, 3000, 3000);
    // Slower now, but still remembering some of the fast part.
    expect(slow.perSec).toBeLessThan(700);
    expect(slow.perSec).toBeGreaterThan(250);
  });

  it("a reset (done goes back) starts over", () => {
    const r = updateRate(feed(1000, 3000), 3100, 0);
    expect(r.perSec).toBe(0);
  });

  it("formats", () => {
    expect(formatEta(0.2)).toBe("~1 s");
    expect(formatEta(42)).toBe("~42 s");
    expect(formatEta(130)).toBe("~2 min 10 s");
    expect(formatEta(605)).toBe("~10 min");
    expect(formatEta(3600 + 5 * 60)).toBe("~1 h 5 min");
  });
});
