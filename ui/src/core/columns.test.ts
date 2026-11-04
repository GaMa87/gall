import { describe, expect, it } from "vitest";
import { fitColumnWidths as fit } from "./columns";

const sum = (w: number[]) => w.reduce((a, x) => a + x, 0);
const MIN = 70;
const MAX = 260;

describe("Miller column widths", () => {
  it("few columns span the full width", () => {
    expect(fit(2, 1000, MIN, MAX, null)).toEqual([500, 500]);
    expect(sum(fit(3, 1001, MIN, MAX, null))).toBe(1001);
  });

  it("many columns shrink, older ones first, never below min", () => {
    const w = fit(10, 1000, MIN, MAX, null);
    expect(sum(w)).toBe(1000);
    expect(Math.min(...w)).toBeGreaterThanOrEqual(MIN);
    expect(w[0]).toBeLessThanOrEqual(w[9]);
  });

  it("hovering a wide column changes nothing (no flicker with two columns)", () => {
    for (const h of [0, 1]) expect(fit(2, 1000, MIN, MAX, h)).toEqual(fit(2, 1000, MIN, MAX, null));
  });

  it("a hovered column never gets narrower", () => {
    for (let n = 1; n <= 12; n++) {
      const base = fit(n, 1100, MIN, MAX, null);
      for (let h = 0; h < n; h++) expect(fit(n, 1100, MIN, MAX, h)[h]).toBeGreaterThanOrEqual(base[h]);
    }
  });

  it("a narrow hovered column grows to max while the total stays the same", () => {
    const w = fit(10, 1000, MIN, MAX, 2);
    expect(w[2]).toBe(MAX);
    expect(sum(w)).toBe(1000);
  });
});
