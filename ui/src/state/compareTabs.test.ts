import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CompareSessionsLoaded, DiffSource } from "../api/types";

const load = vi.fn<() => Promise<CompareSessionsLoaded>>();
const save = vi.fn(async (_s: unknown) => {});
const relink = vi.fn<(from: string, to: string, sessions?: unknown) => Promise<CompareSessionsLoaded>>();
vi.mock("../api/ipc", () => ({
  api: {
    compareSessionsLoad: () => load(),
    compareSessionsSave: (s: unknown) => save(s),
    compareSessionsRelink: (from: string, to: string, sessions?: unknown) => relink(from, to, sessions),
  },
}));

const file = (path: string): DiffSource => ({ kind: "file", path });
const earlierSession = (foreignVersion = false): CompareSessionsLoaded => ({
  sessions: {
    version: 3,
    active: 0,
    tabs: [
      {
        left: { source: file("/e/a"), label: "a" },
        right: { source: file("/e/b"), label: "b" },
        mode: "unified",
        options: { ignoreCase: true } as never,
        scroll: { left: false, line: 27 },
        activeHunk: 2,
      },
      { left: { source: file("/old/dir/gone.txt"), label: "gone.txt" }, right: { source: file("/old/dir/b2.txt"), label: "b2.txt" }, mode: "split", options: {} as never },
      { left: { source: null, label: "pasted" }, right: { source: file("/e/c"), label: "c" }, mode: "split", options: {} as never },
    ],
  },
  checks: [
    { index: 0, left: "changed", right: "ready", openable: true },
    { index: 1, left: "missing", right: "missing", openable: false },
    { index: 2, left: "notSaved", right: "ready", openable: false },
  ],
  foreignVersion,
});

describe("open comparisons as tabs", () => {
  beforeEach(async () => {
    vi.useFakeTimers();
    load.mockReset();
    save.mockClear();
    relink.mockReset();
    (await import("./compareTabs")).resetCompareTabs();
  });
  afterEach(() => vi.useRealTimers());

  it("registers pairs once, saves view state with the version read, and brings earlier tabs back", async () => {
    load.mockResolvedValue(earlierSession());
    const m = await import("./compareTabs");
    m.registerPair(file("/x"), file("/y"));
    m.registerPair(file("/x"), file("/y"));
    m.updateTabState(file("/x"), file("/y"), { mode: "unified", options: { ignoreEol: true }, scroll: { left: false, line: 19 }, activeHunk: 1 });
    m.registerPair(file("/p"), file("/q"));
    await vi.runOnlyPendingTimersAsync();
    expect(save).toHaveBeenCalledTimes(1);
    const saved = save.mock.calls[0][0] as { version: number; tabs: Array<{ mode: string; scroll: unknown; activeHunk: number | null }>; active: number };
    expect(saved.tabs).toHaveLength(2);
    expect(saved.active).toBe(1);
    expect(saved.tabs[0]).toMatchObject({ mode: "unified", scroll: { left: false, line: 19 }, activeHunk: 1 });
    // All three earlier tabs are listed with their check states; only the openable one is in `earlier`.
    expect(m.getCompareTabsState().earlierAll).toHaveLength(3);
    expect(m.getCompareTabsState().earlierAll.map((e) => e.check.left)).toEqual(["changed", "missing", "notSaved"]);
    const next = m.restoreEarlier();
    expect(next).toMatchObject({ label: "a ↔ b", mode: "unified", scroll: { left: false, line: 27 }, activeHunk: 2 });
    await vi.runOnlyPendingTimersAsync();
    expect(save).toHaveBeenCalledTimes(2);
    expect((save.mock.calls[1][0] as { version: number; tabs: unknown[] }).tabs).toHaveLength(3);
    expect((save.mock.calls[1][0] as { version: number }).version).toBe(3);
    // Nothing openable left to bring back, while missing & notSaved remain visible in earlierAll.
    expect(m.restoreEarlier()).toBeNull();
    expect(m.getCompareTabsState().earlierAll).toHaveLength(2);
  });

  it("relinks missing earlier tabs through relinkFolders and compare_sessions_relink", async () => {
    load.mockResolvedValue(earlierSession());
    relink.mockResolvedValue({
      sessions: {
        version: 3,
        active: 0,
        tabs: [
          { left: { source: file("/new/dir/gone.txt"), label: "gone.txt" }, right: { source: file("/new/dir/b2.txt"), label: "b2.txt" }, mode: "split", options: {} as never },
          { left: { source: null, label: "pasted" }, right: { source: file("/e/c"), label: "c" }, mode: "split", options: {} as never },
        ],
      },
      checks: [
        { index: 0, left: "ready", right: "ready", openable: true },
        { index: 1, left: "notSaved", right: "ready", openable: false },
      ],
      foreignVersion: false,
    });
    const m = await import("./compareTabs");
    m.registerPair(file("/x"), file("/y"));
    await vi.runOnlyPendingTimersAsync();
    m.restoreEarlier();
    const res = await m.relinkEarlier("/old/dir/gone.txt", "/new/dir/gone.txt");
    expect(res.pair).toEqual({ from: "/old", to: "/new" });
    expect(res.newlyOpenable).toBe(1);
    expect(relink).toHaveBeenCalledWith("/old", "/new", expect.objectContaining({ version: 3 }));
    const opened = m.restoreEarlierAt(0);
    expect(opened).toMatchObject({ label: "gone.txt ↔ b2.txt" });
    // Unrelated file name yields null without invoking IPC.
    relink.mockClear();
    const bad = await m.relinkEarlier("/old/dir/gone.txt", "/new/dir/other.txt");
    expect(bad.pair).toBeNull();
    expect(relink).not.toHaveBeenCalled();
  });

  it("closes a tab and shows its neighbour, and a file of a newer version is never overwritten", async () => {
    load.mockResolvedValue(earlierSession(true));
    const m = await import("./compareTabs");
    m.registerPair(file("/1"), file("/2"));
    m.registerPair(file("/3"), file("/4"));
    await vi.runOnlyPendingTimersAsync();
    expect(save).not.toHaveBeenCalled();
    const shown = m.closeTabAt(1);
    expect(shown).toMatchObject({ label: "1 ↔ 2" });
    expect(m.closeTabAt(0)).toBeNull();
    await vi.runOnlyPendingTimersAsync();
    expect(save).not.toHaveBeenCalled();
  });

  it("finds the neighbour tab, wrapping round, and none when there is only one", async () => {
    load.mockResolvedValue(earlierSession());
    const m = await import("./compareTabs");
    m.registerPair(file("/1"), file("/2"));
    expect(m.neighbourTab(1)).toBeNull();
    m.registerPair(file("/3"), file("/4"));
    m.registerPair(file("/5"), file("/6"));
    // The shown tab is the last one: next wraps to the first, previous is the one before.
    expect(m.neighbourTab(1)).toMatchObject({ label: "1 ↔ 2" });
    expect(m.neighbourTab(-1)).toMatchObject({ label: "3 ↔ 4" });
    expect(m.neighbourTab(-3)).toMatchObject({ label: "5 ↔ 6" });
  });
});
