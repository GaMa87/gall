import { describe, expect, it } from "vitest";
import { HEX_BLOCK, HEX_MAX_CACHED_BLOCKS, pruneBlockCache, visibleBlockKeys } from "../core/hexBlocks";

describe("HexDiff chunked block cache", () => {
  it("requests only the blocks covering the visible viewport even for multi-GB sparse files", () => {
    const fourGiB = 4 * 1024 * 1024 * 1024;
    const startRow = Math.floor((fourGiB - 512) / 16);
    const keys = visibleBlockKeys(startRow, startRow + 40, fourGiB, fourGiB);
    expect(keys.length).toBe(2);
    expect(keys[0].block).toBe(Math.floor((fourGiB - 512) / HEX_BLOCK));
    expect(keys[1].side).toBe(1);
  });

  it("bounds cached blocks and preserves currently visible entries during fast scroll", () => {
    const cache = new Map<string, Uint8Array>();
    for (let i = 0; i < HEX_MAX_CACHED_BLOCKS + 20; i++) {
      cache.set(`0:${i}`, new Uint8Array(16));
    }
    const visible = new Set(["0:0", `0:${HEX_MAX_CACHED_BLOCKS + 19}`]);
    pruneBlockCache(cache, visible);
    expect(cache.size).toBe(HEX_MAX_CACHED_BLOCKS);
    expect(cache.has("0:0")).toBe(true);
    expect(cache.has(`0:${HEX_MAX_CACHED_BLOCKS + 19}`)).toBe(true);
  });
});
