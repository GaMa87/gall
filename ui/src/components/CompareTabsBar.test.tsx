import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactElement } from "react";

const hooks = vi.hoisted(() => ({ cursor: 0, values: [] as unknown[] }));
const tabsState = vi.hoisted(() => ({
  value: {
    tabs: [] as { a: { kind: "file"; path: string }; b: { kind: "file"; path: string }; label: string }[],
    active: 0,
    earlier: [] as { a: { kind: "file"; path: string }; b: { kind: "file"; path: string }; label: string }[],
    earlierAll: [] as never[],
  },
}));
const mocks = vi.hoisted(() => ({
  openOverlay: vi.fn(),
  closeTabAt: vi.fn(),
  restoreEarlier: vi.fn(),
  restoreEarlierAt: vi.fn(),
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
  useEffect: () => {},
}));
vi.mock("../i18n", () => ({ tt: (text: string) => text }));
vi.mock("../state/actions", () => ({ openOverlay: mocks.openOverlay, notify: vi.fn(), fail: vi.fn() }));
vi.mock("../state/compareTabs", () => ({
  closeTabAt: mocks.closeTabAt,
  dismissEarlier: vi.fn(),
  registerPair: vi.fn(),
  relinkEarlier: vi.fn(),
  restoreEarlier: mocks.restoreEarlier,
  restoreEarlierAt: mocks.restoreEarlierAt,
  useCompareTabs: () => tabsState.value,
}));

import { CompareTabsBar } from "./CompareTabsBar";

type Props = {
  children?: unknown;
  role?: string;
  title?: string;
  className?: string;
  onClick?: (event?: { stopPropagation: () => void }) => void;
  onKeyDown?: (event: {
    key: string;
    target: { closest: (selector: string) => unknown };
    preventDefault: () => void;
    stopPropagation: () => void;
  }) => void;
};

function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}

const file = (path: string) => ({ kind: "file" as const, path });
const first = { a: file("one-left.txt"), b: file("one-right.txt"), label: "one" };
const second = { a: file("two-left.txt"), b: file("two-right.txt"), label: "two" };

function renderBar(beforeNavigate: () => Promise<boolean>) {
  hooks.cursor = 0;
  hooks.values = [];
  return elements(CompareTabsBar({ a: first.a, b: first.b, beforeNavigate }));
}

const flush = async () => { for (let i = 0; i < 4; i++) await Promise.resolve(); };

beforeEach(() => {
  hooks.cursor = 0;
  hooks.values = [];
  tabsState.value = { tabs: [first, second], active: 0, earlier: [], earlierAll: [] };
  vi.clearAllMocks();
  mocks.closeTabAt.mockReturnValue(second);
  mocks.restoreEarlier.mockReturnValue(second);
  mocks.restoreEarlierAt.mockReturnValue(second);
});

describe("comparison tab navigation guard", () => {
  it("asks before mouse and keyboard tab switching", async () => {
    const beforeNavigate = vi.fn(async () => false);
    let tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.role === "tab" && el.props.title === "two")?.props.onClick?.();
    await flush();
    expect(beforeNavigate).toHaveBeenCalledOnce();
    expect(mocks.openOverlay).not.toHaveBeenCalled();

    beforeNavigate.mockResolvedValue(true);
    tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.role === "tablist")?.props.onKeyDown?.({
      key: "ArrowRight",
      target: { closest: () => null },
      preventDefault: vi.fn(),
      stopPropagation: vi.fn(),
    });
    await flush();
    expect(beforeNavigate).toHaveBeenCalledTimes(2);
    expect(mocks.openOverlay).toHaveBeenCalledWith({
      type: "diff", a: second.a, b: second.b, back: undefined,
    });
  });

  it("does not close the active tab until navigation is confirmed", async () => {
    const beforeNavigate = vi.fn(async () => false);
    let tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.className === "cmp-tab-x")?.props.onClick?.({ stopPropagation: vi.fn() });
    await flush();
    expect(beforeNavigate).toHaveBeenCalledOnce();
    expect(mocks.closeTabAt).not.toHaveBeenCalled();

    beforeNavigate.mockResolvedValue(true);
    tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.role === "tablist")?.props.onKeyDown?.({
      key: "Delete",
      target: { closest: () => ({}) },
      preventDefault: vi.fn(),
      stopPropagation: vi.fn(),
    });
    await flush();
    expect(mocks.closeTabAt).toHaveBeenCalledWith(0);
    expect(mocks.openOverlay).toHaveBeenCalledWith({
      type: "diff", a: second.a, b: second.b, back: undefined,
    });
  });

  it("confirms before restoring earlier tabs", async () => {
    tabsState.value.earlier = [second];
    const beforeNavigate = vi.fn(async () => false);
    let tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.className === "cmp-tabs-restore")?.props.onClick?.();
    await flush();
    expect(mocks.restoreEarlier).not.toHaveBeenCalled();
    expect(mocks.openOverlay).not.toHaveBeenCalled();

    beforeNavigate.mockResolvedValue(true);
    tree = renderBar(beforeNavigate);
    tree.find((el) => el.props.className === "cmp-tabs-restore")?.props.onClick?.();
    await flush();
    expect(mocks.restoreEarlier).toHaveBeenCalledOnce();
    expect(mocks.openOverlay).toHaveBeenCalledWith({
      type: "diff", a: second.a, b: second.b, back: undefined,
    });
  });
});
