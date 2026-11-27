// The conflict-first overview of a three-way merge: one card per conflict with a few lines of context on each side, and a
// shared gap between cards for what is left out. Pure, so it is unit-tested; the merge view only draws it.
//
// The input is the model of the core (`gall-core::merge`, IPC `mergeThreeWay`): chunks with line spans of each source.
// Lines keep their terminators, so what is shown and what is composed are the original bytes.
import type { MergeChoice, MergeChunkInfo, MergeModel, MergeSpan } from "../api/types";

/** Lines of context shown above and below a conflict, in each pane. */
export const CONTEXT = 3;

/** How a side differs from the base in a conflict. An empty side is shown as such (a deletion or nothing added). */
export type SideKind = "same" | "added" | "deleted" | "changed";

export interface CardSide {
  span: MergeSpan;
  lines: string[];
  kind: SideKind;
  /** Up to CONTEXT lines of this source just above and below the conflict, never lines of a neighbouring card. */
  before: string[];
  after: string[];
}

export interface Card {
  id: string;
  /** 1-based position among the conflicts. */
  number: number;
  base: CardSide;
  left: CardSide;
  right: CardSide;
}

/** What lies between two cards (or before the first, after the last): the lines left out, per pane. */
export interface Gap {
  /** The card above and the card below; null at the top and at the end. */
  above: string | null;
  below: string | null;
  hidden: { base: number; left: number; right: number };
  /** The hidden lines themselves, for a click that shows them. */
  lines: { base: string[]; left: string[]; right: string[] };
}

export interface Overview {
  cards: Card[];
  /** One more than the cards: before the first, between neighbours, after the last. A merge without a conflict has one gap with everything. */
  gaps: Gap[];
}

export interface Sources { base: string[]; left: string[]; right: string[] }

type Pane = "base" | "left" | "right";
const PANES: Pane[] = ["base", "left", "right"];

const same = (a: string[], b: string[]) => a.length === b.length && a.every((line, i) => line === b[i]);

function kindOf(lines: string[], base: string[]): SideKind {
  if (same(lines, base)) return "same";
  if (!lines.length) return "deleted";
  if (!base.length) return "added";
  return "changed";
}

/**
 * The cards and gaps of a merge. The context of a card is shared with its neighbour: when two conflicts are closer than
 * twice the context, the lines between them are split between the two cards and nothing is left out; further apart, each
 * takes `context` lines and the rest is a gap. The three panes use the same numbers (the smallest distance of the three
 * sources decides), so a row of one pane has its row in the others.
 */
export function overview(model: MergeModel, sources: Sources, context = CONTEXT): Overview {
  const conflicts = model.chunks.filter((chunk) => chunk.kind === "conflict");
  const chunkOf = (chunk: MergeChunkInfo, pane: Pane) => ({ span: chunk[pane], lines: pane === "base" ? chunk.baseLines : pane === "left" ? chunk.leftLines : chunk.rightLines });
  const end = (card: MergeChunkInfo | undefined, pane: Pane) => (card ? card[pane].start + card[pane].len : 0);
  // The room (in lines of each source) before the first card, between neighbours, and after the last.
  const room = (above: MergeChunkInfo | undefined, below: MergeChunkInfo | undefined, pane: Pane) => (below ? below[pane].start : sources[pane].length) - end(above, pane);
  const take: { after: number; before: number }[] = [];
  const gapsAt = conflicts.length + 1;
  for (let g = 0; g < gapsAt; g++) {
    const above = conflicts[g - 1], below = conflicts[g];
    const smallest = Math.min(...PANES.map((pane) => room(above, below, pane)));
    if (!above && !below) take.push({ after: 0, before: 0 });
    else if (!above) take.push({ after: 0, before: Math.min(context, smallest) });
    else if (!below) take.push({ after: Math.min(context, smallest), before: 0 });
    else if (smallest <= 2 * context) { const after = Math.ceil(smallest / 2); take.push({ after, before: smallest - after }); }
    else take.push({ after: context, before: context });
  }
  const cards: Card[] = conflicts.map((chunk, i) => {
    const side = (pane: Pane): CardSide => {
      const { span, lines } = chunkOf(chunk, pane);
      const source = sources[pane];
      const before = take[i].before, after = take[i + 1].after;
      return {
        span,
        lines,
        kind: pane === "base" ? "same" : kindOf(lines, chunk.baseLines),
        before: source.slice(span.start - before, span.start),
        after: source.slice(span.start + span.len, span.start + span.len + after),
      };
    };
    return { id: chunk.id, number: i + 1, base: side("base"), left: side("left"), right: side("right") };
  });
  const gaps: Gap[] = [];
  for (let g = 0; g < gapsAt; g++) {
    const above = conflicts[g - 1], below = conflicts[g];
    const lines = { base: [] as string[], left: [] as string[], right: [] as string[] };
    for (const pane of PANES) {
      const from = end(above, pane) + take[g].after;
      const to = (below ? below[pane].start : sources[pane].length) - take[g].before;
      lines[pane] = sources[pane].slice(from, Math.max(from, to));
    }
    gaps.push({ above: above?.id ?? null, below: below?.id ?? null, hidden: { base: lines.base.length, left: lines.left.length, right: lines.right.length }, lines });
  }
  return { cards, gaps };
}

/** Both takes each complete block; shared visible text will therefore occur twice. */
export function bothRepeatsLines(left: string[], right: string[]): boolean {
  const normalized = (line: string) => line.replace(/\r?\n$/, "");
  const lines = new Set(left.map(normalized));
  return right.some((line) => lines.has(normalized(line)));
}

/** Whether a gap has something to show. */
export const gapHasLines = (gap: Gap): boolean => gap.hidden.base + gap.hidden.left + gap.hidden.right > 0;

/** The card after (direction 1) or before (-1) the card at `index`, wrapping around; -1 when there are no cards. */
export function stepCard(index: number, direction: 1 | -1, count: number): number {
  if (count <= 0) return -1;
  return (Math.max(index, 0) + direction + count) % count;
}

// ── the result ──────────────────────────────────────────────────────────────────────────────────────────────────

export const eolOf = (model: MergeModel): string =>
  model.chunks.some((chunk) => [...chunk.baseLines, ...chunk.leftLines, ...chunk.rightLines].some((line) => line.endsWith("\r\n"))) ? "\r\n" : "\n";

/** Appends lines, ending the previous one with the file's terminator when it has none (a file without a final newline). */
export function append(out: string[], part: string[], eol: string): void {
  if (part.length && out.length && !out[out.length - 1].endsWith("\n")) out[out.length - 1] += eol;
  out.push(...part);
}

/** The lines of a chunk for a choice; null for a conflict with no choice yet. Mirrors `gall-core::merge`. */
export function chunkLines(chunk: MergeChunkInfo, choice: MergeChoice | undefined, base: string[], eol: string): string[] | null {
  switch (chunk.kind) {
    case "stable": return base.slice(chunk.base.start, chunk.base.start + chunk.base.len);
    case "left": case "same": return chunk.leftLines;
    case "right": return chunk.rightLines;
    case "conflict": {
      if (!choice) return null;
      if (choice === "left") return chunk.leftLines;
      if (choice === "right") return chunk.rightLines;
      if (choice === "base") return chunk.baseLines;
      if (choice === "combine") return chunk.combinedLines;
      const both = [...chunk.leftLines];
      append(both, chunk.rightLines, eol);
      return both;
    }
  }
}

/** The lines the output has for one conflict with a choice (null: no choice yet), for the output column of its card. */
export function cardOutput(model: MergeModel, id: string, base: string[], choice: MergeChoice | undefined): string[] | null {
  const chunk = model.chunks.find((c) => c.id === id);
  return chunk ? chunkLines(chunk, choice, base, eolOf(model)) : null;
}

/** A range of lines for a heading: "3–5", or the place of an empty one. */
export function lineRange(span: MergeSpan): string {
  return span.len ? (span.len === 1 ? `${span.start + 1}` : `${span.start + 1}–${span.start + span.len}`) : `${span.start + 1}`;
}

/**
 * The text with the choices made. A conflict without a choice is shown between Git-style markers (so the preview can be edited by hand);
 * `unresolved` lists those conflicts. The composition is what `gall-core::merge::resolve` gives when nothing is unresolved.
 */
export function compose(model: MergeModel, base: string[], choices: Record<string, MergeChoice | undefined>): { text: string; unresolved: string[] } {
  const eol = eolOf(model);
  const out: string[] = [];
  const unresolved: string[] = [];
  for (const chunk of model.chunks) {
    const lines = chunkLines(chunk, choices[chunk.id], base, eol);
    if (lines) append(out, lines, eol);
    else {
      unresolved.push(chunk.id);
      append(out, [`<<<<<<< ours${eol}`], eol);
      append(out, chunk.leftLines, eol);
      append(out, [`=======${eol}`], eol);
      append(out, chunk.rightLines, eol);
      append(out, [`>>>>>>> theirs${eol}`], eol);
    }
  }
  return { text: out.join(""), unresolved };
}

/** Match an ordered subsequence against the base, limiting work on unusually large conflict blocks. */
function matchBase(base: string[], side: string[]): Map<number, number> {
  const matched = new Map<number, number>();
  const width = side.length + 1;
  if ((base.length + 1) * width <= 2_000_000) {
    // LCS avoids losing all subsequent matches when just one line moves.
    const lengths = new Uint32Array((base.length + 1) * width);
    for (let b = base.length - 1; b >= 0; b--) {
      for (let i = side.length - 1; i >= 0; i--) {
        lengths[b * width + i] = base[b] === side[i] ? 1 + lengths[(b + 1) * width + i + 1]
          : Math.max(lengths[(b + 1) * width + i], lengths[b * width + i + 1]);
      }
    }
    let b = 0, i = 0;
    while (b < base.length && i < side.length) {
      if (base[b] === side[i]) { matched.set(b++, i++); }
      else if (lengths[(b + 1) * width + i] >= lengths[b * width + i + 1]) b++;
      else i++;
    }
    return matched;
  }
  // Bounded fallback: it may leave more lines visible, but never hides an unmatched line.
  const positions = new Map<string, number[]>();
  side.forEach((line, i) => { const list = positions.get(line); if (list) list.push(i); else positions.set(line, [i]); });
  let last = -1;
  base.forEach((line, b) => {
    const list = positions.get(line);
    if (!list) return;
    let lo = 0, hi = list.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (list[mid] <= last) lo = mid + 1; else hi = mid; }
    if (lo < list.length) { last = list[lo]; matched.set(b, last); }
  });
  return matched;
}

/** Match common text in order, treating LF and CRLF as visually identical while preserving and reporting their endings. */
export function commonLineIndices(base: string[], left: string[], right: string[]): { base: Set<number>; left: Set<number>; right: Set<number>; endingsDiffer: boolean } {
  const normalized = (lines: string[]) => lines.map((line) => line.replace(/\r\n$/, "\n"));
  const baseText = normalized(base), leftMatches = matchBase(baseText, normalized(left)), rightMatches = matchBase(baseText, normalized(right));
  const common = { base: new Set<number>(), left: new Set<number>(), right: new Set<number>(), endingsDiffer: false };
  for (const [b, l] of leftMatches) {
    const r = rightMatches.get(b);
    if (r === undefined) continue;
    common.base.add(b); common.left.add(l); common.right.add(r);
    if (base[b] !== left[l] || base[b] !== right[r]) common.endingsDiffer = true;
  }
  return common;
}
