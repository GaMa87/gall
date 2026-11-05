import { describe, expect, it } from "vitest";
import { chordOf, primaryMod } from "./keymap";

const ev = (ctrlKey: boolean, metaKey: boolean) => ({ ctrlKey, metaKey });

describe("Ctrl, or Cmd on macOS, in windows that read keys themselves", () => {
  it("Cmd counts like Ctrl (Cmd+S saves the merge, Cmd+Enter = Create & open)", () => {
    expect(primaryMod(ev(false, true))).toBe(true);
    expect(primaryMod(ev(true, false))).toBe(true);
    expect(primaryMod(ev(false, false))).toBe(false);
  });
  it("matches the global shortcuts, where Cmd is Ctrl", () => {
    const cmdS = { key: "s", ctrlKey: false, metaKey: true, altKey: false, shiftKey: false };
    expect(chordOf(cmdS)).toBe("Ctrl+S");
    expect(primaryMod(cmdS)).toBe(true);
  });
});
