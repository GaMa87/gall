import { describe, expect, it } from "vitest";
import type { MergeChunkInfo, MergeModel } from "../api/types";
import { CONTEXT, bothRepeatsLines, cardOutput, chunkLines, commonLineIndices, compose, gapHasLines, lineRange, overview, stepCard, type Sources } from "./mergeCards";

type Part =
  | { k: "stable"; lines: string[] }
  | { k: "conflict" | "left" | "right" | "same"; base: string[]; left: string[]; right: string[] };

/** A model and its three sources from parts in order, with the spans a merge of them would have. */
function fixture(parts: Part[]): { model: MergeModel; sources: Sources } {
  const sources: Sources = { base: [], left: [], right: [] };
  const chunks: MergeChunkInfo[] = parts.map((part, i) => {
    const [base, left, right] = part.k === "stable" ? [part.lines, part.lines, part.lines] : [part.base, part.left, part.right];
    const span = (source: string[], lines: string[]) => ({ start: source.length, len: lines.length });
    const chunk: MergeChunkInfo = {
      id: part.k === "stable" ? `s${i}` : `${part.k[0]}${i}`,
      kind: part.k,
      base: span(sources.base, base),
      left: span(sources.left, left),
      right: span(sources.right, right),
      baseLines: part.k === "stable" ? [] : base,
      leftLines: part.k === "stable" ? [] : left,
      rightLines: part.k === "stable" ? [] : right,
      combinedLines: null,
    };
    sources.base.push(...base);
    sources.left.push(...left);
    sources.right.push(...right);
    return chunk;
  });
  return { model: { chunks, conflicts: chunks.filter((c) => c.kind === "conflict").map((c) => c.id) }, sources };
}

const run = (from: number, count: number, tag = "l") => Array.from({ length: count }, (_, i) => `${tag}${from + i}\n`);
const conflict = (n: number, len = 1): Part => ({ k: "conflict", base: run(n, len, "b"), left: run(n, len, "L"), right: run(n, len, "R") });

describe("overview", () => {
  it("gives each conflict a card with three lines of context and leaves the rest in a shared gap", () => {
    // 30 lines; conflicts at lines 10 and 20.
    const { model, sources } = fixture([{ k: "stable", lines: run(0, 10) }, conflict(10), { k: "stable", lines: run(11, 9) }, conflict(20), { k: "stable", lines: run(21, 9) }]);
    const o = overview(model, sources);
    expect(o.cards.map((c) => [c.id, c.number])).toEqual([["c1", 1], ["c3", 2]]);
    expect(o.cards[0].base.before).toEqual(run(7, CONTEXT));
    expect(o.cards[0].base.after).toEqual(run(11, CONTEXT));
    expect(o.cards[1].left.before).toEqual(run(17, CONTEXT));
    expect(o.cards[1].right.after).toEqual(run(21, CONTEXT));
    expect(o.gaps).toHaveLength(3);
    // Before the first card 7 lines are left out, between the cards 3, after the last 6.
    expect(o.gaps.map((g) => g.hidden.base)).toEqual([7, 3, 6]);
    expect(o.gaps[0].lines.base).toEqual(run(0, 7));
    expect(o.gaps[1].lines.base).toEqual(run(14, 3));
    expect(o.gaps[2].lines.base).toEqual(run(24, 6));
    expect(o.gaps.map((g) => [g.above, g.below])).toEqual([[null, "c1"], ["c1", "c3"], ["c3", null]]);
    // The three panes agree here.
    for (const gap of o.gaps) expect(new Set([gap.hidden.base, gap.hidden.left, gap.hidden.right]).size).toBe(1);
  });

  it("shares the lines between two close conflicts: nothing is shown twice and nothing is left out", () => {
    // Conflicts at 10 and 14: three lines (11, 12, 13) between them, split two and one.
    const { model, sources } = fixture([{ k: "stable", lines: run(0, 10) }, conflict(10), { k: "stable", lines: run(11, 3) }, conflict(14), { k: "stable", lines: run(15, 5) }]);
    const o = overview(model, sources);
    expect(o.cards[0].base.after).toEqual(run(11, 2));
    expect(o.cards[1].base.before).toEqual(run(13, 1));
    expect(o.gaps[1].hidden).toEqual({ base: 0, left: 0, right: 0 });
    expect(gapHasLines(o.gaps[1])).toBe(false);
    // Conflicts that touch have no lines between them at all; the first and the last still show their own.
    const close = fixture([{ k: "stable", lines: run(0, 5) }, conflict(5), conflict(6), { k: "stable", lines: run(7, 5) }]);
    const touching = overview(close.model, close.sources);
    expect(touching.cards[0].base.after).toEqual([]);
    expect(touching.cards[1].base.before).toEqual([]);
    expect(touching.cards[0].base.before).toEqual(run(2, 3));
    expect(touching.cards[1].base.after).toEqual(run(7, 3));
  });

  it("splits an odd distance in favour of the card above, up to twice the context", () => {
    for (const between of [1, 2, 3, 4, 5, 6]) {
      const { model, sources } = fixture([{ k: "stable", lines: run(0, 8) }, conflict(8), { k: "stable", lines: run(9, between) }, conflict(9 + between), { k: "stable", lines: run(10 + between, 8) }]);
      const o = overview(model, sources);
      const shown = o.cards[0].base.after.length + o.cards[1].base.before.length;
      expect(shown, `${between} lines apart`).toBe(between);
      expect(o.cards[0].base.after.length).toBe(Math.ceil(between / 2));
      expect(o.gaps[1].hidden.base).toBe(0);
    }
    // One line more than twice the context: the card takes its context and one line stays in the gap.
    const { model, sources } = fixture([{ k: "stable", lines: run(0, 8) }, conflict(8), { k: "stable", lines: run(9, 2 * CONTEXT + 1) }, conflict(9 + 2 * CONTEXT + 1), { k: "stable", lines: run(20, 4) }]);
    const o = overview(model, sources);
    expect(o.cards[0].base.after).toHaveLength(CONTEXT);
    expect(o.cards[1].base.before).toHaveLength(CONTEXT);
    expect(o.gaps[1].hidden.base).toBe(1);
  });

  it("calls an empty side deleted, an empty base added, and tells changed from same", () => {
    const { model, sources } = fixture([
      { k: "stable", lines: run(0, 2) },
      { k: "conflict", base: ["x\n"], left: [], right: ["y\n"] },
      { k: "stable", lines: run(2, 2) },
      { k: "conflict", base: [], left: ["new\n"], right: ["other\n"] },
      { k: "stable", lines: run(4, 2) },
      { k: "conflict", base: ["a\n"], left: ["a\n"], right: ["b\n"] },
    ]);
    const o = overview(model, sources);
    expect(o.cards.map((c) => [c.left.kind, c.right.kind])).toEqual([["deleted", "changed"], ["added", "added"], ["same", "changed"]]);
    expect(o.cards.map((c) => c.base.kind)).toEqual(["same", "same", "same"]);
    // An empty side keeps its span so a pane can say where nothing is.
    expect(o.cards[0].left.span).toEqual({ start: 2, len: 0 });
    expect(o.cards[0].left.lines).toEqual([]);
  });

  it("uses the smallest distance of the three sources, so the panes stay aligned, and hides what only some panes have", () => {
    // The left side added two lines at the top (an auto-merged change): its first gap is longer than the others'.
    const { model, sources } = fixture([
      { k: "stable", lines: run(0, 3) },
      { k: "left", base: [], left: ["x\n", "y\n"], right: [] },
      { k: "stable", lines: run(3, 7) },
      conflict(10),
      { k: "stable", lines: run(11, 9) },
    ]);
    expect(sources.left).toHaveLength(22);
    const o = overview(model, sources);
    const first = o.gaps[0];
    expect(first.hidden).toEqual({ base: 7, left: 9, right: 7 });
    expect(o.cards[0].left.before).toEqual(sources.left.slice(12 - CONTEXT, 12));
    expect(o.cards[0].base.before).toEqual(sources.base.slice(10 - CONTEXT, 10));
    expect(first.lines.left).toEqual(sources.left.slice(0, 9));
  });

  it("has no cards for a merge without a conflict, only one gap with everything", () => {
    const { model, sources } = fixture([{ k: "stable", lines: run(0, 4) }, { k: "left", base: ["a\n"], left: ["A\n"], right: ["a\n"] }, { k: "stable", lines: run(4, 4) }]);
    const o = overview(model, sources);
    expect(o.cards).toEqual([]);
    expect(o.gaps).toHaveLength(1);
    expect(o.gaps[0].above).toBeNull();
    expect(o.gaps[0].hidden).toEqual({ base: 9, left: 9, right: 9 });
    expect(gapHasLines(o.gaps[0])).toBe(true);
    const empty = overview({ chunks: [], conflicts: [] }, { base: [], left: [], right: [] });
    expect(empty.cards).toEqual([]);
    expect(gapHasLines(empty.gaps[0])).toBe(false);
  });
});

describe("stepCard", () => {
  it("wraps in both directions and has nothing to step to without cards", () => {
    expect(stepCard(0, 1, 3)).toBe(1);
    expect(stepCard(2, 1, 3)).toBe(0);
    expect(stepCard(0, -1, 3)).toBe(2);
    expect(stepCard(-1, 1, 3)).toBe(1);
    expect(stepCard(0, 1, 0)).toBe(-1);
  });
});

describe("compose", () => {
  const model = () => fixture([
    { k: "stable", lines: ["a\n"] },
    { k: "conflict", base: ["b\n"], left: ["O1\n"], right: ["T1\n"] },
    { k: "stable", lines: ["c\n"] },
    { k: "conflict", base: ["d\n"], left: ["O2\n"], right: ["T2\n"] },
    { k: "stable", lines: ["e\n"] },
  ]);

  it("builds the result from a choice per conflict and shows an open one between markers", () => {
    const { model: m, sources } = model();
    expect(compose(m, sources.base, { c1: "left", c3: "right" })).toEqual({ text: "a\nO1\nc\nT2\ne\n", unresolved: [] });
    expect(compose(m, sources.base, { c1: "both", c3: "base" }).text).toBe("a\nO1\nT1\nc\nd\ne\n");
    const open = compose(m, sources.base, { c1: "left" });
    expect(open.unresolved).toEqual(["c3"]);
    expect(open.text).toBe("a\nO1\nc\n<<<<<<< ours\nO2\n=======\nT2\n>>>>>>> theirs\ne\n");
  });

  it("takes one-sided and identical changes as they are", () => {
    const { model: m, sources } = fixture([
      { k: "stable", lines: ["a\n"] },
      { k: "left", base: ["b\n"], left: ["B\n"], right: ["b\n"] },
      { k: "right", base: ["c\n"], left: ["c\n"], right: ["C\n"] },
      { k: "same", base: ["d\n"], left: ["x\n"], right: ["x\n"] },
    ]);
    expect(compose(m, sources.base, {})).toEqual({ text: "a\nB\nC\nx\n", unresolved: [] });
  });

  it("keeps CRLF and a missing final newline byte for byte", () => {
    const crlf = fixture([
      { k: "stable", lines: ["a\r\n"] },
      { k: "conflict", base: ["b\r\n"], left: ["L"], right: ["R\r\n"] },
    ]);
    // The left part has no terminator: it is ended with the terminator of the file, CRLF.
    expect(compose(crlf.model, crlf.sources.base, { c1: "both" }).text).toBe("a\r\nL\r\nR\r\n");
    expect(compose(crlf.model, crlf.sources.base, {}).text).toBe("a\r\n<<<<<<< ours\r\nL\r\n=======\r\nR\r\n>>>>>>> theirs\r\n");
    const bare = fixture([{ k: "stable", lines: ["a\n"] }, { k: "conflict", base: ["b"], left: ["ours"], right: ["theirs"] }]);
    expect(compose(bare.model, bare.sources.base, { c1: "both" }).text).toBe("a\nours\ntheirs");
    expect(compose(bare.model, bare.sources.base, { c1: "left" }).text).toBe("a\nours");
    expect(compose(bare.model, bare.sources.base, { c1: "base" }).text).toBe("a\nb");
  });

  it("composes lines the cards leave out exactly: the whole file is the same with every gap hidden", () => {
    const stable = run(0, 5000);
    const { model: m, sources } = fixture([{ k: "stable", lines: stable.slice(0, 2500) }, conflict(2500), { k: "stable", lines: stable.slice(2501) }]);
    const o = overview(m, sources);
    // The cards and gaps cover only a few hundred bytes of the file, yet the result holds every line.
    const shown = o.cards.reduce((n, c) => n + c.base.before.length + c.base.after.length, 0);
    expect(shown).toBe(2 * CONTEXT);
    const text = compose(m, sources.base, { c1: "left" }).text;
    expect(text).toBe([...stable.slice(0, 2500), "L2500\n", ...stable.slice(2501)].join(""));
    expect(chunkLines(m.chunks[0], undefined, sources.base, "\n")).toHaveLength(2500);
  });

  it("returns no lines for an open conflict", () => {
    const { model: m, sources } = model();
    expect(chunkLines(m.chunks[1], undefined, sources.base, "\n")).toBeNull();
  });
});

describe("cardOutput and lineRange", () => {
  it("gives the lines a choice puts in the output for one conflict, and nothing for an open one", () => {
    const { model, sources } = fixture([{ k: "stable", lines: ["a\n"] }, { k: "conflict", base: ["b\n"], left: ["L\n"], right: ["R\n"] }]);
    expect(cardOutput(model, "c1", sources.base, "left")).toEqual(["L\n"]);
    expect(cardOutput(model, "c1", sources.base, "both")).toEqual(["L\n", "R\n"]);
    expect(cardOutput(model, "c1", sources.base, "base")).toEqual(["b\n"]);
    expect(cardOutput(model, "c1", sources.base, undefined)).toBeNull();
    expect(cardOutput(model, "nothing", sources.base, "left")).toBeNull();
  });
  it("numbers lines from 1 and says where an empty range is", () => {
    expect(lineRange({ start: 0, len: 1 })).toBe("1");
    expect(lineRange({ start: 2, len: 3 })).toBe("3–5");
    expect(lineRange({ start: 4, len: 0 })).toBe("5");
  });
});

describe("both versions", () => {
  it("warns for shared text, including hidden or CRLF lines, without changing Both's result", () => {
    expect(bothRepeatsLines(["header\n", "ours\n"], ["header\r\n", "theirs\n"])).toBe(true);
    expect(bothRepeatsLines(["header"], ["header\n"])).toBe(true);
    expect(bothRepeatsLines(["ours\n"], ["theirs\n"])).toBe(false);
    expect(bothRepeatsLines([], ["theirs\n"])).toBe(false);
    const { model, sources } = fixture([{ k: "conflict", base: ["header\n", "old\n"], left: ["header\n", "ours\n"], right: ["header\n", "theirs\n"] }]);
    expect(compose(model, sources.base, { c0: "both" }).text).toBe("header\nours\nheader\ntheirs\n");
  });
});

describe("common lines in a conflict", () => {
  it("hides only lines that occur in the same order in all three versions", () => {
    const common = commonLineIndices(["same\n", "old\n", "tail\n"], ["same\n", "left\n", "tail\n"], ["same\n", "right\n", "tail\n"]);
    expect([...common.base]).toEqual([0, 2]);
    expect([...common.left]).toEqual([0, 2]);
    expect([...common.right]).toEqual([0, 2]);
    // One side moves a line past a long common stretch, as in the .gitignore conflict.
    const base = ["header\n", "lock\n", "managed\n", "build\n", "release\n", "log\n", "tail\n"];
    const left = [...base, "ours\n"];
    const right = ["header\n", "managed\n", "build\n", "release\n", "log\n", "lock\n", "tail\n", "theirs\n"];
    const reordered = commonLineIndices(base, left, right);
    expect([...reordered.base].map((i) => base[i])).toEqual(["header\n", "managed\n", "build\n", "release\n", "log\n", "tail\n"]);
    expect(reordered.left.has(1)).toBe(false);
    expect(reordered.right.has(5)).toBe(false);
    const large = Array.from({ length: 1_500 }, (_, i) => `${i}\n`);
    const bounded = commonLineIndices(large, large, ["new\n", ...large]);
    expect(bounded.base.size).toBe(large.length);
    expect(bounded.right.has(0)).toBe(false);
    const crlf = commonLineIndices(["same\r\n", "tail"], ["same\n", "tail\n"], ["same\r\n", "tail\n"]);
    expect([...crlf.left]).toEqual([0]);
    expect(crlf.endingsDiffer).toBe(true);
    const moved = commonLineIndices(["a\n", "b\n"], ["a\n", "b\n"], ["b\n", "a\n"]);
    expect(moved.left.size).toBe(1);
    expect(commonLineIndices(["x\n"], [], ["x\n"]).base.size).toBe(0);
    const { model, sources } = fixture([{ k: "conflict", base: ["same\n", "old\n"], left: ["same\n", "left\n"], right: ["same\n", "right\n"] }]);
    expect(commonLineIndices(sources.base, sources.left, sources.right).left.has(0)).toBe(true);
    expect(compose(model, sources.base, { c0: "left" }).text).toBe("same\nleft\n");
  });
});


describe("combining conflict changes", () => {
  it("uses the core result and keeps an unavailable combination unresolved", () => {
    const { model, sources } = fixture([{ k: "conflict", base: ["a\n"], left: ["a\n", "left\n"], right: ["a\n", "right\n"] }]);
    const conflict = model.chunks[0];
    expect(compose(model, sources.base, { [conflict.id]: "combine" }).unresolved).toEqual([conflict.id]);
    conflict.combinedLines = ["a\n", "left\n", "right\n"];
    expect(compose(model, sources.base, { [conflict.id]: "combine" })).toEqual({ text: "a\nleft\nright\n", unresolved: [] });
    conflict.combinedLines = [];
    expect(compose(model, sources.base, { [conflict.id]: "combine" })).toEqual({ text: "", unresolved: [] });
  });
});
