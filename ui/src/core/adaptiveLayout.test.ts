import { describe, expect, it } from "vitest";
import { rawAdaptiveMode, resolveAdaptiveLayout, sidebarAutoHidden, windowButtonsShown, workspaceWidth, HYSTERESIS } from "./adaptiveLayout";

describe("adaptive layout thresholds", () => {
  it("maps typical tiles", () => {
    expect(rawAdaptiveMode(1920, 1080)).toBe("horizontal"); // fullscreen
    expect(rawAdaptiveMode(960, 1080)).toBe("horizontal"); // half of a 1920 screen is still wide
    expect(rawAdaptiveMode(900, 1000)).toBe("vertical"); // tall half tile
    expect(rawAdaptiveMode(700, 900)).toBe("vertical");
    expect(rawAdaptiveMode(900, 500)).toBe("single"); // quarter tile
    expect(rawAdaptiveMode(900, 700)).toBe("single"); // not tall: h < w * 0.85 is false at 765 -> single
    expect(rawAdaptiveMode(400, 1000)).toBe("single"); // very narrow
    expect(rawAdaptiveMode(479, 2000)).toBe("single");
    expect(rawAdaptiveMode(480, 620)).toBe("vertical");
  });

  it("is a pure function of size and previous mode", () => {
    expect(resolveAdaptiveLayout(1200, 800, null)).toBe("horizontal");
    expect(resolveAdaptiveLayout(1200, 800, "single")).toBe("horizontal");
    expect(resolveAdaptiveLayout(800, 900, null)).toBe("vertical");
    expect(resolveAdaptiveLayout(800, 500, null)).toBe("single");
  });

  it("does not flicker around 920 px", () => {
    // Growing from single / vertical: stays until clearly past the band.
    expect(resolveAdaptiveLayout(920, 500, "single")).toBe("single");
    expect(resolveAdaptiveLayout(920 + HYSTERESIS - 1, 500, "single")).toBe("single");
    expect(resolveAdaptiveLayout(920 + HYSTERESIS, 500, "single")).toBe("horizontal");
    expect(resolveAdaptiveLayout(940, 900, "vertical")).toBe("vertical");
    expect(resolveAdaptiveLayout(920 + HYSTERESIS, 900, "vertical")).toBe("horizontal");
    // Shrinking from horizontal: stays until clearly below.
    expect(resolveAdaptiveLayout(910, 900, "horizontal")).toBe("horizontal");
    expect(resolveAdaptiveLayout(920 - HYSTERESIS, 900, "horizontal")).toBe("horizontal");
    expect(resolveAdaptiveLayout(920 - HYSTERESIS - 1, 900, "horizontal")).toBe("vertical");
    expect(resolveAdaptiveLayout(920 - HYSTERESIS - 1, 500, "horizontal")).toBe("single");
  });

  it("does not flicker around the height and width limits", () => {
    expect(resolveAdaptiveLayout(700, 620 - 10, "vertical")).toBe("vertical");
    expect(resolveAdaptiveLayout(700, 620 - HYSTERESIS - 1, "vertical")).toBe("single");
    expect(resolveAdaptiveLayout(700, 620 + 10, "single")).toBe("single");
    expect(resolveAdaptiveLayout(700, 620 + HYSTERESIS, "single")).toBe("vertical");
    expect(resolveAdaptiveLayout(480 + 10, 1000, "single")).toBe("single");
    expect(resolveAdaptiveLayout(480 + HYSTERESIS, 1000, "single")).toBe("vertical");
    expect(resolveAdaptiveLayout(480 - 10, 1000, "vertical")).toBe("vertical");
    expect(resolveAdaptiveLayout(480 - HYSTERESIS - 1, 1000, "vertical")).toBe("single");
  });

  it("is deterministic when swept in both directions", () => {
    let m = resolveAdaptiveLayout(300, 900, null);
    const up: string[] = [];
    for (let w = 300; w <= 1400; w += 4) { m = resolveAdaptiveLayout(w, 900, m); if (up.at(-1) !== m) up.push(m); }
    expect(up).toEqual(["single", "vertical", "horizontal"]);
    const down: string[] = [];
    for (let w = 1400; w >= 300; w -= 4) { m = resolveAdaptiveLayout(w, 900, m); if (down.at(-1) !== m) down.push(m); }
    expect(down).toEqual(["horizontal", "vertical", "single"]);
  });

  it("hides the sidebar below 820 px with hysteresis", () => {
    expect(sidebarAutoHidden(1000, false)).toBe(false);
    expect(sidebarAutoHidden(800, false)).toBe(false); // inside the band: stays shown
    expect(sidebarAutoHidden(820 - HYSTERESIS - 1, false)).toBe(true);
    expect(sidebarAutoHidden(830, true)).toBe(true); // inside the band: stays hidden
    expect(sidebarAutoHidden(820 + HYSTERESIS, true)).toBe(false);
  });

  it("decides the window buttons from the setting and the window manager", () => {
    expect(windowButtonsShown("auto", false)).toBe(true);
    expect(windowButtonsShown("auto", true)).toBe(false); // tiling WM (tiled workspace)
    expect(windowButtonsShown("auto", true, true)).toBe(true); // Omarchy floating workspace
    expect(windowButtonsShown(undefined, true)).toBe(false);
    expect(windowButtonsShown(undefined, true, true)).toBe(true);
    expect(windowButtonsShown("always", true)).toBe(true);
    expect(windowButtonsShown("never", false)).toBe(false);
    expect(windowButtonsShown("never", true, true)).toBe(false);
  });
});

describe("adaptive layout against the workspace width", () => {
  const SB = 230;
  const ws = (w: number, open: boolean) => workspaceWidth(w, open, SB);

  it("subtracts the sidebar only while it is shown", () => {
    expect(ws(1000, true)).toBe(770);
    expect(ws(1000, false)).toBe(1000);
    expect(workspaceWidth(100, true, 230)).toBe(0);
  });

  it("does not put two panes beside an open sidebar in a ~900 px window", () => {
    // 900 px with the sidebar: 670 px for the panes, ~335 each.
    expect(rawAdaptiveMode(900, 500, ws(900, true))).toBe("single");
    expect(rawAdaptiveMode(900, 1000, ws(900, true))).toBe("vertical");
    // The same window without the sidebar keeps the old behaviour.
    expect(rawAdaptiveMode(960, 1000, ws(960, false))).toBe("horizontal");
  });

  it("switches at the same pane width with and without the sidebar, with hysteresis", () => {
    const at = (w: number, open: boolean, prev: "horizontal" | "single" | null) =>
      resolveAdaptiveLayout(w, 500, prev, { workspaceW: ws(w, open) });
    // Open sidebar: the threshold moves right by the sidebar width (920 + 230 = 1150).
    expect(at(1149, true, null)).toBe("single");
    expect(at(1150, true, null)).toBe("horizontal");
    expect(at(1150 + HYSTERESIS - 1, true, "single")).toBe("single");
    expect(at(1150 + HYSTERESIS, true, "single")).toBe("horizontal");
    expect(at(1150 - HYSTERESIS, true, "horizontal")).toBe("horizontal");
    expect(at(1150 - HYSTERESIS - 1, true, "horizontal")).toBe("single");
    // Closing the sidebar in a 1000 px window gives the panes the room back.
    expect(at(1000, true, "single")).toBe("single");
    expect(at(1000, false, "single")).toBe("horizontal");
    // And opening it takes the dual layout away again once clearly below the band.
    expect(at(1000, true, "horizontal")).toBe("single");
    expect(at(1120, true, "horizontal")).toBe("horizontal");
  });

  it("defaults to the window width when no workspace width is given", () => {
    expect(resolveAdaptiveLayout(940, 500, "single")).toBe("single");
    expect(rawAdaptiveMode(920, 500)).toBe("horizontal");
  });
});
