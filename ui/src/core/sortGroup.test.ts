import { describe, expect, it } from "vitest";
import type { FileEntry, GroupMode, GroupOrder } from "../api/types";
import { sortEntries, type SortSpec } from "./sort";
import { groupAt, groupEntries, groupId, layoutOf, MAX_TYPE_GROUPS, moveCursor, openGroups, placeOf, toggleFolders, windowOf, type GroupSpec } from "./sortGroup";
import cases from "./sortGroupCases.json";

const entry = (name: string, isDir: boolean, size: number, modified: number): FileEntry => {
  const dot = name.lastIndexOf(".");
  return { name, path: `/${name}`, kind: isDir ? "dir" : "file", isDir, isHidden: false, isSymlink: false, size, modified, created: modified, ext: isDir || dot <= 0 || dot === name.length - 1 ? "" : name.slice(dot + 1).toLowerCase(), readonly: false } as FileEntry;
};
const listing = (cases.listing as [string, boolean, number, number][]).map((e) => entry(...e));
const byName: SortSpec = { key: "name", descending: false, foldersOnTop: true, foldersByName: false };
const spec = (mode: GroupMode, order: GroupOrder = "name", byKind = false): GroupSpec => ({ mode, order, byKind });

describe("the cases shared with sort.rs", () => {
  it.each(cases.classify as [string, boolean, string, string][])("%s", (name, dir, byType, byKind) => {
    expect([groupId(entry(name, dir, 1, 1), false), groupId(entry(name, dir, 1, 1), true)]).toEqual([byType, byKind]);
  });
  it.each(cases.orders.map((c) => [c.title, c] as const))("%s", (_, c) => {
    for (const list of [listing, [...listing].reverse()]) {
      const { entries, groups } = groupEntries(list, c.sort as SortSpec, c.group as GroupSpec);
      expect(entries.map((e) => e.name)).toEqual(c.names);
      expect((groups ?? []).map((g) => [g.id, g.count, g.size])).toEqual(c.groups);
    }
  });
});

it("modes that do not group sort as before", () => {
  for (const key of ["name", "ext", "modified", "created", "size"] as const) for (const descending of [false, true]) for (const foldersByName of [false, true]) {
    for (const [mode, foldersOnTop] of [["none", false], ["folders", true]] as const) {
      const s = { key, descending, foldersOnTop, foldersByName };
      expect(groupEntries(listing, { ...s, foldersOnTop: !foldersOnTop }, spec(mode)).entries).toEqual(sortEntries(listing, s));
    }
  }
  expect(["none", "folders", "type", "foldersType"].map((m) => toggleFolders(m as GroupMode))).toEqual(["folders", "none", "foldersType", "type"]);
});

it("the rarest of thousands of types share the last group", () => {
  const many = Array.from({ length: 3000 }, (_, i) => entry(`f.x${String(i).padStart(4, "0")}`, false, 1, i));
  many.push(...Array.from({ length: 5 }, (_, i) => entry(`n${i}.zzz`, false, 1, 0)), entry("dir", true, 0, 0), entry("plain", false, 1, 0));
  for (const order of ["name", "count", "recent", "size"] as const) {
    const { entries, groups } = groupEntries(many, byName, spec("foldersType", order));
    expect(groups!.length).toBe(MAX_TYPE_GROUPS + 3);
    expect(groups![0].id).toBe("dir");
    expect(groups!.find((g) => g.id === "ext:zzz")?.count).toBe(5);
    expect(groups![groups!.length - 1]).toMatchObject({ id: "rest", count: 2801, types: 2801 });
    expect(entries.length).toBe(many.length);
  }
});

describe("layout", () => {
  const { entries, groups } = groupEntries(listing, byName, spec("foldersType"));
  // dir 2, gz 1, md 1, png 4, txt 2, dot 1, none 3
  const layout = (closed: string[], cols = 1) => {
    const open = openGroups(entries, groups, new Set(closed), false);
    return { ...open, l: layoutOf(open.visible.length, groups, open.closed, cols, 20, 30) };
  };
  it("leaves the entries of collapsed groups out", () => {
    const { visible, l } = layout(["ext:png", "ext:nothing"]);
    expect(visible.length).toBe(10);
    expect(visible.some((e) => e.ext === "png")).toBe(false);
    expect(l.height).toBe(7 * 30 + 10 * 20);
    // Entry 4 is the first TXT: under the headers of DIR, GZ, MD, PNG and TXT and four rows.
    expect(placeOf(l, 4)).toEqual({ top: 5 * 30 + 4 * 20, col: 0 });
    expect(groupAt(l, 4)).toBe("ext:txt");
    expect(groupAt(l, 3)).toBe("ext:md");
  });
  it("opens collapsed groups for a search only when nothing else matches", () => {
    const png = groupEntries(listing.filter((e) => e.ext === "png"), byName, spec("type"));
    expect(openGroups(png.entries, png.groups, new Set(["ext:png"]), true).visible.length).toBe(4);
    expect(openGroups(png.entries, png.groups, new Set(["ext:png"]), false).visible.length).toBe(0);
    expect(openGroups(entries, groups, new Set(["ext:png"]), true).visible.length).toBe(10);
  });
  it("places grid cells row by row inside a group", () => {
    const { l } = layout([], 3);
    expect(placeOf(l, 4)).toEqual({ top: 30 * 4 + 20 * 3, col: 0 });
    expect(placeOf(l, 7)).toEqual({ top: 30 * 4 + 20 * 4, col: 0 });
    expect(l.height).toBe(7 * 30 + 8 * 20);
  });
  it("windows hold exactly what can be seen", () => {
    for (const cols of [1, 3]) for (const closed of [[], ["dir"], ["ext:png", "none"], groups!.map((g) => g.id)]) {
      const { visible, l } = layout(closed, cols);
      for (let y = 0; y < l.height + 40; y += 7) {
        const { from, to, heads } = windowOf(l, y, 50, 0);
        const shown = visible.map((_, i) => i).filter((i) => placeOf(l, i).top + 20 > y && placeOf(l, i).top < y + 50);
        for (const i of shown) expect(i >= from && i < to).toBe(true);
        expect(to - from).toBeLessThanOrEqual(shown.length + 2 * cols);
        for (const s of l.sections) if (s.top + 30 > y && s.top < y + 50) expect(heads).toContain(s);
      }
    }
    expect(windowOf(layoutOf(0, [], new Set(), 1, 20, 30), 0, 50, 0)).toEqual({ from: 0, to: 0, heads: [] });
    expect(windowOf(layoutOf(1000, null, new Set(), 1, 20, 30), 2000, 100, 0)).toEqual({ from: 100, to: 105, heads: [] });
  });
  it("moves the cursor through headers and entries", () => {
    const { l } = layout(["ext:gz"]);
    expect(moveCursor(l, null, 1)).toEqual({ head: "dir" });
    expect(moveCursor(l, { head: "dir" }, 1)).toEqual({ index: 0 });
    expect(moveCursor(l, { index: 1 }, 1)).toEqual({ head: "ext:gz" });
    expect(moveCursor(l, { head: "ext:gz" }, 1)).toEqual({ head: "ext:md" });
    expect(moveCursor(l, { index: 2 }, -1)).toEqual({ head: "ext:md" });
    expect(moveCursor(l, { index: 1 }, 1, true)).toEqual({ index: 2 });
    expect(moveCursor(l, { index: 2 }, -1, true)).toEqual({ index: 1 });
    expect(moveCursor(l, { index: 0 }, -1, true)).toEqual({ index: 0 });
    expect(moveCursor(l, { index: 0 }, -99)).toEqual({ head: "dir" });
    expect(moveCursor(l, { head: "dir" }, 99)).toEqual({ index: 12 });
    expect(moveCursor(layoutOf(0, [], new Set(), 1, 20, 30), null, 1)).toBeNull();
  });
  it("keeps the column of a grid", () => {
    const { l } = layout([], 3);
    expect(moveCursor(l, { index: 5 }, 1)).toEqual({ index: 7 });
    expect(moveCursor(l, { index: 5 }, -1)).toEqual({ head: "ext:png" });
    expect(moveCursor(layoutOf(10, null, new Set(), 4, 20, 30), { index: 1 }, 1)).toEqual({ index: 5 });
    expect(moveCursor(layoutOf(10, null, new Set(), 4, 20, 30), { index: 7 }, 1)).toEqual({ index: 9 });
    expect(moveCursor(layoutOf(10, null, new Set(), 4, 20, 30), { index: 2 }, -1)).toEqual({ index: 2 });
  });
});

it("grouping 100 000 entries costs no more than sorting them", () => {
  const many = Array.from({ length: 100_000 }, (_, i) => entry(`file ${(i * 7919) % 100_000}.t${(i * 7919) % 40}`, false, (i * 31) % 5000, (i * 104_729) % 1_000_003));
  const best = (run: () => void) => Math.min(...[1, 2, 3, 4, 5].map(() => { const t = performance.now(); run(); return performance.now() - t; }));
  const s: SortSpec = { ...byName, key: "modified" };
  const plain = best(() => sortEntries(many, s)), grouped = best(() => groupEntries(many, s, spec("foldersType")));
  console.info(`sort ${plain.toFixed(0)} ms, grouped sort ${grouped.toFixed(0)} ms`);
  expect(grouped).toBeLessThan(plain * 1.5 + 30);
});
