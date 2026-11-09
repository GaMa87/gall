import { beforeEach, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Updates } from "./Updates";

const fixture = vi.hoisted(() => ({
  update: { phase: "idle" } as { phase: string; version?: string; notes?: string; reason?: string; error?: string; bytes?: number },
  tasks: {} as Record<string, unknown>,
}));
vi.mock("../state/store", () => ({ useStore: (selector: (state: unknown) => unknown) => selector({ info: { version: "1.2.3" }, tasks: fixture.tasks }) }));
vi.mock("../state/updates", () => ({ useUpdates: () => fixture.update, checkUpdates: vi.fn(), installUpdate: vi.fn(), postponeUpdate: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("./Icon", () => ({ Icon: ({ name }: { name: string }) => createElement("svg", { "aria-hidden": "true", "data-icon": name }) }));

const render = () => renderToStaticMarkup(createElement(Updates));
describe("update card presentation", () => {
  beforeEach(() => { fixture.update = { phase: "idle" }; fixture.tasks = {}; });
  it("shows the installed version and an app-styled check action", () => {
    const html = render();
    expect(html).toContain('class="updates-card"');
    expect(html).toContain("Installed version 1.2.3");
    expect(html).toContain('class="btn updates-check primary"');
    expect(html).toContain("Keep Gall up to date");
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
  it("keeps release notes expandable and install consent explicit", () => {
    fixture.update = { phase: "available", version: "2.0.0", notes: "Fixes\nNew features" };
    const html = render();
    expect(html).toContain("Gall 2.0.0 is available");
    expect(html).toContain('class="updates-notes"');
    expect(html).toContain("Fixes\nNew features");
    expect(html).toContain("Install update and restart");
    expect(html).toContain(">Later</button>");
  });
  it("keeps the manual-distribution explanation beside a download action", () => {
    fixture.update = { phase: "available", version: "2.0.0", reason: "Use your package manager." };
    const html = render();
    expect(html).toContain("Use your package manager.");
    expect(html).toContain("Download update");
    expect(html).not.toContain("Install update and restart");
  });
  it("explains why installation is disabled while file operations are running", () => {
    fixture.tasks = { "1": {} };
    fixture.update = { phase: "available", version: "2.0.0" };
    const html = render();
    expect(html).toMatch(/<button[^>]+disabled=""[^>]*>.*?Install update and restart/);
    expect(html).toContain("Wait for file operations to finish");
  });
  it("presents indeterminate progress with downloaded bytes instead of a made-up percentage", () => {
    fixture.update = { phase: "installing", version: "2.0.0", bytes: 4096 };
    const html = render();
    expect(html).toContain('<progress aria-label="Installing update"></progress>');
    expect(html).toContain("4 KB downloaded");
    expect(html).toContain("Gall will restart when installation finishes");
  });
});
