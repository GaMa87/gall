import { describe, expect, it } from "vitest";
import { annotationKey, BUG_REPORT_INSTRUCTIONS, issueLink, moveMark, normalizedBox, reportBodyPreview, reproduction, type Mark } from "./bugReport";
import { beginReportDraft, completeReportDraft, getReportDraft, subscribeReportDraft, updateReportDraft } from "./reportDraft";

describe("bug report GitHub handoff", () => {
  const compact = "## What happened?\nFile not found\n\n## Environment\n- Gall: 0.1.3\n\n## Recent actions and errors\n```text\nerror: File not found\n```\n";
  it("fills the short report directly into the issue form without requiring paste", () => {
    const result = issueLink("Bug report: Gall 0.1.3 on linux", compact);
    const url = new URL(result.url);
    expect(url.origin + url.pathname).toBe("https://github.com/GaMa87/gall/issues/new");
    expect(url.searchParams.get("template")).toBe("bug_report_app.md");
    expect(url.searchParams.get("body")).toBe(compact);
    expect(result.needsAttachment).toBe(false);
  });
  it("recommends a ZIP attachment instead of an oversized GitHub link", () => {
    const report = compact + "č😀".repeat(3000);
    const result = issueLink("Long reproduction", report);
    expect(result.needsAttachment).toBe(true);
    expect(result.url.length).toBeLessThanOrEqual(7000);
    expect(new URL(result.url).searchParams.get("body")).toBe(BUG_REPORT_INSTRUCTIONS);
    expect(BUG_REPORT_INSTRUCTIONS).toContain("ZIP");
    expect(BUG_REPORT_INSTRUCTIONS).toContain("screenshots or videos");
    expect(BUG_REPORT_INSTRUCTIONS).toContain("Drag the ZIP here");
    expect(BUG_REPORT_INSTRUCTIONS).toContain("only through Save report");
  });
  it("allows the exact link budget and warns on the next encoded character", () => {
    const remaining = issueLink("Title", "").remaining;
    const exact = issueLink("Title", "a".repeat(remaining));
    expect(exact.remaining).toBe(0);
    expect(exact.url.length).toBe(7000);
    expect(exact.needsAttachment).toBe(false);
    expect(issueLink("Title", "a".repeat(remaining + 1)).needsAttachment).toBe(true);
    expect(issueLink("Title", "a".repeat(remaining + 1)).remaining).toBe(-1);
  });
  it("counts URL overhead, title, reserved characters, accents and emoji", () => {
    const empty = issueLink("", "").remaining;
    expect(issueLink("", "a ").remaining).toBe(empty - 2);
    expect(issueLink("", "&").remaining).toBe(empty - 3);
    expect(issueLink("", "č").remaining).toBe(empty - 6);
    expect(issueLink("", "😀").remaining).toBe(empty - 12);
    expect(issueLink("č", "").remaining).toBe(empty - 6);
    expect(issueLink("", compact).remaining).toBeLessThan(empty);
  });
  it("updates the budget immediately while preserving the prepared environment and error", () => {
    const description = "File not found";
    const replacement = "Unicode č😀\n\n## Environment\nThis is user text";
    const next = reportBodyPreview({ body: compact, description }, replacement);
    expect(next).toBe(compact.replace(description, replacement));
    expect(issueLink("", next).remaining).toBe(issueLink("", compact).remaining - (new URLSearchParams({ d: replacement }).toString().length - new URLSearchParams({ d: description }).toString().length));
  });

  describe("report draft during capture", () => {
    it("preserves unfinished drafts but starts fresh after a successful handoff", () => {
      updateReportDraft({ title: "Not sent", description: "My issue", steps: "One", expected: "Two", includeLog: false, captureError: "Old error", media: [{ id: "image", name: "image.png", kind: "image", blob: new Blob(["png"]) }] });
      beginReportDraft();
      expect(getReportDraft().title).toBe("Not sent");
      expect(getReportDraft().includeLog).toBe(false);
      completeReportDraft();
      expect(getReportDraft().media).toHaveLength(1);
      beginReportDraft();
      expect(getReportDraft()).toEqual({ title: "", description: "", steps: "", expected: "", includeLog: true, media: [], capturing: false, captureError: "" });
      updateReportDraft({ title: "New unsent report" });
      beginReportDraft();
      expect(getReportDraft().title).toBe("New unsent report");
    });
    it("does not discard a capture that is still in progress", () => {
      updateReportDraft({ title: "Capture", capturing: true });
      completeReportDraft();
      beginReportDraft();
      expect(getReportDraft().capturing).toBe(true);
      expect(getReportDraft().title).toBe("Capture");
      updateReportDraft({ capturing: false });
      beginReportDraft();
      expect(getReportDraft().title).toBe("");
    });
    it("keeps evidence captured after handoff when the capture restores its report", () => {
      updateReportDraft({ title: "Already shared", media: [] });
      completeReportDraft();
      updateReportDraft({ capturing: true });
      const media = { id: "after-handoff", name: "capture.png", kind: "image" as const, blob: new Blob(["png"]) };
      updateReportDraft({ capturing: false, media: [media] });
      beginReportDraft(true);
      expect(getReportDraft().media).toEqual([media]);
      expect(getReportDraft().title).toBe("Already shared");
      beginReportDraft();
      expect(getReportDraft().media).toEqual([]);
      expect(getReportDraft().title).toBe("");
    });
    it("notifies an already open reporter and preserves a capture through later typing", () => {
      updateReportDraft({ title: "Before capture", media: [] });
      const observed: number[] = [];
      const unsubscribe = subscribeReportDraft(() => observed.push(getReportDraft().media.length));
      const media = { id: "capture", name: "capture.png", kind: "image" as const, blob: new Blob(["png"]) };
      updateReportDraft((current) => ({ media: [...current.media, media] }));
      updateReportDraft({ title: "Edited while recording" });
      expect(observed).toEqual([1, 1]);
      expect(getReportDraft().media).toEqual([media]);
      unsubscribe();
    });
    it("does not resurrect a removed attachment when an old editor finishes", () => {
      updateReportDraft({ media: [{ id: "old", name: "old.png", kind: "image", blob: new Blob(["before"]) }] });
      updateReportDraft({ media: [] });
      updateReportDraft((current) => ({ media: current.media.map((media) => media.id === "old" ? { ...media, blob: new Blob(["after"]) } : media) }));
      expect(getReportDraft().media).toEqual([]);
    });
  });

  describe("report description and accessible annotation", () => {
    it("adds only filled reproduction sections without truncating Unicode", () => {
      expect(reproduction("  Napaka 日本語  ", "", "")).toBe("Napaka 日本語");
      expect(reproduction("List stuck", "Open a folder", "List updates")).toBe("List stuck\n\n### Steps to reproduce\nOpen a folder\n\n### Expected behavior\nList updates");
    });
    const mark: Mark = { tool: "redact", x: 0.8, y: 0.9, endX: 1, endY: 1 };
    it("keeps a moved redaction inside the image without changing its size", () => {
      expect(moveMark(mark, "ArrowRight")).toEqual(mark);
      const moved = moveMark(mark, "ArrowLeft");
      expect(moved.x).toBeCloseTo(0.79);
      expect(moved.endX).toBeCloseTo(0.99);
      expect(moved.endX - moved.x).toBeCloseTo(0.2);
    });
    it("resizes only the endpoint and supports reverse drags", () => {
      expect(moveMark(mark, "ArrowUp", true)).toEqual({ ...mark, endY: 0.99 });
      expect(normalizedBox({ ...mark, x: 1, y: 1, endX: 0.2, endY: 0.3 })).toEqual({ x: 0.2, y: 0.3, width: 0.8, height: 0.7 });
    });
    it("uses Vim movement only when enabled while always allowing arrow keys", () => {
      expect(annotationKey("j", false)).toBe(false);
      expect(annotationKey("j", true)).toBe(true);
      expect(annotationKey("ArrowDown", false)).toBe(true);
      expect(moveMark(mark, "h")).toEqual(moveMark(mark, "ArrowLeft"));
      expect(annotationKey("a", true)).toBe(false);
    });
  });
  it("encodes titles without injecting extra query parameters", () => {
    const title = "č😀 & body=private diagnostic text";
    const url = new URL(issueLink(title, compact + "&template=other\n日本語").url);
    expect(url.searchParams.get("title")).toBe(title);
    expect(url.searchParams.get("body")).toBe(compact + "&template=other\n日本語");
    expect(Array.from(url.searchParams.keys())).toEqual(["template", "title", "body"]);
  });
  it("keeps links short even if a malformed title is very long", () => {
    const result = issueLink("😀".repeat(10000), compact);
    expect(new TextEncoder().encode(result.url).length).toBeLessThanOrEqual(7000);
    expect(new URL(result.url).searchParams.get("body")).toBe(compact);
    expect(new URL(result.url).searchParams.get("title")).not.toContain("�");
  });
});
