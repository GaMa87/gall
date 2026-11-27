import { beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateCheckResult } from "../api/types";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  notify: vi.fn(),
  openOverlay: vi.fn(),
  tasks: {} as Record<string, unknown>,
  autoDownloadUpdates: true,
  overlay: null as { type?: string } | null,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("./store", () => ({
  getState: () => ({
    tasks: mocks.tasks,
    settings: { autoDownloadUpdates: mocks.autoDownloadUpdates },
    overlay: mocks.overlay,
  }),
}));
vi.mock("./actions", () => ({
  notify: mocks.notify,
  openOverlay: mocks.openOverlay,
}));

function availableResult(overrides: Partial<UpdateCheckResult> = {}): UpdateCheckResult {
  return {
    available: true,
    version: "2.0.0",
    notes: "Changes",
    staged: false,
    strategy: "overlay",
    installKind: "system-package",
    target: "linux-x86_64-bin",
    reason: "",
    installDir: "/usr/bin",
    updatesDir: "/home/u/.local/share/gall/updates",
    baseExePath: "/usr/bin/gall",
    baseVersion: "1.0.0",
    runningOverlay: false,
    activeOverlayVersion: null,
    ...overrides,
  };
}

describe("application updates", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    mocks.tasks = {};
    mocks.autoDownloadUpdates = true;
    mocks.overlay = null;
    mocks.listen.mockResolvedValue(() => {});
  });

  it("checks only once while another request is pending", async () => {
    let finish!: (value: UpdateCheckResult) => void;
    mocks.invoke.mockImplementation((cmd: string) =>
      cmd === "check_update"
        ? new Promise<UpdateCheckResult>((resolve) => { finish = resolve; })
        : Promise.resolve(null),
    );
    const updates = await import("./updates");
    const first = updates.checkUpdates();
    await Promise.resolve();
    await updates.checkUpdates();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "check_update")).toHaveLength(1);
    finish(availableResult({ available: false, version: null, notes: null }));
    await first;
  });

  it("auto-downloads in background when enabled and notifies Restart to update once ready", async () => {
    mocks.autoDownloadUpdates = true;
    mocks.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "check_update") return availableResult();
      if (cmd === "download_update") return { version: "2.0.0" };
      if (cmd === "apply_update") return null;
      return null;
    });
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.invoke.mock.calls.map(([c]) => c)).toEqual(["check_update", "download_update"]);
    expect(mocks.notify).toHaveBeenCalledWith(
      "Gall 2.0.0 is ready to install.",
      "info",
      expect.objectContaining({ label: "Restart to update" }),
    );
    const action = mocks.notify.mock.calls[0][2] as { run: () => void };
    action.run();
    await Promise.resolve();
    await Promise.resolve();
    expect(mocks.invoke.mock.calls.some(([c]) => c === "apply_update")).toBe(true);
  });

  it("notifies without downloading when auto-download is disabled", async () => {
    mocks.autoDownloadUpdates = false;
    mocks.invoke.mockResolvedValue(availableResult());
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.invoke.mock.calls.map(([c]) => c)).toEqual(["check_update"]);
    expect(mocks.notify).toHaveBeenCalledWith(
      "Gall 2.0.0 is available.",
      "info",
      expect.objectContaining({ label: "Update…" }),
    );
  });

  it("falls back cleanly to available notice if background auto-download fails", async () => {
    mocks.autoDownloadUpdates = true;
    mocks.invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "check_update") return availableResult();
      if (cmd === "download_update") throw new Error("Network reset");
      return null;
    });
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.notify).toHaveBeenCalledWith(
      "Gall 2.0.0 is available.",
      "info",
      expect.objectContaining({ label: "Update…" }),
    );
  });

  it("skips re-downloading when check_update reports the release is already staged", async () => {
    mocks.invoke.mockResolvedValue(availableResult({ staged: true }));
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.invoke.mock.calls.map(([c]) => c)).toEqual(["check_update"]);
    expect(mocks.notify).toHaveBeenCalledWith(
      "Gall 2.0.0 is ready to install.",
      "info",
      expect.objectContaining({ label: "Restart to update" }),
    );
  });

  it("does not apply or install while file operations are active", async () => {
    mocks.autoDownloadUpdates = false;
    mocks.invoke.mockResolvedValue(availableResult());
    const updates = await import("./updates");
    await updates.checkUpdates();
    mocks.tasks = { "1": {} };
    await updates.installUpdate();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "install_update")).toHaveLength(0);

    mocks.invoke.mockResolvedValue(availableResult({ staged: true }));
    await updates.checkUpdates();
    await updates.applyUpdate();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "apply_update")).toHaveLength(0);
  });

  it("reserves installation before awaiting backend work", async () => {
    mocks.autoDownloadUpdates = false;
    mocks.invoke.mockResolvedValueOnce(availableResult());
    const updates = await import("./updates");
    await updates.checkUpdates();
    let finish!: () => void;
    mocks.invoke.mockImplementation((cmd: string) =>
      cmd === "install_update" ? new Promise<void>((resolve) => { finish = resolve; }) : Promise.resolve(null),
    );
    const first = updates.installUpdate();
    await updates.installUpdate();
    await Promise.resolve();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "install_update")).toHaveLength(1);
    finish();
    await first;
  });

  it("allows retry after a failed installation", async () => {
    mocks.autoDownloadUpdates = false;
    mocks.invoke.mockResolvedValueOnce(availableResult());
    const updates = await import("./updates");
    await updates.checkUpdates();
    mocks.invoke.mockRejectedValueOnce(new Error("Invalid signature"));
    await updates.installUpdate();
    mocks.invoke.mockResolvedValueOnce(undefined);
    await updates.installUpdate();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "install_update")).toHaveLength(2);
  });

  it("keeps background connection failures quiet", async () => {
    mocks.invoke.mockRejectedValue(new Error("Offline"));
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.notify).not.toHaveBeenCalled();
  });

  it("does not download or install a store-managed distribution", async () => {
    mocks.invoke.mockResolvedValue(availableResult({
      strategy: "storeManaged",
      installKind: "flatpak",
      reason: "Managed by Flatpak.",
    }));
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    await updates.installUpdate();
    expect(mocks.invoke.mock.calls.filter(([c]) => c === "download_update" || c === "install_update")).toHaveLength(0);
  });

  it("reads backend startup update notices and falls back to legacy localStorage", async () => {
    mocks.invoke.mockResolvedValueOnce({ kind: "rolledBack", failedVersion: "0.4.0", restoredVersion: "0.3.0", reason: "crash" });
    const updates = await import("./updates");
    expect(await updates.takeStartupUpdateNotice()).toEqual({
      kind: "rolledBack",
      failedVersion: "0.4.0",
      restoredVersion: "0.3.0",
      reason: "crash",
    });

    const storage = new Map<string, string>([["gall.updatedTo", "0.3.0"]]);
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => storage.get(k) ?? null,
      setItem: (k: string, v: string) => storage.set(k, v),
      removeItem: (k: string) => storage.delete(k),
    });
    mocks.invoke.mockResolvedValueOnce(null);
    expect(await updates.takeStartupUpdateNotice()).toEqual({
      kind: "updated",
      version: "0.3.0",
      fromVersion: null,
    });
    expect(storage.has("gall.updatedTo")).toBe(false);
    vi.unstubAllGlobals();
  });

  it("notifies when applyUpdate from a toast fails due to active tasks outside Settings", async () => {
    mocks.overlay = null;
    mocks.invoke.mockResolvedValue(availableResult({ staged: true }));
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    mocks.notify.mockClear();
    mocks.tasks = { "1": {} };
    await updates.applyUpdate();
    expect(mocks.notify).toHaveBeenCalledWith(
      "Wait for file operations to finish before installing an update.",
      "error",
    );
  });
});
