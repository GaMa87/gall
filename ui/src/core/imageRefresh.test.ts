import { afterEach, expect, it, vi } from "vitest";
import { imageRefresh, loadImageSnapshot } from "./imageRefresh";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

it("reloads the same path after overwrite and atomic replacement, retaining valid content during a write", async () => {
  vi.useFakeTimers();
  const load = vi.fn().mockResolvedValueOnce("A").mockRejectedValueOnce(Error("rename gap"))
    .mockRejectedValueOnce(Error("partial image")).mockResolvedValueOnce("B");
  const show = vi.fn(), release = vi.fn();
  const refresh = imageRefresh(load, show, release);
  refresh.refresh();
  await vi.advanceTimersByTimeAsync(0);
  refresh.refresh();
  await vi.advanceTimersByTimeAsync(150);
  expect(show.mock.calls).toEqual([["A"]]);
  await vi.advanceTimersByTimeAsync(300);
  expect(show.mock.calls).toEqual([["A"], ["B"]]);
  expect(release).toHaveBeenCalledWith("A");
  refresh.stop();
  expect(release).toHaveBeenCalledWith("B");
});

it("keeps the latest of rapid overwrites even if older loads finish later", async () => {
  const pending: ((value: string) => void)[] = [];
  const load = vi.fn(() => new Promise<string>((resolve) => pending.push(resolve)));
  const show = vi.fn(), release = vi.fn();
  const refresh = imageRefresh(load, show, release);
  refresh.refresh(); refresh.refresh(); refresh.refresh();
  pending[2]("latest"); await Promise.resolve();
  pending[1]("middle"); pending[0]("old"); await Promise.resolve();
  expect(show.mock.calls).toEqual([["latest"]]);
  expect(release.mock.calls).toEqual([["middle"], ["old"]]);
  refresh.stop();
});

it("aborts and ignores pending refreshes when selection changes, and clears retries", async () => {
  vi.useFakeTimers();
  let resolve!: (s: string) => void;
  let signal!: AbortSignal;
  const show = vi.fn(), release = vi.fn();
  const old = imageRefresh((s) => { signal = s; return new Promise<string>((r) => { resolve = r; }); }, show, release);
  old.refresh(); old.stop();
  expect(signal.aborted).toBe(true);
  resolve("old selection"); await Promise.resolve();
  expect(show).not.toHaveBeenCalled();
  expect(release).toHaveBeenCalledWith("old selection");
  const load = vi.fn().mockRejectedValue(Error("incomplete"));
  const next = imageRefresh(load, show, release);
  next.refresh(); await vi.advanceTimersByTimeAsync(0);
  next.stop(); await vi.runAllTimersAsync();
  expect(load).toHaveBeenCalledTimes(1);
});

it("fetches fresh bytes and releases snapshots that cannot decode", async () => {
  const fetcher = vi.fn().mockResolvedValue({ ok: true, blob: async () => new Blob(["partial"]) });
  vi.stubGlobal("fetch", fetcher);
  vi.stubGlobal("Image", class { src = ""; decode() { return Promise.reject(Error("invalid")); } });
  const revoke = vi.spyOn(URL, "revokeObjectURL");
  const signal = new AbortController().signal;
  await expect(loadImageSnapshot("asset://same/path", signal)).rejects.toThrow("invalid");
  expect(fetcher).toHaveBeenCalledWith("asset://same/path", { cache: "no-store", signal });
  expect(revoke).toHaveBeenCalledTimes(1);
  revoke.mockRestore();
});
