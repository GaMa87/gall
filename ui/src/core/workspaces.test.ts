import { describe, expect, it } from "vitest";
import type { Session, WorkspaceSummary } from "../api/types";
import { buildKeymap, chordOf, typedChar } from "./keymap";
import {
  resolveDropPortalTargets,
  syncActiveWorkspaceSummary,
  workspacePillLabel,
  workspaceTooltip,
} from "./workspaces";

const ws = (overrides: Partial<WorkspaceSummary> = {}): WorkspaceSummary => ({
  id: 2,
  index: 1,
  name: "Release",
  accent: "#3fb27f",
  layout: "standard",
  activeBrowser: 0,
  leftPath: "/home/u/projects/gall",
  rightPath: "/home/u/staging/gall",
  leftTabs: 2,
  rightTabs: 1,
  pairCount: 0,
  ...overrides,
});

describe("workspaces and Drop Portal", () => {
  it("resolves dual Drop Portal targets for inactive standard/columns workspaces", () => {
    const standard = resolveDropPortalTargets(ws(), 1, "/home/u");
    expect(standard).toHaveLength(2);
    expect(standard![0]).toMatchObject({ side: 0, sideLabel: "Left", path: "/home/u/projects/gall", name: "gall", parentPath: "~/projects" });
    expect(standard![1]).toMatchObject({ side: 1, sideLabel: "Right", path: "/home/u/staging/gall", name: "gall", parentPath: "~/staging" });

    const columns = resolveDropPortalTargets(ws({ layout: "columns" }), 1, "/home/u");
    expect(columns?.map((t) => t.sideLabel)).toEqual(["Top", "Bottom"]);
  });

  it("resolves a single target in Classic layout and returns null for the active workspace", () => {
    expect(resolveDropPortalTargets(ws({ id: 2 }), 2)).toBeNull();

    const classic = resolveDropPortalTargets(ws({ layout: "classic", activeBrowser: 1 }), 1, "/home/u");
    expect(classic).toHaveLength(1);
    expect(classic![0]).toMatchObject({ side: 1, sideLabel: "Active", path: "/home/u/staging/gall" });
  });

  it("formats pill labels and tooltips concisely", () => {
    expect(workspacePillLabel({ name: null }, 0)).toEqual({ num: "1", name: null });
    expect(workspacePillLabel({ name: "  Dev  " }, 2)).toEqual({ num: "3", name: "Dev" });
    const tip = workspaceTooltip(ws(), 1, "/home/u", "Alt+2");
    expect(tip).toContain("2. Release (Alt+2)");
    expect(tip).toContain("~/projects/gall");
    expect(tip).toContain("~/staging/gall");
  });

  it("keeps the active workspace summary synchronized after session mutations", () => {
    const list = [ws({ id: 1, index: 0, leftPath: "/a", rightPath: "/b" }), ws({ id: 2, index: 1 })];
    const session: Session = {
      name: null,
      layout: "columns",
      activeBrowser: 1,
      browsers: [
        { tabs: [{ id: 1, path: "/new-left", alias: null, keepAlias: false, history: { back: [], forward: [] }, lastUsed: 0 }], active: 0, closed: [], collapsed: false },
        { tabs: [{ id: 2, path: "/new-right", alias: null, keepAlias: false, history: { back: [], forward: [] }, lastUsed: 0 }], active: 0, closed: [], collapsed: false },
      ],
    };
    const updated = syncActiveWorkspaceSummary(list, 0, session);
    expect(updated[0].leftPath).toBe("/new-left");
    expect(updated[0].rightPath).toBe("/new-right");
    expect(updated[0].layout).toBe("columns");
    expect(updated[1]).toBe(list[1]);
  });

  it("binds Alt+1..9 for workspace switching without breaking Option+[ typing on macOS", () => {
    const km = buildKeymap({});
    expect(km.get("Alt+1")).toBe("workspace1");
    expect(km.get("Alt+9")).toBe("workspace9");
    // US Mac Option+2 produces "™" (non-ASCII): normalizes to Alt+2 shortcut.
    expect(chordOf({ key: "™", code: "Digit2", ctrlKey: false, metaKey: false, altKey: true, shiftKey: false })).toBe("Alt+2");
    // EU Mac Option+5 produces "[" (ASCII): stays typed text for quick filter.
    expect(typedChar({ key: "[", code: "Digit5", ctrlKey: false, metaKey: false, altKey: true, shiftKey: false }, (c) => km.has(c), true)).toBe(true);
  });
});
