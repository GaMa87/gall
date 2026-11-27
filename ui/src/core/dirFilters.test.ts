import { describe, expect, it } from "vitest";
import { activeFilterCount, EMPTY_FORM, formOf, parseDate, parseFilters, parseGlobs, parseSize } from "./dirFilters";

describe("sizes", () => {
  it("reads bytes and 1024-based units, and refuses anything else", () => {
    expect(parseSize("500")).toBe(500);
    expect(parseSize("10k")).toBe(10 * 1024);
    expect(parseSize("1.5 MB")).toBe(1.5 * 1024 * 1024);
    expect(parseSize(" 2 gib ")).toBe(2 * 1024 ** 3);
    for (const bad of ["", "abc", "-5", "1e3", "10 parsecs", "1,5k", "99999999999999999999g"]) expect(parseSize(bad), bad).toBeNull();
  });
});

describe("dates", () => {
  it("is the start of the day, or the last second of it, in local time, and refuses dates that do not exist", () => {
    const start = parseDate("2025-03-09", false)!, end = parseDate("2025-03-09", true)!;
    expect(new Date(start * 1000).getHours()).toBe(0);
    expect(new Date(end * 1000).getHours()).toBe(23);
    expect(end - start).toBeGreaterThanOrEqual(86399 - 3600);
    for (const bad of ["2025-02-31", "2025-13-01", "25-01-01", "2025/01/01", "", "tomorrow"]) expect(parseDate(bad, false), bad).toBeNull();
  });
});

describe("globs", () => {
  it("are split by commas and line breaks and trimmed", () => {
    expect(parseGlobs("*.log, build/\n node_modules ,,")).toEqual(["*.log", "build/", "node_modules"]);
    expect(parseGlobs("  ")).toEqual([]);
  });
});

describe("the whole form", () => {
  it("is empty filters when nothing is typed", () => {
    expect(parseFilters(EMPTY_FORM)).toEqual({ ok: true, fields: { include: [], exclude: [], minSize: null, maxSize: null, modifiedAfter: null, modifiedBefore: null } });
  });
  it("says what is wrong, including a range that can match nothing", () => {
    const bad = (patch: object) => { const r = parseFilters({ ...EMPTY_FORM, ...patch }); return r.ok ? "" : r.error; };
    expect(bad({ minSize: "ten" })).toContain("Smallest size");
    expect(bad({ maxSize: "1x" })).toContain("Largest size");
    expect(bad({ minSize: "2k", maxSize: "1k" })).toContain("larger than the largest");
    expect(bad({ after: "2025-02-30" })).toContain("Modified from");
    expect(bad({ before: "nope" })).toContain("Modified until");
    expect(bad({ after: "2025-05-02", before: "2025-05-01" })).toContain("after the last");
  });
  it("round-trips through the fields", () => {
    const r = parseFilters({ include: "*.rs, *.toml", exclude: "target/", minSize: "1k", maxSize: "1 MB", after: "2025-01-01", before: "2025-12-31" });
    if (!r.ok) throw new Error(r.error);
    expect(r.fields.include).toEqual(["*.rs", "*.toml"]);
    expect(r.fields.minSize).toBe(1024);
    expect(formOf(r.fields)).toEqual({ include: "*.rs, *.toml", exclude: "target/", minSize: "1024", maxSize: "1048576", after: "2025-01-01", before: "2025-12-31" });
    expect(activeFilterCount(r.fields)).toBe(6);
    expect(activeFilterCount({})).toBe(0);
  });
});
