// Where the cursor of a file list goes when the item it was on disappears.

/** The focused item `gone` is no longer listed: the first item after it in the old order
 *  that is still there (the one that took its place), else the nearest one before it;
 *  null when none is left or `gone` was not in the old list. */
export function focusAfterRemoval(prev: string[], gone: string, has: (p: string) => boolean): string | null {
  const i = prev.indexOf(gone);
  if (i < 0) return null;
  for (let j = i + 1; j < prev.length; j++) if (has(prev[j])) return prev[j];
  for (let j = i - 1; j >= 0; j--) if (has(prev[j])) return prev[j];
  return null;
}
