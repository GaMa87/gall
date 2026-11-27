import { describe, expect, it } from "vitest";
import type { DirItem, DirStatus } from "../api/types";
import { planSync, sidePath, syncBlocked, visibleRows } from "./syncPlan";

const m = (size = 10, mtime = 100) => ({ isDir: false, size, mtime });
const d = { isDir: true, size: 0, mtime: 0 };
function item(rel: string, status: DirStatus, o: Partial<DirItem> = {}): DirItem {
  const isDir = o.isDir ?? false;
  const meta = isDir ? d : m();
  return {
    rel, name: rel.split("/").pop()!, depth: rel.split("/").length - 1, isDir, status,
    reason: status === "different" ? (isDir ? "content" : "size") : null, newer: null,
    left: status === "rightOnly" ? null : meta, right: status === "leftOnly" ? null : meta, ...o,
  };
}

// tree: docs/ (different) → a.txt newer left, b.txt newer right, new/ (left only) → x.txt ; old.txt right only; same.txt
const items: DirItem[] = [
  item("docs", "different", { isDir: true }),
  item("docs/a.txt", "different", { newer: "left" }),
  item("docs/b.txt", "different", { newer: "right" }),
  item("docs/new", "leftOnly", { isDir: true }),
  item("docs/new/x.txt", "leftOnly"),
  item("old.txt", "rightOnly"),
  item("same.txt", "same"),
];

const snapshot = (items: DirItem[], truncated = false) => ({ items, truncated, incompleteReasons: [] });

describe("sync plan", () => {
  it("update → copies missing and newer-on-left only", () => {
    const p = planSync(snapshot(items), "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([
      { op: "copy", src: "/L/docs/a.txt", dest: "/R/docs/a.txt" },
      { op: "copy", src: "/L/docs/new", dest: "/R/docs/new" },
    ]);
    expect(p.replaces).toBe(1);
    expect(p.trashes).toBe(0);
  });

  it("Windows roots get native separators", () => {
    const p = planSync(snapshot(items), "C:\\L", "D:\\R\\", "toRight", "mirror");
    expect(p.ops[0]).toEqual({ op: "copy", src: "C:\\L\\docs\\a.txt", dest: "D:\\R\\docs\\a.txt" });
    expect(p.ops.find((o) => o.op === "trash")).toEqual({ op: "trash", path: "D:\\R\\old.txt" });
  });

  it("mirror → copies every difference and trashes extras (top-most only)", () => {
    const p = planSync(snapshot(items), "/L", "/R", "toRight", "mirror");
    expect(p.ops.map((o) => (o.op === "copy" ? `copy ${o.dest}` : `trash ${o.path}`))).toEqual([
      "copy /R/docs/a.txt", "copy /R/docs/b.txt", "copy /R/docs/new", "trash /R/old.txt",
    ]);
    expect(p.actions.get("old.txt")).toBe("trashRight");
    expect(p.actions.has("docs/new/x.txt")).toBe(false);
  });

  it("← direction is symmetric", () => {
    const p = planSync(snapshot(items), "/L", "/R", "toLeft", "update");
    expect(p.ops).toEqual([
      { op: "copy", src: "/R/docs/b.txt", dest: "/L/docs/b.txt" },
      { op: "copy", src: "/R/old.txt", dest: "/L/old.txt" },
    ]);
  });

  it("selected rows include their descendants", () => {
    const p = planSync(snapshot(items), "/L", "/R", "toRight", "selected", new Set(["docs"]));
    expect(p.ops.length).toBe(3); // a, b (selected = no newer check), new/
    expect(p.ops.some((o) => o.op === "trash")).toBe(false);
  });

  it("visible rows keep ancestors of matches and honour collapsed folders", () => {
    const diffOnly = visibleRows(items, (it) => it.status !== "same" && !it.isDir, new Set());
    expect(diffOnly.map((r) => r.rel)).toEqual(["docs", "docs/a.txt", "docs/b.txt", "docs/new", "docs/new/x.txt", "old.txt"]);
    const collapsed = visibleRows(items, () => true, new Set(["docs"]));
    expect(collapsed.map((r) => r.rel)).toEqual(["docs", "old.txt", "same.txt"]);
  });
});

describe("syncBlocked", () => {
  it("refuses update and mirror on a truncated comparison", () => {
    expect(syncBlocked("update", snapshot([], true))).toMatch(/incomplete/);
    expect(syncBlocked("mirror", snapshot([], true))).toMatch(/incomplete/);
  });
  it("allows hand-picked copies, and everything on a complete comparison", () => {
    expect(syncBlocked("selected", snapshot([], true))).toBeNull();
    expect(syncBlocked("update", snapshot([], false))).toBeNull();
    expect(syncBlocked("mirror", snapshot([], false))).toBeNull();
  });
  it("never plans writes when a scan misses entries or finds ambiguous paths", () => {
    for (const reason of ["traversal", "metadata", "pathIdentity"] as const) {
      const incomplete = {
        ...snapshot([item("inaccessible/keep.txt", "rightOnly"), item("other.txt", "leftOnly")]),
        incompleteReasons: [reason],
      };
      expect(planSync(incomplete, "/L", "/R", "toRight", "mirror").ops).toEqual([]);
      expect(planSync(incomplete, "/L", "/R", "toLeft", "mirror").ops).toEqual([]);
      expect(planSync(incomplete, "/L", "/R", "toRight", "update").ops).toEqual([]);
      expect(planSync(incomplete, "/L", "/R", "toRight", "selected", new Set(["other.txt"])).ops).toEqual([]);
    }
    const truncated = snapshot([item("keep.txt", "rightOnly")], true);
    expect(planSync(truncated, "/L", "/R", "toRight", "mirror").ops).toEqual([]);
  });
});

describe("planSync with differently spelled names", () => {
  const nfc = "Poro\u{10d}ilo.txt";
  const nfd = "Poroc\u{30c}ilo.txt";

  it("reads and replaces the files by the spelling each side really has", () => {
    // Left has the composed name, right the decomposed one; the left copy is newer.
    const row = item(nfc, "different", { newer: "left", rightRel: nfd });
    const toRight = planSync(snapshot([row]), "/L", "/R", "toRight", "update");
    expect(toRight.ops).toEqual([{ op: "copy", src: `/L/${nfc}`, dest: `/R/${nfd}` }]);
    const row2 = item(nfc, "different", { newer: "right", rightRel: nfd });
    const toLeft = planSync(snapshot([row2]), "/L", "/R", "toLeft", "update");
    expect(toLeft.ops).toEqual([{ op: "copy", src: `/R/${nfd}`, dest: `/L/${nfc}` }]);
  });

  it("copies a one-sided file under its own spelling and trashes the real path", () => {
    const only = item(nfc, "leftOnly", { leftRel: nfd });
    expect(planSync(snapshot([only]), "/L", "/R", "toRight", "update").ops).toEqual([{ op: "copy", src: `/L/${nfd}`, dest: `/R/${nfd}` }]);
    const extra = item(nfc, "rightOnly", { rightRel: nfd });
    expect(planSync(snapshot([extra]), "/L", "/R", "toRight", "mirror").ops).toEqual([{ op: "trash", path: `/R/${nfd}` }]);
  });

  it("keeps plain names exactly as before", () => {
    const p = planSync(snapshot([item("a.txt", "leftOnly")]), "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([{ op: "copy", src: "/L/a.txt", dest: "/R/a.txt" }]);
  });
});

describe("sidePath", () => {
  const nfc = "Poro\u{10d}ilo.txt";
  const nfd = "Poroc\u{30c}ilo.txt";

  it("joins the name each side really has, for opening the two files", () => {
    const row = item(`docs/${nfc}`, "different", { rightRel: `docs/${nfd}` });
    expect(sidePath("/L", row, "left")).toBe(`/L/docs/${nfc}`);
    expect(sidePath("/R", row, "right")).toBe(`/R/docs/${nfd}`);
    const left = item(`docs/${nfc}`, "leftOnly", { leftRel: `docs/${nfd}` });
    expect(sidePath("/L", left, "left")).toBe(`/L/docs/${nfd}`);
  });

  it("is the plain join when the spelling is the same", () => {
    expect(sidePath("/L", item("a/b.txt", "same"), "left")).toBe("/L/a/b.txt");
    expect(sidePath("C:\\L", item("a/b.txt", "same"), "right")).toBe("C:\\L\\a\\b.txt");
  });
});

describe("where a path that exists on one side only is copied", () => {
  const nfc = "caf\u00e9", nfd = "cafe\u0301";
  it("goes under the destination's own spelling of the nearest folder above it", () => {
    // Left has the composed `café/new.txt`, right the decomposed `café/`: the folders are one row.
    const rows: DirItem[] = [
      item(nfc, "different", { isDir: true, leftRel: nfc, rightRel: nfd }),
      item(`${nfc}/new.txt`, "leftOnly", { leftRel: `${nfc}/new.txt` }),
    ];
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([{ op: "copy", src: `/L/${nfc}/new.txt`, dest: `/R/${nfd}/new.txt` }]);
    // And the other way round: a right-only file under a folder that is composed on the left.
    const back = planSync(snapshot([
      item(nfc, "different", { isDir: true, leftRel: nfc, rightRel: nfd }),
      item(`${nfc}/old.txt`, "rightOnly", { rightRel: `${nfd}/old.txt` }),
    ]), "/L", "/R", "toLeft", "update");
    expect(back.ops).toEqual([{ op: "copy", src: `/R/${nfd}/old.txt`, dest: `/L/${nfc}/old.txt` }]);
  });

  it("uses the nearest folder that exists on the destination, with the new folders below it in the source's spelling", () => {
    const rows: DirItem[] = [
      item(nfc, "different", { isDir: true, leftRel: nfc, rightRel: nfd }),
      item(`${nfc}/sub`, "leftOnly", { isDir: true, leftRel: `${nfc}/sub` }),
      item(`${nfc}/sub/x.txt`, "leftOnly", { leftRel: `${nfc}/sub/x.txt` }),
    ];
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "update");
    // The new folder is copied whole into the existing decomposed one; its file is covered by it.
    expect(p.ops).toEqual([{ op: "copy", src: `/L/${nfc}/sub`, dest: `/R/${nfd}/sub` }]);
  });

  it("keeps the source spelling at the top and when no folder above exists on the destination", () => {
    const rows: DirItem[] = [item("only.txt", "leftOnly"), item("newdir", "leftOnly", { isDir: true }), item("newdir/f.txt", "leftOnly")];
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([
      { op: "copy", src: "/L/only.txt", dest: "/R/only.txt" },
      { op: "copy", src: "/L/newdir", dest: "/R/newdir" },
    ]);
  });

  it("falls back to the source spelling when an ancestor does not exist on the destination", () => {
    // The row above exists on the left only: nothing to place the file under.
    const rows: DirItem[] = [item("a", "leftOnly", { isDir: true }), item("a/b.txt", "leftOnly")];
    expect(planSync(snapshot(rows), "/L", "/R", "toRight", "selected", new Set(["a/b.txt"])).ops).toEqual([{ op: "copy", src: "/L/a/b.txt", dest: "/R/a/b.txt" }]);
  });
});

describe("an unreadable file", () => {
  it("is never replaced, and the whole scan is blocked while it is reported", () => {
    const rows: DirItem[] = [item("a.txt", "different", { reason: "unreadable" }), item("b.txt", "different", { reason: "content" })];
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "mirror");
    expect(p.ops).toEqual([{ op: "copy", src: "/L/b.txt", dest: "/R/b.txt" }]);
    expect(syncBlocked("selected", { truncated: false, incompleteReasons: ["read"] })).not.toBeNull();
    const blocked = planSync({ items: rows, truncated: false, incompleteReasons: ["read"] }, "/L", "/R", "toRight", "mirror");
    expect(blocked.ops).toEqual([]);
  });
});

describe("sync plan with filters", () => {
  // The right folder `keep/` has a listed file and a file the filters left out (so it is flagged); `gone/` only has
  // listed files. The left folder `fresh/` is flagged too.
  const rows: DirItem[] = [
    item("keep", "rightOnly", { isDir: true, hasExcluded: true }),
    item("keep/listed.txt", "rightOnly"),
    item("gone", "rightOnly", { isDir: true }),
    item("gone/a.txt", "rightOnly"),
    item("fresh", "leftOnly", { isDir: true, hasExcluded: true }),
    item("fresh/new.txt", "leftOnly"),
    item("fresh/sub", "leftOnly", { isDir: true }),
    item("fresh/sub/deep.txt", "leftOnly"),
  ];
  it("a mirror trashes only the listed paths below a folder that holds left-out files, never the folder", () => {
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "mirror");
    const trashed = p.ops.filter((o) => o.op === "trash").map((o) => (o.op === "trash" ? o.path : ""));
    expect(trashed).toEqual(["/R/keep/listed.txt", "/R/gone"]);
    expect(trashed).not.toContain("/R/keep");
    expect(p.actions.has("keep")).toBe(false);
  });

  it("a folder that holds only hidden files (flagged, no listed child) is never trashed and has no operation of its own", () => {
    const only: DirItem[] = [item("hidden-only", "rightOnly", { isDir: true, hasExcluded: true })];
    const p = planSync(snapshot(only), "/L", "/R", "toRight", "mirror");
    expect(p.ops).toEqual([]);
    expect(p.checks).toEqual([]);
    expect(p.actions.has("hidden-only")).toBe(false);
    // The same folder only on the left is not copied as a whole either (its hidden content is not listed).
    const left: DirItem[] = [item("hidden-only", "leftOnly", { isDir: true, hasExcluded: true })];
    expect(planSync(snapshot(left), "/L", "/R", "toRight", "update").ops).toEqual([]);
  });

  it("a copy of a folder that holds left-out files is made file by file, with whole subfolders that hold none", () => {
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "update");
    const copies = p.ops.filter((o) => o.op === "copy").map((o) => (o.op === "copy" ? o.dest : ""));
    expect(copies).toEqual(["/R/fresh/new.txt", "/R/fresh/sub"]);
    expect(copies).not.toContain("/R/fresh");
  });

  it("a folder that is a file on the other side is not replaced when it holds left-out files", () => {
    const type: DirItem[] = [item("x", "different", { isDir: true, reason: "type", hasExcluded: true, newer: "left" })];
    expect(planSync(snapshot(type), "/L", "/R", "toRight", "mirror").ops).toEqual([]);
    const plain: DirItem[] = [item("x", "different", { isDir: true, reason: "type", newer: "left", left: d, right: m() })];
    expect(planSync(snapshot(plain), "/L", "/R", "toRight", "mirror").ops).toHaveLength(1);
  });

  it("selecting a flagged folder acts on its listed descendants only", () => {
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "selected", new Set(["keep"]));
    expect(p.ops).toEqual([]);
  });
});

describe("sync plan checks", () => {
  it("lists the source and the destination of a file copy and the trashed file, with the stamps of the comparison", () => {
    const rows: DirItem[] = [
      item("a.txt", "leftOnly", { left: { isDir: false, size: 5, mtime: 50 }, right: null }),
      item("b.txt", "different", { newer: "left", left: { isDir: false, size: 7, mtime: 70 }, right: { isDir: false, size: 3, mtime: 30 } }),
      item("old.txt", "rightOnly", { left: null, right: { isDir: false, size: 9, mtime: 90 } }),
      item("dir", "leftOnly", { isDir: true }),
    ];
    const p = planSync(snapshot(rows), "/L", "/R", "toRight", "mirror");
    expect(p.checks).toEqual([
      { path: "/L/a.txt", stamp: { size: 5, mtime: 50 } },
      { path: "/R/a.txt", stamp: null },
      { path: "/L/b.txt", stamp: { size: 7, mtime: 70 } },
      { path: "/R/b.txt", stamp: { size: 3, mtime: 30 } },
      { path: "/R/old.txt", stamp: { size: 9, mtime: 90 } },
      { path: "/L/dir", stamp: null, dir: true },
      { path: "/R/dir", stamp: null },
    ]);
  });
  it("a blocked plan has no checks and no ops", () => {
    const p = planSync({ items: [item("a.txt", "leftOnly")], truncated: true, incompleteReasons: [] }, "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([]);
    expect(p.checks).toEqual([]);
  });
});

describe("sync plan: folders and type conflicts", () => {
  it("a folder trashed as a whole must still be a folder", () => {
    const p = planSync(snapshot([item("gone", "rightOnly", { isDir: true })]), "/L", "/R", "toRight", "mirror");
    expect(p.checks).toEqual([{ path: "/R/gone", stamp: null, dir: true }]);
  });
  it("a folder on the destination is never replaced by a file", () => {
    const fileLeftFolderRight: DirItem[] = [item("notes", "different", { isDir: true, reason: "type", newer: "left", left: { isDir: false, size: 3, mtime: 1 }, right: d })];
    expect(planSync(snapshot(fileLeftFolderRight), "/L", "/R", "toRight", "mirror").ops).toEqual([]);
    // A file on the destination may be replaced by the folder of the source.
    const folderLeftFileRight: DirItem[] = [item("notes", "different", { isDir: true, reason: "type", newer: "left", left: d, right: { isDir: false, size: 3, mtime: 1 } })];
    expect(planSync(snapshot(folderLeftFileRight), "/L", "/R", "toRight", "mirror").ops).toHaveLength(1);
  });
});
