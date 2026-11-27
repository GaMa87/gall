import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CliArgs } from "../api/types";

const { compare, compareFolders, finishCompareWait, open, openOverlay, pathResolve } = vi.hoisted(() => ({
  compare: vi.fn(),
  compareFolders: vi.fn(),
  finishCompareWait: vi.fn(),
  open: vi.fn(),
  openOverlay: vi.fn(),
  pathResolve: vi.fn(),
}));

vi.mock("../api/ipc", () => ({
  api: { finishCompareWait, pathResolve, uiLog: vi.fn().mockResolvedValue(undefined) },
  events: {},
}));
vi.mock("../i18n", () => ({ tt: (text: string) => text }));
vi.mock("./actions", () => ({ compare, compareFolders, openOverlay }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));

function args(overrides: Partial<CliArgs>): CliArgs {
  return {
    path1: null, path2: null, paths: [], selectPaths: [], newTab: false, newWindow: false, noWindow: false,
    daemon: false, compare: null, wait: false, merge: null, comparePicker: false, compareSeed: null, gitRepo: null,
    selftest: false, selftestMode: "", pick: null, ...overrides,
  };
}

describe("comparison launches", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    pathResolve.mockResolvedValue({ exists: true, isDir: false });
    finishCompareWait.mockResolvedValue(undefined);
  });

  it("passes all caller-supplied merge paths to the handoff, preserving whitespace", async () => {
    const { applyLaunch } = await import("./launch");
    await applyLaunch(args({ wait: true, merge: { base: null, local: "/tmp/local ", remote: "/tmp/-remote", output: "/tmp/output file" } }));
    expect(openOverlay).toHaveBeenCalledWith({
      type: "standaloneMerge", base: undefined, left: "/tmp/local ", right: "/tmp/-remote", waitOutput: "/tmp/output file",
    });
    expect(finishCompareWait).not.toHaveBeenCalled();
  });

  it("opens a waiting folder comparison without resolving the caller early", async () => {
    const { applyLaunch } = await import("./launch");
    pathResolve.mockResolvedValue({ exists: true, isDir: true });
    await applyLaunch(args({ wait: true, compare: ["/tmp/left folder", "/tmp/right folder"] }));
    expect(compareFolders).toHaveBeenCalledWith("/tmp/left folder", "/tmp/right folder", true);
    expect(finishCompareWait).not.toHaveBeenCalled();
  });

  it("ends an invalid waiting comparison with an error", async () => {
    const { applyLaunch } = await import("./launch");
    pathResolve.mockResolvedValueOnce({ exists: false, isDir: false });
    await applyLaunch(args({ wait: true, compare: ["/tmp/missing", "/tmp/present"] }));
    expect(compare).not.toHaveBeenCalled();
    expect(finishCompareWait).toHaveBeenCalledWith("error", undefined, expect.stringContaining("/tmp/missing"));
  });

  it("asks for just the other file when the shell supplied the first", async () => {
    const { applyLaunch } = await import("./launch");
    open.mockResolvedValueOnce("/tmp/New folder/-second");
    await applyLaunch(args({ comparePicker: true, compareSeed: "/tmp/New folder/first" }));
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "/tmp/New folder" }));
    expect(compare).toHaveBeenCalledWith({ kind: "file", path: "/tmp/New folder/first" }, { kind: "file", path: "/tmp/New folder/-second" }, false);
  });

  it("closes the picker flow without opening a comparison when a selection is cancelled", async () => {
    const { applyLaunch } = await import("./launch");
    open.mockResolvedValueOnce("/tmp/first").mockResolvedValueOnce(null);
    await applyLaunch(args({ comparePicker: true }));
    expect(open).toHaveBeenCalledTimes(2);
    expect(compare).not.toHaveBeenCalled();
    expect(finishCompareWait).not.toHaveBeenCalled();
  });
});
