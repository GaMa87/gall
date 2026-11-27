import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactElement } from "react";

const hooks = vi.hoisted(() => ({ cursor: 0, values: [] as unknown[], opened: false }));
const tabState = vi.hoisted(() => ({
  next: null as { a: { kind: "file"; path: string }; b: { kind: "file"; path: string } } | null,
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
  useMemo: <T,>(fn: () => T) => fn(),
  useCallback: <T,>(fn: T) => fn,
  useEffect: () => {},
  useLayoutEffect: (fn: () => void) => {
    if (!hooks.opened) { hooks.opened = true; fn(); }
  },
}));

const mocks = vi.hoisted(() => ({
  open: vi.fn(), small: vi.fn(), hunks: vi.fn(), lines: vi.fn(), save: vi.fn(),
  confirm: vi.fn(), openOverlay: vi.fn(), notify: vi.fn(),
}));
vi.mock("../api/ipc", () => ({ api: {
  diffPageOpen: mocks.open, diffPageSmall: mocks.small, diffPageHunks: mocks.hunks,
  diffPageLines: mocks.lines, saveTextWithUndo: mocks.save,
} }));
vi.mock("../i18n", () => ({ tt: (text: string) => text }));
vi.mock("../state/store", () => ({ useStore: (select: (s: object) => unknown) => select({ settings: {} }) }));
vi.mock("../state/compareTabs", () => ({
  useCompareTabs: () => ({ tabs: [] }), findTab: () => undefined,
  getCompareTabsState: () => ({ tabs: [] }), neighbourTab: () => tabState.next, updateTabState: vi.fn(),
}));
vi.mock("../state/launch", () => ({ useCompareWaitClose: () => {}, finishCompareWait: vi.fn() }));
vi.mock("../state/gitBlame", () => ({ useBlame: () => null, blameShort: vi.fn(), blameTitle: vi.fn() }));
vi.mock("../state/actions", () => ({
  confirm: mocks.confirm, notify: mocks.notify, openOverlay: mocks.openOverlay,
  fail: vi.fn(), refreshUndo: vi.fn(), invalidateAllFolderSizes: vi.fn(),
}));
vi.mock("../state/controllers", () => ({ refreshAll: vi.fn() }));

import { DiffView } from "./DiffView";

type Props = {
  children?: unknown;
  className?: string;
  title?: string;
  checked?: boolean;
  disabled?: boolean;
  onChange?: (e: { target: { checked: boolean } }) => void;
  onKeyDown?: (e: Record<string, unknown>) => void;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
const file = (path: string) => ({ kind: "file" as const, path });
function render() {
  hooks.cursor = 0;
  return elements(DiffView({ a: file("left.txt"), b: file("right.txt") }));
}
function controls() {
  const tree = render();
  return {
    permission: tree.find((el) => el.type === "input" && el.props.checked !== undefined)!,
    root: tree.find((el) => el.props.className === "diffv")!,
    save: tree.find((el) => el.type === "button" && el.props.title === "Save the changed file(s) (Ctrl+S)"),
    dirty: tree.some((el) => el.props.className === "dv-dirty"),
    copy: tree.filter((el) => el.type === "button" && el.props.title?.startsWith("Copy this change")),
  };
}
function key(root: ReactElement<Props>, k: string, modifiers: Record<string, boolean>) {
  root.props.onKeyDown!({ key: k, target: { tagName: "DIV" }, stopPropagation: () => {}, preventDefault: () => {}, ...modifiers });
}
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

beforeEach(() => {
  hooks.cursor = 0; hooks.values = []; hooks.opened = false;
  tabState.next = null;
  vi.stubGlobal("localStorage", { getItem: () => null, setItem: () => {} });
  vi.clearAllMocks();
  mocks.confirm.mockResolvedValue(false);
  const side = { label: "file", path: null, size: 2, crlf: false, finalNewline: true };
  const shape = { lines: 1, maxLineColumns: 1, crlf: false, finalNewline: true };
  mocks.open.mockResolvedValue({ left: side, right: side, binary: null, limit: null,
    text: { left: shape, right: shape, added: 1, removed: 1, ratio: 0, onlyCrlfDiffers: false } });
  mocks.small.mockResolvedValue({ leftLines: ["a"], rightLines: ["b"],
    ops: [{ tag: "replace", a: 0, aLen: 1, b: 0, bLen: 1 }], pairs: [], added: 1, removed: 1,
    ratio: 0, moves: [], leftMarks: {}, rightMarks: {} });
  mocks.save.mockResolvedValue({ undoable: true });
});

describe("diff edit permission", () => {
  it("blocks copying by default and saving after permission is revoked", async () => {
    render(); await flush();
    let c = controls();
    expect(c.permission.props.checked).toBe(false);
    expect(c.copy).toHaveLength(0);
    key(c.root, "ArrowRight", { altKey: true }); await flush();
    expect(controls().dirty).toBe(false);

    c.permission.props.onChange!({ target: { checked: true } });
    c = controls();
    expect(c.copy).toHaveLength(2);
    key(c.root, "ArrowRight", { altKey: true }); await flush();
    c = controls();
    expect(c.dirty).toBe(true);

    c.permission.props.onChange!({ target: { checked: false } });
    c = controls();
    expect(c.save?.props.disabled).toBe(true);
    key(c.root, "s", { ctrlKey: true }); await flush();
    expect(mocks.save).not.toHaveBeenCalled();
    expect(controls().dirty).toBe(true);

    controls().permission.props.onChange!({ target: { checked: true } });
    key(controls().root, "s", { ctrlKey: true }); await flush();
    expect(mocks.save).toHaveBeenCalledOnce();
    expect(controls().dirty).toBe(false);
  });

  it("cancels a paged copy when permission is revoked and re-enabled", async () => {
    mocks.small.mockResolvedValue(null);
    mocks.hunks.mockResolvedValue({ items: [{ tag: "replace", a: 0, aLen: 1, b: 0, bLen: 1 }], next: null });
    const releases: ((value: object) => void)[] = [];
    mocks.lines.mockImplementation(() => new Promise((resolve) => { releases.push(resolve); }));
    render(); await flush();
    let c = controls();
    c.permission.props.onChange!({ target: { checked: true } });
    c = controls();
    key(c.root, "ArrowRight", { altKey: true });
    c.permission.props.onChange!({ target: { checked: false } });
    controls().permission.props.onChange!({ target: { checked: true } });
    expect(releases.length).toBeGreaterThan(0);
    for (const release of releases) release({ items: ["a"], next: null });
    await flush();
    expect(controls().dirty).toBe(false);
  });

  it("locks the permission and prevents duplicate saves while saving", async () => {
    let finish!: (value: object) => void;
    mocks.save.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    render(); await flush();
    controls().permission.props.onChange!({ target: { checked: true } });
    key(controls().root, "ArrowRight", { altKey: true }); await flush();
    key(controls().root, "s", { ctrlKey: true });
    const c = controls();
    expect(c.permission.props.disabled).toBe(true);
    expect(c.save?.props.disabled).toBe(true);
    key(c.root, "s", { ctrlKey: true });
    expect(mocks.save).toHaveBeenCalledOnce();
    finish({ undoable: true }); await flush();
    expect(controls().permission.props.disabled).toBe(false);
  });

  it("asks before switching tabs and keeps copied edits when the user cancels", async () => {
    tabState.next = { a: file("next-left.txt"), b: file("next-right.txt") };
    render(); await flush();
    controls().permission.props.onChange!({ target: { checked: true } });
    key(controls().root, "ArrowRight", { altKey: true }); await flush();
    expect(controls().dirty).toBe(true);

    key(controls().root, "Tab", { ctrlKey: true }); await flush();
    expect(mocks.confirm).toHaveBeenCalledOnce();
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    expect(controls().dirty).toBe(true);
    expect(mocks.save).not.toHaveBeenCalled();

    mocks.confirm.mockResolvedValue(true);
    key(controls().root, "Tab", { ctrlKey: true }); await flush();
    expect(mocks.openOverlay).toHaveBeenCalledWith({
      type: "diff", a: file("next-left.txt"), b: file("next-right.txt"), back: undefined,
    });
    expect(mocks.save).not.toHaveBeenCalled();
  });

  it("blocks tab switching until an in-flight paged copy finishes", async () => {
    tabState.next = { a: file("next-left.txt"), b: file("next-right.txt") };
    mocks.small.mockResolvedValue(null);
    mocks.hunks.mockResolvedValue({ items: [{ tag: "replace", a: 0, aLen: 1, b: 0, bLen: 1 }], next: null });
    const releases: ((value: { items: string[]; next: number | null }) => void)[] = [];
    mocks.lines.mockImplementation(() => new Promise((resolve) => { releases.push(resolve); }));
    render(); await flush();
    controls().permission.props.onChange!({ target: { checked: true } });
    key(controls().root, "ArrowRight", { altKey: true });

    key(controls().root, "Tab", { ctrlKey: true }); await flush();
    expect(mocks.confirm).not.toHaveBeenCalled();
    expect(mocks.notify).toHaveBeenCalledWith("Wait for the comparison to finish before switching tabs or closing it.", "info");
    expect(mocks.openOverlay).not.toHaveBeenCalled();

    for (const release of releases) release({ items: ["copied"], next: null });
    await flush();
    expect(controls().dirty).toBe(true);
    key(controls().root, "Tab", { ctrlKey: true }); await flush();
    expect(mocks.confirm).toHaveBeenCalledOnce();
    expect(mocks.openOverlay).not.toHaveBeenCalled();
    expect(mocks.save).not.toHaveBeenCalled();
  });
});
