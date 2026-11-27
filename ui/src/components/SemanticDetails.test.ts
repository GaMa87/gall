import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SemanticChange } from "../api/types";
import { resolveSemanticKey, SemanticDetails } from "./SemanticDetails";

const SAMPLE: SemanticChange[] = [
  { kind: "behavior", name: "Counter::inc", left: { start: 30, end: 64, line: 3 }, right: { start: 30, end: 64, line: 4 } },
  { kind: "removed", name: "Counter::old", left: { start: 70, end: 95, line: 8 }, right: null },
  { kind: "added", name: "Counter::reset", left: null, right: { start: 80, end: 112, line: 9 } },
];

describe("SemanticDetails", () => {
  it("renders change kinds, qualified member names and both left and right jump buttons", () => {
    const html = renderToStaticMarkup(
      createElement(SemanticDetails, {
        changes: SAMPLE,
        vim: true,
        onJump: () => {},
        onClose: () => {},
      }),
    );
    expect(html).toContain("Syntax changes");
    expect(html).toContain("Counter::inc");
    expect(html).toContain("Behavior");
    expect(html).toContain("Left :3");
    expect(html).toContain("Right :4");
    expect(html).toContain("Counter::old");
    expect(html).toContain("Left :8");
    expect(html).toContain("Counter::reset");
    expect(html).toContain("Right :9");
  });

  it("navigates with arrows and jumps to left or right lines", () => {
    expect(resolveSemanticKey(SAMPLE, 0, "ArrowDown", false, true)).toEqual({ type: "move", index: 1 });
    expect(resolveSemanticKey(SAMPLE, 1, "ArrowUp", false, true)).toEqual({ type: "move", index: 0 });
    expect(resolveSemanticKey(SAMPLE, 0, "End", false, true)).toEqual({ type: "move", index: 2 });
    expect(resolveSemanticKey(SAMPLE, 2, "Home", false, true)).toEqual({ type: "move", index: 0 });
    expect(resolveSemanticKey(SAMPLE, 0, "ArrowLeft", false, true)).toEqual({ type: "jump", side: "l", line: 3 });
    expect(resolveSemanticKey(SAMPLE, 0, "ArrowRight", false, true)).toEqual({ type: "jump", side: "r", line: 4 });
    expect(resolveSemanticKey(SAMPLE, 0, "Enter", false, true)).toEqual({ type: "jump", side: "r", line: 4 });
    expect(resolveSemanticKey(SAMPLE, 1, "Enter", false, true)).toEqual({ type: "jump", side: "l", line: 8 });
    expect(resolveSemanticKey(SAMPLE, 2, "ArrowLeft", false, true)).toBeNull();
    expect(resolveSemanticKey(SAMPLE, 0, "Escape", false, true)).toEqual({ type: "close" });
    // Enter on a nested button should not be intercepted by the listbox.
    expect(resolveSemanticKey(SAMPLE, 0, "Enter", false, false)).toBeNull();
  });

  it("supports Vim motion keys only when Vim keys are enabled", () => {
    expect(resolveSemanticKey(SAMPLE, 0, "j", true, true)).toEqual({ type: "move", index: 1 });
    expect(resolveSemanticKey(SAMPLE, 1, "k", true, true)).toEqual({ type: "move", index: 0 });
    expect(resolveSemanticKey(SAMPLE, 0, "G", true, true)).toEqual({ type: "move", index: 2 });
    expect(resolveSemanticKey(SAMPLE, 2, "g", true, true)).toEqual({ type: "move", index: 0 });
    expect(resolveSemanticKey(SAMPLE, 0, "h", true, true)).toEqual({ type: "jump", side: "l", line: 3 });
    expect(resolveSemanticKey(SAMPLE, 0, "l", true, true)).toEqual({ type: "jump", side: "r", line: 4 });

    for (const k of ["j", "k", "g", "G", "h", "l"]) {
      expect(resolveSemanticKey(SAMPLE, 0, k, false, true)).toBeNull();
    }
  });
});
