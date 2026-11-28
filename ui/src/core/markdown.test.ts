import { describe, expect, it, vi } from "vitest";
import {
  MARKDOWN_FORBID_TAGS,
  MARKDOWN_SANITIZE_OPTIONS,
  activateMarkdownLink,
  classifyMarkdownLink,
  isRunnable,
  resolveRel,
  slugify,
} from "./markdown";

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
    expect(isRunnable("C:\\repo\\setup.exe.")).toBe(true);
    expect(isRunnable("C:\\repo\\install.bat ")).toBe(true);
    expect(isRunnable("/repo/tools/build.sh")).toBe(true);
    expect(isRunnable("/repo/Gall.AppImage")).toBe(true);
    expect(isRunnable("/repo/docs/guide.md")).toBe(false);
    expect(isRunnable("/repo/photo.png")).toBe(false);
    expect(isRunnable("/repo/.bashrc")).toBe(false);
  });

  it("forbids embedded media, image maps, non-HTML profiles, and interactive form/style tags in Markdown HTML", () => {
    expect(MARKDOWN_SANITIZE_OPTIONS.USE_PROFILES).toEqual({ html: true });
    expect(MARKDOWN_SANITIZE_OPTIONS.FORBID_ATTR).toContain("xlink:href");
    expect(MARKDOWN_SANITIZE_OPTIONS.FORBID_ATTR).toContain("usemap");
    for (const tag of ["audio", "video", "source", "track", "map", "area", "style", "form", "input", "button", "textarea", "select"]) {
      expect(MARKDOWN_FORBID_TAGS).toContain(tag);
    }
  });

  it("classifies anchors, external links, local targets, and unsupported schemes", () => {
    expect(classifyMarkdownLink("/repo", "#getting-started")).toEqual({ kind: "anchor", id: "getting-started" });
    expect(classifyMarkdownLink("/repo", "#%C5%A1umniki")).toEqual({ kind: "anchor", id: "šumniki" });
    expect(classifyMarkdownLink("/repo", "#%E0%A4%A")).toEqual({ kind: "anchor", id: "%E0%A4%A" });
    expect(classifyMarkdownLink("/repo", "https://example.com/docs")).toEqual({ kind: "external", url: "https://example.com/docs" });
    expect(classifyMarkdownLink("/repo", "MAILTO:user@example.com")).toEqual({ kind: "external", url: "MAILTO:user@example.com" });
    expect(classifyMarkdownLink("/repo", "./payload")).toEqual({ kind: "local", path: "/repo/payload", runnable: false });
    expect(classifyMarkdownLink("/repo", "docs/guide.md")).toEqual({ kind: "local", path: "/repo/docs/guide.md", runnable: false });
    expect(classifyMarkdownLink("/repo", "tools/install.sh")).toEqual({ kind: "local", path: "/repo/tools/install.sh", runnable: true });
    expect(classifyMarkdownLink("/repo", "file:///etc/passwd")).toEqual({ kind: "ignore" });
    expect(classifyMarkdownLink("/repo", "javascript:alert(1)")).toEqual({ kind: "ignore" });
    expect(classifyMarkdownLink("/repo", "/etc/passwd")).toEqual({ kind: "ignore" });
    expect(classifyMarkdownLink("/repo", "//server/share/file")).toEqual({ kind: "ignore" });
    expect(classifyMarkdownLink("/repo", "   ")).toEqual({ kind: "ignore" });
  });

  it("requires confirmation for every local link and never opens when rejected", async () => {
    for (const [href, expectedPath, expectedRunnable] of [
      ["./payload", "/repo/payload", false],
      ["docs/guide.md", "/repo/docs/guide.md", false],
      ["tools/setup.sh", "/repo/tools/setup.sh", true],
    ] as const) {
      const scrollToAnchor = vi.fn();
      const openExternal = vi.fn();
      const confirmLocal = vi.fn(async () => false);
      const openLocal = vi.fn();

      const kind = await activateMarkdownLink("/repo", href, { scrollToAnchor, openExternal, confirmLocal, openLocal });

      expect(kind).toBe("local");
      expect(confirmLocal).toHaveBeenCalledWith(expectedPath, expectedRunnable);
      expect(openLocal).not.toHaveBeenCalled();
      expect(openExternal).not.toHaveBeenCalled();
      expect(scrollToAnchor).not.toHaveBeenCalled();
    }
  });

  it("opens local targets only after explicit confirmation", async () => {
    const confirmLocal = vi.fn(async () => true);
    const openLocal = vi.fn();

    await activateMarkdownLink("/repo", "./payload", {
      scrollToAnchor: vi.fn(),
      openExternal: vi.fn(),
      confirmLocal,
      openLocal,
    });
    expect(confirmLocal).toHaveBeenCalledWith("/repo/payload", false);
    expect(openLocal).toHaveBeenCalledWith("/repo/payload");

    await activateMarkdownLink("/repo", "run.AppImage", {
      scrollToAnchor: vi.fn(),
      openExternal: vi.fn(),
      confirmLocal,
      openLocal,
    });
    expect(confirmLocal).toHaveBeenCalledWith("/repo/run.AppImage", true);
    expect(openLocal).toHaveBeenCalledWith("/repo/run.AppImage");
  });

  it("handles anchors, supported web links, and ignored links without local launch prompts", async () => {
    const scrollToAnchor = vi.fn();
    const openExternal = vi.fn();
    const confirmLocal = vi.fn(async () => true);
    const openLocal = vi.fn();
    const handlers = { scrollToAnchor, openExternal, confirmLocal, openLocal };

    expect(await activateMarkdownLink("/repo", "#usage", handlers)).toBe("anchor");
    expect(scrollToAnchor).toHaveBeenCalledWith("usage");

    expect(await activateMarkdownLink("/repo", "https://example.com", handlers)).toBe("external");
    expect(openExternal).toHaveBeenCalledWith("https://example.com");

    expect(await activateMarkdownLink("/repo", "file:///etc/passwd", handlers)).toBe("ignore");
    expect(confirmLocal).not.toHaveBeenCalled();
    expect(openLocal).not.toHaveBeenCalled();
  });
});
