import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type KeyboardEvent, type ReactElement } from "react";

const hooks = vi.hoisted(() => ({ cursor: 0, values: [] as unknown[], deps: new Map<number, readonly unknown[]>(), cleanups: new Map<number, () => void>(), pending: [] as (() => void)[] }));
vi.mock("react", async (original) => ({
  ...await original<typeof import("react")>(),
  useState: <T,>(initial: T) => {
    const i = hooks.cursor++;
    if (!(i in hooks.values)) hooks.values[i] = initial;
    return [hooks.values[i] as T, (value: T) => { hooks.values[i] = value; }];
  },
  useRef: <T,>(value: T) => { const i = hooks.cursor++; return hooks.values[i] ??= { current: value }; },
  useCallback: <T,>(callback: T, deps: readonly unknown[]) => {
    const i = hooks.cursor++, previous = hooks.deps.get(i);
    if (!previous || previous.some((v, j) => !Object.is(v, deps[j]))) { hooks.values[i] = callback; hooks.deps.set(i, deps); }
    return hooks.values[i];
  },
  useEffect: (effect: () => void | (() => void), deps: readonly unknown[]) => {
    const i = hooks.cursor++, previous = hooks.deps.get(i);
    if (previous && previous.length === deps.length && previous.every((v, j) => Object.is(v, deps[j]))) return;
    hooks.deps.set(i, deps);
    hooks.pending.push(() => { hooks.cleanups.get(i)?.(); const cleanup = effect(); if (cleanup) hooks.cleanups.set(i, cleanup); });
  },
}));
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), confirm: vi.fn(), navigate: vi.fn(), openOverlay: vi.fn(), create: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../state/store", () => ({ getState: () => ({ session: { activeBrowser: 1 } }) }));
vi.mock("../state/actions", () => ({ confirm: mocks.confirm, navigate: mocks.navigate, openOverlay: mocks.openOverlay }));
import { GitWorktreeList } from "./GitWorktreeList";

type Props = { children?: unknown; className?: string; title?: string; onKeyDown?: (e: KeyboardEvent) => void };
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
let path = "/repo-a", vim = false;
const callbacks = { onBusy: vi.fn(), onChanged: vi.fn(), onCreate: mocks.create };
function render() {
  hooks.cursor = 0;
  const tree = GitWorktreeList({ path, vim, reloadKey: 0, busy: false, ...callbacks });
  hooks.pending.splice(0).forEach((effect) => effect());
  return tree;
}
const rows = () => elements(render()).filter((n) => n.props.className?.includes("gg-worktree-main"));
const snapshot = (root: string) => ({ repository: root, token: root, worktrees: [{ path: root + "-linked", head: "abcd1234", branch: "feature", bare: false, locked: false, prunable: false }] });
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
function key(name: string) {
  const event = { key: name, ctrlKey: false, metaKey: false, altKey: false, preventDefault: vi.fn(), stopPropagation: vi.fn() };
  rows()[0].props.onKeyDown!(event as unknown as KeyboardEvent);
  return event;
}
beforeEach(() => {
  vi.clearAllMocks(); hooks.cursor = 0; hooks.values = []; hooks.deps.clear(); hooks.cleanups.clear(); hooks.pending = [];
  path = "/repo-a"; vim = false;
  mocks.invoke.mockImplementation((_command: string, args: { path: string }) => Promise.resolve(snapshot(args.path)));
  mocks.confirm.mockResolvedValue(false);
});
afterEach(() => hooks.cleanups.forEach((cleanup) => cleanup()));

describe("worktree repository and keyboard boundaries", () => {
  it("hides previous repository rows immediately and rejects its late result", async () => {
    const late = deferred<ReturnType<typeof snapshot>>();
    mocks.invoke.mockReturnValueOnce(late.promise);
    render();
    path = "/repo-b";
    expect(rows()).toHaveLength(0);
    await flush();
    expect(rows().map((row) => row.props.title)).toEqual(["/repo-b-linked"]);
    late.resolve(snapshot("/repo-a")); await flush();
    expect(rows().map((row) => row.props.title)).toEqual(["/repo-b-linked"]);
    key("Enter");
    expect(mocks.navigate).toHaveBeenCalledWith(1, "/repo-b-linked");
  });
  it("does not retain already loaded rows while another repository loads", async () => {
    render(); await flush(); expect(rows()).toHaveLength(1);
    const next = deferred<ReturnType<typeof snapshot>>(); mocks.invoke.mockReturnValueOnce(next.promise);
    path = "/repo-b"; expect(rows()).toHaveLength(0);
    next.resolve(snapshot(path)); await flush(); expect(rows()[0].props.title).toBe("/repo-b-linked");
  });
  it("ignores a late error from the previous repository", async () => {
    const late = deferred<ReturnType<typeof snapshot>>(); mocks.invoke.mockReturnValueOnce(late.promise);
    render(); path = "/repo-b"; render(); await flush();
    late.reject(new Error("old repository unavailable")); await flush();
    expect(JSON.stringify(render())).not.toContain("old repository unavailable");
    expect(rows()[0].props.title).toBe("/repo-b-linked");
  });
  it("leaves d and n inactive with Vim disabled and preserves Enter", async () => {
    render(); await flush();
    expect(key("d").preventDefault).not.toHaveBeenCalled(); key("n"); await flush();
    expect(mocks.confirm).not.toHaveBeenCalled(); expect(mocks.create).not.toHaveBeenCalled();
    expect(key("Enter").stopPropagation).toHaveBeenCalled();
    expect(mocks.navigate).toHaveBeenCalledWith(1, "/repo-a-linked");
  });
  it("allows Vim create and confirmed remove shortcuts without bubbling", async () => {
    vim = true; render(); await flush();
    expect(key("n").stopPropagation).toHaveBeenCalled(); expect(mocks.create).toHaveBeenCalledOnce();
    expect(key("d").stopPropagation).toHaveBeenCalled(); await flush();
    expect(mocks.confirm).toHaveBeenCalledWith("Remove worktree", expect.stringContaining("/repo-a-linked"), "Remove", true);
    expect(mocks.invoke).not.toHaveBeenCalledWith("git_advanced_action", expect.anything());
  });
});
