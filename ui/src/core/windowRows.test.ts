import { describe, expect, it } from "vitest";
import { MAX_WINDOW_HEIGHT, ROW_HEIGHT, rowCount, rowText, textRows, windowHeight, windowRange } from "./windowRows";

describe("window range", () => {
  it("renders the rows in view with a few around them and never outside the list", () => {
    expect(windowRange(0, 100, 20, 1000, 4)).toEqual({ start: 0, end: 9 });
    expect(windowRange(2000, 100, 20, 1000, 4)).toEqual({ start: 96, end: 109 });
    // Past the end (a stale scroll position) is the end, never an empty or inverted range.
    expect(windowRange(1_000_000, 100, 20, 1000, 4)).toEqual({ start: 991, end: 1000 });
  });
  it("is empty without rows and ignores negative scroll", () => {
    expect(windowRange(50, 100, 20, 0)).toEqual({ start: 0, end: 0 });
    expect(windowRange(-30, 100, 20, 10, 0)).toEqual({ start: 0, end: 5 });
  });
  it("keeps the number of rendered rows small for a 5,000-line file at any position", () => {
    for (const top of [0, 12_345, 80_000, 89_900]) {
      const { start, end } = windowRange(top, MAX_WINDOW_HEIGHT, ROW_HEIGHT, 5000);
      expect(end - start).toBeLessThanOrEqual(Math.ceil(MAX_WINDOW_HEIGHT / ROW_HEIGHT) + 2 * 8 + 1);
      expect(start).toBeGreaterThanOrEqual(0);
      expect(end).toBeLessThanOrEqual(5000);
    }
  });
});

describe("window height and rows", () => {
  it("grows with the rows up to the largest height", () => {
    expect(windowHeight(10)).toBe(10 * ROW_HEIGHT);
    expect(windowHeight(100_000)).toBe(MAX_WINDOW_HEIGHT);
    expect(windowHeight(-3)).toBe(0);
  });
  it("cuts the terminators and does not invent an empty last row", () => {
    expect(rowText("abc\r\n")).toBe("abc");
    expect(rowText("abc\n")).toBe("abc");
    expect(rowText("abc")).toBe("abc");
    expect(textRows("a\r\nb\n")).toEqual(["a", "b"]);
    expect(textRows("a\n\n")).toEqual(["a", ""]);
    expect(textRows("")).toEqual([]);
    expect(textRows("no newline")).toEqual(["no newline"]);
  });
  it("aligns columns on the longest one", () => {
    expect(rowCount([["a", "b"], ["a"], []])).toBe(2);
    expect(rowCount([])).toBe(0);
  });
});
