import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: { cmd: string; args: Record<string, unknown> }[] = [];
let handler: (cmd: string, args: Record<string, unknown>) => unknown = () => null;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: Record<string, unknown>) => {
    calls.push({ cmd, args });
    try {
      return Promise.resolve(handler(cmd, args));
    } catch (e) {
      return Promise.reject(e);
    }
  },
  convertFileSrc: (p: string) => p,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.stubGlobal("window", { setTimeout, clearTimeout });

import * as A from "./actions";
import { getState, setState } from "./store";
import type { AppInfo, FileEntry, Session } from "../api/types";

const session = {
  activeBrowser: 0,
  layout: "standard",
  browsers: [
    { active: 0, tabs: [{ path: "/home/u/Documents" }] },
    { active: 0, tabs: [{ path: "/home/u" }] },
  ],
} as unknown as Session;

beforeEach(() => {
  calls.length = 0;
  handler = () => null;
  setState({
    session,
    info: { platform: "linux" } as AppInfo,
    overlay: null,
    menu: null,
    notifications: [],
    notificationLog: [],
  });
});

describe("Cross-platform file opening actions", () => {
  it("batches multiple selected files into a single open_paths call", async () => {
    const files = [
      { path: "/home/u/Documents/a.md", name: "a.md", ext: "md", isDir: false } as FileEntry,
      { path: "/home/u/Documents/b.md", name: "b.md", ext: "md", isDir: false } as FileEntry,
    ];
    await A.openEntries(0, files);
    expect(calls.filter((c) => c.cmd === "open_path")).toHaveLength(0);
    expect(calls.find((c) => c.cmd === "open_paths")?.args.paths).toEqual([
      "/home/u/Documents/a.md",
      "/home/u/Documents/b.md",
    ]);
  });

  it("opens the Open with palette automatically when no default application is registered", async () => {
    handler = (cmd) => {
      if (cmd === "open_path") throw "no default application is set to open '/home/u/file.xyz'; use Open with…";
      return null;
    };
    const file = { path: "/home/u/file.xyz", name: "file.xyz", ext: "xyz", isDir: false } as FileEntry;
    await A.openEntries(0, [file]);
    await new Promise((r) => setTimeout(r, 0));
    expect(getState().overlay).toEqual({ type: "openWith", paths: ["/home/u/file.xyz"], mode: "file" });
  });

  it("launches .app bundles on macOS instead of navigating inside", async () => {
    setState({ info: { platform: "macos" } as AppInfo });
    const app = { path: "/Applications/Zed.app", name: "Zed.app", ext: "app", isDir: true } as FileEntry;
    await A.openEntries(0, [app]);
    expect(calls.find((c) => c.cmd === "open_path")?.args.path).toBe("/Applications/Zed.app");
    expect(calls.some((c) => c.cmd === "session_cmd")).toBe(false);
  });

  it("navigates to a symlink target folder via openTargetLocation", async () => {
    handler = (cmd) => {
      if (cmd === "path_resolve") return { path: "/opt/docs/readme.md", isDir: false, exists: true };
      return session;
    };
    const link = { path: "/home/u/Documents/readme.md", name: "readme.md", ext: "md", isDir: false, isSymlink: true } as FileEntry;
    await A.openTargetLocation(0, link);
    expect(calls.find((c) => c.cmd === "session_cmd")?.args.cmd).toEqual({ op: "navigate", b: 0, path: "/opt/docs" });
  });

  it("updates only the matching menu token without clobbering newer menus", () => {
    const anchor = { x: 10, y: 20 };
    const t1 = A.showMenuAt(anchor, [{ label: "First" }]);
    A.updateOpenMenu(t1, (items) => [...items, { label: "Added" }]);
    expect(getState().menu?.items.map((i) => i.label)).toEqual(["First", "Added"]);
    const t2 = A.showMenuAt(anchor, [{ label: "Second" }]);
    A.updateOpenMenu(t1, () => [{ label: "Stale" }]);
    expect(getState().menu?.items.map((i) => i.label)).toEqual(["Second"]);
    A.updateOpenMenu(t2, (items) => [...items, { label: "Fresh" }]);
    expect(getState().menu?.items.map((i) => i.label)).toEqual(["Second", "Fresh"]);
  });
});
