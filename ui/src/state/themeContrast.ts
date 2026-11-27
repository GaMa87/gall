import { api } from "../api/ipc";
import { contrastRatio } from "../core/omarchyTheme";
import {
  analyzeCustomStylesheet,
  compositeOver,
  contrastReportEqual,
  extractBackgroundSurfaces,
  GRAPH_COLOURS,
  parseCssColor,
  readableForeground,
  readableSyntax,
  readableText,
  resolveNativeWindowBackground,
  rgbaToHex,
  SYNTAX_TONES,
  TEXT_TONES,
  type Rgba,
  type ThemeContrastReport,
} from "../core/themeContrast";
import { getState, setState } from "./store";

export const READABLE_KEYS = [
  ...SYNTAX_TONES.map((tone) => `--readable-sx-${tone}`),
  ...TEXT_TONES.map((tone) => `--readable-${tone}`),
  ...GRAPH_COLOURS.map((value) => `--readable-graph-${value}`),
  "--readable-primary-fg",
  "--readable-danger-fg",
  "--readable-btn-fg",
  "--readable-btn-hover-fg",
  "--readable-sel-fg",
  "--readable-sel-dim",
  "--readable-mark-fg",
  "--readable-focus",
] as const;

const BLACK: Rgba = { r: 0, g: 0, b: 0, a: 1 };
const WHITE: Rgba = { r: 255, g: 255, b: 255, a: 1 };

function readCanvasRgba(context: CanvasRenderingContext2D, resolved: string): Rgba | null {
  // Invalid canvas colours leave the old fillStyle in place. Two sentinels distinguish that from a valid colour.
  context.fillStyle = "#010203";
  context.fillStyle = resolved;
  const first = context.fillStyle;
  context.fillStyle = "#040506";
  context.fillStyle = resolved;
  if (first !== context.fillStyle) return null;
  context.clearRect(0, 0, 1, 1);
  context.fillStyle = resolved;
  context.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
  return { r, g, b, a: a / 255 };
}

/** Recompute derived foreground colours from actual CSS after every palette or custom stylesheet change. */
export function watchThemeContrast(): () => void {
  const root = document.documentElement;
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return () => {};
  canvas.width = canvas.height = 1;
  let frame = 0;
  let lastNativeBg = "";

  const update = () => {
    frame = 0;
    // Temporarily clear our own derived inline properties so probing reads the pure theme tokens
    // and never feeds a previous theme's --readable-* values back into the next calculation.
    for (const key of READABLE_KEYS) {
      if (root.style.getPropertyValue(key)) root.style.removeProperty(key);
    }

    const sample = document.createElement("span");
    sample.style.cssText = "position:fixed;visibility:hidden;pointer-events:none";
    document.body.append(sample);

    const resolveColorString = (cssColor: string): Rgba | null => {
      const direct = parseCssColor(cssColor);
      if (direct) return direct;
      sample.style.color = "";
      sample.style.color = cssColor;
      const resolved = getComputedStyle(sample).color;
      return parseCssColor(resolved) ?? readCanvasRgba(context, resolved);
    };

    const readColor = (cssExpr: string): Rgba | null => {
      sample.style.color = "";
      sample.style.color = cssExpr;
      const resolved = getComputedStyle(sample).color;
      if (!resolved) return null;
      return parseCssColor(resolved) ?? readCanvasRgba(context, resolved);
    };

    const readBackground = (varName: string, backdrops: (string | Rgba)[]) => {
      sample.style.background = "none";
      sample.style.backgroundColor = "transparent";
      sample.style.background = `var(${varName})`;
      const cs = getComputedStyle(sample);
      return extractBackgroundSurfaces(cs.backgroundColor, cs.backgroundImage, backdrops, resolveColorString);
    };

    // Detect whether a custom stylesheet directly overrides --primary-fg or --danger-fg instead of
    // inheriting --readable-primary-fg / --readable-danger-fg.
    const isDirectOverride = (fgVar: string, readableVar: string): boolean => {
      root.style.setProperty(readableVar, "#010203");
      sample.style.color = "";
      sample.style.color = `var(${fgVar})`;
      const probed = resolveColorString(getComputedStyle(sample).color);
      root.style.removeProperty(readableVar);
      return !probed || Math.round(probed.r) !== 1 || Math.round(probed.g) !== 2 || Math.round(probed.b) !== 3;
    };

    const rootBackdrop = root.dataset.theme === "light" ? WHITE : BLACK;
    const bgRaw = readColor("var(--bg)");
    const bg2Raw = readColor("var(--bg-2)");
    const bg3Raw = readColor("var(--bg-3)");
    const bgHoverRaw = readColor("var(--bg-hover)");
    const accentRaw = readColor("var(--accent)");
    const markFgRaw = readColor("var(--mark-fg)");
    const primaryFgRaw = readColor("var(--primary-fg)");
    const dangerFgRaw = readColor("var(--danger-fg)");

    const translucentWindow = (!!bgRaw && bgRaw.a < 0.995) || root.classList.contains("omarchy-glassy");
    const bgRgba = bgRaw ? compositeOver(rootBackdrop, bgRaw) : null;
    const nativeBg = resolveNativeWindowBackground(bgRgba, translucentWindow);
    if (nativeBg) {
      const key = `${nativeBg.r},${nativeBg.g},${nativeBg.b},${nativeBg.a}`;
      if (key !== lastNativeBg) {
        lastNativeBg = key;
        void api.setWindowBackgroundColor(nativeBg.r, nativeBg.g, nativeBg.b, nativeBg.a).catch(() => {});
      }
    }
    const panelRgba = bgRgba && bg2Raw ? compositeOver(bgRgba, bg2Raw) : null;
    const raisedRgba = panelRgba && bg3Raw ? compositeOver(panelRgba, bg3Raw) : null;
    const bgHoverRgba = panelRgba && bgHoverRaw ? compositeOver(panelRgba, bgHoverRaw) : null;

    const tones = panelRgba
      ? Object.fromEntries(
        SYNTAX_TONES.map((tone): [string, Rgba | null] => [tone, readColor(`var(--sx-${tone})`)]).filter((pair): pair is [string, Rgba] => pair[1] !== null),
      )
      : null;
    const text = panelRgba
      ? Object.fromEntries(
        TEXT_TONES.map((tone): [string, Rgba | null] => [tone, readColor(`var(--${tone})`)]).filter((pair): pair is [string, Rgba] => pair[1] !== null),
      )
      : null;

    const imageSurfaces: string[] = [];
    const mixedGradientSurfaces: string[] = [];
    const customForegroundVars: string[] = [];

    const values: Record<string, string> = {};
    if (bgRgba && panelRgba && raisedRgba && accentRaw) {
      const panelBackdrops: Rgba[] = [panelRgba];
      const generalBackdrops: Rgba[] = [panelRgba, bgRgba];

      const primarySpec = readBackground("--primary-gradient", panelBackdrops);
      const dangerSpec = readBackground("--danger-bg", panelBackdrops);
      const btnSpec = readBackground("--btn-bg", panelBackdrops);
      const btnHoverSpec = readBackground("--btn-bg-hover", panelBackdrops);
      const selSpec = readBackground("--sel-gradient", panelBackdrops);
      const markSpec = readBackground("--mark", panelBackdrops);
      const panelBgSpec = readBackground("--panel-bg", generalBackdrops);
      const barBgSpec = readBackground("--bar-bg", generalBackdrops);
      const footBgSpec = readBackground("--foot-bg", generalBackdrops);
      const wellBgSpec = readBackground("--well-bg", panelBackdrops);
      const hoverSpec = readBackground("--hover", panelBackdrops);
      const rowHoverSpec = readBackground("--row-hover", panelBackdrops);

      const inspectedSpecs: [string, ReturnType<typeof readBackground>][] = [
        ["--primary-gradient", primarySpec],
        ["--danger-bg", dangerSpec],
        ["--btn-bg", btnSpec],
        ["--btn-bg-hover", btnHoverSpec],
        ["--sel-gradient", selSpec],
        ["--mark", markSpec],
        ["--panel-bg", panelBgSpec],
        ["--bar-bg", barBgSpec],
        ["--foot-bg", footBgSpec],
        ["--well-bg", wellBgSpec],
      ];
      for (const [name, spec] of inspectedSpecs) {
        if (spec.hasImage) imageSurfaces.push(name);
      }

      const extraGeneral = [
        ...(bgHoverRgba ? [rgbaToHex(bgHoverRgba)] : []),
        ...panelBgSpec.surfaces,
        ...barBgSpec.surfaces,
        ...footBgSpec.surfaces,
        ...wellBgSpec.surfaces,
        ...hoverSpec.surfaces,
        ...rowHoverSpec.surfaces,
      ];

      if (tones && text?.danger && text.ok) {
        Object.assign(values, readableSyntax(tones, [panelRgba, bgRgba, raisedRgba], [text.danger, text.ok], accentRaw));
      }
      if (text?.fg && text.danger) {
        Object.assign(values, readableText(text, [panelRgba, bgRgba, raisedRgba], accentRaw, {
          primary: primarySpec.surfaces,
          primaryFg: primaryFgRaw ?? text.fg,
          danger: dangerSpec.surfaces,
          dangerFg: dangerFgRaw ?? text.fg,
          button: btnSpec.surfaces,
          buttonHover: btnHoverSpec.surfaces,
          selected: selSpec.surfaces,
          mark: markSpec.surfaces,
          markFg: markFgRaw ?? text.fg,
          extraGeneral,
        }));

        const checkGradient = (name: string, surfaces: string[], preferred: Rgba) => {
          if (surfaces.length > 1 && !readableForeground(preferred, surfaces, 4.5).meetsAll) {
            mixedGradientSurfaces.push(name);
          }
        };
        checkGradient("--primary-gradient", primarySpec.surfaces, primaryFgRaw ?? text.fg);
        checkGradient("--danger-bg", dangerSpec.surfaces, dangerFgRaw ?? text.fg);
        checkGradient("--btn-bg", btnSpec.surfaces, text.fg);
        checkGradient("--btn-bg-hover", btnHoverSpec.surfaces, text.fg);
        checkGradient("--sel-gradient", selSpec.surfaces, text.fg);
        checkGradient("--mark", markSpec.surfaces, markFgRaw ?? text.fg);

        const checkDirectFg = (fgVar: string, readableVar: string, fgRaw: Rgba | null, surfaces: string[]) => {
          if (!fgRaw || !surfaces.length) return;
          if (!isDirectOverride(fgVar, readableVar)) return;
          const bad = surfaces.some((surface) => {
            const base = parseCssColor(surface);
            const eff = base && fgRaw.a < 0.995 ? rgbaToHex(compositeOver({ ...base, a: 1 }, fgRaw)) : rgbaToHex(fgRaw);
            return contrastRatio(eff, surface) < 4.5;
          });
          if (bad) customForegroundVars.push(fgVar);
        };
        checkDirectFg("--primary-fg", "--readable-primary-fg", primaryFgRaw, primarySpec.surfaces);
        checkDirectFg("--danger-fg", "--readable-danger-fg", dangerFgRaw, dangerSpec.surfaces);
      }
    }

    sample.remove();

    for (const key of READABLE_KEYS) {
      const value = values[key];
      if (value) root.style.setProperty(key, value);
    }

    const cssText = [document.getElementById("user-theme")?.textContent ?? "", document.getElementById("omarchy-custom")?.textContent ?? ""]
      .filter(Boolean)
      .join("\n");
    const analysis = cssText ? analyzeCustomStylesheet(cssText) : { hasElementColorRules: false, hasImageRules: false, hasOpacityRules: false };
    const report: ThemeContrastReport = {
      imageSurfaces,
      mixedGradientSurfaces,
      customForegroundVars,
      hasCustomCssRules: analysis.hasElementColorRules || (analysis.hasImageRules && imageSurfaces.length === 0),
      hasOpacityRules: analysis.hasOpacityRules,
      translucentWindow,
    };
    if (!contrastReportEqual(getState().themeContrastReport, report)) {
      setState({ themeContrastReport: report });
    }

    // Discard mutations caused by updating our own --readable-* properties so the observer cannot loop.
    observer.takeRecords();
  };

  const ignoredLayoutProps = /^--(sp-\d+|hair|pane-frame|line-h|fp-head-h|font-size|row-pad|anim)$/;
  const readableKeySet = new Set<string>(READABLE_KEYS);
  const themeSignature = () => {
    const parts: string[] = [root.dataset.theme ?? "", root.className];
    for (let i = 0; i < root.style.length; i++) {
      const prop = root.style[i];
      if (readableKeySet.has(prop) || ignoredLayoutProps.test(prop)) continue;
      parts.push(`${prop}=${root.style.getPropertyValue(prop)}`);
    }
    return parts.join(";");
  };
  let lastRootSig = themeSignature();

  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(update);
  };
  const onMutation = (records: MutationRecord[]) => {
    const headChanged = records.some((r) => r.target !== root);
    const sig = themeSignature();
    if (!headChanged && sig === lastRootSig) return;
    lastRootSig = sig;
    schedule();
  };
  const observer = new MutationObserver(onMutation);
  observer.observe(root, { attributes: true, attributeFilter: ["style", "class", "data-theme"] });
  observer.observe(document.head, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["media", "disabled", "href"] });
  schedule();

  return () => {
    observer.disconnect();
    if (frame) cancelAnimationFrame(frame);
    for (const key of READABLE_KEYS) root.style.removeProperty(key);
    if (getState().themeContrastReport !== null) setState({ themeContrastReport: null });
  };
}
