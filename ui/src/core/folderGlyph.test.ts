import { describe, expect, it } from "vitest";
import { emblemRing, emblemScale, folderGeometry } from "./folderGlyph";

// Icon sizes in device pixels: 16 px at 100 %, 20 at 125 %, 24 at 150 %, 28 at 175 %, 32 at 200 %, 48 at 300 %.
const SIZES = [16, 18, 20, 22, 24, 28, 32, 40, 48];

/** The x/y numbers of a path made of M, H, V, L and A commands, without the arc radii and flags. */
function coordinates(d: string): number[] {
  const out: number[] = [];
  for (const m of d.matchAll(/([MHVLAZ])([^MHVLAZ]*)/g)) {
    const n = m[2].trim().split(/[\s,]+/).filter(Boolean).map(Number);
    if (m[1] === "A") out.push(n[5], n[6]); // end point only (radii, rotation and flags are not positions)
    else out.push(...n);
  }
  return out;
}

describe("folder glyph geometry", () => {
  it.each(SIZES)("every edge of the %i px folder is on a whole device pixel", (size) => {
    const g = folderGeometry(size);
    expect(g.size).toBe(size);
    for (const part of [g.back, g.front]) {
      const nums = coordinates(part);
      expect(nums.length).toBeGreaterThan(10);
      for (const v of nums) expect(Number.isInteger(v), `${size}: ${part} has ${v}`).toBe(true);
    }
    // The highlight line is centred on a whole pixel: its 1 px stroke covers one row exactly.
    expect(g.line.y % 1).toBe(0.5);
  });

  it.each(SIZES)("the %i px folder body and emblem stay inside the icon", (size) => {
    const g = folderGeometry(size);
    const b = g.body;
    expect(b.left).toBeGreaterThanOrEqual(0);
    expect(b.top).toBeGreaterThanOrEqual(0);
    expect(b.right).toBeLessThanOrEqual(size);
    expect(b.bottom).toBeLessThanOrEqual(size);
    // The emblem's far corner (unit 16) is the bottom-right of its drawing.
    expect(g.emblem.right).toBeLessThanOrEqual(b.right);
    expect(g.emblem.bottom).toBeLessThanOrEqual(size);
    // Offsets and scale are whole numbers, so 1-unit strokes are whole pixels wide.
    expect(Number.isInteger(g.emblem.scale)).toBe(true);
    const m = /translate\((-?\d+) (-?\d+)\) scale\((\d+)\)/.exec(g.emblem.transform);
    expect(m, g.emblem.transform).not.toBeNull();
    // The 8-unit glyph (units 8..16) lies inside the box after the transform.
    const k = Number(m![3]);
    expect(Number(m![1]) + 8 * k).toBeGreaterThanOrEqual(0);
    expect(Number(m![2]) + 8 * k).toBeGreaterThanOrEqual(0);
  });

  it("matches the 16-unit art at 100 %", () => {
    const g = folderGeometry(16);
    expect(g.body).toEqual({ left: 1, right: 15, top: 2, bottom: 14 });
    expect(g.emblem.scale).toBe(1);
    expect(g.emblem.transform).toBe("translate(-1 0) scale(1)");
  });

  it("at 150 % the emblem is drawn at 2 device pixels per unit", () => {
    const g = folderGeometry(24);
    expect(g.emblem.scale).toBe(2);
    expect(g.emblem.halo).toBe(2); // 2 px strokes plus a one pixel ring on each side, in units of 2 px
    expect(g.body).toEqual({ left: 1, right: 23, top: 3, bottom: 21 });
  });

  it("the emblem grows in whole steps with the icon", () => {
    expect([16, 20, 22, 24, 32, 33, 44, 48, 108].map(emblemScale)).toEqual([1, 1, 1, 2, 2, 2, 3, 3, 7]);
    // About half of the icon at every size from 100 %: never more than two thirds.
    for (const size of [16, 20, 24, 32, 48, 72, 108, 144]) expect((8 * emblemScale(size)) / size, `${size}`).toBeLessThanOrEqual(2 / 3 + 1e-9);
    expect([1, 2, 4, 7].map(emblemRing)).toEqual([1, 1, 2, 4]);
  });
});
