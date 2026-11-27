import { describe, expect, it } from "vitest";
import { activeFilters, defaultOpts, migrateOpts, parseRules } from "./diffOptions";

describe("migrateOpts", () => {
  it("turns the old whitespace toggle into the mode all and keeps the other toggles", () => {
    const o = migrateOpts({ ignoreWhitespace: true, ignoreCase: true, ignoreEol: false, patience: true });
    expect(o.whitespace).toBe("all");
    expect(o.ignoreCase).toBe(true);
    expect(o.ignoreEol).toBe(false);
    expect(Object.keys(o)).not.toContain("patience");
  });
  it("prefers a saved mode to the old toggle and refuses unknown values", () => {
    expect(migrateOpts({ ignoreWhitespace: true, whitespace: "leading" }).whitespace).toBe("leading");
    expect(migrateOpts({ whitespace: "everything" }).whitespace).toBe("none");
    expect(migrateOpts({ granularity: "char" }).granularity).toBe("char");
    expect(migrateOpts({ granularity: "sentence" }).granularity).toBe("word");
  });
  it("keeps only string rules within the limits", () => {
    const many = Array.from({ length: 40 }, (_, i) => `r${i}`);
    expect(migrateOpts({ ignoreLines: many }).ignoreLines).toHaveLength(16);
    expect(migrateOpts({ ignoreText: ["a", 1, null, "b"] }).ignoreText).toEqual(["a", "b"]);
    expect(migrateOpts({ ignoreText: ["x".repeat(900)] }).ignoreText![0]).toHaveLength(512);
    expect(migrateOpts({ ignoreLines: "not a list" }).ignoreLines).toEqual([]);
  });
  it("starts from the defaults for nothing saved", () => {
    expect(migrateOpts({})).toEqual(defaultOpts);
  });
});

describe("parseRules", () => {
  it("takes one rule per line and drops empty ones", () => {
    expect(parseRules("a\n\n  b  \n\t\nc")).toEqual(["a", "b", "c"]);
    expect(parseRules("")).toEqual([]);
  });
  it("stops at the limit of the core", () => {
    expect(parseRules(Array.from({ length: 30 }, (_, i) => `r${i}`).join("\n"))).toHaveLength(16);
  });
});

describe("activeFilters", () => {
  it("counts the filters that hide differences, not the way changes are marked", () => {
    expect(activeFilters(defaultOpts)).toBe(0);
    expect(activeFilters({ ...defaultOpts, granularity: "char" })).toBe(0);
    expect(activeFilters({ ...defaultOpts, whitespace: "trailing", ignoreComments: true, ignoreLines: ["x"], ignoreText: ["y"] })).toBe(4);
  });
});
