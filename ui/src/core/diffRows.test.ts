import { describe, expect, it } from "vitest";
import type { DiffOp } from "../api/types";
import { applyBlock, buildSplit, buildUnified, joinLines, moveTarget, offsets, rowAt, segments, visualWidth, wrappedLines } from "./diffRows";

const eq = (a: number, b: number, len: number): DiffOp => ({ tag: "equal", a, aLen: len, b, bLen: len });
const all = { context: Infinity, expanded: new Set<string>() };

describe("diff rows", () => {
  // a: 0..9 equal, line 10 replaced, 11..30 equal, then 1 inserted line
  const ops: DiffOp[] = [eq(0, 0, 10), { tag: "replace", a: 10, aLen: 1, b: 10, bLen: 1 }, eq(11, 11, 20), { tag: "insert", a: 31, aLen: 0, b: 31, bLen: 1 }];
  const pairs = [{ a: 10, b: 10, aSpans: [[2, 4]] as [number, number][], bSpans: [[2, 5]] as [number, number][] }];

  it("split view pairs changed lines and marks blocks", () => {
    const { rows, blocks } = buildSplit(ops, pairs, all);
    expect(rows.length).toBe(10 + 1 + 20 + 1);
    expect(blocks).toEqual([10, 31]);
    const chg = rows[10];
    expect(chg.kind === "line" && chg.l?.t).toBe("chg");
    expect(chg.kind === "line" && chg.r?.spans).toEqual([[2, 5]]);
    const ins = rows[31];
    expect(ins.kind === "line" && !ins.l && ins.r?.t).toBe("ins");
  });

  it("folds long unchanged runs, keeping context, and expands on request", () => {
    const o = { context: 3, expanded: new Set<string>() };
    const { rows } = buildSplit(ops, pairs, o);
    const folds = rows.filter((r) => r.kind === "fold");
    // leading run keeps 3 before the change; middle run keeps 3 + 3
    expect(folds.map((f) => f.kind === "fold" && f.count)).toEqual([7, 14]);
    const id = folds[1].kind === "fold" ? folds[1].id : "";
    const open = buildSplit(ops, pairs, { context: 3, expanded: new Set([id]) }).rows.filter((r) => r.kind === "fold");
    expect(open.length).toBe(1);
  });

  it("unified view lists deletions before insertions", () => {
    const { rows, blocks } = buildUnified(ops, pairs, all);
    const b0 = blocks[0];
    const t = (i: number) => { const r = rows[i]; return r.kind === "line" ? r.t : "fold"; };
    expect(t(b0)).toBe("del");
    expect(t(b0 + 1)).toBe("ins");
    expect(rows.length).toBe(10 + 2 + 20 + 1);
  });

  it("segments by UTF-16 spans", () => {
    expect(segments("let x = 1", [[4, 5]])).toEqual([["let ", false, 0], ["x", true, 0], [" = 1", false, 0]]);
    expect(segments("abc")).toEqual([["abc", false, 0]]);
  });
  it("combines changed spans with syntax runs", () => {
    // "let" keyword (5), "1" number (3); "x = 1" changed
    expect(segments("let x = 1", [[4, 9]], [[0, 3, 5], [8, 9, 3]])).toEqual([
      ["let", false, 5], [" ", false, 0], ["x = ", true, 0], ["1", true, 3],
    ]);
    expect(segments("ab", undefined, [[0, 2, 1]])).toEqual([["ab", false, 1]]);
  });
});

describe("merge", () => {
  const L = ["a", "b", "c", "d"];
  const R = ["a", "B", "B2", "d", "e"];
  it("copies a replace block either way", () => {
    const op = { tag: "replace" as const, a: 1, aLen: 2, b: 1, bLen: 2 };
    expect(applyBlock(L, R, op, "toRight").right).toEqual(["a", "b", "c", "d", "e"]);
    expect(applyBlock(L, R, op, "toLeft").left).toEqual(["a", "B", "B2", "d"]);
  });
  it("insert/delete blocks", () => {
    const ins = { tag: "insert" as const, a: 4, aLen: 0, b: 4, bLen: 1 };
    expect(applyBlock(L, R, ins, "toRight").right).toEqual(["a", "B", "B2", "d"]); // remove extra line
    expect(applyBlock(L, R, ins, "toLeft").left).toEqual(["a", "b", "c", "d", "e"]);
  });
  it("keeps line endings", () => {
    expect(joinLines(["x", "y"], true, true)).toBe("x\r\ny\r\n");
    expect(joinLines(["x"], false, false)).toBe("x");
  });
});

describe("pairing inside a change block", () => {
  it("aligns paired lines and puts unpaired ones side by side", () => {
    // left 0..2 replaced by right 0..3; left 1 pairs with right 2 (right 0, 1 are new)
    const ops: DiffOp[] = [{ tag: "replace", a: 0, aLen: 3, b: 0, bLen: 4 }];
    const pairs = [{ a: 1, b: 2, aSpans: [[0, 1]] as [number, number][], bSpans: [[0, 2]] as [number, number][] }];
    const { rows } = buildSplit(ops, pairs, all);
    const view = rows.map((r) => (r.kind === "line" ? `${r.l ? r.l.t + r.l.n : "-"}|${r.r ? r.r.t + r.r.n : "-"}` : "fold"));
    expect(view).toEqual(["del0|ins0", "-|ins1", "chg1|chg2", "del2|ins3"]);
  });
});

describe("moved lines", () => {
  // left 0..1 deleted, right 3..4 inserted: the same two lines, moved
  const ops: DiffOp[] = [{ tag: "delete", a: 0, aLen: 2, b: 0, bLen: 0 }, eq(2, 0, 3), { tag: "insert", a: 5, aLen: 0, b: 3, bLen: 2 }];
  const moves = [{ a: 0, b: 3, len: 2 }];
  it("marks moved cells and finds the other end", () => {
    const { rows } = buildSplit(ops, [], { ...all, moves });
    const l = rows[0].kind === "line" ? rows[0].l : undefined;
    expect(l?.mv).toBe(0);
    expect(moveTarget(moves, 0, "l", 1)).toBe(4);
    expect(moveTarget(moves, 0, "r", 3)).toBe(0);
    const u = buildUnified(ops, [], { ...all, moves }).rows;
    expect(u.filter((r) => r.kind === "line" && r.mv === 0).length).toBe(4);
  });
});

describe("line wrapping", () => {
  it("expands tabs to tab stops", () => {
    expect(visualWidth("abc")).toBe(3);
    expect(visualWidth("\tx")).toBe(9);
    expect(visualWidth("ab\tx")).toBe(9);
  });
  it("counts screen lines, at least one", () => {
    expect(wrappedLines("", 80)).toBe(1);
    expect(wrappedLines("x".repeat(80), 80)).toBe(1);
    expect(wrappedLines("x".repeat(81), 80)).toBe(2);
    expect(wrappedLines("x".repeat(10), 0)).toBe(10);
  });
  it("finds the row at a y position", () => {
    const off = offsets([20, 40, 20]);
    expect(off).toEqual([0, 20, 60, 80]);
    expect(rowAt(off, 0)).toBe(0);
    expect(rowAt(off, 19)).toBe(0);
    expect(rowAt(off, 20)).toBe(1);
    expect(rowAt(off, 59)).toBe(1);
    expect(rowAt(off, 60)).toBe(2);
    expect(rowAt(off, 500)).toBe(2);
    expect(rowAt(offsets([]), 10)).toBe(0);
  });
});
