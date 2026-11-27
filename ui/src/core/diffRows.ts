// Turns a line diff (ops from the backend) into display rows for the compare view:
// side-by-side or unified, with long unchanged stretches folded. Pure, so it is
// unit-tested; the view only renders rows.
import type { DiffLinePair, DiffMarks, DiffMove, DiffOp } from "../api/types";

export type Spans = [number, number][];

/**
 * One side of a split row. `t`: eq = unchanged, del/ins = only on this side, chg = changed line.
 * `mv`: the line was moved (index into the moves), not removed / added.
 * `fmt`: only the layout of the line differs (structural diff), its content is unchanged.
 */
export interface Cell { n: number; t: "eq" | "del" | "ins" | "chg"; spans?: Spans; mv?: number; fmt?: boolean }

export type SplitRow =
  | { kind: "line"; l?: Cell; r?: Cell; block?: number }
  | { kind: "fold"; id: string; count: number; a: number; b: number };

export type UnifiedRow =
  | { kind: "line"; t: "eq" | "del" | "ins"; an?: number; bn?: number; side: "a" | "b"; spans?: Spans; block?: number; mv?: number; fmt?: boolean }
  | { kind: "fold"; id: string; count: number; a: number; b: number };

export interface BuildOptions {
  /** Unchanged lines kept around each change; longer runs fold. Infinity = no folding. */
  context: number;
  /** Fold ids the user expanded. */
  expanded: Set<string>;
  /** Moved blocks (removed lines that reappear among the added ones). */
  moves?: DiffMove[];
  /** Structural diff: what changed on the lines of changed blocks, per side. */
  marks?: { a: DiffMarks; b: DiffMarks };
}

/**
 * Spans and layout-only flag of a changed line: from the structural marks when the line
 * has one (an empty mark = layout only), else the word spans of its pair. In a cosmetic
 * block every line is layout only.
 */
function lineMark(marks: DiffMarks | undefined, n: number, cosmetic: boolean | undefined, fallback?: Spans): { spans?: Spans; fmt?: boolean } {
  const m = marks?.[n];
  if (cosmetic || (m && m.length === 0)) return { fmt: true };
  return { spans: m ?? fallback };
}

/** Line → move index, for each side. */
function moveMaps(moves: DiffMove[] = []) {
  const a = new Map<number, number>(), b = new Map<number, number>();
  moves.forEach((m, k) => {
    for (let i = 0; i < m.len; i++) a.set(m.a + i, k);
    for (let i = 0; i < m.bLen; i++) b.set(m.b + i, k);
  });
  return { a, b };
}

/** The line a moved line went to (side "l") or came from (side "r"). */
export function moveTarget(moves: DiffMove[], mv: number, side: "l" | "r", n: number): number {
  const m = moves[mv];
  // An edited move can change its length: stay inside the other end.
  return side === "l" ? m.b + Math.min(n - m.a, m.bLen - 1) : m.a + Math.min(n - m.b, m.len - 1);
}

/** The two ends of every moved block, in reading order (left start, right start), for keyboard jumps. */
export function moveEnds(moves: DiffMove[]): { mv: number; side: "l" | "r"; n: number }[] {
  return moves.flatMap((m, mv) => [{ mv, side: "l" as const, n: m.a }, { mv, side: "r" as const, n: m.b }]);
}

/**
 * The end to jump to from the row at `rowOf(end)`: the first end below `from` (or the last above it when
 * `back`), wrapping around. `rowOf` gives the row of an end, or -1 when it is not shown.
 */
export function nextMoveEnd(moves: DiffMove[], rowOf: (e: { side: "l" | "r"; n: number }) => number, from: number, back: boolean) {
  const shown = moveEnds(moves).map((e) => ({ e, row: rowOf(e) })).filter((x) => x.row >= 0).sort((x, y) => x.row - y.row);
  if (!shown.length) return undefined;
  const hit = back ? [...shown].reverse().find((x) => x.row < from) : shown.find((x) => x.row > from);
  return (hit ?? (back ? shown[shown.length - 1] : shown[0])).e;
}

interface Built<R> { rows: R[]; blocks: number[] }

function spanMaps(pairs: DiffLinePair[]) {
  const a = new Map<number, Spans>(), b = new Map<number, Spans>(), pair = new Map<number, DiffLinePair>();
  for (const p of pairs) {
    a.set(p.a, p.aSpans);
    b.set(p.b, p.bSpans);
    pair.set(p.a, p);
  }
  return { a, b, pair };
}

/** Splits an equal run into kept head / folded middle / kept tail. */
function foldRun(op: DiffOp, first: boolean, last: boolean, o: BuildOptions): { keep: [number, number][]; fold?: { id: string; a: number; b: number; count: number } } {
  const len = op.aLen;
  const head = first ? 0 : o.context;
  const tail = last ? 0 : o.context;
  const id = `${op.a}:${op.b}`;
  if (!Number.isFinite(o.context) || len <= head + tail + 1 || o.expanded.has(id)) return { keep: [[0, len]] };
  return {
    keep: [[0, head], [len - tail, len]],
    fold: { id, a: op.a + head, b: op.b + head, count: len - head - tail },
  };
}

export function buildSplit(ops: DiffOp[], pairs: DiffLinePair[], o: BuildOptions): Built<SplitRow> {
  const sp = spanMaps(pairs);
  const mv = moveMaps(o.moves);
  const rows: SplitRow[] = [];
  const blocks: number[] = [];
  ops.forEach((op, i) => {
    if (op.tag === "equal") {
      const f = foldRun(op, i === 0, i === ops.length - 1, o);
      const [k0, k1] = f.keep;
      for (let j = k0[0]; j < k0[1]; j++) rows.push({ kind: "line", l: { n: op.a + j, t: "eq" }, r: { n: op.b + j, t: "eq" } });
      if (f.fold) rows.push({ kind: "fold", ...f.fold });
      if (k1) for (let j = k1[0]; j < k1[1]; j++) rows.push({ kind: "line", l: { n: op.a + j, t: "eq" }, r: { n: op.b + j, t: "eq" } });
      return;
    }
    // A block where only the layout differs is shown, but is no change to step to or merge.
    const block = op.cosmetic ? undefined : blocks.length;
    if (!op.cosmetic) blocks.push(rows.length);
    // Paired lines (edits of each other) share a row; between pairs, removed and
    // added lines sit side by side without being compared.
    const ps: DiffLinePair[] = [];
    for (let k = op.a; k < op.a + op.aLen; k++) {
      const p = sp.pair.get(k);
      if (p) ps.push(p);
    }
    let ca = op.a, cb = op.b;
    const gap = (ai: number, bj: number) => {
      const dl = ai - ca, dr = bj - cb;
      for (let k = 0; k < Math.max(dl, dr); k++) {
        rows.push({
          kind: "line",
          block,
          l: k < dl ? { n: ca + k, t: "del", mv: mv.a.get(ca + k), ...lineMark(o.marks?.a, ca + k, op.cosmetic) } : undefined,
          r: k < dr ? { n: cb + k, t: "ins", mv: mv.b.get(cb + k), ...lineMark(o.marks?.b, cb + k, op.cosmetic) } : undefined,
        });
      }
    };
    for (const p of ps) {
      gap(p.a, p.b);
      rows.push({ kind: "line", block, l: { n: p.a, t: "chg", ...lineMark(o.marks?.a, p.a, op.cosmetic, p.aSpans) }, r: { n: p.b, t: "chg", ...lineMark(o.marks?.b, p.b, op.cosmetic, p.bSpans) } });
      ca = p.a + 1;
      cb = p.b + 1;
    }
    gap(op.a + op.aLen, op.b + op.bLen);
  });
  return { rows, blocks };
}

export function buildUnified(ops: DiffOp[], pairs: DiffLinePair[], o: BuildOptions): Built<UnifiedRow> {
  const sp = spanMaps(pairs);
  const mv = moveMaps(o.moves);
  const rows: UnifiedRow[] = [];
  const blocks: number[] = [];
  ops.forEach((op, i) => {
    if (op.tag === "equal") {
      const f = foldRun(op, i === 0, i === ops.length - 1, o);
      const [k0, k1] = f.keep;
      const eq = (j: number): UnifiedRow => ({ kind: "line", t: "eq", an: op.a + j, bn: op.b + j, side: "b" });
      for (let j = k0[0]; j < k0[1]; j++) rows.push(eq(j));
      if (f.fold) rows.push({ kind: "fold", ...f.fold });
      if (k1) for (let j = k1[0]; j < k1[1]; j++) rows.push(eq(j));
      return;
    }
    const block = op.cosmetic ? undefined : blocks.length;
    if (!op.cosmetic) blocks.push(rows.length);
    for (let j = 0; j < op.aLen; j++) rows.push({ kind: "line", t: "del", an: op.a + j, side: "a", block, mv: mv.a.get(op.a + j), ...lineMark(o.marks?.a, op.a + j, op.cosmetic, sp.a.get(op.a + j)) });
    for (let j = 0; j < op.bLen; j++) rows.push({ kind: "line", t: "ins", bn: op.b + j, side: "b", block, mv: mv.b.get(op.b + j), ...lineMark(o.marks?.b, op.b + j, op.cosmetic, sp.b.get(op.b + j)) });
  });
  return { rows, blocks };
}

/** Syntax runs of one line: [start, end, category], UTF-16 offsets. */
export type SyntaxRuns = [number, number, number][];

/**
 * Splits `text` into [segment, changed, syntax category] parts: changed by the
 * diff spans, coloured by the syntax runs (0 = plain). UTF-16 offsets.
 */
export function segments(text: string, spans?: Spans, sx?: SyntaxRuns): [string, boolean, number][] {
  if (!spans?.length && !sx?.length) return [[text, false, 0]];
  const cuts = new Set<number>([0, text.length]);
  for (const [s, e] of spans ?? []) { cuts.add(s); cuts.add(e); }
  for (const [s, e] of sx ?? []) { cuts.add(s); cuts.add(e); }
  const pts = [...cuts].filter((p) => p >= 0 && p <= text.length).sort((x, y) => x - y);
  const out: [string, boolean, number][] = [];
  let si = 0, xi = 0;
  for (let k = 0; k + 1 < pts.length; k++) {
    const p = pts[k], q = pts[k + 1];
    while (spans && si < spans.length && spans[si][1] <= p) si++;
    while (sx && xi < sx.length && sx[xi][1] <= p) xi++;
    const hi = !!spans && si < spans.length && spans[si][0] <= p;
    const cat = sx && xi < sx.length && sx[xi][0] <= p ? sx[xi][2] : 0;
    const last = out[out.length - 1];
    if (last && last[1] === hi && last[2] === cat) last[0] += text.slice(p, q);
    else out.push([text.slice(p, q), hi, cat]);
  }
  return out;
}

/** Merge: replace the target side of one change block with the source side's lines. */
export function applyBlock(left: string[], right: string[], op: DiffOp, dir: "toRight" | "toLeft"): { left: string[]; right: string[] } {
  if (dir === "toRight") {
    return { left, right: [...right.slice(0, op.b), ...left.slice(op.a, op.a + op.aLen), ...right.slice(op.b + op.bLen)] };
  }
  return { left: [...left.slice(0, op.a), ...right.slice(op.b, op.b + op.bLen), ...left.slice(op.a + op.aLen)], right };
}

/** Joins lines back into file text, keeping the file's line endings and final newline. */
export function joinLines(lines: string[], crlf: boolean, finalNewline: boolean): string {
  const eol = crlf ? "\r\n" : "\n";
  return lines.join(eol) + (finalNewline && lines.length ? eol : "");
}

/** Display width of a line in columns: tabs advance to the next multiple of `tab`. */
export function visualWidth(text: string, tab = 8): number {
  let w = 0;
  for (const ch of text) w = ch === "\t" ? w + tab - (w % tab) : w + 1;
  return w;
}

/** Screen lines a wrapped line takes in `cols` columns (at least 1). */
export function wrappedLines(text: string, cols: number): number {
  return Math.max(1, Math.ceil(visualWidth(text) / Math.max(1, cols)));
}

/** Top offset of every row plus the total height at the end; `heights[i]` in px. */
export function offsets(heights: number[]): number[] {
  const off = new Array<number>(heights.length + 1);
  off[0] = 0;
  for (let i = 0; i < heights.length; i++) off[i + 1] = off[i] + heights[i];
  return off;
}

/** Index of the row containing y (0 ≤ result < off.length - 1). */
export function rowAt(off: number[], y: number): number {
  let lo = 0, hi = off.length - 2;
  if (hi < 0) return 0;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (off[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

type CompactRun =
  | { start: number; length: number; kind: "equal"; a: number; b: number }
  | { start: number; length: 1; kind: "fold"; row: Extract<SplitRow, { kind: "fold" }> }
  | { start: number; length: number; kind: "change"; op: DiffOp; block?: number };

interface CompactBlock { start: number; length: number; op: DiffOp }

/** Index of display rows for large comparisons. Expanded equal runs and
 * unpaired edits take constant space per operation, regardless of line count. */
export function indexDiffRows(ops: DiffOp[], mode: "split" | "unified", options: BuildOptions) {
  const runs: CompactRun[] = [];
  const blocks: CompactBlock[] = [];
  let length = 0;
  for (const [i, op] of ops.entries()) {
    if (op.tag === "equal") {
      const { keep, fold } = foldRun(op, i === 0, i === ops.length - 1, options);
      for (const [j, [start, end]] of keep.entries()) {
        if (j === 1 && fold) {
          runs.push({ start: length, length: 1, kind: "fold", row: { kind: "fold", ...fold } });
          length++;
        }
        if (end > start) {
          runs.push({ start: length, length: end - start, kind: "equal", a: op.a + start, b: op.b + start });
          length += end - start;
        }
      }
      continue;
    }
    const size = mode === "split" ? Math.max(op.aLen, op.bLen) : op.aLen + op.bLen;
    if (!size) continue;
    const block = op.cosmetic ? undefined : blocks.length;
    if (block != null) blocks.push({ start: length, length: size, op });
    runs.push({ start: length, length: size, kind: "change", op, block });
    length += size;
  }

  function rowAtIndex(index: number): SplitRow | UnifiedRow | undefined {
    if (index < 0 || index >= length) return;
    let lo = 0, hi = runs.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (runs[mid].start <= index) lo = mid + 1;
      else hi = mid;
    }
    const run = runs[lo - 1];
    const offset = index - run.start;
    if (run.kind === "fold") return run.row;
    if (run.kind === "equal") {
      const a = run.a + offset, b = run.b + offset;
      return mode === "split"
        ? { kind: "line", l: { n: a, t: "eq" }, r: { n: b, t: "eq" } }
        : { kind: "line", t: "eq", an: a, bn: b, side: "b" };
    }
    const { op, block } = run;
    const fmt = op.cosmetic || undefined;
    if (mode === "split") return {
      kind: "line", block,
      l: offset < op.aLen ? { n: op.a + offset, t: "del", fmt } : undefined,
      r: offset < op.bLen ? { n: op.b + offset, t: "ins", fmt } : undefined,
    };
    return offset < op.aLen
      ? { kind: "line", block, t: "del", an: op.a + offset, side: "a", fmt }
      : { kind: "line", block, t: "ins", bn: op.b + offset - op.aLen, side: "b", fmt };
  }

  return { length, blocks, rowAt: rowAtIndex };
}

/** A change on the overview ruler: where it is in the document (pixels) and its kind. */
export interface RulerMark { top: number; height: number; tag: "ins" | "del" | "chg" }
/** Changes that share a slot of the ruler: one element instead of one per change. */
export interface RulerBucket { top: number; height: number; tag: "ins" | "del" | "chg"; count: number; first: number; last: number }

/**
 * Aggregates the marks of the overview ruler into at most `slots` buckets (never more than one per
 * displayed row or per few pixels of the ruler), so a file with thousands of changes draws a bounded
 * number of elements. A bucket holds the marks that fall in the same slot of the document; it spans
 * from the first to the end of the last, has the kind of its marks (or "chg" when they differ) and
 * knows the indexes of its first and last change, so the current change can be shown in it.
 */
export function rulerBuckets(marks: RulerMark[], total: number, slots: number): RulerBucket[] {
  const n = Math.max(1, Math.floor(slots));
  const size = Math.max(total, 1);
  const out = new Map<number, RulerBucket>();
  marks.forEach((m, index) => {
    const slot = Math.min(n - 1, Math.max(0, Math.floor((m.top / size) * n)));
    const bottom = m.top + m.height;
    const b = out.get(slot);
    if (!b) out.set(slot, { top: m.top, height: m.height, tag: m.tag, count: 1, first: index, last: index });
    else {
      const end = Math.max(b.top + b.height, bottom);
      b.top = Math.min(b.top, m.top);
      b.height = end - b.top;
      if (b.tag !== m.tag) b.tag = "chg";
      b.count++;
      b.last = index;
    }
  });
  return [...out.entries()].sort((x, y) => x[0] - y[0]).map(([, b]) => b);
}
