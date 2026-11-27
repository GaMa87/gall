import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AiItem, AiOutcome } from "../api/types";
import { CompareExplanation } from "../components/CompareExplanation";
import { categoryCounts, confidenceText, filterItems, isCurrent, isSnapshotCurrent, outcomeMessage, scopeLabel } from "./explain";

const item = (hunk: number, category: AiItem["category"]): AiItem => ({ hunk, category, confidence: 0.5, note: "" });
const items = [item(0, "bugfix"), item(1, "refactor"), item(2, "bugfix"), item(3, "unknown")];

describe("explanation filters", () => {
  it("shows everything without a filter and only the chosen categories with one", () => {
    expect(filterItems(items, new Set())).toHaveLength(4);
    expect(filterItems(items, new Set(["bugfix"])).map((i) => i.hunk)).toEqual([0, 2]);
    expect(filterItems(items, new Set(["bugfix", "unknown"])).map((i) => i.hunk)).toEqual([0, 2, 3]);
    expect(filterItems(items, new Set(["performance"]))).toEqual([]);
  });
  it("counts the items of each category", () => {
    expect(categoryCounts(items)).toEqual({ bugfix: 2, feature: 0, refactor: 1, performance: 0, formatting: 0, unknown: 1 });
  });
});

describe("confidence", () => {
  it("is a rounded percentage inside 0 to 100", () => {
    expect(confidenceText(0.7)).toBe("70 %");
    expect(confidenceText(0.666)).toBe("67 %");
    expect(confidenceText(2)).toBe("100 %");
    expect(confidenceText(-1)).toBe("0 %");
    expect(confidenceText(Number.NaN)).toBe("0 %");
  });
});

describe("outcomes", () => {
  const explanation = { protocol: 1, summary: "", why: "", risk: "", uncertainty: "", category: "unknown" as const, confidence: 0, items: [], evidence: [] };
  it("has no message for an explanation and words for every other end", () => {
    expect(outcomeMessage({ outcome: "explained", explanation, model: "m", cached: false })).toBeNull();
    const others: AiOutcome[] = [
      { outcome: "disabled" }, { outcome: "refused", reason: "relative path" }, { outcome: "tooLarge" }, { outcome: "timeout" },
      { outcome: "cancelled" }, { outcome: "failed", message: "boom" }, { outcome: "malformed", reason: "bad evidence" },
    ];
    for (const o of others) expect(outcomeMessage(o)).toBeTruthy();
    expect(outcomeMessage({ outcome: "disabled" })).toContain("compare-ai.json");
    expect(outcomeMessage({ outcome: "failed", message: "boom" })).toContain("boom");
    expect(outcomeMessage({ outcome: "malformed", reason: "bad evidence" })).toContain("ignored");
  });
});

describe("stale answers", () => {
  it("belong to the texts they were asked for and no others", () => {
    expect(isCurrent({ left: "a", right: "b" }, { left: "a", right: "b" })).toBe(true);
    expect(isCurrent({ left: "a", right: "b" }, { left: "a", right: "c" })).toBe(false);
    expect(isCurrent(null, { left: "a", right: "b" })).toBe(false);
  });

  it("tracks snapshot freshness by line-array references and diff options without joining texts", () => {
    const l = ["a", "b"], r = ["a", "c"];
    expect(isSnapshotCurrent({ leftLines: l, rightLines: r, optsKey: "{}" }, { leftLines: l, rightLines: r, optsKey: "{}" })).toBe(true);
    expect(isSnapshotCurrent({ leftLines: l, rightLines: r, optsKey: "{}" }, { leftLines: l, rightLines: r, optsKey: '{"minimal":true}' })).toBe(false);
    expect(isSnapshotCurrent({ leftLines: l, rightLines: r, optsKey: "{}" }, { leftLines: [...l], rightLines: r, optsKey: "{}" })).toBe(false);
  });
});

describe("scope labels and hunk links", () => {
  it("formats all, single-hunk and selection scopes", () => {
    expect(scopeLabel({ kind: "all" })).toBe("All changes");
    expect(scopeLabel({ kind: "hunk", index: 2 })).toBe("Change 3");
    expect(scopeLabel({ kind: "selection", side: "right", startLine: 8, endLine: 4 })).toBe("Selected right lines 4–8");
  });

  it("renders scope buttons and clickable links from each item to its hunk", () => {
    const html = renderToStaticMarkup(
      createElement(CompareExplanation, {
        outcome: {
          outcome: "explained",
          model: "local-1",
          cached: false,
          explanation: {
            protocol: 1,
            summary: "Adjusted block 2",
            why: "Fix",
            risk: "Low",
            uncertainty: "None",
            category: "bugfix",
            confidence: 0.8,
            items: [{ hunk: 1, category: "bugfix", confidence: 0.8, note: "guard check" }],
            evidence: [{ hunk: 1, side: "right", startLine: 10, endLine: 11 }],
          },
        },
        scope: { kind: "hunk", index: 1 },
        hunkCount: 3,
        activeHunk: 1,
        selectionScope: { kind: "selection", side: "right", startLine: 10, endLine: 11 },
        stale: false,
        onScope: () => {},
        onHunk: () => {},
        onEvidence: () => {},
        onCancel: () => {},
        onClose: () => {},
      }),
    );
    expect(html).toContain("All changes");
    expect(html).toContain("Change 2");
    expect(html).toContain("Selection");
    expect(html).toContain('aria-label="Jump to change 2"');
    expect(html).toContain("Change 2: Bugfix, 80 %");
  });
});
