import { describe, expect, it } from "vitest";
import type { Session, TabPair } from "../api/types";
import { buildKeymap } from "./keymap";
import {
  activePair,
  findPairForTab,
  flowSourceTabIsActive,
  nextPairMode,
  pairColor,
  pairGlyph,
  partnerTabOf,
  resolveFlowDispatch,
  tandemFallbackParent,
  tandemRelativeDisplay,
} from "./pairs";

const samplePair = (overrides: Partial<TabPair> = {}): TabPair => ({
  id: 10,
  leftTabId: 1,
  rightTabId: 2,
  mode: "linked",
  leftRoot: "/work/repo-a",
  rightRoot: "/work/repo-b",
  flowDir: "leftToRight",
  flowOp: "copy",
  colorSlot: 0,
  ...overrides,
});

const sampleSession = (pairs: TabPair[] = []): Session => ({
  name: null,
  layout: "standard",
  activeBrowser: 0,
  pairs,
  browsers: [
    {
      tabs: [
        { id: 1, path: "/work/repo-a/src", alias: null, keepAlias: false, history: { back: [], forward: [] }, lastUsed: 0 },
        { id: 3, path: "/tmp", alias: null, keepAlias: false, history: { back: [], forward: [] }, lastUsed: 0 },
      ],
      active: 0,
      closed: [],
      collapsed: false,
    },
    {
      tabs: [
        { id: 2, path: "/work/repo-b/src", alias: null, keepAlias: false, history: { back: [], forward: [] }, lastUsed: 0 },
      ],
      active: 0,
      closed: [],
      collapsed: false,
    },
  ],
});

describe("Live Tab Pairs (WN-180)", () => {
  it("finds active and per-tab pairs and resolves partner tab", () => {
    const p = samplePair({ mode: "tandem" });
    const s = sampleSession([p]);
    expect(activePair(s)).toEqual(p);
    expect(findPairForTab(s, 0, 1)).toEqual(p);
    expect(findPairForTab(s, 1, 2)).toEqual(p);
    expect(findPairForTab(s, 0, 3)).toBeNull();
    expect(partnerTabOf(s, p, 0)?.tab.id).toBe(2);
    expect(partnerTabOf(s, p, 1)?.tab.id).toBe(1);
    expect(pairColor(p)).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("cycles pair modes and produces expected semantic glyphs", () => {
    expect(nextPairMode("linked")).toBe("tandem");
    expect(nextPairMode("tandem")).toBe("flow");
    expect(nextPairMode("flow")).toBe("linked");

    expect(pairGlyph(samplePair({ mode: "linked" }))).toBe("⇄");
    expect(pairGlyph(samplePair({ mode: "tandem" }))).toBe("🔗");
    expect(pairGlyph(samplePair({ mode: "flow", flowDir: "leftToRight" }))).toBe("→");
    expect(pairGlyph(samplePair({ mode: "flow", flowDir: "rightToLeft" }))).toBe("←");
  });

  it("resolves Flow dispatch direction, destination path, and operation", () => {
    const pRight = samplePair({ mode: "flow", flowDir: "leftToRight", flowOp: "move" });
    const s = sampleSession([pRight]);
    expect(resolveFlowDispatch(s, pRight)).toEqual({
      srcBrowser: 0,
      dstBrowser: 1,
      dstPath: "/work/repo-b/src",
      op: "move",
    });

    const pLeft = samplePair({ mode: "flow", flowDir: "rightToLeft", flowOp: "copy" });
    expect(resolveFlowDispatch(sampleSession([pLeft]), pLeft)).toEqual({
      srcBrowser: 1,
      dstBrowser: 0,
      dstPath: "/work/repo-a/src",
      op: "copy",
    });

    expect(resolveFlowDispatch(s, samplePair({ mode: "linked" }))).toBeNull();
  });

  it("only sends Flow selections from the active source tab", () => {
    const pair = samplePair({ mode: "flow" });
    const session = sampleSession([pair]);
    expect(flowSourceTabIsActive(session, pair)).toBe(true);

    session.browsers[0].active = 1;
    expect(flowSourceTabIsActive(session, pair)).toBe(false);

    const reversePair = samplePair({ mode: "flow", flowDir: "rightToLeft" });
    const reverseSession = sampleSession([reversePair]);
    expect(flowSourceTabIsActive(reverseSession, reversePair)).toBe(true);

    reverseSession.browsers[1].tabs.push({
      id: 4,
      path: "/tmp",
      alias: null,
      keepAlias: false,
      history: { back: [], forward: [] },
      lastUsed: 0,
    });
    reverseSession.browsers[1].active = 1;
    expect(flowSourceTabIsActive(reverseSession, reversePair)).toBe(false);
  });

  it("formats Tandem relative subpaths and fallback parents for missing-folder recovery", () => {
    expect(tandemRelativeDisplay("/work/repo-b/src/new-feature", "/work/repo-b")).toBe("src/new-feature");
    expect(tandemFallbackParent("/work/repo-b/src/new-feature", "/work/repo-b")).toBe("/work/repo-b/src");
  });

  it("binds default pair shortcuts cleanly without conflicts", () => {
    const km = buildKeymap({});
    expect(km.get("Ctrl+Alt+P")).toBe("pairToggle");
    expect(km.get("Ctrl+Alt+M")).toBe("pairCycleMode");
    expect(km.get("Ctrl+Shift+Enter")).toBe("pairFlowSend");
    expect(km.get("Alt+G")).toBe("gravityAccept");
  });
});
