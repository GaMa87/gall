import { beforeEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Updates } from "./Updates";

const fixture = vi.hoisted(() => ({
  update: { phase: "idle" } as {
    phase: string;
    version?: string;
    notes?: string;
    reason?: string;
    error?: string;
    stage?: string;
    downloaded?: number;
    total?: number | null;
    strategy?: string;
    installDir?: string;
    updatesDir?: string;
    baseExePath?: string;
    baseVersion?: string;
    runningOverlay?: boolean;
  },
  tasks: {} as Record<string, unknown>,
  autoDownloadUpdates: true,
  info: {
    version: "1.2.3",
    installDir: "/usr/bin",
    updatesDir: "/home/u/.local/share/gall/updates",
    updateStrategy: "overlay",
    installKind: "system-package",
    baseExePath: "/usr/bin/gall",
    baseVersion: "1.0.0",
    runningOverlay: false,
  },
}));

vi.mock("../state/store", () => ({
  useStore: (selector: (state: unknown) => unknown) =>
    selector({
      info: fixture.info,
      tasks: fixture.tasks,
      settings: { autoDownloadUpdates: fixture.autoDownloadUpdates },
    }),
}));
vi.mock("../state/actions", () => ({
  updateSettings: vi.fn(),
}));
vi.mock("../state/updates", () => ({
  useUpdates: () => fixture.update,
  checkUpdates: vi.fn(),
  installUpdate: vi.fn(),
  applyUpdate: vi.fn(),
  postponeUpdate: vi.fn(),
  revertOverlayUpdate: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("./FolderLink", () => ({
  FolderLink: ({ path }: { path: string }) => createElement("span", { className: "folder-link" }, path),
}));
vi.mock("./Icon", () => ({
  Icon: ({ name }: { name: string }) => createElement("svg", { "aria-hidden": "true", "data-icon": name }),
}));

const render = () => renderToStaticMarkup(createElement(Updates));

describe("update card presentation", () => {
  beforeEach(() => {
    fixture.update = { phase: "idle" };
    fixture.tasks = {};
    fixture.autoDownloadUpdates = true;
    fixture.info = {
      version: "1.2.3",
      installDir: "/usr/bin",
      updatesDir: "/home/u/.local/share/gall/updates",
      updateStrategy: "overlay",
      installKind: "system-package",
      baseExePath: "/usr/bin/gall",
      baseVersion: "1.0.0",
      runningOverlay: false,
    };
  });

  it("shows the installed version, automatic download switch, and update storage folder", () => {
    const html = render();
    expect(html).toContain('class="updates-card"');
    expect(html).toContain("Installed version 1.2.3");
    expect(html).toContain('class="btn updates-check primary"');
    expect(html).toContain("Download updates automatically");
    expect(html).toContain('checked=""');
    expect(html).toContain("/home/u/.local/share/gall/updates");
    expect(html).toContain("/usr/bin/gall");
    expect(html).not.toContain('role="alert"');
  });

  it("announces checking and disables the action", () => {
    fixture.update = { phase: "checking" };
    const html = render();
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('role="status"');
    expect(html).toContain("Checking for updates…");
    expect(html).toMatch(/<button[^>]+disabled=""/);
  });

  it("confirms the latest stable version without an error", () => {
    fixture.update = { phase: "current" };
    const html = render();
    expect(html).toContain("Up to date");
    expect(html).toContain("latest stable version of Gall");
    expect(html).not.toContain('role="alert"');
  });

  it("offers a friendly error and preserves escaped technical evidence in closed details", () => {
    fixture.update = { phase: "error", error: "Network error <host>: no release JSON" };
    const html = render();
    expect(html).toContain("Couldn&#x27;t check for updates");
    expect(html).toContain('role="alert"');
    expect(html).toContain("Technical details");
    expect(html).toContain("Network error &lt;host&gt;: no release JSON");
    expect(html).not.toMatch(/<details[^>]*\sopen/);
  });

  it("keeps release notes expandable and install consent explicit when available", () => {
    fixture.update = { phase: "available", version: "2.0.0", notes: "Fixes\nNew features" };
    const html = render();
    expect(html).toContain("Gall 2.0.0 is available");
    expect(html).toContain('class="updates-notes"');
    expect(html).toContain("Fixes\nNew features");
    expect(html).toContain("Install update and restart");
    expect(html).toContain(">Later</button>");
  });

  it("presents determinate download progress and stage labels", () => {
    fixture.update = {
      phase: "downloading",
      version: "2.0.0",
      stage: "downloading",
      downloaded: 5 * 1024 * 1024,
      total: 10 * 1024 * 1024,
    };
    const html = render();
    expect(html).toContain('<progress aria-label="Preparing update" value="5242880" max="10485760"></progress>');
    expect(html).toContain("Downloading update… 5.0 MB / 10.0 MB (50%)");
  });

  it("renders Restart to update action when staged and ready", () => {
    fixture.update = { phase: "ready", version: "2.0.0", notes: "Ready notes" };
    const html = render();
    expect(html).toContain("Ready to restart");
    expect(html).toContain("Gall 2.0.0 is ready to install");
    expect(html).toContain("Restart to update");
    expect(html).toContain(">Later</button>");
  });

  it("shows store-managed explanation beside a download action", () => {
    fixture.update = { phase: "available", version: "2.0.0", reason: "Managed by Flatpak.", strategy: "storeManaged" };
    const html = render();
    expect(html).toContain("Managed by Flatpak.");
    expect(html).toContain("Download update");
    expect(html).not.toContain("Install update and restart");
  });

  it("shows only guidance for a distribution package", () => {
    fixture.info.installKind = "distro-package";
    fixture.info.updateStrategy = "storeManaged";
    fixture.update = { phase: "available", version: "2.0.0", reason: "Managed by pacman.", strategy: "storeManaged" };
    const html = render();
    fixture.info.installKind = "system-package";
    fixture.info.updateStrategy = "overlay";
    expect(html).toContain("omarchy update");
    expect(html).toContain("Managed by pacman.");
    expect(html).not.toContain("Download update");
    expect(html).not.toContain("Install update and restart");
    expect(html).not.toContain("Download updates automatically");
    expect(html).toContain("Check for updates");
  });

  it("disables applying updates while file operations are running", () => {
    fixture.tasks = { "1": {} };
    fixture.update = { phase: "ready", version: "2.0.0" };
    const html = render();
    expect(html).toMatch(/<button[^>]+disabled=""[^>]*>.*?Restart to update/);
    expect(html).toContain("Wait for file operations to finish");
  });

  it("shows Revert button when running as an overlay and supports in-place info", () => {
    fixture.info.runningOverlay = true;
    const htmlOverlay = render();
    expect(htmlOverlay).toContain("Installed version 1.2.3 (updated over 1.0.0)");
    expect(htmlOverlay).toContain("Revert to 1.0.0");

    fixture.info = {
      ...fixture.info,
      updateStrategy: "inPlace",
      installDir: "C:\\Tools\\Gall",
      runningOverlay: false,
    };
    const htmlInPlace = render();
    expect(htmlInPlace).toContain("In-place application folder");
    expect(htmlInPlace).toContain("C:\\Tools\\Gall");
    expect(htmlInPlace).not.toContain("Revert to");
  });
});
