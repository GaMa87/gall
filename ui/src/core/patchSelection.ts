/** Pure helpers of the Working Changes diff: which lines a range, a hunk or a jump touches. */
export interface DiffLine { index: number; text: string; kind: string; hunk?: number | null }

const changed = (line: DiffLine) => line.kind === "added" || line.kind === "deleted";

/** The changed lines from `from` to `to` (either order), context lines skipped. */
export function changedBetween(lines: DiffLine[], from: number, to: number): number[] {
  const low = Math.min(from, to), high = Math.max(from, to);
  return lines.filter((l) => l.index >= low && l.index <= high && changed(l)).map((l) => l.index);
}

/** All changed lines of one hunk. */
export function changedInHunk(lines: DiffLine[], hunk: number | null | undefined): number[] {
  return hunk === null || hunk === undefined ? [] : lines.filter((l) => l.hunk === hunk && changed(l)).map((l) => l.index);
}

/** The `@@` header of the next (direction 1) or previous (-1) hunk relative to `line`; null when there is none. */
export function hunkHeaderFrom(lines: DiffLine[], line: number, direction: 1 | -1): number | null {
  const heads = lines.filter((l) => l.text.startsWith("@@ ")).map((l) => l.index);
  const found = direction > 0 ? heads.find((i) => i > line) : [...heads].reverse().find((i) => i < line);
  return found ?? null;
}