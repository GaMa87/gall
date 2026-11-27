// Helpers of the saved comparison tabs: the scroll anchor (a line of one side that does not depend on the size of the
// window) and the relink of a renamed or moved folder. Pure, so they are unit-tested; the window only calls them.

export type ScrollAnchor = { left: boolean; line: number };
/** The line numbers (1-based) that a row of the comparison shows on each side; `null` where that side has no line. */
export interface AnchorRow { left: number | null; right: number | null }

/**
 * The anchor of a window position: a line of the right side when the first visible row has one, else of the left side.
 * `null` when there is no row or no line to hold on to (an empty comparison).
 */
export function scrollAnchor(row: AnchorRow | null | undefined): ScrollAnchor | null {
  if (!row) return null;
  if (row.right != null) return { left: false, line: row.right };
  if (row.left != null) return { left: true, line: row.left };
  return null;
}

/**
 * The index of the row to scroll to for an anchor: the row that shows that line, else the first row that shows a later
 * line of that side (the line was removed or the text changed), else the last row. 0 for no anchor or no rows.
 */
export function rowOfAnchor(rows: readonly AnchorRow[], anchor: ScrollAnchor | null | undefined): number {
  if (!anchor || rows.length === 0) return 0;
  const lineOf = (row: AnchorRow) => (anchor.left ? row.left : row.right);
  let later = -1;
  for (let i = 0; i < rows.length; i++) {
    const line = lineOf(rows[i]);
    if (line == null) continue;
    if (line === anchor.line) return i;
    if (line > anchor.line && later < 0) later = i;
  }
  return later >= 0 ? later : rows.length - 1;
}

const parts = (path: string) => path.split(/[\\/]+/).filter((p) => p !== "");

/**
 * The folder pair of a relink, found from one file: `missing` is the path a tab remembers and `chosen` the path of the
 * same file in its new place. The longest common ending of the two (the part below the moved folder) is taken away; what
 * stays in front of it is the old and the new folder. `null` when the two paths share no ending (then no folder move is
 * known) or are the same path.
 */
export function relinkFolders(missing: string, chosen: string): { from: string; to: string } | null {
  const a = parts(missing), b = parts(chosen);
  let common = 0;
  while (common < a.length && common < b.length && a[a.length - 1 - common] === b[b.length - 1 - common]) common++;
  // The same path, or no common ending at all (not even the file name).
  if (common === 0 || (common === a.length && common === b.length)) return null;
  const lead = (path: string, kept: string[]) => {
    const head = kept.slice(0, kept.length - common).join("/");
    const root = /^[\\/]/.test(path) ? "/" : /^[A-Za-z]:/.test(path) ? "" : "";
    return root + head;
  };
  const from = lead(missing, a), to = lead(chosen, b);
  return from === to ? null : { from, to };
}
