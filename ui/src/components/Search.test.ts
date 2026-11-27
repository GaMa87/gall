import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type KeyboardEvent, type ReactElement } from "react";
import type { SearchHit } from "../api/types";

const hooks = vi.hoisted(() => ({
  cursor: 0,
  values: [] as unknown[],
  deps: new Map<number, readonly unknown[]>(),
  cleanups: new Map<number, () => void>(),
  pending: [] as (() => void)[],
}));
vi.mock("react", async (original) => ({
  ...await original<typeof import("react")>(),
  memo: <T,>(c: T) => c,
  useCallback: <T,>(fn: T) => fn,
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
  useEffect: (effect: () => void | (() => void), deps?: readonly unknown[]) => {
    const i = hooks.cursor++;
    const prev = hooks.deps.get(i);
    if (deps && prev && prev.length === deps.length && prev.every((v, j) => Object.is(v, deps[j]))) return;
    if (deps) hooks.deps.set(i, deps);
    hooks.pending.push(() => {
      hooks.cleanups.get(i)?.();
      const cleanup = effect();
      if (cleanup) hooks.cleanups.set(i, cleanup);
    });
  },
}));

const mocks = vi.hoisted(() => ({
  searchStart: vi.fn(),
  searchCancel: vi.fn(),
  tagged: vi.fn(),
  onBatch: null as null | ((p: { id: number; entries: SearchHit[] }) => void),
  onDone: null as null | ((p: { id: number; hits: number; cancelled: boolean }) => void),
  onHidden: null as null | ((p: { id: number; count: number; capped: boolean }) => void),
  openOverlay: vi.fn(),
  fail: vi.fn(),
}));
vi.mock("../api/ipc", () => ({
  api: {
    searchStart: mocks.searchStart,
    searchCancel: mocks.searchCancel,
    tagged: mocks.tagged,
  },
  events: {
    searchBatch: (cb: NonNullable<typeof mocks.onBatch>) => { mocks.onBatch = cb; return Promise.resolve(() => {}); },
    searchDone: (cb: NonNullable<typeof mocks.onDone>) => { mocks.onDone = cb; return Promise.resolve(() => {}); },
    searchHidden: (cb: NonNullable<typeof mocks.onHidden>) => { mocks.onHidden = cb; return Promise.resolve(() => {}); },
  },
}));
vi.mock("../state/actions", () => ({
  activeBrowser: () => 0,
  openOverlay: mocks.openOverlay,
  navigate: vi.fn(),
  openEntries: vi.fn(),
  fail: mocks.fail,
}));
vi.mock("../state/controllers", () => ({ controller: () => null }));
vi.mock("./Icon", () => ({ FileIcon: () => null, Icon: () => null, TAG_COLORS: [] }));
vi.mock("./ThemedSelect", () => ({ ThemedSelect: ({ children }: { children?: unknown }) => children }));

import { SearchWindow } from "./Search";

type Props = {
  children?: unknown;
  className?: string;
  entry?: SearchHit;
  onKeyDown?: (e: KeyboardEvent) => void;
  onChange?: (e: { target: { value: string; checked?: boolean } }) => void;
  onClick?: () => void;
  type?: string;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
function render(props: { root?: string; initialPattern?: string; initialEverything?: boolean } = {}) {
  hooks.cursor = 0;
  const tree = SearchWindow({ root: props.root ?? "/home/u/proj", initialPattern: props.initialPattern, initialEverything: props.initialEverything });
  hooks.pending.splice(0).forEach((effect) => effect());
  return tree;
}
function key(k: string, extra = {}) {
  const e = { key: k, altKey: false, ctrlKey: false, metaKey: false, stopPropagation: vi.fn(), preventDefault: vi.fn(), ...extra };
  elements(render()).find((n) => n.props.className === "modal search-win")!.props.onKeyDown!(e as unknown as KeyboardEvent);
}
const hit = (name: string, path: string, hidden?: boolean): SearchHit => ({
  name, path, kind: "file", isDir: false, isHidden: false, isSymlink: false,
  size: 10, modified: null, created: null, ext: "ts", readonly: false,
  ...(hidden ? { hidden: true } : {}),
});

beforeEach(() => {
  vi.clearAllMocks();
  hooks.cursor = 0;
  hooks.values = [];
  hooks.deps.clear();
  hooks.cleanups.clear();
  hooks.pending = [];
  mocks.searchStart.mockResolvedValue(undefined);
  mocks.searchCancel.mockResolvedValue(undefined);
  mocks.tagged.mockResolvedValue([]);
});
afterEach(() => {
  hooks.cleanups.forEach((f) => f());
});

describe("F3 SearchWindow hidden-filter UX", () => {
  it("shows empty-state hidden hint for zero visible + N hidden, and omits it when nothing is hidden", () => {
    render({ initialPattern: "secret*" });
    const id = mocks.searchStart.mock.calls[0][0] as number;
    mocks.onDone!({ id, hits: 0, cancelled: false });

    // Nothing hidden -> plain "No results" with no Alt+I hint in the empty state.
    const plainEmpty = elements(render()).find((n) => n.props.className === "search-empty");
    expect(JSON.stringify(plainEmpty)).toContain("No results");
    expect(JSON.stringify(plainEmpty)).not.toContain("Alt+I");

    // Hidden matches arrive.
    mocks.onHidden!({ id, count: 2, capped: false });
    const hiddenEmpty = elements(render()).find((n) => n.props.className === "search-empty");
    expect(JSON.stringify(hiddenEmpty)).toContain("No results.");
    expect(JSON.stringify(hiddenEmpty)).toContain("2 hidden files match");
    expect(JSON.stringify(hiddenEmpty)).toContain("Alt+I");
  });

  it("shows subtle footer indicator when both visible and hidden results exist", () => {
    render({ initialPattern: "report*" });
    const id = mocks.searchStart.mock.calls[0][0] as number;
    mocks.onBatch!({ id, entries: [hit("report.ts", "/home/u/proj/src/report.ts")] });
    mocks.onDone!({ id, hits: 1, cancelled: false });
    mocks.onHidden!({ id, count: 3, capped: false });

    const bar = elements(render()).find((n) => n.props.className === "hidden-bar");
    expect(bar).toBeDefined();
    expect(JSON.stringify(bar)).toContain("+3 hidden by filters");
    expect(JSON.stringify(bar)).toContain("Alt+I");
  });

  it("re-runs search immediately on Alt+I, marks hidden entries, and ignores AltGr", () => {
    render({ initialPattern: "report*" });
    expect(mocks.searchStart).toHaveBeenCalledTimes(1);
    expect(mocks.searchStart.mock.calls[0][4]).toBe(false);

    // AltGr must not re-run.
    key("i", { altKey: true, ctrlKey: true });
    expect(mocks.searchStart).toHaveBeenCalledTimes(1);

    // Alt+I toggles everything ON and immediately re-runs with everything=true.
    key("i", { altKey: true });
    expect(mocks.searchStart).toHaveBeenCalledTimes(2);
    expect(mocks.searchStart.mock.calls[1][4]).toBe(true);
    const id2 = mocks.searchStart.mock.calls[1][0] as number;
    mocks.onBatch!({
      id: id2,
      entries: [
        hit("report.ts", "/home/u/proj/src/report.ts"),
        hit("report_gen.ts", "/home/u/proj/gen/report_gen.ts", true),
      ],
    });
    const rows = elements(render()).filter((n) => n.props.entry);
    expect(rows.map((r) => [r.props.entry!.name, !!r.props.entry!.hidden])).toEqual([
      ["report.ts", false],
      ["report_gen.ts", true],
    ]);

    // Second Alt+I toggles everything back OFF and immediately re-runs.
    key("i", { altKey: true });
    expect(mocks.searchStart).toHaveBeenCalledTimes(3);
    expect(mocks.searchStart.mock.calls[2][4]).toBe(false);
  });
});
