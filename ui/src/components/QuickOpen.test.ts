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
  indexCovers: vi.fn(),
  findExisting: vi.fn(),
  fsChanged: null as null | (() => void),
  openOverlay: vi.fn(),
  navigate: vi.fn(),
  openEntries: vi.fn(),
  pushRecent: vi.fn(),
  fail: vi.fn(),
}));
vi.mock("../api/ipc", () => ({
  api: {
    indexQuery: mocks.indexQuery, indexCovers: mocks.indexCovers,
    findExisting: mocks.findExisting,
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
vi.mock("../state/run", () => ({ run: vi.fn() }));
vi.mock("./Icon", () => ({ FileIcon: () => null, Icon: () => null }));
vi.mock("./Preview", () => ({ Preview: () => null, usePreviewInfo: () => null }));

import { QuickOpen } from "./QuickOpen";

type Props = {
  children?: unknown;
  className?: string;
  hit?: IndexHit;
  on?: boolean;
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

  it("still opens the ignored-file search in a real folder", () => {
    key("i", { altKey: true });
    expect(mocks.openOverlay).toHaveBeenCalledWith({
      type: "search", root: mocks.path, pattern: "report", everything: true,
    });
  });

  it("does not interpret AltGr input as Alt+I", () => {
    key("i", { altKey: true, ctrlKey: true });
    expect(mocks.openOverlay).not.toHaveBeenCalled();
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
