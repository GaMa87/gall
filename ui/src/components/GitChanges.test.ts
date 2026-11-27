import { describe, expect, it } from "vitest";
import { changedBetween, changedInHunk, hunkHeaderFrom, type DiffLine } from "../core/patchSelection";

const lines: DiffLine[] = [
  { index: 0, text: "diff --git a/f b/f", kind: "meta", hunk: null },
  { index: 1, text: "@@ -1,3 +1,3 @@", kind: "hunk", hunk: 0 },
  { index: 2, text: " keep", kind: "context", hunk: 0 },
  { index: 3, text: "-old", kind: "deleted", hunk: 0 },
  { index: 4, text: "+new", kind: "added", hunk: 0 },
  { index: 5, text: "@@ -20,2 +20,3 @@", kind: "hunk", hunk: 1 },
  { index: 6, text: "+extra", kind: "added", hunk: 1 },
  { index: 7, text: " tail", kind: "context", hunk: 1 },
];

describe("patch selection helpers", () => {
  it("takes the changed lines of a range in either direction and skips context", () => {
    expect(changedBetween(lines, 2, 4)).toEqual([3, 4]);
    expect(changedBetween(lines, 4, 2)).toEqual([3, 4]);
    expect(changedBetween(lines, 2, 2)).toEqual([]);
  });
  it("takes every changed line of one hunk", () => {
    expect(changedInHunk(lines, 0)).toEqual([3, 4]);
    expect(changedInHunk(lines, 1)).toEqual([6]);
    expect(changedInHunk(lines, null)).toEqual([]);
  });
  it("jumps between hunk headers", () => {
    expect(hunkHeaderFrom(lines, 0, 1)).toBe(1);
    expect(hunkHeaderFrom(lines, 1, 1)).toBe(5);
    expect(hunkHeaderFrom(lines, 5, 1)).toBeNull();
    expect(hunkHeaderFrom(lines, 7, -1)).toBe(5);
    expect(hunkHeaderFrom(lines, 1, -1)).toBeNull();
  });
});