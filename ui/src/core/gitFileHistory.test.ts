import { describe, expect, it, vi } from "vitest";
import { diffRange, fileHistoryRows, lineActionsEnabled, PromiseCache, scrubBase, scrubBaseText, scrubChange, parseRange, restoreFileHistoryCursor, RevisionCache, selectedLines, type HistoryEntry } from "./gitFileHistory";

const patch = ["diff --git a/f b/f", "index 1..2", "--- a/f", "+++ b/f", "@@ -10,4 +20,5 @@ fn", " ten", "-eleven", "+twenty-one", "+twenty-two", " twelve", "@@ -40 +51 @@", "-forty", "+fifty-one", "\\ No newline at end of file", ""];

describe("selection", () => {
  it("counts whole lines", () => {
    expect(selectedLines("", "a")).toEqual([0, 0]);
    expect(selectedLines("a\nb\n", "c\nd")).toEqual([2, 3]);
    expect(selectedLines("a\n", "b\nc\n")).toEqual([1, 2]);
    expect(selectedLines("a\n", " \n ")).toBeNull();
  });
  it("maps lines of a diff to lines of the file", () => {
    expect(diffRange(patch, 5, 9)).toEqual({ start: 20, end: 23, side: "new" });
    expect(diffRange(patch, 7, 7)).toEqual({ start: 21, end: 21, side: "new" });
    expect(diffRange(patch, 6, 6)).toEqual({ start: 11, end: 11, side: "old" });
    expect(diffRange(patch, 11, 11)).toEqual({ start: 40, end: 40, side: "old" });
    expect(diffRange(patch, 10, 14)).toEqual({ start: 51, end: 51, side: "new" });
    expect(diffRange(patch, 0, 4)).toBeNull();
  });
  it("reads typed ranges", () => {
    expect(parseRange("3-7", 10)).toEqual({ start: 3, end: 7 });
    expect(parseRange(" 7 , 3 ", 10)).toEqual({ start: 3, end: 7 });
    expect(parseRange("5", 10)).toEqual({ start: 5, end: 5 });
    expect(parseRange("5-99", 10)).toEqual({ start: 5, end: 10 });
    for (const bad of ["", "0", "11", "a", "1-b", "-3"]) expect(parseRange(bad, 10), bad).toBeNull();
  });
});

describe("cache of revisions", () => {
  const entry = (id: string): HistoryEntry => ({ id, subject: "", author: "", date: "", file: "f" });
  it("loads a revision once and drops the least recently used", async () => {
    const load = vi.fn(async (e: HistoryEntry) => e.id);
    const cache = new RevisionCache(load, 2);
    expect(await cache.get(entry("a"))).toBe("a");
    await cache.get(entry("b")); await cache.get(entry("a")); await cache.get(entry("c"));
    expect(load).toHaveBeenCalledTimes(3);
    await cache.get(entry("a"));
    expect(load).toHaveBeenCalledTimes(3);
    await cache.get(entry("b"));
    expect(load).toHaveBeenCalledTimes(4);
  });
  it("asks again after a failure", async () => {
    let fail = true;
    const cache = new RevisionCache(async () => { if (fail) throw new Error("no"); return "yes"; });
    await expect(cache.get(entry("a"))).rejects.toThrow("no");
    fail = false;
    expect(await cache.get(entry("a"))).toBe("yes");
  });
});

it("restores file history by OID and falls back when it disappears", () => {
  expect(restoreFileHistoryCursor(["new", "old"], "old")).toBe("old");
  expect(restoreFileHistoryCursor(["new", "old"], "gone")).toBe("new");
  expect(restoreFileHistoryCursor([], "gone")).toBeNull();
});

it("encodes filtered history without inventing parent edges", () => {
  const rows = fileHistoryRows([{ id: "oid", subject: "subject\ncontinued", author: "author", date: "date", file: "file" }]);
  expect(rows.split("\0")).toEqual(["oid", "", "", "subject continued", "author", "date"]);
});

describe("lineActionsEnabled", () => {
  it("allows the lines of a diff or a file and nothing while the scrubber moves", () => {
    expect(lineActionsEnabled({ view: "file", transition: false })).toBe(true);
    expect(lineActionsEnabled({ view: "diff", transition: false })).toBe(true);
    expect(lineActionsEnabled({ view: "file", transition: true })).toBe(false);
    expect(lineActionsEnabled({ view: "blame", transition: false })).toBe(false);
    expect(lineActionsEnabled({ view: "lines", transition: false })).toBe(false);
  });
});

describe("scrubber diff base", () => {
  it("is the first parent by default, the chosen parent of a merge, and an empty tree for a first commit", () => {
    expect(scrubBase(["a"])).toEqual({ kind: "parent", base: "a", index: 0, count: 1 });
    expect(scrubBase(["a", "b"])).toEqual({ kind: "parent", base: "a", index: 0, count: 2 });
    expect(scrubBase(["a", "b"], 1)).toEqual({ kind: "parent", base: "b", index: 1, count: 2 });
    expect(scrubBase(["a", "b"], 7).base).toBe("b");
    expect(scrubBase(["a", "b"], -3).base).toBe("a");
    expect(scrubBase([])).toEqual({ kind: "root", base: null });
    expect(scrubBase([], 4)).toEqual({ kind: "root", base: null });
  });
  it("says what the revision is compared with", () => {
    expect(scrubBaseText(scrubBase([]))).toContain("empty tree");
    expect(scrubBaseText(scrubBase(["a"]))).toBe("Compared with its parent");
    expect(scrubBaseText(scrubBase(["a", "b"], 1))).toBe("Merge: compared with parent 2 of 2");
  });
  it("turns line actions off in the scrubber's diff", () => {
    expect(lineActionsEnabled({ view: "file", transition: false, scrubDiff: true })).toBe(false);
    expect(lineActionsEnabled({ view: "file", transition: false, scrubDiff: false })).toBe(true);
  });
});

describe("PromiseCache", () => {
  it("loads each key once, drops the least recently used and forgets failures", async () => {
    const cache = new PromiseCache<number>(2);
    let loads = 0;
    const load = (n: number) => () => { loads++; return Promise.resolve(n); };
    expect(await cache.get("a", load(1))).toBe(1);
    expect(await cache.get("a", load(99))).toBe(1);
    expect(loads).toBe(1);
    await cache.get("b", load(2));
    await cache.get("a", load(1)); // a is now the newest
    await cache.get("c", load(3)); // b is dropped
    expect(cache.size).toBe(2);
    expect(await cache.get("b", load(22))).toBe(22);
    // A failed load is not kept: the next ask loads again, and no rejection is left unhandled.
    let tries = 0;
    await expect(cache.get("bad", () => { tries++; return Promise.reject(new Error("x")); })).rejects.toThrow("x");
    await Promise.resolve();
    expect(await cache.get("bad", () => { tries++; return Promise.resolve(5); })).toBe(5);
    expect(tries).toBe(2);
  });
});

describe("scrubber change of a revision", () => {
  const merge: HistoryEntry = { id: "m", subject: "", author: "", date: "", file: "f", parents: ["p1", "p2"] };
  const io = (load = vi.fn(async () => "patch")) => {
    const parentsOf = vi.fn(async () => ["q"]);
    const cache = new PromiseCache<{ text: string; none: boolean }>();
    return { parentsOf, load, io: { parentsOf, load, patch: (k: string, l: () => Promise<{ text: string; none: boolean }>) => cache.get(k, l) } };
  };
  it("needs no request for the parents when the history has them", async () => {
    const t = io();
    const got = await scrubChange(merge, 1, t.io);
    expect(t.parentsOf).not.toHaveBeenCalled();
    expect(t.load).toHaveBeenCalledWith(merge, "p2");
    expect(got.base).toEqual({ kind: "parent", base: "p2", index: 1, count: 2 });
    await scrubChange(merge, 1, t.io);
    expect(t.load).toHaveBeenCalledTimes(1);
  });
  it("asks the commit only for an entry without parents, and shows a root against nothing", async () => {
    const t = io();
    await scrubChange({ ...merge, parents: undefined }, 0, t.io);
    expect(t.parentsOf).toHaveBeenCalledTimes(1);
    const root = await scrubChange({ ...merge, id: "r", parents: [] }, 0, t.io);
    expect(root.base).toEqual({ kind: "root", base: null });
  });
  it("answers none when a merge does not list the file, and passes other failures on", async () => {
    const none = await scrubChange(merge, 0, io(vi.fn(async () => { throw new Error("not-in-comparison: f"); })).io);
    expect(none).toMatchObject({ text: "", none: true });
    await expect(scrubChange(merge, 0, io(vi.fn(async () => { throw new Error("boom"); })).io)).rejects.toThrow("boom");
  });
});
