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
import { onFolderChanged } from "./controllers";
import type { FileEntry, Session } from "../api/types";

const entry = (path: string) => ({ path, name: path.split("/").pop() } as FileEntry);
const texts = () => getState().notifications.map((n) => `${n.kind}: ${n.text}`);

beforeEach(() => {
  calls.length = 0;
  setState({ notifications: [], notificationLog: [] });
});

describe("Windows shortcut activation", () => {
  const shortcut = { path: "C:\\links\\target.lnk", name: "target.lnk", ext: "lnk", isDir: false } as FileEntry;
  const session = { activeBrowser: 0, layout: "standard", browsers: [{ active: 0, tabs: [{ path: "C:\\links" }] }, { active: 0, tabs: [{ path: "C:\\" }] }] } as unknown as Session;

  it("navigates to a folder target inside Gall instead of launching Explorer", async () => {
    setState({ session });
    handler = (cmd) => cmd === "shortcut_resolve"
      ? { path: "C:\\Documents", name: "Documents", ext: "", isDir: true }
      : session;
    await A.openEntries(0, [shortcut]);
    expect(calls.find((c) => c.cmd === "session_cmd")?.args.cmd).toEqual({ op: "navigate", b: 0, path: "C:\\Documents" });
    expect(calls.some((c) => c.cmd === "open_path")).toBe(false);
  });

  it("uses the normal internal file flow for a note target", async () => {
    setState({ session });
    handler = (cmd) => {
      if (cmd === "shortcut_resolve") return { path: "C:\\Documents\\note.2do", name: "note.2do", ext: "2do", isDir: false };
      if (cmd === "preview_info") return { text: "My note" };
      return null;
    };
    await A.openEntries(0, [shortcut]);
    expect(getState().overlay).toMatchObject({ type: "note", path: "C:\\Documents\\note.2do", text: "My note" });
    expect(calls.some((c) => c.cmd === "open_path")).toBe(false);
  });

  it("passes document targets to the normal opener and preserves program launchers", async () => {
    for (const target of [{ path: "C:\\Documents\\file.txt", name: "file.txt", ext: "txt", isDir: false }, null]) {
      calls.length = 0;
      handler = (cmd) => cmd === "shortcut_resolve" ? target : null;
      await A.openEntries(0, [shortcut]);
      expect(calls.find((c) => c.cmd === "open_path")?.args.path).toBe(target?.path ?? shortcut.path);
    }
  });

  it("reports broken targets and still opens independent selected files", async () => {
    handler = (cmd) => {
      if (cmd === "shortcut_resolve") throw "shortcut target does not exist";
      return null;
    };
    await A.openEntries(0, [shortcut, { path: "C:\\file.txt", ext: "txt", name: "file.txt", isDir: false } as FileEntry]);
    expect(texts()).toEqual(["error: shortcut target does not exist"]);
    expect(calls.filter((c) => c.cmd === "open_path").map((c) => c.args.path)).toEqual(["C:\\file.txt"]);
  });
});

describe("Trash file lists", () => {
  it.each([["success", false], ["partial failure", true]] as const)(
    "refreshes displayed entries after %s without a watcher event",
    async (_, partialFailure) => {
      let disk = ["/x/a.txt", "/x/b.txt"];
      let displayed = [...disk];
      const unsubscribe = onFolderChanged("/x", () => { displayed = [...disk]; });
      handler = (cmd) => {
        if (cmd === "trash") {
          disk = partialFailure ? ["/x/b.txt"] : [];
          if (partialFailure) throw "access denied";
        }
        return null;
      };
      try {
        await A.trashPaths(["/x/a.txt", "/x/b.txt"]);
        expect(displayed).toEqual(partialFailure ? ["/x/b.txt"] : []);
        expect(getState().notifications.at(-1)?.kind).toBe(partialFailure ? "error" : "info");
      } finally { unsubscribe(); }
    },
  );
});

describe("Copy path", () => {
  it("says it failed, not “Copied path”, when the clipboard refuses", async () => {
    handler = (cmd, args) => {
      if (cmd === "path_format") return args.path;
      if (cmd === "clip_set_text") throw "clipboard unavailable";
      return null;
    };
    await A.copyPaths([entry("/a/b.txt")], "/a");
    expect(texts()).toEqual(["error: clipboard unavailable"]);
  });

  it("reports a failed path format instead of an unhandled rejection", async () => {
    handler = (cmd) => {
      if (cmd === "path_format") throw "bad path";
      return null;
    };
    await expect(A.copyPaths([entry("/a/b.txt")], "/a")).resolves.toBeUndefined();
    expect(texts()).toEqual(["error: bad path"]);
  });
});

describe("New tab from clipboard path", () => {
  const session = { activeBrowser: 0, layout: "standard", browsers: [{ active: 0, tabs: [{ path: "/home" }] }, { active: 0, tabs: [{ path: "/home" }] }] } as unknown as Session;

  it("does not open a tab for a path that does not exist", async () => {
    setState({ session });
    handler = (cmd) => {
      if (cmd === "clip_inspect") return { kind: "paths", paths: ["/gone/x.txt"] };
      if (cmd === "path_resolve") return { path: "/gone/x.txt", isDir: false, exists: false };
      return session;
    };
    await A.tabFromClipboard(0);
    expect(calls.some((c) => c.cmd === "session_cmd")).toBe(false);
    expect(texts()[0]).toMatch(/^warn: .*does not exist/);
  });

  it("reports a failed lookup instead of an unhandled rejection", async () => {
    handler = (cmd) => {
      if (cmd === "clip_inspect") return { kind: "paths", paths: ["/x"] };
      if (cmd === "path_resolve") throw "boom";
      return session;
    };
    await expect(A.tabFromClipboard(0)).resolves.toBeUndefined();
    expect(texts()).toEqual(["error: boom"]);
  });
});

describe("Go to a typed path (path editor)", () => {
  it("reports a failed lookup instead of an unhandled rejection", async () => {
    handler = (cmd) => {
      if (cmd === "path_resolve") throw "invalid path";
      return null;
    };
    await expect(A.goToTyped(0, "\0bad")).resolves.toBeUndefined();
    expect(texts()).toEqual(["error: invalid path"]);
  });
});

describe("Paste twice after Cut", () => {
  const session = { activeBrowser: 0, layout: "standard", browsers: [{ active: 0, tabs: [{ path: "/y" }] }, { active: 0, tabs: [{ path: "/z" }] }] } as unknown as Session;
  it("the second paste neither copies the moved items again nor pastes their paths as a text file", async () => {
    setState({ session, clipboard: { paths: ["/x/a.txt"], cut: true } });
    let moved = false;
    handler = (cmd) => {
      if (cmd === "clip_inspect")
        // The system clipboard still holds the paths as text: while the move runs they
        // exist (paths), after it they are only text.
        return moved ? { kind: "text", text: "/x/a.txt" } : { kind: "paths", paths: ["/x/a.txt"] };
      if (cmd === "conflict_info") return { paths: [], folders: 0, filesWhenMerged: 0, sample: [] };
      if (cmd === "clip_paste_files") return 7;
      return null;
    };
    await A.paste(0);
    expect(calls.filter((c) => c.cmd === "clip_paste_files")).toHaveLength(1);
    for (const m of [false, true]) {
      moved = m;
      calls.length = 0;
      setState({ notifications: [] });
      await A.paste(1);
      const cmds = calls.map((c) => c.cmd);
      expect(cmds).not.toContain("start_task");
      expect(cmds).not.toContain("clip_paste_files");
      expect(cmds).not.toContain("clip_paste_as_file");
      expect(texts()[0]).toMatch(/already moved/);
    }
  });
});
