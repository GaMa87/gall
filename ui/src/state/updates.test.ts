import { beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), check: vi.fn(), notify: vi.fn(), relaunch: vi.fn(), tasks: {} as Record<string, unknown> }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("./store", () => ({ getState: () => ({ tasks: mocks.tasks }) }));
vi.mock("./actions", () => ({ notify: mocks.notify }));
describe("application updates", () => {
  beforeEach(() => { vi.resetModules(); vi.clearAllMocks(); mocks.tasks = {}; mocks.invoke.mockResolvedValue(""); });
  it("checks only once while another request is pending", async () => {
    let finish!: (value: null) => void;
    mocks.check.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const updates = await import("./updates");
    const first = updates.checkUpdates();
    await Promise.resolve();
    await updates.checkUpdates();
    expect(mocks.check).toHaveBeenCalledTimes(1);
    finish(null); await first;
  });
  it("announces a stable update after a background check", async () => {
    mocks.check.mockResolvedValue({ version: "2.0.0", body: "Changes", close: vi.fn() });
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.notify).toHaveBeenCalledWith("Gall 2.0.0 is available. Open Settings → Updates to update.", "info");
  });
  it("does not offer prereleases", async () => {
    const close = vi.fn();
    mocks.check.mockResolvedValue({ version: "2.0.0-beta.1", close });
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.notify).not.toHaveBeenCalled(); expect(close).toHaveBeenCalled();
  });
  it("does not download while file operations are active", async () => {
    const download = vi.fn();
    mocks.check.mockResolvedValue({ version: "2.0.0", download, close: vi.fn() });
    const updates = await import("./updates");
    await updates.checkUpdates(); mocks.tasks = { "1": {} };
    await updates.installUpdate(); expect(download).not.toHaveBeenCalled();
  });
  it("reserves installation before awaiting backend work", async () => {
    mocks.check.mockResolvedValue({ version: "2.0.0", close: vi.fn() });
    const updates = await import("./updates");
    await updates.checkUpdates();
    let finish!: () => void;
    mocks.invoke.mockImplementation((command: string) => command === "install_update" ? new Promise<void>((resolve) => { finish = resolve; }) : Promise.resolve(""));
    const first = updates.installUpdate();
    await updates.installUpdate();
    await Promise.resolve();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "install_update")).toHaveLength(1);
    finish(); await first;
  });
  it("allows retry after a failed installation", async () => {
    mocks.check.mockResolvedValue({ version: "2.0.0", close: vi.fn() });
    const updates = await import("./updates");
    await updates.checkUpdates();
    mocks.invoke.mockRejectedValue(new Error("Invalid signature"));
    await updates.installUpdate();
    mocks.invoke.mockResolvedValue(undefined);
    await updates.installUpdate();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "install_update")).toHaveLength(2);
  });
  it("keeps background connection failures quiet", async () => {
    mocks.check.mockRejectedValue(new Error("Offline"));
    const updates = await import("./updates");
    await updates.checkUpdates(false);
    expect(mocks.notify).not.toHaveBeenCalled();
  });
  it("does not install a manually managed distribution", async () => {
    mocks.invoke.mockResolvedValue("Use your package manager.");
    const download = vi.fn(); mocks.check.mockResolvedValue({ version: "2.0.0", download });
    const updates = await import("./updates");
    await updates.checkUpdates(); await updates.installUpdate(); expect(download).not.toHaveBeenCalled();
  });
});
