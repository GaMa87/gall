import { describe, expect, it } from "vitest";
import type { DiffOp, GeneratedReason, OutlineNode } from "../api/types";
import { blocksIn, changeCount, collapsedKeyBehavior, collapsedRows, GENERATED_REASON, generatedSummary, idsWithChanges, outlineErrorMessage, parentIds, pressKey, rowLabel, stepVisible, suppressed, visibleRows, type OutlineState } from "./outline";

const node = (kind: OutlineNode["kind"], name: string | null, line: number, endLine: number, children: OutlineNode[] = []): OutlineNode =>
  ({ kind, name, span: { start: 0, end: 0, line }, endLine, children });

// module m (1-10) { struct S (2-2), impl S (3-9) { method a (4-5), method b (6-8) { closure (7-7) } } } ; function top (11-12)
const tree: OutlineNode[] = [
  node("module", "m", 1, 10, [node("struct", "S", 2, 2), node("impl", "S", 3, 9, [node("method", "a", 4, 5), node("method", "b", 6, 8, [node("closure", null, 7, 7)])])]),
  node("function", "top", 11, 12),
];

const state = (active: string | null, ...open: string[]): OutlineState => ({ active, expanded: new Set(open) });

describe("outline rows", () => {
  it("lists only what is open and gives path ids", () => {
    expect(visibleRows(tree, new Set()).map((r) => r.id)).toEqual(["0", "1"]);
    const rows = visibleRows(tree, new Set(["0", "0.1"]));
    expect(rows.map((r) => `${r.id}:${r.depth}`)).toEqual(["0:0", "0.0:1", "0.1:1", "0.1.0:2", "0.1.1:2", "1:0"]);
    expect(rows[2]).toMatchObject({ hasChildren: true, open: true });
    expect(rows[1]).toMatchObject({ hasChildren: false, open: false });
    expect(parentIds(tree)).toEqual(["0", "0.1", "0.1.1"]);
  });
});

describe("outline keys (ARIA tree)", () => {
  it("moves with the arrows, Home and End and stays inside the list", () => {
    let s = state("0");
    s = pressKey(tree, s, "ArrowDown").state;
    expect(s.active).toBe("1");
    expect(pressKey(tree, s, "ArrowDown").state.active).toBe("1");
    expect(pressKey(tree, s, "ArrowUp").state.active).toBe("0");
    expect(pressKey(tree, s, "Home").state.active).toBe("0");
    expect(pressKey(tree, state("0"), "End").state.active).toBe("1");
  });

  it("Right opens, then enters; Left closes, then goes to the parent", () => {
    let s = pressKey(tree, state("0"), "ArrowRight").state;
    expect(s.expanded.has("0")).toBe(true);
    expect(s.active).toBe("0");
    s = pressKey(tree, s, "ArrowRight").state;
    expect(s.active).toBe("0.0");
    // A leaf: Right does nothing; Left goes to the parent.
    expect(pressKey(tree, s, "ArrowRight").state.active).toBe("0.0");
    s = pressKey(tree, s, "ArrowLeft").state;
    expect(s.active).toBe("0");
    s = pressKey(tree, s, "ArrowLeft").state;
    expect(s.expanded.has("0")).toBe(false);
    // At the top level a closed node stays where it is.
    expect(pressKey(tree, s, "ArrowLeft").state.active).toBe("0");
  });

  it("Enter and Space activate the row", () => {
    expect(pressKey(tree, state("1"), "Enter")).toMatchObject({ activate: true, state: { active: "1" } });
    expect(pressKey(tree, state("1"), " ").activate).toBe(true);
    expect(pressKey(tree, state("1"), "ArrowDown").activate).toBe(false);
  });

  it("an empty outline does nothing and an unknown active id falls back to the first row", () => {
    expect(pressKey([], state(null), "ArrowDown").state.active).toBeNull();
    expect(pressKey(tree, state("9.9"), "ArrowDown").state.active).toBe("1");
  });
});

describe("changes in nodes", () => {
  // Right side: a replace at line 7 (0-based 6, inside method b and the closure), an insert between lines 11 and 12,
  // a cosmetic block that is no change.
  const ops: DiffOp[] = [
    { tag: "equal", a: 0, aLen: 6, b: 0, bLen: 6 },
    { tag: "replace", a: 6, aLen: 1, b: 6, bLen: 1 },
    { tag: "equal", a: 7, aLen: 3, b: 7, bLen: 3 },
    { tag: "replace", a: 10, aLen: 1, b: 10, bLen: 1, cosmetic: true },
    { tag: "insert", a: 11, aLen: 0, b: 11, bLen: 1 },
  ];
  it("links blocks to the nodes by line and counts each block once", () => {
    const m = tree[0];
    expect(blocksIn(m, ops, "b")).toEqual([0]);
    expect(blocksIn(m.children[1].children[1], ops, "b")).toEqual([0]);
    expect(blocksIn(m.children[1].children[0], ops, "b")).toEqual([]);
    // The cosmetic block is skipped and does not shift the numbering: the insert is block 1.
    expect(blocksIn(tree[1], ops, "b")).toEqual([1]);
    expect(changeCount(m, ops, "b")).toBe(1);
  });
  it("opens the outline to the changes", () => {
    expect([...idsWithChanges(tree, ops, "b")].sort()).toEqual(["0", "0.1", "0.1.1", "0.1.1.0", "1"].sort());
    expect(idsWithChanges(tree, [], "b").size).toBe(0);
  });
  it("uses the left lines for the left side", () => {
    expect(blocksIn(tree[0].children[1].children[1], ops, "a")).toEqual([0]);
  });
});

describe("labels and suppression", () => {
  it("names anonymous nodes and counts changes", () => {
    expect(rowLabel(tree[0], 0)).toBe("module m");
    expect(rowLabel(tree[0].children[1].children[1].children[0], 2)).toBe("closure anonymous, 2 changes");
    expect(rowLabel(tree[1], 1)).toBe("function top, 1 change");
  });
  it("collapses a generated file until it is revealed for this view", () => {
    expect(suppressed("lockfile", false)).toBe(true);
    expect(suppressed("lockfile", true)).toBe(false);
    expect(suppressed(null, false)).toBe(false);
    expect(suppressed(undefined, false)).toBe(false);
  });
});

describe("generated files in the Git changes list", () => {
  const rows = [
    { path: "src/a.ts", staged: true }, { path: "package-lock.json", staged: true }, { path: "Cargo.lock", staged: true },
    { path: "src/b.ts", staged: false }, { path: "package-lock.json", staged: false },
  ];
  const generated = new Map<string, GeneratedReason>([["package-lock.json", "lockfile"], ["Cargo.lock", "lockfile"]]);
  it("collapses generated rows per list and keeps every row in the list", () => {
    expect(collapsedRows(rows, generated, { staged: false, unstaged: false })).toEqual([false, true, true, false, true]);
    // Revealing one list leaves the other collapsed.
    expect(collapsedRows(rows, generated, { staged: true, unstaged: false })).toEqual([false, false, false, false, true]);
    expect(collapsedRows(rows, new Map(), { staged: false, unstaged: false })).toEqual([false, false, false, false, false]);
  });
  it("counts the generated files of a list and names the reasons once", () => {
    expect(generatedSummary(rows, generated, true)).toEqual({ count: 2, reasons: ["lockfile"] });
    expect(generatedSummary(rows, generated, false)).toEqual({ count: 1, reasons: ["lockfile"] });
  });
  it("moves the arrow keys over the rows that are drawn", () => {
    const hidden = collapsedRows(rows, generated, { staged: false, unstaged: false });
    expect(stepVisible(hidden, 0, 1)).toBe(3);
    expect(stepVisible(hidden, 3, -1)).toBe(0);
    expect(stepVisible(hidden, 3, 1)).toBe(3);
    expect(stepVisible(hidden, 0, -1)).toBe(0);
  });
});

describe("positions among siblings", () => {
  it("numbers the rows within their parent for screen readers", () => {
    const rows = visibleRows([node("module", "a", 1, 5, [node("function", "a1", 2, 3), node("function", "a2", 4, 5)]), node("function", "b", 6, 7)], new Set(["0"]));
    expect(rows.map((r) => [r.id, r.position, r.siblings])).toEqual([["0", 1, 2], ["0.0", 1, 2], ["0.1", 2, 2], ["1", 2, 2]]);
  });
});

describe("outline errors, extra patterns and collapsed body keys", () => {
  it("names which side has a parse error and its line", () => {
    expect(outlineErrorMessage({ verdict: "unknown", reason: { why: "parseError", side: "left", line: 7 } }))
      .toBe("Could not read the outline of the left side: parse error on line 7.");
    expect(outlineErrorMessage({ verdict: "unknown", reason: { why: "parseError", side: "right", line: 12 } }))
      .toBe("Could not read the outline of the right side: parse error on line 12.");
    expect(outlineErrorMessage({ verdict: "unknown", reason: { why: "unsupported" } }))
      .toBe("No outline for this language.");
  });

  it("maps customPattern reason and guards collapsed body keys without blocking H, y or o", () => {
    expect(GENERATED_REASON.customPattern).toBe("matching pattern");
    for (const k of ["n", "N", "F8", "[", "]", "x", "X"]) {
      expect(collapsedKeyBehavior(k, false, false)).toBe("notify");
    }
    expect(collapsedKeyBehavior("ArrowDown", false, true)).toBe("notify");
    for (const k of ["j", "k", "ArrowDown", "ArrowUp", "PageDown", "PageUp", " ", "g", "G", "Home", "End"]) {
      expect(collapsedKeyBehavior(k, false, false)).toBe("ignore");
    }
    expect(collapsedKeyBehavior("d", true, false)).toBe("ignore");
    for (const k of ["H", "y", "p", "o", "O", "S", "E", "s", "r", "Escape"]) {
      expect(collapsedKeyBehavior(k, false, false)).toBe("allow");
    }
  });
});
