import { describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { AppInfo } from "../api/types";
import { AboutInfo } from "./AboutInfo";

const state = vi.hoisted(() => ({
  info: {} as AppInfo,
}));

vi.mock("../state/store", () => ({
  useStore: (selector: (s: { info: AppInfo }) => unknown) => selector({ info: state.info }),
}));
vi.mock("../state/actions", () => ({ openOverlay: vi.fn() }));
vi.mock("./FolderLink", () => ({
  FolderLink: ({ path }: { path: string }) => createElement("span", { className: "folder-link" }, path),
}));
vi.mock("./Icon", () => ({
  Icon: ({ name }: { name: string }) => createElement("svg", { "aria-hidden": "true", "data-icon": name }),
}));
vi.mock("./Tips", () => ({
  FeaturedTips: () => null,
  TipCard: () => null,
  TipsList: () => null,
}));
vi.mock("./TitleBar", () => ({ Logo: () => null }));
vi.mock("../../../docs/support/revtag.svg?url", () => ({ default: "revtag.svg" }));

describe("AboutInfo installation and update paths", () => {
  it("shows overlay update location and base installation status clearly", () => {
    state.info = {
      version: "0.4.0",
      platform: "linux",
      home: "/home/u",
      separator: "/",
      configDir: "/home/u/.config/gall",
      dataDir: "/home/u/.local/share/gall",
      templatesDir: "/home/u/.config/gall/templates",
      trashDir: null,
      firstRun: false,
      mediaBase: null,
      installDir: "/usr/bin",
      updatesDir: "/home/u/.local/share/gall/updates",
      updateStrategy: "overlay",
      installKind: "system-package",
      baseExePath: "/usr/bin/gall",
      baseVersion: "0.3.0",
      runningOverlay: true,
    };
    const html = renderToStaticMarkup(createElement(AboutInfo));
    expect(html).toContain("(updated over 0.3.0)");
    expect(html).toContain("/usr/bin");
    expect(html).toContain("/home/u/.local/share/gall/updates");
    expect(html).toContain("In-app updates install here (system install stays untouched)");
  });

  it("shows in-place update location for writable installs", () => {
    state.info = {
      version: "0.4.0",
      platform: "windows",
      home: "C:\\Users\\u",
      separator: "\\",
      configDir: "C:\\Users\\u\\AppData\\Roaming\\gall",
      dataDir: "C:\\Users\\u\\AppData\\Roaming\\gall",
      templatesDir: "C:\\Users\\u\\AppData\\Roaming\\gall\\templates",
      trashDir: null,
      firstRun: false,
      mediaBase: null,
      installDir: "D:\\Portable\\Gall",
      updatesDir: "D:\\Portable\\Gall\\UserData\\updates",
      updateStrategy: "inPlace",
      installKind: "portable",
      baseExePath: "D:\\Portable\\Gall\\gall.exe",
      baseVersion: "0.4.0",
      runningOverlay: false,
    };
    const html = renderToStaticMarkup(createElement(AboutInfo));
    expect(html).toContain("D:\\Portable\\Gall");
    expect(html).toContain("Updated in place");
    expect(html).not.toContain("updated over");
  });
});
