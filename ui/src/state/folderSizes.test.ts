import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: (p: string) => p }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
import { api } from "../api/ipc";
import { applyFolderSizeProgress, calcFolderSizes, cancelFolderSizes, invalidateAllFolderSizes, invalidateFolderSizes } from "./actions";
import { registerController, type BrowserController } from "./controllers";
import type { Settings } from "../api/types";
import { getState, setState } from "./store";

const total = (bytes: number) => ({ bytes, files: 1, dirs: 0, unreadable: 0 });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}
beforeEach(() => {
  vi.useFakeTimers();
  cancelFolderSizes("test");
  cancelFolderSizes(0);
  cancelFolderSizes("properties");
  vi.restoreAllMocks();
  vi.spyOn(api, "folderSizesCancel").mockResolvedValue();
  setState({ folderSizes: {}, settings: null });
});

afterEach(async () => {
  setState({ settings: null });
  registerController(0, null);
  await vi.runOnlyPendingTimersAsync();
  vi.useRealTimers();
});

describe("folder size calculations", () => {
  it("retries an invalidated finished row after a busy owner finishes another row", async () => {
    setState({ settings: { folderSizesAuto: true } as Settings });
    registerController(0, { path: () => "/", entries: () => [{ path: "/a", isDir: true }, { path: "/b", isDir: true }] } as BrowserController);
    const slow = deferred<ReturnType<typeof total>>();
    const count = vi.spyOn(api, "folderSize").mockResolvedValueOnce(total(100)).mockReturnValueOnce(slow.promise).mockResolvedValue(total(0));
    const run = calcFolderSizes(0, ["/a", "/b"], 120_000);
    await Promise.resolve();
    await Promise.resolve();
    invalidateFolderSizes("/a/nested");
    await vi.advanceTimersByTimeAsync(300);
    expect(count).toHaveBeenCalledTimes(2);
    slow.resolve(total(10));
    await run;
    await Promise.resolve();
    expect(count).toHaveBeenCalledTimes(3);
    expect(getState().folderSizes["/a"].bytes).toBe(0);
  });

  it("Properties shares a panel count and cannot erase it on close", async () => {
    const pending = deferred<ReturnType<typeof total>>();
    const count = vi.spyOn(api, "folderSize").mockReturnValue(pending.promise);
    const run = calcFolderSizes("test", ["/a"]);
    const properties = calcFolderSizes("properties", ["/a"]);
    cancelFolderSizes("properties");
    pending.resolve(total(100));
    await Promise.all([run, properties]);
    expect(count).toHaveBeenCalledTimes(1);
    expect(getState().folderSizes["/a"].bytes).toBe(100);
    expect(getState().folderSizes["/a"].done).toBe(true);
  });
  it.each([
    ["properties", "test"],
    ["test", "properties"],
  ])("%s cancellation lets interested %s finish even with automatic calculation off", async (source, interested) => {
    const pending = deferred<ReturnType<typeof total> | null>();
    const count = vi.spyOn(api, "folderSize").mockReturnValueOnce(pending.promise).mockResolvedValue(total(200));
    vi.spyOn(api, "folderSizesCancel").mockResolvedValue();
    const first = calcFolderSizes(source, ["/a"]);
    const second = calcFolderSizes(interested, ["/a"], 30_000);
    expect(count).toHaveBeenCalledTimes(1);
    cancelFolderSizes(source);
    await second;
    pending.resolve(total(100));
    await first;
    expect(count).toHaveBeenCalledTimes(2);
    expect(getState().folderSizes["/a"].bytes).toBe(200);
    expect(getState().folderSizes["/a"].done).toBe(true);
  });

  it("waiting owners share one replacement after the source fails", async () => {
    const pending = deferred<ReturnType<typeof total> | null>();
    const replacement = deferred<ReturnType<typeof total>>();
    const count = vi.spyOn(api, "folderSize").mockReturnValueOnce(pending.promise).mockReturnValueOnce(replacement.promise);
    const first = calcFolderSizes("test", ["/a"]);
    const second = calcFolderSizes("properties", ["/a"]);
    const third = calcFolderSizes(0, ["/a"]);
    pending.resolve(null);
    await first;
    expect(count).toHaveBeenCalledTimes(2);
    replacement.resolve(total(50));
    await Promise.all([second, third]);
    expect(count).toHaveBeenCalledTimes(2);
    expect(getState().folderSizes["/a"].bytes).toBe(50);
  });

  it("automatic listing refreshes do not cancel or restart a manual /proc count", async () => {
    const pending = deferred<ReturnType<typeof total>>();
    const count = vi.spyOn(api, "folderSize").mockReturnValue(pending.promise);
    const cancel = vi.spyOn(api, "folderSizesCancel").mockResolvedValue();
    const run = calcFolderSizes("test", ["/proc"]);
    const job = getState().folderSizes["/proc"].job;
    await calcFolderSizes("test", ["/proc"], 120_000);
    expect(getState().folderSizes["/proc"].job).toBe(job);
    expect(count).toHaveBeenCalledTimes(1);
    expect(cancel).not.toHaveBeenCalled();
    pending.resolve(total(10));
    await run;
    await calcFolderSizes("test", ["/proc"], 120_000);
    expect(count).toHaveBeenCalledTimes(1);
    expect(getState().folderSizes["/proc"].done).toBe(true);
  });

  it("late progress from another ticket cannot modify the active row", () => {
    setState({ folderSizes: { "/a": { ...total(20), at: 1, done: false, job: 2 } } });
    applyFolderSizeProgress({ path: "/a", size: total(100), ticket: 1 });
    expect(getState().folderSizes["/a"].bytes).toBe(20);
    applyFolderSizeProgress({ path: "/a", size: total(30), ticket: 2 });
    expect(getState().folderSizes["/a"].bytes).toBe(30);
    expect(getState().folderSizes["/a"].at).toBe(1);
  });

  it("a superseded successful response cannot overwrite a newer total", async () => {
    const old = deferred<ReturnType<typeof total>>();
    vi.spyOn(api, "folderSizesCancel").mockResolvedValue();
    vi.spyOn(api, "folderSize").mockReturnValueOnce(old.promise).mockResolvedValueOnce(total(20));
    const first = calcFolderSizes("test", ["/a"]);
    await calcFolderSizes("test", ["/a"]);
    old.resolve(total(100));
    await first;
    expect(getState().folderSizes["/a"].bytes).toBe(20);
  });

  it("cancellation restores a known total and discards late success", async () => {
    setState({ folderSizes: { "/a": { ...total(5), at: Date.now(), done: true } } });
    const pending = deferred<ReturnType<typeof total>>();
    vi.spyOn(api, "folderSize").mockReturnValue(pending.promise);
    vi.spyOn(api, "folderSizesCancel").mockResolvedValue();
    const run = calcFolderSizes("test", ["/a"]);
    cancelFolderSizes("test");
    pending.resolve(total(100));
    await run;
    expect(getState().folderSizes["/a"].bytes).toBe(5);
    expect(getState().folderSizes["/a"].done).toBe(true);
  });

  it("a deletion in a nested folder invalidates ancestors without matching sibling prefixes", async () => {
    setState({ folderSizes: Object.fromEntries(["/a", "/a/nested", "/ab"].map((p) => [p, { ...total(100), done: true, at: Date.now() }])) });
    invalidateFolderSizes("/a/nested");
    expect(getState().folderSizes["/a"].at).toBe(0);
    expect(getState().folderSizes["/a/nested"].at).toBe(0);
    expect(getState().folderSizes["/ab"].at).toBeGreaterThan(0);
    vi.spyOn(api, "folderSize").mockResolvedValue(total(0));
    await calcFolderSizes("test", ["/a"], 120_000);
    expect(getState().folderSizes["/a"].bytes).toBe(0);
  });

  it("a finished task makes every cached total stale, also folders the watcher does not see", () => {
    setState({ folderSizes: Object.fromEntries(["/a", "/b/sub"].map((p) => [p, { ...total(100), done: true, at: Date.now() }])) });
    invalidateAllFolderSizes();
    expect(getState().folderSizes["/a"].at).toBe(0);
    expect(getState().folderSizes["/b/sub"].at).toBe(0);
  });

  it("changes during a count keep the result stale for a follow-up count", async () => {
    const pending = deferred<ReturnType<typeof total>>();
    vi.spyOn(api, "folderSize").mockReturnValue(pending.promise);
    const run = calcFolderSizes("test", ["/a"]);
    invalidateFolderSizes("/a/nested");
    pending.resolve(total(100));
    await run;
    expect(getState().folderSizes["/a"].at).toBe(0);
  });
});
