// Pure viewport block-window planner and bounded LRU cache for `HexDiff`,
// keeping chunked reads bounded and dropping stale completions on fast scroll.

export const HEX_PER_ROW = 16;
export const HEX_BLOCK = 16 * 1024;
export const HEX_MAX_CACHED_BLOCKS = 64;

export function visibleBlockKeys(
  firstRow: number,
  lastRow: number,
  leftSize: number,
  rightSize: number,
): { side: 0 | 1; block: number; key: string }[] {
  const out: { side: 0 | 1; block: number; key: string }[] = [];
  if (lastRow <= firstRow) return out;
  const from = Math.max(0, firstRow * HEX_PER_ROW);
  const to = lastRow * HEX_PER_ROW;
  for (const side of [0, 1] as const) {
    const size = side ? rightSize : leftSize;
    const clampedTo = Math.min(to, size);
    if (clampedTo <= from) continue;
    const firstBlock = Math.floor(from / HEX_BLOCK);
    const lastBlock = Math.floor((clampedTo - 1) / HEX_BLOCK);
    for (let block = firstBlock; block <= lastBlock; block++) {
      out.push({ side, block, key: `${side}:${block}` });
    }
  }
  return out;
}

/** Evict oldest non-visible cached blocks so memory stays bounded during long scrolls. */
export function pruneBlockCache<T>(cache: Map<string, T>, keepVisible: Set<string>, limit = HEX_MAX_CACHED_BLOCKS): void {
  if (cache.size <= limit) return;
  for (const key of cache.keys()) {
    if (cache.size <= limit) break;
    if (!keepVisible.has(key)) cache.delete(key);
  }
}
