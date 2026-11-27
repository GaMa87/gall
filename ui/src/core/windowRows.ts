// Windowed (virtual) rows for the large views of the merge editor: only the rows in view (and a few around them) are
// rendered, so a 5,000-line file keeps the scroll as light as the comparison view does. Pure, so it is unit-tested.

/** Height of one row in pixels; the stylesheet gives `.gm-win` the same line height. */
export const ROW_HEIGHT = 18;
/** More lines than this in a gap or a result are windowed; fewer are plain text. */
export const WINDOW_FROM = 200;
/** Tallest a window grows (pixels) before it scrolls inside itself. */
export const MAX_WINDOW_HEIGHT = 480;

/** The first and one past the last row to render for a scroll position, with `overscan` rows on each side. */
export function windowRange(scrollTop: number, viewHeight: number, rowHeight: number, count: number, overscan = 8): { start: number; end: number } {
  if (count <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const view = Math.max(0, viewHeight);
  // A scroll position past the end (a stale value after the list got shorter) is the end.
  const top = Math.min(Math.max(0, scrollTop), Math.max(0, count * rowHeight - view));
  const first = Math.floor(top / rowHeight);
  const last = Math.ceil((top + view) / rowHeight);
  return { start: Math.max(0, first - overscan), end: Math.min(count, last + overscan) };
}

/** The height of a window of `count` rows. */
export function windowHeight(count: number, rowHeight = ROW_HEIGHT, max = MAX_WINDOW_HEIGHT): number {
  return Math.min(max, Math.max(0, count) * rowHeight);
}

/** A line without its terminator (`\n` or `\r\n`), for one row. */
export function rowText(line: string): string {
  return line.replace(/\r?\n$/, "");
}

/** The lines of a text, each as one row (a final line break does not make an empty last row). */
export function textRows(text: string): string[] {
  if (text === "") return [];
  const rows = text.split("\n");
  if (rows[rows.length - 1] === "") rows.pop();
  return rows.map((r) => r.replace(/\r$/, ""));
}

/** The number of rows of columns that share one alignment: the longest column. */
export function rowCount(columns: string[][]): number {
  return columns.reduce((n, c) => Math.max(n, c.length), 0);
}
