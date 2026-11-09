export type MergeChunk = { conflict: boolean; base: string[]; ours: string[]; theirs: string[]; merged: string[] };
export type MergeChoice = "ours" | "theirs" | "both" | "base";
type Hunk = { bs: number; be: number; ss: number; se: number };

export const splitLines = (text: string): string[] => text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
const MAX_CELLS = 4_000_000;

/** Longest-common-subsequence pairs after trimming the shared prefix and suffix. */
function matches(a: string[], b: string[]): [number, number][] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length, endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) { endA--; endB--; }
  const pairs: [number, number][] = [];
  for (let i = 0; i < start; i++) pairs.push([i, i]);
  const n = endA - start, m = endB - start;
  if (n > 0 && m > 0 && n * m <= MAX_CELLS) {
    const width = m + 1;
    const table = new Uint32Array((n + 1) * width);
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) table[i * width + j] = a[start + i] === b[start + j] ? table[(i + 1) * width + j + 1] + 1 : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (a[start + i] === b[start + j]) { pairs.push([start + i, start + j]); i++; j++; }
      else if (table[(i + 1) * width + j] >= table[i * width + j + 1]) i++;
      else j++;
    }
  }
  for (let k = 0; endA + k < a.length; k++) pairs.push([endA + k, endB + k]);
  return pairs;
}

function hunks(base: string[], side: string[]): Hunk[] {
  const result: Hunk[] = [];
  let pi = 0, pj = 0;
  for (const [i, j] of [...matches(base, side), [base.length, side.length] as [number, number]]) {
    if (i > pi || j > pj) result.push({ bs: pi, be: i, ss: pj, se: j });
    pi = i + 1; pj = j + 1;
  }
  return result;
}

const same = (a: string[], b: string[]) => a.length === b.length && a.every((line, index) => line === b[index]);

/** Three-way merge: unchanged and one-sided regions merge automatically; overlapping edits become conflicts. */
export function mergeThreeWay(baseText: string, oursText: string, theirsText: string): MergeChunk[] {
  const base = splitLines(baseText), ours = splitLines(oursText), theirs = splitLines(theirsText);
  const tagged = [...hunks(base, ours).map((h) => ({ ...h, from: "ours" as const })), ...hunks(base, theirs).map((h) => ({ ...h, from: "theirs" as const }))].sort((a, b) => a.bs - b.bs || a.be - b.be);
  const chunks: MergeChunk[] = [];
  const stable = (lines: string[]) => {
    if (!lines.length) return;
    const last = chunks[chunks.length - 1];
    if (last && !last.conflict && same(last.base, last.merged) && same(last.ours, last.base) && same(last.theirs, last.base)) { last.base = last.base.concat(lines); last.ours = last.base; last.theirs = last.base; last.merged = last.base; }
    else chunks.push({ conflict: false, base: lines, ours: lines, theirs: lines, merged: lines });
  };
  let cursor = 0, index = 0;
  while (index < tagged.length) {
    const first = tagged[index], group = [first];
    let end = first.be;
    index++;
    while (index < tagged.length && tagged[index].bs <= end) { end = Math.max(end, tagged[index].be); group.push(tagged[index]); index++; }
    stable(base.slice(cursor, first.bs));
    const side = (name: "ours" | "theirs", lines: string[]) => {
      const mine = group.filter((h) => h.from === name);
      if (!mine.length) return null;
      return lines.slice(mine[0].ss - (mine[0].bs - first.bs), mine[mine.length - 1].se + (end - mine[mine.length - 1].be));
    };
    const o = side("ours", ours), t = side("theirs", theirs), b = base.slice(first.bs, end);
    const merged = o && t ? (same(o, t) ? o : null) : o ?? t ?? b;
    chunks.push({ conflict: merged === null, base: b, ours: o ?? b, theirs: t ?? b, merged: merged ?? [] });
    cursor = end;
  }
  stable(base.slice(cursor));
  return chunks;
}

export function chosenLines(chunk: MergeChunk, choice: MergeChoice | undefined): string[] | null {
  if (!chunk.conflict) return chunk.merged;
  if (!choice) return null;
  if (choice === "both") {
    const first = chunk.ours.length && !chunk.ours[chunk.ours.length - 1].endsWith("\n") && chunk.theirs.length ? [...chunk.ours.slice(0, -1), `${chunk.ours[chunk.ours.length - 1]}\n`] : chunk.ours;
    return [...first, ...chunk.theirs];
  }
  return chunk[choice];
}

export const conflictMarkers = (chunk: MergeChunk): string[] => [
  "<<<<<<< ours\n", ...chunk.ours, ...(chunk.ours.length && !chunk.ours[chunk.ours.length - 1].endsWith("\n") ? ["\n"] : []),
  "=======\n", ...chunk.theirs, ...(chunk.theirs.length && !chunk.theirs[chunk.theirs.length - 1].endsWith("\n") ? ["\n"] : []), ">>>>>>> theirs\n",
];

/** Resolved text; unresolved conflicts are emitted with Git-style markers so they can be edited by hand. */
export function composeResult(chunks: MergeChunk[], choices: Record<number, MergeChoice | undefined>): { text: string; unresolved: number } {
  let unresolved = 0;
  const out: string[] = [];
  chunks.forEach((chunk, index) => {
    const lines = chosenLines(chunk, choices[index]);
    if (lines) out.push(...lines);
    else { unresolved++; out.push(...conflictMarkers(chunk)); }
  });
  return { text: out.join(""), unresolved };
}
