import { describe, expect, it } from "vitest";
import { currentAction } from "./dnd";
import { setState } from "./store";
import type { Settings } from "../api/types";

const withDefault = (dragBehavior: Settings["dragBehavior"]) =>
  setState({ settings: { dragBehavior } as Settings, drives: [
    { short: "/", label: "", mount: "/", fs: "btrfs", total: 1, available: 1, removable: false, lockable: false },
    { short: "USB", label: "", mount: "/run/media/u/USB", fs: "vfat", total: 1, available: 1, removable: true, lockable: false },
  ] });
const keys = (ctrlKey = false, shiftKey = false, altKey = false) => ({ ctrlKey, shiftKey, altKey });

describe("drop action: held keys override the default only while held", () => {
  it("default move", () => {
    withDefault("consistent");
    expect(currentAction(["/a/x"], "/b", keys())).toBe("move");
    expect(currentAction(["/a/x"], "/b", keys(true))).toBe("copy");
    expect(currentAction(["/a/x"], "/b", keys(false, true))).toBe("move");
    expect(currentAction(["/a/x"], "/b", keys(true, false, true))).toBe("link");
    expect(currentAction(["/a/x"], "/b", keys(false, false, true))).toBe("link");
  });
  it("default copy, Shift moves", () => {
    withDefault("copy");
    expect(currentAction(["/a/x"], "/b", keys())).toBe("copy");
    expect(currentAction(["/a/x"], "/b", keys(false, true))).toBe("move");
  });
  it("system: copy across drives", () => {
    withDefault("system");
    expect(currentAction(["/home/x"], "/tmp", keys())).toBe("move");
    expect(currentAction(["/home/x"], "/run/media/u/USB", keys())).toBe("copy");
  });
  it("ask", () => {
    withDefault("ask");
    expect(currentAction(["/a/x"], "/b", keys())).toBe("menu");
    expect(currentAction(["/a/x"], "/b", keys(true))).toBe("copy");
    expect(currentAction(["/a/x"], "/b", keys(false, false, true))).toBe("link");
  });
});
