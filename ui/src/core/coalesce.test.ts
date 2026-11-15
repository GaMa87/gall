import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { coalesced } from "./coalesce";

describe("coalesced saves", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("typing writes once, the newest value, after the pause", async () => {
    const written: string[] = [];
    const c = coalesced(async (v: string) => void written.push(v), 300);
    for (const v of ["a", "ab", "abc"]) {
      c.later(v);
      await vi.advanceTimersByTimeAsync(100);
    }
    expect(written).toEqual([]);
    await vi.advanceTimersByTimeAsync(300);
    expect(written).toEqual(["abc"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(written).toEqual(["abc"]);
  });

  it("an immediate write replaces what was waiting and is awaited", async () => {
    const written: string[] = [];
    const c = coalesced(async (v: string) => void written.push(v), 300);
    c.later("typed");
    await c.now("clicked");
    expect(written).toEqual(["clicked"]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(written).toEqual(["clicked"]);
  });

  it("flush writes what waits; with nothing waiting it writes nothing", async () => {
    const written: string[] = [];
    const c = coalesced(async (v: string) => void written.push(v), 300);
    await c.flush();
    c.later("x");
    await c.flush();
    expect(written).toEqual(["x"]);
  });

  it("writes never overlap and keep their order", async () => {
    const log: string[] = [];
    const c = coalesced(async (v: string) => {
      log.push(`start ${v}`);
      await new Promise((r) => setTimeout(r, 50));
      log.push(`end ${v}`);
    }, 300);
    const first = c.now("1");
    const second = c.now("2");
    await vi.advanceTimersByTimeAsync(200);
    await Promise.all([first, second]);
    expect(log).toEqual(["start 1", "end 1", "start 2", "end 2"]);
  });
});
