// Pure helpers for the live Omarchy theme: palette tokens → CSS custom properties, Hyprland
// rounding → corner radii, font family quoting. No DOM access, so they are unit tested.

/** The radius custom properties a rounding value controls (all in px). */
export interface RadiusVars {
  "--radius-xs": string;
  "--radius-sm": string;
  "--radius": string;
  "--radius-lg": string;
  "--radius-xl": string;
}

const MAX_RADIUS = 28;

/**
 * Corner radii for Hyprland's `decoration:rounding`. 0 gives the crisp Omarchy look: square panels,
 * buttons and windows, with at most 2px on the smallest marks inside a row. Larger values scale
 * the steps from small marks to dialogs from the compositor's own radius.
 */
export function radiusVars(rounding: number): RadiusVars {
  const r = Math.max(0, Math.min(MAX_RADIUS, Math.round(Number.isFinite(rounding) ? rounding : 0)));
  if (r === 0) return { "--radius-xs": "0px", "--radius-sm": "2px", "--radius": "0px", "--radius-lg": "0px", "--radius-xl": "0px" };
  const px = (n: number) => `${Math.min(MAX_RADIUS, Math.max(2, Math.round(n)))}px`;
  return { "--radius-xs": px(r * 0.35), "--radius-sm": px(r * 0.6), "--radius": px(r), "--radius-lg": px(r * 1.25), "--radius-xl": px(r * 1.6) };
}

const TOKEN_NAME = /^--[a-z0-9][a-z0-9-]*$/;
// A token value is a single colour (or similar) expression: no declaration or rule breaks.
const UNSAFE_VALUE = /[;{}<>\\]|url\s*\(|expression\s*\(|@import/i;

/** Palette tokens as `--name → value`, dropping malformed names and values. Accepts keys with or without `--`. */
export function paletteCssVars(tokens: Record<string, string> | null | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(tokens ?? {})) {
    const name = k.startsWith("--") ? k : `--${k}`;
    const value = String(v).trim();
    if (TOKEN_NAME.test(name) && value && !UNSAFE_VALUE.test(value)) out[name] = value;
  }
  return out;
}

type Rgb = { r: number; g: number; b: number };
function hexRgb(value: string | undefined): Rgb | null {
  const s = value?.trim().match(/^#([\da-f]{3}|[\da-f]{6})$/i)?.[1];
  if (!s) return null;
  const h = s.length === 3 ? [...s].map((c) => c + c).join("") : s;
  return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16) };
}
const luminance = ({ r, g, b }: Rgb) => {
  const linear = (c: number) => { const x = c / 255; return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
};
export function contrastRatio(a: string, b: string): number {
  const ca = hexRgb(a); const cb = hexRgb(b);
  if (!ca || !cb) return 1;
  const l1 = luminance(ca); const l2 = luminance(cb);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}
/** Pick the more readable black or white foreground for a known solid hex colour. */
export function contrastingText(background: string): string {
  return contrastRatio("#000000", background) >= contrastRatio("#ffffff", background) ? "#000000" : "#ffffff";
}
const hex = ({ r, g, b }: Rgb) => `#${[r, g, b].map((n) => Math.round(n).toString(16).padStart(2, "0")).join("")}`;
function mix(a: Rgb, b: Rgb, t: number): Rgb { return { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t }; }
export function withContrast(color: string, against: string | string[], minimum = 3): string {
  const surfaces = typeof against === "string" ? [against] : against;
  const meets = (candidate: string) => surfaces.every((bg) => contrastRatio(candidate, bg) >= minimum);
  if (meets(color)) return color;
  const c = hexRgb(color);
  if (!c || surfaces.some((bg) => !hexRgb(bg))) return color;
  const ends: Rgb[] = [{ r: 0, g: 0, b: 0 }, { r: 255, g: 255, b: 255 }];
  const possible = ends.filter((end) => meets(hex(end)));
  const end = possible.sort((x, y) => {
    const amount = (e: Rgb) => { let lo = 0; let hi = 1; for (let i = 0; i < 18; i++) { const mid = (lo + hi) / 2; if (meets(hex(mix(c, e, mid)))) hi = mid; else lo = mid; } return hi; };
    return amount(x) - amount(y);
  })[0];
  if (!end) {
    // Several surfaces can exclude both endpoints while allowing a middle luminance.
    // Search both paths and keep the smallest change; impossible combinations stay unchanged.
    let best: { value: string; amount: number } | null = null;
    for (const candidate of ends) {
      for (let step = 1; step <= 512; step++) {
        const amount = step / 512, value = hex(mix(c, candidate, amount));
        if (meets(value)) { if (!best || amount < best.amount) best = { value, amount }; break; }
      }
    }
    return best?.value ?? color;
  }
  let lo = 0; let hi = 1;
  for (let i = 0; i < 18; i++) { const mid = (lo + hi) / 2; if (meets(hex(mix(c, end, mid)))) hi = mid; else lo = mid; }
  return hex(mix(c, end, hi));
}

/** Keep imported foregrounds readable across the adjacent Omarchy surfaces. */
export function omarchyTextVars(tokens: Record<string, string>): Record<string, string> {
  const surfaces = ["--bg", "--bg-2", "--bg-3", "--bg-hover"]
    .map((name) => tokens[name])
    .filter((color) => hexRgb(color));
  return Object.fromEntries(["--fg", "--fg-dim", "--fg-faint"]
    .filter((name) => tokens[name])
    .map((name) => [name, withContrast(tokens[name], surfaces, 4.5)]));
}

/** Control colors derived from Omarchy's live palette, with 3:1 non-text contrast. */
export function omarchyControlVars(tokens: Record<string, string>): Record<string, string> {
  const bg = tokens["--bg-2"] ?? "#222222";
  const raised = tokens["--bg-3"] ?? bg;
  const fg = tokens["--fg"] ?? "#ffffff";
  const accent = withContrast(tokens["--accent"] ?? fg, bg);
  const track = withContrast(raised, bg);
  const border = withContrast(tokens["--border"] ?? fg, bg);
  const offKnob = withContrast(fg, track);
  const onKnob = ["#000000", "#ffffff"].sort((a, b) => contrastRatio(b, accent) - contrastRatio(a, accent))[0];
  const primaryText = onKnob;
  const danger = withContrast(tokens["--danger"] ?? fg, bg);
  const dangerText = ["#000000", "#ffffff"].sort((a, b) => contrastRatio(b, danger) - contrastRatio(a, danger))[0];
  const hover = withContrast(tokens["--bg-hover"] ?? track, bg);
  const warn = withContrast(tokens["--warn"] ?? fg, track, 4.5);
  return {
    "--om-control-quiet-text": withContrast(fg, bg, 4.5),
    "--om-control-bg": hexRgb(track) ? track : bg,
    "--om-control-border": border,
    "--om-control-hover": hover,
    "--om-control-hover-text": withContrast(fg, hover, 4.5),
    "--om-control-warn-text": warn,
    "--om-control-accent": accent,
    "--om-control-danger": danger,
    "--om-control-danger-text": dangerText,
    "--om-control-text": withContrast(fg, track, 4.5),
    "--om-control-off-knob": offKnob,
    "--om-control-on-knob": onKnob,
    "--om-control-primary-text": primaryText,
  };
}

/** `dark` / `light` from Omarchy's mode; anything else counts as dark (Omarchy's default). */
export function omarchyMode(themeMode: string | null | undefined): "dark" | "light" {
  return themeMode === "light" ? "light" : "dark";
}

/** A font family for CSS: a single name with spaces is quoted ("Segoe UI"); a list stays as written. */
export function cssFontFamily(family: string): string {
  const fam = family.trim();
  return fam && !/[,"']/.test(fam) ? `"${fam}"` : fam;
}

/** The `--font` value for a family, with the generic fallbacks. Empty when there is no family. */
export function fontStack(family: string): string {
  const css = cssFontFamily(family);
  return css ? `${css}, system-ui, sans-serif` : "";
}

/** The font Omarchy's monospace choice gives, only when allowed and the user picked no font. */
export function omarchyFontStack(opts: { follow: boolean; userFont: string; monospace: string | null | undefined }): string {
  if (!opts.follow || opts.userFont.trim() || !opts.monospace?.trim()) return "";
  return `${cssFontFamily(opts.monospace)}, monospace`;
}

/** Custom CSS from the Omarchy theme: kept only when non-blank. */
export function omarchyCustomCss(css: string | null | undefined): string {
  return css && css.trim() ? css : "";
}

/** Whether the Omarchy theme drives the UI: chosen in settings and Omarchy was detected. */
export function omarchyThemeOn(theme: string | undefined, omarchyActive: boolean | undefined): boolean {
  return theme === "omarchy" && !!omarchyActive;
}

/** Whether the Glassy look applies (translucent surfaces, sharp text). */
export function omarchyGlassyOn(themeOn: boolean, settingOn: boolean | undefined, stateGlassy: boolean | undefined): boolean {
  return themeOn && !!settingOn && !!stateGlassy;
}
