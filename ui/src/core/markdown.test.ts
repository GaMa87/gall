import { describe, expect, it } from "vitest";
import { isRunnable, resolveRel, slugify } from "./markdown";

describe("markdown helpers", () => {
  it("slugifies headings like GitHub", () => {
    expect(slugify("Getting Started")).toBe("getting-started");
    expect(slugify("What's new in v2.0?")).toBe("whats-new-in-v20");
    expect(slugify("Šumniki čšž")).toBe("šumniki-čšž");
  });

  it("resolves relative links against the file's folder", () => {
    expect(resolveRel("/home/u/proj", "docs/a.md")).toBe("/home/u/proj/docs/a.md");
    expect(resolveRel("/home/u/proj", "./img/x.png")).toBe("/home/u/proj/img/x.png");
    expect(resolveRel("/home/u/proj/docs", "../README.md#usage")).toBe("/home/u/proj/README.md");
    expect(resolveRel("C:\\Projects\\colonnade", "docs/my%20notes.md")).toBe("C:\\Projects\\colonnade\\docs\\my notes.md");
  });

  it("leaves URLs, anchors and repo-root links alone", () => {
    expect(resolveRel("/p", "https://example.com/a.png")).toBeNull();
    expect(resolveRel("/p", "mailto:a@b.c")).toBeNull();
    expect(resolveRel("/p", "#usage")).toBeNull();
    expect(resolveRel("/p", "/docs/a.md")).toBeNull();
    expect(resolveRel("C:\\p", "C:/x.png")).toBeNull();
  });

  it("flags links to files that run as programs", () => {
    expect(isRunnable("/repo/setup.exe")).toBe(true);
    expect(isRunnable("C:\\repo\\install.BAT")).toBe(true);
    expect(isRunnable("/repo/tools/build.sh")).toBe(true);
    expect(isRunnable("/repo/Gall.AppImage")).toBe(true);
    expect(isRunnable("/repo/docs/guide.md")).toBe(false);
    expect(isRunnable("/repo/photo.png")).toBe(false);
    expect(isRunnable("/repo/.bashrc")).toBe(false);
  });
});
