import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactElement } from "react";

const hooks = vi.hoisted(() => ({
  cursor: 0,
  values: [] as unknown[],
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
  useEffect: () => {},
}));

const mocks = vi.hoisted(() => ({
  overlay: null as { type: string; [key: string]: unknown } | null,
  updateNote: vi.fn(),
  createNote: vi.fn(),
  openOverlay: vi.fn((overlay: typeof mocks.overlay) => { mocks.overlay = overlay; }),
  fail: vi.fn(),
  refreshUndo: vi.fn(),
}));
vi.mock("react-dom", async (original) => ({
  ...await original<typeof import("react-dom")>(),
  createPortal: (node: unknown) => node,
}));
vi.mock("../api/ipc", () => ({ api: { updateNote: mocks.updateNote, createNote: mocks.createNote } }));
vi.mock("../state/actions", () => ({
  openOverlay: mocks.openOverlay, fail: mocks.fail, refreshUndo: mocks.refreshUndo,
}));
vi.mock("../state/store", () => ({
  useStore: () => mocks.overlay,
  getState: () => ({ overlay: mocks.overlay }),
  setState: vi.fn(),
}));
vi.mock("../state/controllers", () => ({ controller: () => null }));
vi.mock("../state/hooks", () => ({ bumpNonce: vi.fn() }));
vi.mock("../state/keyLabels", () => ({ useKeyLabels: () => ({ action: () => null }) }));
vi.mock("./DiffView", () => ({ DiffView: () => null }));
vi.mock("./DirCompare", () => ({ DirCompare: () => null }));
vi.mock("./SimilarityMatrix", () => ({ SimilarityMatrix: () => null }));
vi.mock("./AboutInfo", () => ({ AboutInfo: () => null }));
vi.mock("./BatchRename", () => ({ BatchRename: () => null }));
vi.mock("./FolderLink", () => ({ FolderLink: () => null }));
vi.mock("./Welcome", () => ({ Welcome: () => null }));
vi.mock("./Icon", () => ({ FileIcon: () => null, Icon: () => null, TAG_COLORS: [] }));
vi.mock("./Preview", () => ({ Preview: () => null, usePreviewInfo: () => null }));
vi.mock("./Search", () => ({ SearchWindow: () => null }));
vi.mock("./Trash", () => ({ TrashWindow: () => null }));
vi.mock("./QuickOpen", () => ({ QuickOpen: () => null }));
vi.mock("./FindInFiles", () => ({ FindInFiles: () => null }));
vi.mock("./WhichKey", () => ({ KeyHelp: () => null }));
vi.mock("./Settings", () => ({ SettingsDialog: () => null }));
vi.mock("./OpenWith", () => ({ OpenWith: () => null }));

import { Overlays } from "./Overlays";

type Props = {
  children?: unknown;
  className?: string;
  disabled?: boolean;
  value?: string;
  onChange?: (e: { target: { value: string } }) => void;
  onClick?: () => void;
  onKeyDown?: (e: Record<string, unknown>) => void;
};
function elements(node: unknown): ReactElement<Props>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<Props>(node)) return [];
  if (typeof node.type === "function") return elements((node.type as (props: Props) => unknown)(node.props));
  return [node, ...elements(node.props.children)];
}
function render() {
  hooks.cursor = 0;
  return elements(Overlays());
}
function noteOverlay() {
  return { type: "note", dir: "C:\\project", path: "C:\\project\\todo.2do", text: "draft" };
}
function newNoteOverlay() {
  return { type: "note", dir: "C:\\project", text: "draft" };
}
function noteControls() {
  const tree = render();
  return {
    textarea: tree.find((el) => el.type === "textarea")!,
    save: tree.find((el) => el.type === "button" && el.props.className === "btn primary")!,
    modal: tree.find((el) => el.props.className === "modal")!,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
const flush = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };

beforeEach(() => {
  hooks.cursor = 0;
  hooks.values = [];
  mocks.overlay = noteOverlay();
  vi.clearAllMocks();
  mocks.openOverlay.mockImplementation((overlay) => { mocks.overlay = overlay; });
  mocks.updateNote.mockResolvedValue(undefined);
  mocks.createNote.mockResolvedValue("C:\\project\\draft.2do");
});

describe("note editor save", () => {
  it("keeps a failed draft open and permits retry", async () => {
    const failed = deferred<void>();
    mocks.updateNote.mockReturnValueOnce(failed.promise);
    noteControls().save.props.onClick!();
    failed.reject(new Error("read-only"));
    await flush();
    expect(mocks.overlay?.type).toBe("note");
    expect(mocks.fail).toHaveBeenCalledOnce();

    noteControls().save.props.onClick!();
    await flush();
    expect(mocks.updateNote).toHaveBeenCalledTimes(2);
    expect(mocks.openOverlay).toHaveBeenCalledWith(null);
    expect(mocks.overlay).toBeNull();
  });

  it("serializes repeated saves and preserves edits made during an async write", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    mocks.updateNote.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    noteControls().save.props.onClick!();
    noteControls().save.props.onClick!();
    expect(mocks.updateNote).toHaveBeenCalledTimes(1);

    noteControls().textarea.props.onChange!({ target: { value: "new draft" } });
    first.resolve();
    await flush();
    expect(mocks.overlay?.type).toBe("note");
    expect(mocks.openOverlay).not.toHaveBeenCalled();

    noteControls().save.props.onClick!();
    expect(mocks.updateNote).toHaveBeenLastCalledWith("C:\\project\\todo.2do", "new draft");
    second.resolve();
    await flush();
    expect(mocks.overlay).toBeNull();
  });

  it("updates a newly created note when its draft was edited during creation", async () => {
    const creation = deferred<string>();
    mocks.overlay = newNoteOverlay();
    mocks.createNote.mockReturnValueOnce(creation.promise);
    noteControls().save.props.onClick!();
    noteControls().textarea.props.onChange!({ target: { value: "edited draft" } });
    creation.resolve("C:\\project\\draft.2do");
    await flush();
    expect(mocks.overlay?.type).toBe("note");

    noteControls().save.props.onClick!();
    await flush();
    expect(mocks.createNote).toHaveBeenCalledOnce();
    expect(mocks.updateNote).toHaveBeenCalledWith("C:\\project\\draft.2do", "edited draft");
    expect(mocks.overlay).toBeNull();
  });

  it("does not close a different overlay when a pending save succeeds", async () => {
    const write = deferred<void>();
    mocks.updateNote.mockReturnValueOnce(write.promise);
    noteControls().save.props.onClick!();
    const next = { type: "settings" };
    mocks.overlay = next;
    write.resolve();
    await flush();
    expect(mocks.overlay).toBe(next);
    expect(mocks.openOverlay).not.toHaveBeenCalled();
  });

  it("ignores composing Escape and Ctrl+Enter", () => {
    const { modal } = noteControls();
    const key = (key: string, ctrlKey = false) => modal.props.onKeyDown!({
      key, ctrlKey, metaKey: false, altKey: false, shiftKey: false,
      nativeEvent: { isComposing: true, keyCode: 229 },
      stopPropagation: vi.fn(), preventDefault: vi.fn(),
    });
    key("Escape");
    key("Enter", true);
    expect(mocks.overlay?.type).toBe("note");
    expect(mocks.updateNote).not.toHaveBeenCalled();
    expect(mocks.openOverlay).not.toHaveBeenCalled();
  });
});
