import { describe, expect, it } from "vitest";
import type { DirItem, DirStatus } from "../api/types";
import { planSync, visibleRows } from "./syncPlan";

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

describe("sync plan", () => {
  it("update → copies missing and newer-on-left only", () => {
    const p = planSync(items, "/L", "/R", "toRight", "update");
    expect(p.ops).toEqual([
      { op: "copy", src: "/L/docs/a.txt", dest: "/R/docs/a.txt" },
      { op: "copy", src: "/L/docs/new", dest: "/R/docs/new" },
    ]);
    expect(p.replaces).toBe(1);
    expect(p.trashes).toBe(0);
  });

  it("Windows roots get native separators", () => {
    const p = planSync(items, "C:\\L", "D:\\R\\", "toRight", "mirror");
    expect(p.ops[0]).toEqual({ op: "copy", src: "C:\\L\\docs\\a.txt", dest: "D:\\R\\docs\\a.txt" });
    expect(p.ops.find((o) => o.op === "trash")).toEqual({ op: "trash", path: "D:\\R\\old.txt" });
  });

  it("mirror → copies every difference and trashes extras (top-most only)", () => {
    const p = planSync(items, "/L", "/R", "toRight", "mirror");
    expect(p.ops.map((o) => (o.op === "copy" ? `copy ${o.dest}` : `trash ${o.path}`))).toEqual([
      "copy /R/docs/a.txt", "copy /R/docs/b.txt", "copy /R/docs/new", "trash /R/old.txt",
    ]);
    expect(p.actions.get("old.txt")).toBe("trashRight");
    expect(p.actions.has("docs/new/x.txt")).toBe(false);
  });

  it("← direction is symmetric", () => {
    const p = planSync(items, "/L", "/R", "toLeft", "update");
    expect(p.ops).toEqual([
      { op: "copy", src: "/R/docs/b.txt", dest: "/L/docs/b.txt" },
      { op: "copy", src: "/R/old.txt", dest: "/L/old.txt" },
    ]);
  });

  it("selected rows include their descendants", () => {
    const p = planSync(items, "/L", "/R", "toRight", "selected", new Set(["docs"]));
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
