/**
 * Splits items, kept in order, into at most `columns` consecutive groups of about equal
 * total weight. Returns the item indexes of each group; groups left empty are dropped.
 * An item goes to the column its middle falls into, so a heavy item never pushes the
 * ones after it past the last column.
 */
export function balanceColumns(weights: readonly number[], columns: number): number[][] {
  const n = Number.isFinite(columns) ? Math.max(1, Math.floor(columns)) : 1;
  weights = weights.map((w) => (Number.isFinite(w) && w > 0 ? w : 0));
  const total = weights.reduce((sum, w) => sum + w, 0);
  const groups: number[][] = Array.from({ length: n }, () => []);
  if (total <= 0) {
    groups[0].push(...weights.keys());
    return groups.filter((g) => g.length);
  }
  const share = total / n;
  let before = 0;
  let column = 0;
  weights.forEach((raw, i) => {
    const w = Math.max(0, raw);
    column = Math.max(column, Math.min(n - 1, Math.floor((before + w / 2) / share)));
    groups[column].push(i);
    before += w;
  });
  return groups.filter((g) => g.length);
}
