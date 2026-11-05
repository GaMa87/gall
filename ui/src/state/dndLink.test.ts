import { describe, expect, it, vi } from "vitest";

const { symlink } = vi.hoisted(() => ({
  symlink: vi.fn((_t: string, _d: string) => Promise.reject("symlink failed for '/b/x': Permission denied (os error 13)")),
}));
vi.mock("../api/ipc", () => ({
  api: {
    modifiers: () => Promise.resolve({ known: true, ctrl: false, shift: false, alt: false }),
    symlink,
    uiLog: () => Promise.resolve(),
    undoPeek: () => Promise.resolve(null),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.stubGlobal("window", { setTimeout, clearTimeout, addEventListener: () => {} });

import { dropInto } from "./dnd";
import { getState, setState } from "./store";
import type { Settings } from "../api/types";

const dropEvent = (uris: string) => ({
  preventDefault() {}, stopPropagation() {}, ctrlKey: false, shiftKey: false, altKey: false, clientX: 1, clientY: 1,
  dataTransfer: { getData: (t: string) => (t === "text/uri-list" ? uris : ""), types: ["text/uri-list"], files: [] },
}) as unknown as React.DragEvent;

describe("drop menu (default drop action: ask)", () => {
  it("“Create symbolic link here” reports a failure instead of swallowing it", async () => {
    setState({ settings: { dragBehavior: "ask" } as Settings, drives: [], notifications: [] });
    dropInto(dropEvent("file:///a/x"), "/b");
    await vi.waitFor(() => expect(getState().menu).not.toBeNull());
    const item = getState().menu!.items.find((i) => i.label?.startsWith("Create symbolic link"))!;
    item.run!();
    await vi.waitFor(() => expect(symlink).toHaveBeenCalledWith("/a/x", "/b"));
    await vi.waitFor(() => expect(getState().notifications.map((n) => n.kind)).toContain("error"));
    expect(getState().notifications.find((n) => n.kind === "error")!.text).toBe("Cannot symlink “x”: you have no permission.");
  });
});
