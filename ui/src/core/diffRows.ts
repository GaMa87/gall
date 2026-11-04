// Turns a line diff (ops from the backend) into display rows for the compare view:
// side-by-side or unified, with long unchanged stretches folded. Pure, so it is
// unit-tested; the view only renders rows.
import type { DiffLinePair, DiffMove, DiffOp } from "../api/types";

export type Spans = [number, number][];

/**
 * One side of a split row. `t`: eq = unchanged, del/ins = only on this side, chg = changed line.
 * `mv`: the line was moved (index into the moves), not removed / added.
 */
export interface Cell { n: number; t: "eq" | "del" | "ins" | "chg"; spans?: Spans; mv?: number }

export type SplitRow =
  | { kind: "line"; l?: Cell; r?: Cell; block?: number }
  | { kind: "fold"; id: string; count: number; a: number; b: number };

export type UnifiedRow =
  | { kind: "line"; t: "eq" | "del" | "ins"; an?: number; bn?: number; side: "a" | "b"; spans?: Spans; block?: number; mv?: number }
  | { kind: "fold"; id: string; count: number; a: number; b: number };

export interface BuildOptions {
  /** Unchanged lines kept around each change; longer runs fold. Infinity = no folding. */
  context: number;
  /** Fold ids the user expanded. */
  expanded: Set<string>;
  /** Moved blocks (removed lines that reappear among the added ones). */
  moves?: DiffMove[];
}

/** Line → move index, for each side. */
function moveMaps(moves: DiffMove[] = []) {
  const a = new Map<number, number>(), b = new Map<number, number>();
  moves.forEach((m, k) => {
    for (let i = 0; i < m.len; i++) {
      a.set(m.a + i, k);
      b.set(m.b + i, k);
    }
  });
  return { a, b };
}

/** The line a moved line went to (side "l") or came from (side "r"). */
export function moveTarget(moves: DiffMove[], mv: number, side: "l" | "r", n: number): number {
  const m = moves[mv];
  return side === "l" ? m.b + (n - m.a) : m.a + (n - m.b);
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
    const block = blocks.length;
    blocks.push(rows.length);
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
          l: k < dl ? { n: ca + k, t: "del", mv: mv.a.get(ca + k) } : undefined,
          r: k < dr ? { n: cb + k, t: "ins", mv: mv.b.get(cb + k) } : undefined,
        });
      }
    };
    for (const p of ps) {
      gap(p.a, p.b);
      rows.push({ kind: "line", block, l: { n: p.a, t: "chg", spans: p.aSpans }, r: { n: p.b, t: "chg", spans: p.bSpans } });
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
    const block = blocks.length;
    blocks.push(rows.length);
    for (let j = 0; j < op.aLen; j++) rows.push({ kind: "line", t: "del", an: op.a + j, side: "a", spans: sp.a.get(op.a + j), block, mv: mv.a.get(op.a + j) });
    for (let j = 0; j < op.bLen; j++) rows.push({ kind: "line", t: "ins", bn: op.b + j, side: "b", spans: sp.b.get(op.b + j), block, mv: mv.b.get(op.b + j) });
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
