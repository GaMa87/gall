import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DiffOp, OutlineNode } from "../api/types";
import { CompareOutline } from "./CompareOutline";

// A static render (no DOM environment is installed): it shows the structure a screen reader gets, not the key handling,
// which is covered by `pressKey` in `core/outline.test.ts`.
const node = (kind: OutlineNode["kind"], name: string | null, line: number, endLine: number, children: OutlineNode[] = []): OutlineNode =>
  ({ kind, name, span: { start: 0, end: 0, line }, endLine, children });
const tree: OutlineNode[] = [node("module", "m", 1, 10, [node("function", "a", 2, 4), node("function", "b", 5, 9)]), node("function", "top", 11, 12)];
const ops: DiffOp[] = [];

function render(onSwitchSide?: () => void, truncated?: boolean): string {
  return renderToStaticMarkup(createElement(CompareOutline, { nodes: tree, truncated, ops, side: "b", onJump: () => {}, onClose: () => {}, onSwitchSide }));
}

describe("CompareOutline structure", () => {
  it("has a tree with tree items only, the header and its buttons outside of it", () => {
    const html = render(() => {});
    const open = html.indexOf('role="tree"');
    expect(open).toBeGreaterThan(-1);
    const inside = html.slice(open);
    expect(inside).not.toContain("dv-outline-head");
    expect(inside).not.toContain("<button");
    expect(html.slice(0, open)).toContain("dv-outline-head");
    expect(html.slice(0, open)).toContain("<button");
    // Every row is a tree item.
    expect((inside.match(/role="treeitem"/g) ?? []).length).toBe(2);
  });
  it("names the active row for assistive technology and gives each row its place among its siblings", () => {
    const html = render();
    expect(html).toContain('aria-activedescendant="ol-0"');
    expect(html).toContain('id="ol-0"');
    // The module has two siblings at the top level (itself and `top`), at positions 1 and 2.
    expect(html).toContain('aria-setsize="2"');
    expect(html).toContain('aria-posinset="1"');
    expect(html).toContain('aria-posinset="2"');
    expect(html).toContain('aria-level="1"');
  });
  it("offers the other side only when it is asked for", () => {
    expect(render()).not.toContain("⇄");
    expect(render(() => {})).toContain("⇄");
  });
  it("says when the list was cut short", () => {
    expect(render(undefined, true)).toContain("only the first part is listed");
    expect(render(undefined, false)).not.toContain("only the first part is listed");
  });
});
