import { describe, expect, it } from "vitest";
import { parseOffset, rowOfOffset } from "./hexOffset";

const ok = (offset: number) => ({ ok: true, offset });

describe("parseOffset", () => {
  it("reads decimal and 0x hex, with spaces around, and the end of the file", () => {
    expect(parseOffset("0", 100)).toEqual(ok(0));
    expect(parseOffset("  64 ", 100)).toEqual(ok(64));
    expect(parseOffset("0x10", 100)).toEqual(ok(16));
    expect(parseOffset("0X1f", 100)).toEqual(ok(31));
    expect(parseOffset("end", 100)).toEqual(ok(100));
    expect(parseOffset("EOF", 100)).toEqual(ok(100));
    // The end itself is allowed, one byte past it is not.
    expect(parseOffset("100", 100)).toEqual(ok(100));
    expect(parseOffset("101", 100).ok).toBe(false);
  });

  it("refuses everything that is not clearly an offset", () => {
    for (const text of ["", "   ", "-1", "+5", "1.5", "1e3", "1_000", "1,000", "1F", "ff", "0x", "0xZZ", "0x1g", "12 34", "٣", "end2", "0b101", "NaN", "Infinity"]) {
      const r = parseOffset(text, 1000);
      expect(r.ok, text).toBe(false);
      if (!r.ok) expect(r.reason.length).toBeGreaterThan(0);
    }
  });

  it("checks bounds, also for hex and huge numbers", () => {
    expect(parseOffset("0x3e8", 1000)).toEqual(ok(1000));
    expect(parseOffset("0x3e9", 1000).ok).toBe(false);
    const huge = parseOffset("99999999999999999999999", Number.MAX_SAFE_INTEGER);
    expect(huge.ok).toBe(false);
    expect(huge.ok === false && huge.reason).toContain("too large");
    // Exactly the largest safe integer is fine for a file that large.
    expect(parseOffset(String(Number.MAX_SAFE_INTEGER), Number.MAX_SAFE_INTEGER)).toEqual(ok(Number.MAX_SAFE_INTEGER));
    // Leading zeros are decimal, not octal.
    expect(parseOffset("010", 100)).toEqual(ok(10));
  });

  it("an empty file has only offset 0 (its end)", () => {
    expect(parseOffset("0", 0)).toEqual(ok(0));
    expect(parseOffset("end", 0)).toEqual(ok(0));
    expect(parseOffset("1", 0).ok).toBe(false);
  });
});

describe("rowOfOffset", () => {
  it("is the row that holds the byte, and the last row for the end of the file", () => {
    expect(rowOfOffset(0, 100, 16)).toBe(0);
    expect(rowOfOffset(15, 100, 16)).toBe(0);
    expect(rowOfOffset(16, 100, 16)).toBe(1);
    expect(rowOfOffset(99, 100, 16)).toBe(6);
    // The end of a file of 96 bytes is the row of byte 95, not a seventh row that does not exist.
    expect(rowOfOffset(96, 96, 16)).toBe(5);
    expect(rowOfOffset(0, 0, 16)).toBe(0);
    expect(rowOfOffset(-5, 100, 16)).toBe(0);
  });
});
