import { describe, expect, it } from "vitest";
import { contrastingText, contrastRatio, cssFontFamily, fontStack, omarchyControlVars, omarchyFontStack, omarchyGlassyOn, omarchyMode, omarchyTextVars, omarchyThemeOn, paletteCssVars, radiusVars } from "./omarchyTheme";

describe("radiusVars", () => {
  it("is crisp at 0", () => {
    const v = radiusVars(0);
    expect(v["--radius"]).toBe("0px");
    expect(v["--radius-lg"]).toBe("0px");
    expect(v["--radius-xl"]).toBe("0px");
    expect(v["--radius-sm"]).toBe("2px");
    expect(v["--radius-xs"]).toBe("0px");
  });
  it("scales from the compositor value", () => {
    expect(radiusVars(8)).toEqual({ "--radius-xs": "3px", "--radius-sm": "5px", "--radius": "8px", "--radius-lg": "10px", "--radius-xl": "13px" });
  });
  it("clamps and ignores junk", () => {
    expect(radiusVars(500)["--radius"]).toBe("28px");
    expect(radiusVars(-4)["--radius"]).toBe("0px");
    expect(radiusVars(NaN)["--radius"]).toBe("0px");
  });
  it("is monotonic", () => {
    for (let r = 1; r < 28; r++) {
      expect(parseInt(radiusVars(r + 1)["--radius"])).toBeGreaterThanOrEqual(parseInt(radiusVars(r)["--radius"]));
    }
  });
});

describe("paletteCssVars", () => {
  it("keeps valid tokens and normalizes names", () => {
    expect(paletteCssVars({ "--bg": "#1e1e2e", fg: "#cdd6f4", "--glass": "rgba(0, 0, 0, 0.8)" })).toEqual({
      "--bg": "#1e1e2e", "--fg": "#cdd6f4", "--glass": "rgba(0, 0, 0, 0.8)",
    });
  });
  it("drops names and values that could break out of a declaration", () => {
    expect(paletteCssVars({ "--bad name": "#000", "--x": "red; background: url(x)", "--y": "url(http://e/x)", "--z": "}", "--e": "" })).toEqual({});
  });
  it("handles null", () => {
    expect(paletteCssVars(null)).toEqual({});
  });
});

describe("solid-colour button text", () => {
  it.each(["#e5534b", "#e8913a", "#e2c541", "#3fb27f", "#3d8bfd", "#a371f7", "#db61a2", "#8b949e", "#101010", "#eeeeee"])('has readable text on %s', (background) => {
    expect(contrastRatio(contrastingText(background), background)).toBeGreaterThanOrEqual(4.5);
  });
});

describe("omarchy control contrast", () => {
  const palettes = [
    { "--bg-2": "#16161e", "--bg-3": "#17171f", "--bg-hover": "#181820", "--fg": "#33333b", "--border": "#181820", "--accent": "#202028" },
    { "--bg-2": "#f5f5f8", "--bg-3": "#f4f4f7", "--bg-hover": "#f0f0f3", "--fg": "#aaaab0", "--border": "#eeeeef", "--accent": "#c5c5cc" },
    { "--bg-2": "#202020", "--bg-3": "#232323", "--bg-hover": "#222222", "--fg": "#303030", "--border": "#212121", "--accent": "#252525" },
  ];
  it.each(palettes)("keeps button borders, surfaces, focus and switch states distinct for a low-contrast palette", (palette) => {
    const vars = omarchyControlVars(palette);
    expect(contrastRatio(vars["--om-control-quiet-text"], palette["--bg-2"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-border"], palette["--bg-2"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-bg"], palette["--bg-2"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-hover"], palette["--bg-2"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-accent"], palette["--bg-2"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-danger"], palette["--bg-2"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-text"], vars["--om-control-bg"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-hover-text"], vars["--om-control-hover"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-warn-text"], vars["--om-control-bg"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-off-knob"], vars["--om-control-bg"])).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(vars["--om-control-on-knob"], vars["--om-control-accent"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-primary-text"], vars["--om-control-accent"])).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(vars["--om-control-danger-text"], vars["--om-control-danger"])).toBeGreaterThanOrEqual(4.5);
  });
});

describe("Omarchy text contrast", () => {
  it("keeps readable text and adjusts quiet labels across dark and light surfaces", () => {
    for (const [base, raised, fg] of [["#222222", "#252525", "#eeeeee"], ["#eeeeee", "#f5f5f5", "#222222"]]) {
      const colors = omarchyTextVars({ "--bg": base, "--bg-3": raised, "--fg": fg, "--fg-dim": base, "--fg-faint": raised });
      expect(colors["--fg"]).toBe(fg);
      for (const color of Object.values(colors)) {
        expect(contrastRatio(color, base)).toBeGreaterThanOrEqual(4.5);
        expect(contrastRatio(color, raised)).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe("mode and gates", () => {
  it("maps modes", () => {
    expect(omarchyMode("light")).toBe("light");
    expect(omarchyMode("dark")).toBe("dark");
    expect(omarchyMode("weird")).toBe("dark");
  });
  it("needs both the setting and a detected Omarchy", () => {
    expect(omarchyThemeOn("omarchy", true)).toBe(true);
    expect(omarchyThemeOn("omarchy", false)).toBe(false);
    expect(omarchyThemeOn("dark", true)).toBe(false);
  });
  it("glassy needs theme, setting and state", () => {
    expect(omarchyGlassyOn(true, true, true)).toBe(true);
    expect(omarchyGlassyOn(false, true, true)).toBe(false);
    expect(omarchyGlassyOn(true, false, true)).toBe(false);
    expect(omarchyGlassyOn(true, true, false)).toBe(false);
  });
});

describe("fonts", () => {
  it("quotes single names, keeps lists", () => {
    expect(cssFontFamily("Segoe UI")).toBe('"Segoe UI"');
    expect(cssFontFamily('"A", B')).toBe('"A", B');
    expect(fontStack("")).toBe("");
    expect(fontStack("Fira Code")).toBe('"Fira Code", system-ui, sans-serif');
  });
  it("follows Omarchy's monospace font only when allowed and unset by the user", () => {
    expect(omarchyFontStack({ follow: true, userFont: "", monospace: "JetBrainsMono Nerd Font" })).toBe('"JetBrainsMono Nerd Font", monospace');
    expect(omarchyFontStack({ follow: false, userFont: "", monospace: "X" })).toBe("");
    expect(omarchyFontStack({ follow: true, userFont: "Inter", monospace: "X" })).toBe("");
    expect(omarchyFontStack({ follow: true, userFont: "", monospace: null })).toBe("");
  });
});
