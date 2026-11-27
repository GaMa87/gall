import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type KeyboardEvent, type ReactElement } from "react";
import type { FileMatches, GrepStats } from "../api/types";

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
  grepStart: vi.fn(),
  grepCancel: vi.fn(),
  grepContext: vi.fn(),
  onBatch: null as null | ((p: { id: number; files: FileMatches[] }) => void),
  onDone: null as null | ((p: { id: number; stats: GrepStats; cancelled: boolean; error: string | null }) => void),
  onHidden: null as null | ((p: { id: number; count: number; capped: boolean }) => void),
  openOverlay: vi.fn(),
  fail: vi.fn(),
}));
vi.mock("../api/ipc", () => ({
  api: {
    grepStart: mocks.grepStart,
    grepCancel: mocks.grepCancel,
    grepContext: mocks.grepContext,
  },
  events: {
    grepBatch: (cb: NonNullable<typeof mocks.onBatch>) => { mocks.onBatch = cb; return Promise.resolve(() => {}); },
    grepDone: (cb: NonNullable<typeof mocks.onDone>) => { mocks.onDone = cb; return Promise.resolve(() => {}); },
    grepHidden: (cb: NonNullable<typeof mocks.onHidden>) => { mocks.onHidden = cb; return Promise.resolve(() => {}); },
  },
}));
vi.mock("../state/store", () => ({
  useStore: (select: (s: { info: { home: string } }) => unknown) => select({ info: { home: "/home/u" } }),
  pushRecent: vi.fn(),
}));
vi.mock("../state/actions", () => ({
  activeBrowser: () => 0,
  otherTarget: () => 1,
  openOverlay: mocks.openOverlay,
  focusBrowser: vi.fn(),
  navigate: vi.fn(),
  openEntries: vi.fn(),
  items: (n: number) => `${n} items`,
  fail: mocks.fail,
}));
vi.mock("../state/controllers", () => ({ controller: () => null }));
vi.mock("./Icon", () => ({ FileIcon: () => null, Icon: () => null }));

import { FindInFiles } from "./FindInFiles";

type Props = {
  children?: unknown;
  className?: string;
  onKeyDown?: (e: KeyboardEvent) => void;
  onChange?: (e: { target: { value: string } }) => void;
  onClick?: () => void;
  "data-role"?: string;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
function render() {
  hooks.cursor = 0;
  const tree = FindInFiles({ root: "/home/u/proj", scope: "folder" });
  hooks.pending.splice(0).forEach((effect) => effect());
  return tree;
}
function typeQuery(value: string) {
  const input = elements(render()).find((n) => n.type === "input" && n.props["data-role"] === "query")!;
  input.props.onChange!({ target: { value } });
  return render();
}
function key(k: string, extra = {}) {
  const e = { key: k, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, target: { tagName: "DIV", dataset: {} }, stopPropagation: vi.fn(), preventDefault: vi.fn(), ...extra };
  elements(render()).find((n) => n.props.className === "fif")!.props.onKeyDown!(e as unknown as KeyboardEvent);
}
const emptyStats: GrepStats = { files: 0, matches: 0, searched: 10, cloudOnly: 0, truncated: false, elapsedMs: 5 };

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  hooks.cursor = 0;
  hooks.values = [];
  hooks.deps.clear();
  hooks.cleanups.clear();
  hooks.pending = [];
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  mocks.grepStart.mockResolvedValue(undefined);
  mocks.grepCancel.mockResolvedValue(undefined);
  mocks.grepContext.mockResolvedValue([]);
});
afterEach(() => {
  hooks.cleanups.forEach((f) => f());
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("FindInFiles hidden-filter UX", () => {
  it("shows empty-state hidden hint for zero visible + N hidden, and hides it when nothing is hidden", async () => {
    render();
    typeQuery("secret");
    await vi.advanceTimersByTimeAsync(250);
    const id = mocks.grepStart.mock.calls[0][0] as number;
    mocks.onDone!({ id, stats: emptyStats, cancelled: false, error: null });

    // Before any hidden matches arrive: plain "No results", no Alt+I hint in empty state.
    const emptyPlain = elements(render()).find((n) => n.props.className === "fif-empty");
    expect(JSON.stringify(emptyPlain)).toContain("No results");
    expect(JSON.stringify(emptyPlain)).not.toContain("Alt+I");

    // Hidden matches arrive asynchronously.
    mocks.onHidden!({ id, count: 4, capped: false });
    const emptyWithHidden = elements(render()).find((n) => n.props.className === "fif-empty");
    expect(JSON.stringify(emptyWithHidden)).toContain("No results.");
    expect(JSON.stringify(emptyWithHidden)).toContain("4 hidden files match");
    expect(JSON.stringify(emptyWithHidden)).toContain("Alt+I");
  });

  it("shows subtle footer indicator when both visible and hidden matches exist", async () => {
    render();
    typeQuery("shared");
    await vi.advanceTimersByTimeAsync(250);
    const id = mocks.grepStart.mock.calls[0][0] as number;
    mocks.onBatch!({ id, files: [{ path: "/home/u/proj/src/a.ts", matches: [{ line: 1, text: "shared", ranges: [[0, 6]] }] }] });
    mocks.onDone!({ id, stats: { ...emptyStats, files: 1, matches: 1 }, cancelled: false, error: null });
    mocks.onHidden!({ id, count: 2, capped: false });

    const bar = elements(render()).find((n) => n.props.className === "hidden-bar");
    expect(bar).toBeDefined();
    expect(JSON.stringify(bar)).toContain("+2 hidden by filters");
    expect(JSON.stringify(bar)).toContain("Alt+I");
  });

  it("toggles everything on Alt+I, re-runs grep, marks hidden results and ignores AltGr", async () => {
    render();
    typeQuery("shared");
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.grepStart).toHaveBeenCalledTimes(1);
    expect(mocks.grepStart.mock.calls[0][3]).toMatchObject({ everything: false });

    // AltGr must not toggle.
    key("i", { altKey: true, ctrlKey: true });
    render();
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.grepStart).toHaveBeenCalledTimes(1);

    // Alt+I enables everything and re-runs grep.
    key("i", { altKey: true });
    render();
    await vi.advanceTimersByTimeAsync(250);
    expect(mocks.grepStart).toHaveBeenCalledTimes(2);
    expect(mocks.grepStart.mock.calls[1][3]).toMatchObject({ everything: true });
    const id2 = mocks.grepStart.mock.calls[1][0] as number;
    mocks.onBatch!({
      id: id2,
      files: [
        { path: "/home/u/proj/src/a.ts", matches: [{ line: 1, text: "shared", ranges: [[0, 6]] }] },
        { path: "/home/u/proj/gen/b.ts", matches: [{ line: 2, text: "shared", ranges: [[0, 6]] }], hidden: true },
      ],
    });
    const tree = render();
    const fileRows = elements(tree).filter((n) => n.props.className?.includes("fif-file"));
    expect(fileRows).toHaveLength(2);
    expect(fileRows[0].props.className).not.toContain("is-hidden");
    expect(fileRows[1].props.className).toContain("is-hidden");
    expect(JSON.stringify(fileRows[1])).toContain("hidden-badge");
  });
});
