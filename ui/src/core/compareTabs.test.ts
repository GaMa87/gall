import { describe, expect, it } from "vitest";
import type { DiffSource } from "../api/types";
import { MAX_TABS, closeTab, fromRecord, openTab, sourceLabel, tabLabel, toSessions, type CompareTab } from "./compareTabs";

const file = (path: string): DiffSource => ({ kind: "file", path });
const text = (label: string): DiffSource => ({ kind: "text", label, text: "x" });

describe("comparison tabs", () => {
  it("names the sides", () => {
    expect(sourceLabel(file("/a/b/c.txt"))).toBe("c.txt");
    expect(sourceLabel({ kind: "gitHead", path: "/r/f.rs" })).toBe("f.rs (HEAD)");
    expect(sourceLabel({ kind: "gitCommit", path: "/r/f.rs", commit: "0123456789abcdef" })).toBe("f.rs (0123456)");
    expect(sourceLabel({ kind: "gitBlob", root: "/r", file: "src/f.rs", commit: "abcdef0123" })).toBe("f.rs (abcdef0)");
    expect(tabLabel(file("/x/a"), text("pasted"))).toBe("a ↔ pasted");
  });
  it("shows a pair that is open and adds a new one after the shown tab", () => {
    let s = openTab([], 0, file("/1"), file("/2"));
    expect(s).toMatchObject({ active: 0 });
    s = openTab(s.tabs, s.active, file("/3"), file("/4"));
    s = openTab(s.tabs, 0, file("/5"), file("/6"));
    expect(s.tabs.map((t) => t.label)).toEqual(["1 ↔ 2", "5 ↔ 6", "3 ↔ 4"]);
    expect(s.active).toBe(1);
    // The same pair again shows the tab it has.
    const again = openTab(s.tabs, 1, file("/3"), file("/4"));
    expect(again.active).toBe(2);
    expect(again.tabs).toHaveLength(3);
  });
  it("drops the oldest tab that is not the new one past the limit", () => {
    let s = { tabs: [] as CompareTab[], active: 0 };
    for (let i = 0; i < MAX_TABS + 3; i++) s = openTab(s.tabs, s.active, file(`/l${i}`), file(`/r${i}`));
    expect(s.tabs).toHaveLength(MAX_TABS);
    expect(s.tabs[s.active].label).toBe(`l${MAX_TABS + 2} ↔ r${MAX_TABS + 2}`);
    expect(s.tabs.some((t) => t.label === "l0 ↔ r0")).toBe(false);
  });
  it("closes a tab and moves to the neighbour", () => {
    const tabs = ["1", "2", "3"].map((n) => ({ a: file(`/a${n}`), b: file(`/b${n}`), label: n }));
    expect(closeTab(tabs, 1, 1)).toMatchObject({ active: 1 });
    expect(closeTab(tabs, 2, 2)).toMatchObject({ active: 1 });
    expect(closeTab(tabs, 0, 2)).toMatchObject({ active: 1 });
    expect(closeTab(tabs, 2, 0)).toMatchObject({ active: 0 });
    expect(closeTab([tabs[0]], 0, 0)).toEqual({ tabs: [], active: null });
    expect(closeTab(tabs, 9, 1).tabs).toHaveLength(3);
  });
  it("saves text only by its label, preserves view state, and restores only tabs whose sources were kept", () => {
    const tabs: CompareTab[] = [
      {
        a: file("/a"),
        b: file("/b"),
        label: "a ↔ b",
        mode: "unified",
        options: { ignoreCase: true, ignoreEol: true },
        scroll: { left: false, line: 42 },
        activeHunk: 3,
      },
      { a: file("/a"), b: text("pasted"), label: "a ↔ pasted" },
    ];
    const saved = toSessions(tabs, 1, 2);
    expect(saved.version).toBe(2);
    expect(saved.active).toBe(1);
    expect(saved.tabs[1].right).toEqual({ source: null, label: "pasted" });
    expect(saved.tabs[0].left.source).toEqual(file("/a"));
    expect(saved.tabs[0].mode).toBe("unified");
    expect(saved.tabs[0].options).toEqual({ ignoreCase: true, ignoreEol: true });
    expect(saved.tabs[0].scroll).toEqual({ left: false, line: 42 });
    expect(saved.tabs[0].activeHunk).toBe(3);
    expect(fromRecord(saved.tabs[0])).toMatchObject({
      label: "a ↔ b",
      mode: "unified",
      options: { ignoreCase: true, ignoreEol: true },
      scroll: { left: false, line: 42 },
      activeHunk: 3,
    });
    expect(fromRecord(saved.tabs[1])).toBeNull();
    // The shown tab is kept within the tabs.
    expect(toSessions(tabs, 9, 1).active).toBe(1);
    expect(toSessions([], 3, 1).active).toBe(0);
  });
});
