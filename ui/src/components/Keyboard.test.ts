import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type MouseEvent, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { BrowserState, Settings } from "../api/types";

vi.mock("react", async (original) => ({
  ...await original<typeof import("react")>(),
  useState: <T,>(initial: T) => [typeof initial === "string" ? mocks.query : initial, vi.fn()],
  useMemo: <T,>(factory: () => T) => factory(),
  useRef: <T,>(current: T) => ({ current }),
  useEffect: () => {},
}));
const mocks = vi.hoisted(() => ({
  settings: {
    vimKeys: false, leaderKey: "Space", leaderKeyBasic: "Ctrl Ctrl",
    keyboardProfile: undefined as Settings["keyboardProfile"],
    shortcuts: {} as Record<string, string>, tabsShowFullPath: false,
  },
  sessionCmd: vi.fn(),
  focusBrowser: vi.fn(),
  query: "",
}));
vi.mock("../state/store", () => ({
  useStore: (select: (s: { settings: typeof mocks.settings }) => unknown) => select({ settings: mocks.settings }),
}));
vi.mock("../state/actions", () => ({ sessionCmd: mocks.sessionCmd, focusBrowser: mocks.focusBrowser }));
vi.mock("../state/dnd", () => ({
  draggedTab: vi.fn(), dropInto: vi.fn(), isFileDrag: vi.fn(),
  onDragOverTarget: vi.fn(), startTabDrag: vi.fn(),
}));
vi.mock("./Icon", () => ({ Icon: () => null }));

import { KeyHelp } from "./WhichKey";
import { TabBar } from "./TabBar";
import { ShortcutKeys, vimSteps } from "./ShortcutKeys";

type Props = {
  children?: unknown;
  className?: string;
  title?: string;
  onMouseDown?: (e: MouseEvent) => void;
  onMouseUp?: (e: MouseEvent) => void;
  onMouseLeave?: () => void;
  onAuxClick?: (e: MouseEvent) => void;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  return [node, ...elements(node.props.children)];
}
function event(button: number, altKey = false) {
  return { button, altKey, preventDefault: vi.fn(), stopPropagation: vi.fn() } as unknown as MouseEvent;
}
function tabBar() {
  const browser: BrowserState = {
    active: 1, closed: [], collapsed: false,
    tabs: [0, 1, 2].map((id) => ({
      id, path: `C:\\folder${id}`, alias: null, keepAlias: false,
      history: { back: [], forward: [] }, lastUsed: 0,
    })),
  };
  return elements(TabBar({ b: 1, browser }));
}

beforeEach(() => {
  mocks.settings.vimKeys = false;
  mocks.settings.keyboardProfile = undefined;
  mocks.settings.leaderKeyBasic = "Ctrl Ctrl";
  mocks.settings.shortcuts = {};
  mocks.query = "";
  vi.clearAllMocks();
});

describe("keyboard reference", () => {
  it("shows basic navigation, commander aliases and working help without vim", () => {
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("File list (standard)");
    expect(html).toContain("type a name");
    expect(html).toContain('aria-label="?');
    expect(html).toContain("Shift and F1 together");
    expect(html).toContain("F6");
    expect(html).toContain("Insert");
    expect(html).toContain("Preview window");
    expect(html).toContain('aria-label="Space"');
    expect(html).toContain("Caps Lock is ignored");
    expect(html).not.toContain("File list (vim)");
    expect(html).toContain("Middle-click");
  });

  it("shows vim pin keys and hides chords captured by the leader or vim", () => {
    mocks.settings.vimKeys = true;
    mocks.settings.shortcuts = { togglePin: "Ctrl+H" };
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("File list (vim)");
    expect(html).toContain("Space, then b, then p");
    expect(html).toContain('aria-label="i"');
    expect(html).not.toContain("Ctrl and H together");
    expect(html).not.toContain('aria-label="Space"');
    mocks.settings.vimKeys = false;
    mocks.settings.leaderKeyBasic = "\\";
    expect(renderToStaticMarkup(KeyHelp())).not.toContain('aria-label="\\, or');
  });

  it("honours disabled leaders and rebound preview shortcuts", () => {
    mocks.settings.leaderKeyBasic = "";
    mocks.settings.shortcuts = { preview: "Alt+V", keyHelp: "F9" };
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("Alt and V together");
    expect(html).toContain("<kbd>F9</kbd>");
    expect(html).not.toContain("Leader sequences");
    expect(html).not.toContain("Ctrl and / together");
  });

  it("keeps literal search syntax out of the keycap notation and searches chord spellings", () => {
    let html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain('<code class="kh-literal">ext:rs,toml</code>');
    expect(html).toContain("How to read shortcuts");
    mocks.query = "Ctrl+Shift+N";
    html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("New folder");
    expect(html).toContain("Ctrl and Shift and N together");
    expect(html).not.toContain("Parent folder");
  });

  it("shows a double Ctrl leader as two taps, not a held combination", () => {
    mocks.query = "Pin";
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("Ctrl, then Ctrl, then b, then p");
    expect(html).not.toContain("Ctrl and Ctrl together");
    expect(html).not.toContain("CtrlTap");
  });
  it("shows the active profile and only its actual function-key actions", () => {
    mocks.settings.keyboardProfile = "totalCommander";
    mocks.query = "Copy to other browser";
    let html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("Total Commander");
    expect(html).toContain('aria-label="F5, or Alt and C together"');
    mocks.settings.keyboardProfile = "explorer";
    html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("Windows Explorer");
    expect(html).not.toContain("F5, or Alt and C together");
    expect(html).toContain("Alt and C together");
  });

  it("documents the keys of find in files, Quick Open, drag & drop and the Git workspace", () => {
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain("Find in files");
    expect(html).toContain('aria-label="Tab, or Shift and Tab together"');
    expect(html).toContain("whole word");
    expect(html).toContain("name search here, including ignored files");
    expect(html).toContain('aria-label="Ctrl and Drag together"');
    expect(html).toContain("Git workspace");
    expect(html).toContain("stop recording");
  });

  it("explains an empty shortcut search while keeping the notation legend available", () => {
    mocks.query = "no-such-shortcut-xyz";
    const html = renderToStaticMarkup(KeyHelp());
    expect(html).toContain('role="status"');
    expect(html).toContain("No shortcuts match");
    expect(html).toContain("How to read shortcuts");
    expect(html).not.toContain('class="kh-row"');
  });
});

describe("shortcut notation", () => {
  it("distinguishes simultaneous chords, ordered steps and alternative bindings", () => {
    const html = renderToStaticMarkup(ShortcutKeys({ alternatives: [["Ctrl+Shift+C"], ["g", "g"]] }));
    expect(html).toContain('aria-label="Ctrl and Shift and C together, or g, then g"');
    expect(html.match(/class="key-plus"/g)).toHaveLength(2);
    expect(html.match(/class="key-then"/g)).toHaveLength(1);
    expect(html.match(/class="key-or"/g)).toHaveLength(1);
    expect(html).toContain('<kbd>g</kbd>');
  });

  it("makes uppercase vim keys and repeated leader taps explicit without splitting literal plus", () => {
    expect(vimSteps(["g", "T"])).toEqual(["g", "Shift+t"]);
    expect(vimSteps(["CtrlTap", "b", "p"], "Ctrl Ctrl")).toEqual(["Ctrl", "Ctrl", "b", "p"]);
    const html = renderToStaticMarkup(ShortcutKeys({ alternatives: [["Ctrl+w", "Shift+g"], ["+"]] }));
    expect(html).toContain("Ctrl and w together, then Shift and g together, or +");
    expect(html).toContain("<kbd>+</kbd>");
  });
});

describe("tab middle-click", () => {
  it("suppresses autoscroll on press and closes the clicked inactive tab on release once", () => {
    const tab = tabBar().find((node) => node.props.className?.startsWith("tab "))!;
    const down = event(1);
    tab.props.onMouseDown!(down);
    expect(down.preventDefault).toHaveBeenCalledOnce();
    expect(down.stopPropagation).toHaveBeenCalledOnce();
    expect(mocks.sessionCmd).not.toHaveBeenCalled();
    const up = event(1);
    tab.props.onMouseUp!(up);
    expect(up.preventDefault).toHaveBeenCalledOnce();
    expect(mocks.sessionCmd).toHaveBeenCalledExactlyOnceWith({ op: "close", b: 1, idx: 0 });
    tab.props.onAuxClick!(event(1));
    expect(mocks.sessionCmd).toHaveBeenCalledTimes(1);
    expect(mocks.focusBrowser).not.toHaveBeenCalled();
  });

  it("preserves Alt+middle transfer and ignores left and right auxiliary clicks", () => {
    const tabs = tabBar().filter((node) => node.props.className?.startsWith("tab "));
    tabs[2].props.onMouseDown!(event(1, true));
    tabs[2].props.onMouseUp!(event(1, true));
    tabs[2].props.onAuxClick!(event(1, true));
    expect(mocks.sessionCmd).toHaveBeenCalledExactlyOnceWith({ op: "transfer", b: 1, idx: 2, copy: false });
    mocks.sessionCmd.mockClear();
    for (const button of [0, 2]) {
      tabs[1].props.onMouseDown!(event(button));
      tabs[1].props.onMouseUp!(event(button));
      tabs[1].props.onAuxClick!(event(button));
    }
    expect(mocks.sessionCmd).not.toHaveBeenCalled();
  });

  it("does not close a tab when the middle press began elsewhere or left the tab", () => {
    const tabs = tabBar().filter((node) => node.props.className?.startsWith("tab "));
    tabs[0].props.onMouseUp!(event(1));
    tabs[0].props.onMouseDown!(event(1));
    tabs[1].props.onMouseUp!(event(1));
    tabs[0].props.onMouseDown!(event(1));
    tabs[0].props.onMouseLeave!();
    tabs[0].props.onMouseUp!(event(1));
    expect(mocks.sessionCmd).not.toHaveBeenCalled();
  });

  it("labels the close button with the current keyboard mode", () => {
    mocks.settings.vimKeys = true;
    expect(tabBar().find((node) => node.props.className?.startsWith("close "))!.props.title).toContain("(q)");
    mocks.settings.vimKeys = false;
    mocks.settings.shortcuts = { closeTab: "Alt+W" };
    expect(tabBar().find((node) => node.props.className?.startsWith("close "))!.props.title).toContain("(Alt+W)");
  });
});
