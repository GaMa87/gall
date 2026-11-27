import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type KeyboardEvent, type ReactElement } from "react";
import type { IndexHit, IndexResult } from "../api/types";

// Exercise the component's handlers and async effects without a browser or new DOM dependencies.
const hooks = vi.hoisted(() => ({
  cursor: 0,
  values: [] as unknown[],
  deps: new Map<number, readonly unknown[]>(),
  cleanups: new Map<number, () => void>(),
  pending: [] as (() => void)[],
}));
vi.mock("react", async (original) => ({
  ...await original<typeof import("react")>(),
  useState: <T,>(initial: T | (() => T)) => {
    const i = hooks.cursor++;
    if (!(i in hooks.values)) hooks.values[i] = typeof initial === "function" ? (initial as () => T)() : initial;
    return [hooks.values[i] as T, (next: T | ((v: T) => T)) => {
      hooks.values[i] = typeof next === "function" ? (next as (v: T) => T)(hooks.values[i] as T) : next;
    }];
  },
  useRef: <T,>(value: T) => {
    const i = hooks.cursor++;
    return hooks.values[i] ??= { current: value };
  },
  useMemo: <T,>(factory: () => T) => factory(),
  useEffect: (effect: () => void | (() => void), deps: readonly unknown[]) => {
    const i = hooks.cursor++;
    const prev = hooks.deps.get(i);
    if (prev && prev.length === deps.length && prev.every((v, j) => Object.is(v, deps[j]))) return;
    hooks.deps.set(i, deps);
    hooks.pending.push(() => {
      hooks.cleanups.get(i)?.();
      const cleanup = effect();
      if (cleanup) hooks.cleanups.set(i, cleanup);
    });
  },
}));

const mocks = vi.hoisted(() => ({
  path: "C:\\project",
  state: { settings: { shortcuts: {} }, recentFiles: [] as string[], history: [] as string[] },
  indexQuery: vi.fn(),
  indexHiddenCount: vi.fn(),
  indexHiddenCancel: vi.fn(),
  indexCovers: vi.fn(),
  findExisting: vi.fn(),
  fsChanged: null as null | (() => void),
  openOverlay: vi.fn(),
  navigate: vi.fn(),
  openEntries: vi.fn(),
  pushRecent: vi.fn(),
  fail: vi.fn(),
  run: vi.fn(),
  zoxideDirs: vi.fn(),
  omarchyTerminalCwd: vi.fn(),
  actionsList: vi.fn(),
  actionsForSelection: vi.fn(),
  actionRun: vi.fn(),
}));
vi.mock("../api/ipc", () => ({
  api: {
    indexQuery: mocks.indexQuery, indexCovers: mocks.indexCovers,
    indexHiddenCount: mocks.indexHiddenCount, indexHiddenCancel: mocks.indexHiddenCancel,
    findExisting: mocks.findExisting,
    zoxideDirs: mocks.zoxideDirs, omarchyTerminalCwd: mocks.omarchyTerminalCwd,
    actionsList: mocks.actionsList, actionsForSelection: mocks.actionsForSelection, actionRun: mocks.actionRun,
    indexStatus: () => Promise.resolve(null), indexRefresh: () => Promise.resolve(false),
  },
  events: {
    indexUpdated: () => Promise.resolve(() => {}),
    fsChanged: (cb: () => void) => { mocks.fsChanged = cb; return Promise.resolve(() => {}); },
  },
}));
vi.mock("../state/store", () => ({
  useStore: (select: (s: typeof mocks.state) => unknown) => select(mocks.state),
  getState: () => ({}), pushRecent: mocks.pushRecent,
}));
vi.mock("../state/actions", () => ({
  activeBrowser: () => 0, activePath: () => mocks.path,
  openOverlay: mocks.openOverlay, navigate: mocks.navigate,
  openEntries: mocks.openEntries, fail: mocks.fail,
}));
vi.mock("../state/run", () => ({ run: mocks.run }));
vi.mock("./Icon", () => ({ FileIcon: () => null, Icon: () => null }));
vi.mock("./Preview", () => ({ Preview: () => null, usePreviewInfo: () => null }));

import { Highlight, QuickOpen } from "./QuickOpen";

type Props = {
  children?: unknown;
  className?: string;
  hit?: IndexHit;
  on?: boolean;
  zoxide?: boolean;
  text?: string;
  onKeyDown?: (e: KeyboardEvent) => void;
  onClick?: () => void;
  onChange?: (e: { target: { value: string } }) => void;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
function render(initial = "report") {
  hooks.cursor = 0;
  const tree = QuickOpen({ initial });
  hooks.pending.splice(0).forEach((effect) => effect());
  return tree;
}
function change(value: string) {
  elements(render()).find((n) => n.type === "input")!.props.onChange!({ target: { value } });
  return render();
}
function key(key: string, extra = {}) {
  const e = { key, altKey: false, nativeEvent: {}, stopPropagation: vi.fn(), preventDefault: vi.fn(), ...extra };
  elements(render()).find((n) => n.props.className === "qo")!.props.onKeyDown!(e as unknown as KeyboardEvent);
}
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: Error) => void;
  const promise = new Promise<T>((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
const result = (path: string, isDir = false): IndexResult => ({
  hits: [{ path, name: "report", isDir, score: 1, positions: [] }],
  total: 1, searched: 1, elapsedMs: 1, scanning: false,
});
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  hooks.cursor = 0;
  hooks.values = [];
  hooks.deps.clear();
  hooks.cleanups.clear();
  hooks.pending = [];
  mocks.path = "C:\\project";
  mocks.state.history = [];
  mocks.state.recentFiles = [];
  mocks.findExisting.mockImplementation((paths: string[]) => Promise.resolve(paths));
  mocks.indexCovers.mockResolvedValue({ covered: true, scanning: false, entries: 0 });
  mocks.indexQuery.mockResolvedValue(result("C:\\project\\report.txt"));
  mocks.indexHiddenCount.mockResolvedValue({ count: 0, capped: false });
  mocks.indexHiddenCancel.mockResolvedValue(undefined);
  mocks.zoxideDirs.mockResolvedValue([]);
  mocks.omarchyTerminalCwd.mockResolvedValue(null);
  mocks.actionsList.mockResolvedValue([]);
  mocks.actionsForSelection.mockResolvedValue([]);
  mocks.actionRun.mockResolvedValue(undefined);
});
afterEach(() => {
  hooks.cleanups.forEach((f) => f());
  vi.useRealTimers();
});

describe("Quick Open archive and async regressions", () => {
  it("omits deleted recent files and folders before they can be opened", async () => {
    mocks.state.recentFiles = ["C:\\project\\deleted.txt", "C:\\project\\live.txt"];
    mocks.state.history = ["C:\\project\\gone"];
    mocks.findExisting.mockResolvedValue(["C:\\project\\live.txt"]);
    render("");
    await flush();
    const paths = elements(render("")).flatMap((n) => n.props.hit ? [n.props.hit.path] : []);
    expect(paths).toEqual(["C:\\project\\live.txt"]);
    key("Enter");
    expect(mocks.openEntries).toHaveBeenCalledWith(0, [expect.objectContaining({ path: "C:\\project\\live.txt" })]);
  });

  it("rechecks recent paths and the active search after a filesystem change", async () => {
    mocks.state.recentFiles = ["C:\\project\\report.txt"];
    render("");
    await flush();
    expect(elements(render("")).some((n) => n.props.hit)).toBe(true);
    mocks.findExisting.mockResolvedValue([]);
    mocks.fsChanged!();
    render("");
    await flush();
    expect(elements(render("")).some((n) => n.props.hit)).toBe(false);
    change("report");
    await vi.advanceTimersByTimeAsync(25);
    mocks.indexQuery.mockResolvedValue({ ...result(""), hits: [], total: 0 });
    mocks.fsChanged!();
    await flush();
    expect(elements(render()).some((n) => n.props.hit)).toBe(false);
  });

  it("keeps the row Enter opens highlighted when earlier recent paths disappear", async () => {
    mocks.state.recentFiles = ["C:\\project\\deleted.txt", "C:\\project\\live.txt"];
    render("");
    await flush();
    key("ArrowDown");
    mocks.findExisting.mockResolvedValue(["C:\\project\\live.txt"]);
    mocks.fsChanged!();
    render("");
    await flush();
    const row = elements(render("")).find((n) => n.props.hit);
    expect(row?.props.on).toBe(true);
    key("Enter");
    expect(mocks.openEntries).toHaveBeenCalledWith(0, [expect.objectContaining({ path: "C:\\project\\live.txt" })]);
  });

  it("does not offer or activate filesystem-only searches inside archives", async () => {
    mocks.path = "C:\\project\\photos.zip\\2026";
    const tree = render();
    key("Tab");
    key("i", { altKey: true });
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.indexCovers).not.toHaveBeenCalled();
    expect(mocks.indexQuery).toHaveBeenCalledWith("report", null, 200);
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    const text = JSON.stringify(tree);
    expect(text).not.toContain("Alt+I");
    expect(text).not.toContain("and its subfolders");
  });

  it("toggles searching everything in-place on Alt+I in a real folder", async () => {
    render();
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.indexQuery).toHaveBeenLastCalledWith("report", mocks.path, 200);

    mocks.indexQuery.mockResolvedValueOnce({
      hits: [
        { path: "C:\\project\\report.txt", name: "report.txt", isDir: false, score: 10, positions: [] },
        { path: "C:\\project\\gen\\report_gen.txt", name: "report_gen.txt", isDir: false, score: 8, positions: [], hidden: true },
      ],
      total: 2, searched: 2, elapsedMs: 1, scanning: false,
    });
    key("i", { altKey: true });
    render();
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    expect(mocks.indexQuery).toHaveBeenLastCalledWith("report", mocks.path, 200, true, undefined);
    const rows = elements(render()).filter((n) => n.props.hit);
    expect(rows.map((r) => [r.props.hit!.name, !!r.props.hit!.hidden])).toEqual([
      ["report.txt", false],
      ["report_gen.txt", true],
    ]);

    // Second Alt+I toggles filters back on.
    key("i", { altKey: true });
    render();
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.indexQuery).toHaveBeenLastCalledWith("report", mocks.path, 200);
  });

  it("counts hidden matches and supports Alt+I in everywhere scope too", async () => {
    mocks.indexQuery.mockResolvedValue({ hits: [], total: 0, searched: 10, elapsedMs: 1, scanning: false });
    mocks.indexHiddenCount.mockResolvedValue({ count: 5, capped: false });
    render("secret");
    // Switch to everywhere via Tab.
    key("Tab");
    render("secret");
    await vi.advanceTimersByTimeAsync(150);
    await flush();
    expect(mocks.indexHiddenCount).toHaveBeenLastCalledWith("secret", null, mocks.path);
    const emptyBox = elements(render("secret")).find((n) => n.props.className === "qo-empty");
    expect(JSON.stringify(emptyBox)).toContain("5 hidden files match");
    expect(JSON.stringify(emptyBox)).toContain("Alt+I");

    // Pressing Alt+I in everywhere keeps everywhere scope (null) and includes hidden files.
    key("i", { altKey: true });
    render("secret");
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.indexQuery).toHaveBeenLastCalledWith("secret", null, 200, true, mocks.path);
  });

  it("does not interpret AltGr input as Alt+I", async () => {
    render();
    await vi.advanceTimersByTimeAsync(25);
    mocks.indexQuery.mockClear();
    key("i", { altKey: true, ctrlKey: true });
    render();
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    expect(mocks.indexQuery).not.toHaveBeenCalled();
  });

  it("shows empty-state hidden hint only when hidden files match and lets Alt+I reveal them", async () => {
    mocks.indexQuery.mockResolvedValue({ hits: [], total: 0, searched: 10, elapsedMs: 1, scanning: false });
    mocks.indexHiddenCount.mockResolvedValue({ count: 3, capped: false });
    render("secret");
    await vi.advanceTimersByTimeAsync(150);
    await flush();
    const emptyBox = elements(render("secret")).find((n) => n.props.className === "qo-empty");
    const emptyText = JSON.stringify(emptyBox);
    expect(emptyText).toContain("No results.");
    expect(emptyText).toContain("3 hidden files match");
    expect(emptyText).toContain("Alt+I");

    // Nothing hidden: no hidden hint shown in the empty state.
    mocks.indexHiddenCount.mockResolvedValue({ count: 0, capped: false });
    change("absent");
    await vi.advanceTimersByTimeAsync(150);
    await flush();
    const nothingBox = elements(render("secret")).find((n) => n.props.className === "qo-empty");
    const nothingText = JSON.stringify(nothingBox);
    expect(nothingText).toContain("No matches");
    expect(nothingText).not.toContain("hidden file");
    expect(nothingText).not.toContain("Alt+I");
  });

  it("drops stale hidden-match counts when the query changes before the count returns", async () => {
    const stale = deferred<{ count: number; capped: boolean }>();
    mocks.indexHiddenCount.mockReturnValueOnce(stale.promise);
    render("first");
    await vi.advanceTimersByTimeAsync(150);
    change("second");
    expect(mocks.indexHiddenCancel).toHaveBeenCalled();
    stale.resolve({ count: 9, capped: false });
    await flush();
    expect(elements(render("first")).some((n) => n.props.className === "hidden-bar")).toBe(false);
  });

  it("shows subtle hidden-match footer when both visible and hidden files match", async () => {
    mocks.indexHiddenCount.mockResolvedValue({ count: 100, capped: true });
    render("report");
    await vi.advanceTimersByTimeAsync(150);
    await flush();
    const tree = render("report");
    const bar = elements(tree).find((n) => n.props.className === "hidden-bar");
    expect(bar).toBeDefined();
    expect(JSON.stringify(bar)).toContain("+100+ hidden by filters");
    expect(JSON.stringify(bar)).toContain("Alt+I");
  });

  it("does not override a manually selected scope when coverage arrives late", async () => {
    const coverage = deferred<{ covered: boolean; scanning: boolean; entries: number }>();
    mocks.indexCovers.mockReturnValue(coverage.promise);
    render();
    key("Tab");
    render();
    coverage.resolve({ covered: false, scanning: true, entries: 0 });
    await flush();
    render();
    await vi.advanceTimersByTimeAsync(25);
    expect(mocks.indexQuery).toHaveBeenLastCalledWith("report", null, 200);
  });

  it("drops pending results when the query is cleared and subsequently changed", async () => {
    const old = deferred<IndexResult>();
    mocks.indexQuery.mockReturnValueOnce(old.promise);
    render();
    await vi.advanceTimersByTimeAsync(25);
    change("");
    await vi.advanceTimersByTimeAsync(25);
    old.resolve(result("C:\\project\\stale.txt"));
    await flush();
    expect(elements(change("new")).some((n) => n.props.hit?.path === "C:\\project\\stale.txt")).toBe(false);
  });

  it("drops the old response while a new query is still debouncing", async () => {
    const old = deferred<IndexResult>();
    mocks.indexQuery.mockReturnValueOnce(old.promise);
    render();
    await vi.advanceTimersByTimeAsync(25);
    change("new");
    old.resolve(result("C:\\project\\stale.txt"));
    await flush();
    expect(elements(render()).some((n) => n.props.hit?.path === "C:\\project\\stale.txt")).toBe(false);
  });

  it.each(["change", "close"])("ignores pending failures after %s", async (action) => {
    const old = deferred<IndexResult>();
    mocks.indexQuery.mockReturnValueOnce(old.promise);
    render();
    await vi.advanceTimersByTimeAsync(25);
    if (action === "change") change("other");
    else hooks.cleanups.forEach((f) => f());
    old.reject(new Error("stale failure"));
    await flush();
    expect(mocks.fail).not.toHaveBeenCalled();
  });

  it("does not open a file while Enter is confirming IME input", async () => {
    render();
    await vi.advanceTimersByTimeAsync(25);
    key("Enter", { nativeEvent: { isComposing: true } });
    key("Enter", { keyCode: 229 });
    expect(mocks.openEntries).not.toHaveBeenCalled();
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    key("Enter");
    expect(mocks.openEntries).toHaveBeenCalledOnce();
    expect(mocks.pushRecent).toHaveBeenCalledWith("C:\\project\\report.txt");
  });

  it("does not record a recent folder as a file", async () => {
    mocks.indexQuery.mockResolvedValue(result("C:\\project\\report", true));
    render();
    await vi.advanceTimersByTimeAsync(25);
    key("Enter");
    expect(mocks.navigate).toHaveBeenCalledWith(0, "C:\\project\\report");
    expect(mocks.pushRecent).not.toHaveBeenCalled();
  });
});

describe("Quick Open highlighting of composed and decomposed names", () => {
  /** The marked and unmarked pieces of a rendered Highlight, in order. */
  const pieces = (el: ReactElement) => {
    const kids = (el.props as { children: unknown }).children;
    const list = (Array.isArray(kids) ? kids : [kids]) as unknown[];
    return list.flat().map((c) => {
      if (typeof c === "string") return { s: c, on: false };
      const e = c as ReactElement<{ children: string }>;
      return { s: e.props.children, on: e.type === "mark" };
    });
  };

  it("marks the exact glyph of an NFD name: positions count composed characters", () => {
    const nfd = "Poroc\u{30c}ilo.txt"; // "c" + combining caron
    // "Poročilo.txt": the "č" is the 5th character (index 4).
    const parts = pieces(Highlight({ text: nfd, positions: [4] }));
    expect(parts.filter((p) => p.on).map((p) => p.s)).toEqual(["\u{10d}"]);
    expect(parts.map((p) => p.s).join("")).toBe("Poro\u{10d}ilo.txt");
    // Without the fix the mark landed on the combining caron's neighbour ("i").
    expect(parts.find((p) => p.on && p.s === "i")).toBeUndefined();
  });

  it("matches an NFC name the same way and counts an emoji as one position", () => {
    const nfc = pieces(Highlight({ text: "Poro\u{10d}ilo.txt", positions: [4] }));
    expect(nfc.filter((p) => p.on).map((p) => p.s)).toEqual(["\u{10d}"]);
    const emoji = pieces(Highlight({ text: "\u{1F600}a.txt", positions: [1] }));
    expect(emoji.filter((p) => p.on).map((p) => p.s)).toEqual(["a"]);
  });

  it("shifts positions by the offset of the name inside the path", () => {
    const parts = pieces(Highlight({ text: "de\u{301}p.txt", positions: [7, 8], offset: 5 }));
    expect(parts.filter((p) => p.on).map((p) => p.s).join("")).toBe("p.");
  });
});

describe("Quick Open zoxide folders", () => {
  const DOCS = "C:\\project\\sub\\docs";
  const zoxideRows = () => elements(render()).filter((n) => n.props.zoxide).map((n) => n.props.hit!.path);
  const allRows = () => elements(render()).flatMap((n) => (n.props.hit ? [n.props.hit.path] : []));
  const type = async (value: string) => { change(value); await vi.advanceTimersByTimeAsync(25); await flush(); };

  it("adds matching zoxide folders after the index hits, inside the searched folder, without duplicates", async () => {
    mocks.indexQuery.mockResolvedValue(result(DOCS, true));
    mocks.zoxideDirs.mockResolvedValue([DOCS, "C:\\project\\docs-old", "C:\\other\\docs", "C:\\project\\unrelated"]);
    render("");
    await type("docs");
    // The index hit comes first and is not repeated; "other" is outside this folder; "unrelated" does not match.
    expect(allRows()).toEqual([DOCS, "C:\\project\\docs-old"]);
    expect(zoxideRows()).toEqual(["C:\\project\\docs-old"]);
    expect(mocks.zoxideDirs).toHaveBeenCalledTimes(1);
  });

  it("includes folders from everywhere after Tab, in zoxide's own order, capped at ten", async () => {
    mocks.zoxideDirs.mockResolvedValue([
      "C:\\other\\docs", ...Array.from({ length: 12 }, (_, i) => `C:\\project\\docs${i}`),
    ]);
    mocks.indexQuery.mockResolvedValue({ hits: [], total: 0, searched: 0, elapsedMs: 1, scanning: false });
    render("");
    await type("docs");
    expect(zoxideRows()).toHaveLength(10);
    expect(zoxideRows()).not.toContain("C:\\other\\docs");
    key("Tab");
    await vi.advanceTimersByTimeAsync(25);
    await flush();
    expect(zoxideRows()[0]).toBe("C:\\other\\docs");
    expect(zoxideRows()).toHaveLength(10);
  });

  it("opens the highlighted zoxide folder with Enter, as a folder (no recent file is recorded)", async () => {
    mocks.indexQuery.mockResolvedValue(result("C:\\project\\report.txt"));
    mocks.zoxideDirs.mockResolvedValue(["C:\\project\\reports-archive"]);
    render("");
    await type("report");
    expect(allRows()).toEqual(["C:\\project\\report.txt", "C:\\project\\reports-archive"]);
    key("ArrowDown");
    key("Enter");
    expect(mocks.navigate).toHaveBeenCalledWith(0, "C:\\project\\reports-archive");
    expect(mocks.openOverlay).toHaveBeenCalledWith(null);
    expect(mocks.pushRecent).not.toHaveBeenCalled();
    expect(mocks.openEntries).not.toHaveBeenCalled();
  });

  it("asks zoxide once, only for a file query: not for an empty query or the command palette", async () => {
    mocks.zoxideDirs.mockResolvedValue(["C:\\project\\docs"]);
    render("");
    await vi.advanceTimersByTimeAsync(25);
    await type(">docs");
    expect(mocks.zoxideDirs).not.toHaveBeenCalled();
    await type("docs");
    await type("docs ");
    await type("do");
    expect(mocks.zoxideDirs).toHaveBeenCalledTimes(1);
  });

  it("shows no zoxide rows when zoxide fails or is missing, and keeps the index hits", async () => {
    mocks.zoxideDirs.mockRejectedValue(new Error("zoxide: not found"));
    render("");
    await type("report");
    expect(zoxideRows()).toEqual([]);
    expect(allRows()).toEqual(["C:\\project\\report.txt"]);
    expect(mocks.fail).not.toHaveBeenCalled();
  });

  it("keeps zoxide's answer when the effect is cleaned up and run again (StrictMode)", async () => {
    const first = deferred<string[]>();
    mocks.zoxideDirs.mockReturnValueOnce(first.promise).mockResolvedValue(["C:\\project\\docs-old"]);
    mocks.indexQuery.mockResolvedValue({ hits: [], total: 0, searched: 0, elapsedMs: 1, scanning: false });
    render("");
    change("docs");
    // StrictMode: every effect is cleaned up and run once more before the first answer is back.
    hooks.deps.clear();
    render();
    first.resolve(["C:\\project\\docs-old"]);
    await vi.advanceTimersByTimeAsync(25);
    await flush();
    expect(zoxideRows()).toEqual(["C:\\project\\docs-old"]);
  });
});

describe("Quick Open command palette (>)", () => {
  const cmdRows = () => elements(render()).filter((n) => typeof n.props.className === "string" && n.props.className.startsWith("qo-row cmd"));
  const labelOf = (row: ReactElement<Props>) => elements(row).find((n) => n.type === Highlight)!.props.text!;
  const labels = () => cmdRows().map(labelOf);
  const onRow = () => cmdRows().findIndex((r) => (r.props.className as string).includes(" on"));
  const type = async (value: string) => { change(value); await flush(); await flush(); };
  const resize = { id: "resize", label: "Resize {count}", pluralLabel: "Resize {count} images" };
  const other = { id: "zip", label: "Zip it" };

  it("lists the built-in commands for '>' and narrows them by fuzzy match", async () => {
    render("");
    await type(">");
    expect(labels()).toContain("Rename");
    expect(labels()).toContain("Open with…");
    await type(">renam");
    expect(labels()[0]).toBe("Rename");
    expect(labels()).not.toContain("Open with…");
  });

  it("runs the highlighted command after closing Quick Open", async () => {
    render("");
    await type(">rename");
    key("Enter");
    expect(mocks.openOverlay).toHaveBeenCalledWith(null);
    expect(mocks.run).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.run).toHaveBeenCalledWith("rename");
  });

  it("moves the highlight with the arrows and Ctrl+N / Ctrl+P, wrapping around, and clicks run that row", async () => {
    render("");
    await type(">rename");
    const n = labels().length;
    expect(n).toBeGreaterThan(1);
    expect(onRow()).toBe(0);
    key("ArrowDown");
    expect(onRow()).toBe(1);
    key("n", { ctrlKey: true });
    expect(onRow()).toBe(n > 2 ? 2 : 0);
    key("ArrowUp");
    key("ArrowUp");
    key("ArrowUp");
    key("p", { ctrlKey: true });
    expect(onRow()).toBeGreaterThanOrEqual(0);
    const target = cmdRows()[1];
    target.props.onClick!();
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.run).toHaveBeenCalledTimes(1);
    expect(mocks.run).toHaveBeenCalledWith(expect.any(String));
  });

  it("says so when no command matches and Enter then does nothing", async () => {
    render("");
    await type(">qqqzzzxxx");
    expect(cmdRows()).toHaveLength(0);
    const empty = elements(render()).find((n) => n.props.className === "qo-empty")!;
    expect(empty.props.children).toBe("No matching command");
    key("Enter");
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.run).not.toHaveBeenCalled();
    expect(mocks.openOverlay).not.toHaveBeenCalled();
  });

  it("offers actions.d actions that match the selection, and runs one on the current folder", async () => {
    mocks.actionsList.mockResolvedValue([resize, other]);
    mocks.actionsForSelection.mockResolvedValue([resize]);
    render("");
    await type(">");
    expect(labels()).toContain("Action: Resize 1");
    expect(labels().some((l) => l.includes("Zip it"))).toBe(false);
    // With nothing selected the action targets the folder itself.
    expect(mocks.actionsForSelection).toHaveBeenCalledWith([{ path: "C:\\project", isDir: true, ext: "" }]);
    await type(">resize");
    expect(labels()[0]).toBe("Action: Resize 1");
    key("Enter");
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.actionRun).toHaveBeenCalledWith("resize", ["C:\\project"], "C:\\project");
    expect(mocks.run).not.toHaveBeenCalled();
  });

  it("reports a failing action instead of dropping the error", async () => {
    mocks.actionsList.mockResolvedValue([resize]);
    mocks.actionsForSelection.mockResolvedValue([resize]);
    const boom = new Error("exit 1");
    mocks.actionRun.mockRejectedValue(boom);
    render("");
    await type(">resize");
    key("Enter");
    await vi.advanceTimersByTimeAsync(1);
    await flush();
    expect(mocks.fail).toHaveBeenCalledWith(boom);
  });

  it("offers the active terminal's folder only when one is found, and goes there", async () => {
    render("");
    await type(">terminal");
    expect(labels().some((l) => l.includes("terminal's folder"))).toBe(false);
    mocks.omarchyTerminalCwd.mockResolvedValue("C:\\work\\term");
    hooks.values = []; hooks.deps.clear(); hooks.cleanups.clear();
    render("");
    await type(">active terminal");
    expect(labels()[0]).toBe("Open active terminal's folder");
    key("Enter");
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.navigate).toHaveBeenCalledWith(0, "C:\\work\\term");
  });

  it("drops entries of a palette that was left before they arrived, and never duplicates them (StrictMode)", async () => {
    const stale = deferred<unknown[]>();
    mocks.actionsList.mockReturnValueOnce(stale.promise).mockResolvedValue([resize]);
    mocks.actionsForSelection.mockResolvedValue([resize]);
    render("");
    await type(">resize");
    // Leave the palette (the effect is cleaned up), then the late answer arrives.
    await type("report");
    stale.resolve([resize]);
    await flush();
    expect(cmdRows()).toHaveLength(0);
    // StrictMode: enter again, clean up and run the effect once more.
    await type(">resize");
    hooks.deps.clear();
    render();
    await flush();
    await flush();
    const found = labels().filter((l) => l === "Action: Resize 1");
    expect(found).toHaveLength(1);
  });
});
