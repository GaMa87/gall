import { describe, expect, it } from "vitest";
import { contrastRatio, withContrast } from "./omarchyTheme";
import {
  analyzeCustomStylesheet,
  compositeOver,
  extractBackgroundSurfaces,
  mixSrgb,
  overlay,
  parseCssColor,
  readableForeground,
  readableSyntax,
  readableText,
  resolveNativeWindowBackground,
  rgbaToHex,
} from "./themeContrast";

describe("theme colour contrast", () => {
  it("finds a middle tone when neither black nor white works on both surfaces", () => {
    const color = withContrast("#555555", ["#000000", "#ffffff"], 4.5);
    expect(contrastRatio(color, "#000000")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(color, "#ffffff")).toBeGreaterThanOrEqual(4.5);
  });

  it("keeps syntax legible on light and dark merge and diff surfaces", () => {
    for (const panel of ["#ffffff", "#1a1b26"]) {
      const danger = panel === "#ffffff" ? "#d03030" : "#f2706f";
      const ok = panel === "#ffffff" ? "#228b50" : "#5fd08f";
      const colours = readableSyntax({ comment: "#8a919c" }, [panel], [danger, ok], panel === "#ffffff" ? "#000000" : "#eeeeee");
      const selected = overlay(panel, [255, 206, 87], 0.16);
      const surfaces = [panel, overlay(panel, [70, 130, 255], 0.18), overlay(selected, [60, 180, 110], 0.18)];
      for (const surface of surfaces) expect(contrastRatio(colours["--readable-sx-comment"], surface)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("changes text tones without changing the theme's input palette", () => {
    const colors = { fg: "#777777", "fg-dim": "#999999", "fg-faint": "#aaaaaa", "accent-text": "#eeeeee", danger: "#f2706f", warn: "#eab54e", ok: "#5fd08f" };
    const original = { ...colors };
    const result = readableText(colors, ["#ffffff"], "#eeeeee");
    expect(colors).toEqual(original);
    expect(contrastRatio(result["--readable-fg"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    const hover = readableText(colors, ["#ffffff"], "#000000");
    expect(contrastRatio(hover["--readable-fg"], overlay("#ffffff", [0, 0, 0], 0.22))).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-accent-text"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
  });

  it("recalculates primary, danger, button, hover, selection and focus colours for custom solid fills", () => {
    const darkColors = { fg: "#e7e9ee", "fg-dim": "#9aa1ae", "fg-faint": "#808897", "accent-text": "#8bb8ff", danger: "#f2706f", warn: "#eab54e", ok: "#5fd08f" };
    // Dark theme whose custom stylesheet sets a white --primary-gradient, white --danger-bg,
    // white --btn-bg / --btn-bg-hover / --sel-gradient, and a very dark low-contrast accent.
    const result = readableText(darkColors, ["#15181e", "#0d0f13", "#1d2129"], "#1a1d24", {
      primary: ["#ffffff"],
      danger: ["#ffffff"],
      button: ["#f5f7fa"],
      buttonHover: ["#e6f0ff"],
      selected: ["#ffffff"],
      mark: ["#111111"],
      markFg: "#222222",
    });
    expect(contrastRatio(result["--readable-primary-fg"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-danger-fg"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-btn-fg"], "#f5f7fa")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-btn-hover-fg"], "#e6f0ff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-sel-fg"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-sel-dim"], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-mark-fg"], "#111111")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(result["--readable-focus"], "#15181e")).toBeGreaterThanOrEqual(3);
  });

  it("extracts solid, alpha, color(srgb) and multi-stop gradient surfaces accurately", () => {
    const solid = extractBackgroundSurfaces("rgb(255, 255, 255)", "none", ["#15181e"]);
    expect(solid.surfaces).toEqual(["#ffffff"]);
    expect(solid.hasImage).toBe(false);

    const alpha = extractBackgroundSurfaces("rgba(255, 255, 255, 0.8)", "none", ["#000000"]);
    expect(alpha.surfaces).toEqual(["#cccccc"]);
    expect(alpha.hasTransparency).toBe(true);

    const grad = extractBackgroundSurfaces(
      "rgba(0, 0, 0, 0)",
      "linear-gradient(135deg, color(srgb 1 1 1), color(srgb 0.8 0.9 1 / 0.5) 100%)",
      ["#101010"],
    );
    expect(grad.hasImage).toBe(false);
    expect(grad.hasTransparency).toBe(true);
    expect(grad.surfaces).toContain("#ffffff");
    // Second stop is 50% of (204, 230, 255) over (16, 16, 16) => (110, 123, 136) => #6e7b88.
    expect(grad.surfaces).toContain("#6e7b88");

    const image = extractBackgroundSurfaces("rgba(0, 0, 0, 0)", 'url("bg.png"), linear-gradient(90deg, #111111, #222222)', ["#000000"]);
    expect(image.hasImage).toBe(true);
    expect(image.surfaces.length).toBeGreaterThan(0);
  });

  it("composites semitransparent status overlays in true RGBA without premature flattening", () => {
    const panel = { r: 0, g: 0, b: 0, a: 1 };
    const semiWhiteDanger = { r: 255, g: 255, b: 255, a: 0.5 };
    // True two-layer CSS compositing: 13% over transparent (alpha 0.065), then 38% over transparent (alpha 0.19).
    const row = compositeOver(panel, mixSrgb(semiWhiteDanger, 0.13, { r: 0, g: 0, b: 0, a: 0 }));
    const word = compositeOver(row, mixSrgb(semiWhiteDanger, 0.38, { r: 0, g: 0, b: 0, a: 0 }));
    // Flattening semiWhiteDanger over panel first would yield ~59 instead of ~62.
    expect(Math.round(word.r)).toBe(62);
    expect(rgbaToHex(word)).toBe("#3e3e3e");

    // Verify readableSyntax respects the exact two-layer composited surface.
    const syntax = readableSyntax({ comment: "#7a7a7a" }, [panel], [semiWhiteDanger], { r: 80, g: 120, b: 255, a: 0.5 });
    expect(contrastRatio(syntax["--readable-sx-comment"], rgbaToHex(word))).toBeGreaterThanOrEqual(4.5);
  });

  it("flags opposing gradients where no single 4.5:1 foreground exists and picks the best fallback", () => {
    const opposing = extractBackgroundSurfaces("rgba(0, 0, 0, 0)", "linear-gradient(90deg, #000000, #ffffff)", ["#15181e"]);
    expect(opposing.surfaces).toContain("#000000");
    expect(opposing.surfaces).toContain("#808080");
    expect(opposing.surfaces).toContain("#ffffff");
    const fg = readableForeground("#eeeeee", opposing.surfaces, 4.5);
    expect(fg.meetsAll).toBe(false);
    expect(contrastRatio(fg.color, opposing.surfaces[0])).toBeGreaterThanOrEqual(4.5);
  });

  it("detects custom element-level colour, opacity and image rules while ignoring root variable blocks", () => {
    const clean = `:root:root[data-theme="dark"] { --bg: #111; --primary-gradient: #fff; } :root:root[data-theme="dark"] .wordmark { background-image: linear-gradient(90deg, #fff, #000); }`;
    expect(analyzeCustomStylesheet(clean)).toEqual({ hasElementColorRules: false, hasImageRules: false, hasOpacityRules: false });

    const risky = `.btn.primary { color: #ccc; opacity: 0.4; background: url("x.png"); }`;
    expect(analyzeCustomStylesheet(risky)).toEqual({ hasElementColorRules: true, hasImageRules: true, hasOpacityRules: true });
  });

  it("parses hex, rgb, rgba and color(srgb) values accurately", () => {
    expect(parseCssColor("#0f88")).toEqual({ r: 0, g: 255, b: 136, a: 0x88 / 255 });
    expect(parseCssColor("rgb(10 20 30 / 50%)")).toEqual({ r: 10, g: 20, b: 30, a: 0.5 });
    expect(parseCssColor("color(srgb 0.5 0.25 1 / 0.4)")).toEqual({ r: 127.5, g: 63.75, b: 255, a: 0.4 });
  });

  it("resolves native window background colours for opaque and translucent themes", () => {
    expect(resolveNativeWindowBackground({ r: 13.4, g: 15.6, b: 19, a: 1 }, false)).toEqual({ r: 13, g: 16, b: 19, a: 255 });
    expect(resolveNativeWindowBackground({ r: 29, g: 32, b: 33, a: 1 }, true)).toEqual({ r: 29, g: 32, b: 33, a: 0 });
    expect(resolveNativeWindowBackground(null, true)).toEqual({ r: 0, g: 0, b: 0, a: 0 });
    expect(resolveNativeWindowBackground(null, false)).toBeNull();
  });
});
