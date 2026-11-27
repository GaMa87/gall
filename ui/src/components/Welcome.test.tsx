import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactElement } from "react";
import type { DefaultFileManagerStatus } from "../api/types";

const hooks = vi.hoisted(() => ({
  cursor: 0,
  values: [] as unknown[],
  effects: [] as (() => void)[],
}));

const storeState = vi.hoisted(() => ({
  settings: {
    theme: "dark",
    accent: "#3d8bfd",
    showNavColumns: true,
    showDetailsPane: true,
    keyboardProfile: "vim",
    defaultView: "details",
    defaultSort: "name",
    defaultDescending: false,
    group: "foldersFirst",
    rememberFolderViews: true,
    showHidden: false,
    singleClickOpen: false,
  } as Record<string, unknown>,
  info: { platform: "macos" } as Record<string, unknown>,
  session: { layout: "standard" } as Record<string, unknown>,
}));

const mocks = vi.hoisted(() => ({
  favoritesImportDetect: vi.fn(async () => [] as unknown[]),
  defaultFileManagerStatus: vi.fn<() => Promise<DefaultFileManagerStatus>>(),
  defaultFileManagerSet: vi.fn<() => Promise<DefaultFileManagerStatus>>(),
  defaultFileManagerRestore: vi.fn<() => Promise<DefaultFileManagerStatus>>(),
  updateSettings: vi.fn(),
  openOverlay: vi.fn(),
  setLayout: vi.fn(),
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
  useEffect: (fn: () => void | (() => void), deps?: unknown[]) => {
    const i = hooks.cursor++;
    const prev = hooks.values[i] as unknown[] | undefined;
    const changed = !prev || !deps || deps.length !== prev.length || deps.some((d, idx) => d !== prev[idx]);
    if (changed) {
      hooks.values[i] = deps ?? [];
      hooks.effects.push(() => { fn(); });
    }
  },
  useRef: <T,>(initial: T) => {
    const i = hooks.cursor++;
    if (!(i in hooks.values)) hooks.values[i] = { current: initial };
    return hooks.values[i] as { current: T };
  },
}));

vi.mock("../api/ipc", () => ({
  api: {
    favoritesImportDetect: mocks.favoritesImportDetect,
    defaultFileManagerStatus: mocks.defaultFileManagerStatus,
    defaultFileManagerSet: mocks.defaultFileManagerSet,
    defaultFileManagerRestore: mocks.defaultFileManagerRestore,
  },
}));
vi.mock("../i18n", () => ({
  tt: (text: string, params?: Record<string, string | number>) =>
    params ? text.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? `{${k}}`)) : text,
}));
vi.mock("../state/actions", () => ({
  updateSettings: mocks.updateSettings,
  openOverlay: mocks.openOverlay,
  setLayout: mocks.setLayout,
}));
vi.mock("../state/store", () => ({
  useStore: (selector: (s: typeof storeState) => unknown) => selector(storeState),
}));
vi.mock("./ThemedSelect", () => ({ ThemedSelect: ({ children }: { children?: unknown }) => children }));
vi.mock("./FavoritesImport", () => ({ FavoritesImportPanel: () => null }));
vi.mock("./FolderPane", () => ({ GROUP_LABEL: { foldersFirst: "Folders first" }, VIEWS: [{ id: "details", label: "Details" }] }));
vi.mock("./Icon", () => ({ Icon: () => null }));
vi.mock("./TitleBar", () => ({ Logo: () => null }));

import { Welcome } from "./Welcome";

type Props = {
  children?: unknown;
  className?: string;
  disabled?: boolean;
  onClick?: () => void;
  title?: string;
  "aria-pressed"?: boolean;
};

function expand(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(expand);
  if (!isValidElement<Props>(node)) return [];
  if (typeof node.type === "function") {
    const rendered = (node.type as (p: Props) => unknown)(node.props);
    return expand(rendered);
  }
  return [node, ...expand(node.props.children)];
}

function textOf(node: unknown): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (!isValidElement<Props>(node)) return "";
  if (typeof node.type === "function") return textOf((node.type as (p: Props) => unknown)(node.props));
  return textOf(node.props.children);
}

const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };

function renderWelcome() {
  hooks.cursor = 0;
  hooks.effects = [];
  const tree = Welcome();
  const pending = hooks.effects.splice(0);
  for (const fx of pending) fx();
  return expand(tree);
}

beforeEach(() => {
  hooks.cursor = 0;
  hooks.values = [];
  hooks.effects = [];
  storeState.info = { platform: "linux" };
  vi.clearAllMocks();
  mocks.favoritesImportDetect.mockResolvedValue([]);
  mocks.defaultFileManagerStatus.mockResolvedValue({
    platform: "linux",
    isDefault: false,
    currentHandler: "Dolphin",
    currentId: "org.kde.dolphin.desktop",
    canSet: true,
    unavailableReason: null,
    canRestore: false,
    restoreTarget: null,
  });
  mocks.defaultFileManagerSet.mockResolvedValue({
    platform: "linux",
    isDefault: true,
    currentHandler: "Gall",
    currentId: "gall.desktop",
    canSet: true,
    unavailableReason: null,
    canRestore: true,
    restoreTarget: "Dolphin",
  });
  mocks.defaultFileManagerRestore.mockResolvedValue({
    platform: "linux",
    isDefault: false,
    currentHandler: "Dolphin",
    currentId: "org.kde.dolphin.desktop",
    canSet: true,
    unavailableReason: null,
    canRestore: false,
    restoreTarget: null,
  });
});

describe("Welcome default file manager step", () => {
  it("shows the Default step and lets the user set and restore Gall", async () => {
    let nodes = renderWelcome();
    await flush();
    nodes = renderWelcome();

    const stepButtons = nodes.filter((el) => el.type === "button" && textOf(el).includes("Default"));
    expect(stepButtons.length).toBeGreaterThan(0);
    stepButtons[0].props.onClick?.();

    nodes = renderWelcome();
    const choices = nodes.filter((el) => el.type === "button" && el.props.className?.includes("wc-choice"));
    expect(choices).toHaveLength(2);
    expect(textOf(choices[0])).toContain("Keep Dolphin");
    expect(textOf(choices[1])).toContain("Set Gall as default");
    expect(choices[1].props.disabled).toBeFalsy();

    choices[1].props.onClick?.();
    await flush();
    expect(mocks.defaultFileManagerSet).toHaveBeenCalledTimes(1);

    nodes = renderWelcome();
    const afterSet = nodes.filter((el) => el.type === "button" && el.props.className?.includes("wc-choice"));
    expect(textOf(afterSet[0])).toContain("Restore Dolphin");
    expect(textOf(afterSet[1])).toContain("Gall is your default");

    afterSet[0].props.onClick?.();
    await flush();
    expect(mocks.defaultFileManagerRestore).toHaveBeenCalledTimes(1);
  });

  it("disables setting default when macOS app bundle is missing", async () => {
    storeState.info = { platform: "macos" };
    mocks.defaultFileManagerStatus.mockResolvedValue({
      platform: "macos",
      isDefault: false,
      currentHandler: "Finder",
      currentId: "com.apple.finder",
      canSet: false,
      unavailableReason: "macosAppBundleRequired",
      canRestore: false,
      restoreTarget: null,
    });

    let nodes = renderWelcome();
    await flush();
    nodes = renderWelcome();

    const defaultStep = nodes.find((el) => el.type === "button" && textOf(el).includes("Default"));
    defaultStep?.props.onClick?.();

    nodes = renderWelcome();
    const choices = nodes.filter((el) => el.type === "button" && el.props.className?.includes("wc-choice"));
    expect(choices).toHaveLength(2);
    expect(textOf(choices[0])).toContain("Keep Finder");
    expect(choices[1].props.disabled).toBe(true);
    expect(nodes.some((el) => textOf(el).includes("Gall.app"))).toBe(true);
  });
});
