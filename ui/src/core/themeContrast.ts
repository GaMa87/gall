import { contrastRatio, withContrast } from "./omarchyTheme";

export const SYNTAX_TONES = ["comment", "string", "number", "keyword", "operator", "function", "type", "variable", "attr"] as const;
export const GRAPH_COLOURS = ["50bba1", "73b5d8", "8f9bb3", "b48af5", "dc779e", "e5a255"] as const;
export const TEXT_TONES = ["fg", "fg-dim", "fg-faint", "accent-text", "danger", "warn", "ok"] as const;

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

export interface SurfaceSpec {
  surfaces: string[];
  hasImage: boolean;
  hasTransparency: boolean;
}

export interface CustomControlSurfaces {
  primary?: string[];
  primaryFg?: string | Rgba;
  danger?: string[];
  dangerFg?: string | Rgba;
  button?: string[];
  buttonHover?: string[];
  selected?: string[];
  mark?: string[];
  markFg?: string | Rgba;
  extraGeneral?: string[];
}

export interface ThemeContrastReport {
  imageSurfaces: string[];
  mixedGradientSurfaces: string[];
  customForegroundVars: string[];
  hasCustomCssRules: boolean;
  hasOpacityRules: boolean;
  translucentWindow: boolean;
}

export interface StylesheetAnalysis {
  hasElementColorRules: boolean;
  hasImageRules: boolean;
  hasOpacityRules: boolean;
}

const TRANSPARENT: Rgba = { r: 0, g: 0, b: 0, a: 0 };
const clamp = (value: number, min: number, max: number) => Math.max(min, Math.min(max, value));

export function rgbaToHex(color: Rgba): string {
  const parts = [color.r, color.g, color.b].map((channel) => clamp(Math.round(channel), 0, 255));
  return `#${parts.map((part) => part.toString(16).padStart(2, "0")).join("")}`;
}

function parseChannel(token: string, max: number): number | null {
  const trimmed = token.trim().toLowerCase();
  if (trimmed === "none") return 0;
  if (trimmed.endsWith("%")) {
    const num = Number(trimmed.slice(0, -1));
    return Number.isFinite(num) ? clamp((num / 100) * max, 0, max) : null;
  }
  const num = Number(trimmed);
  return Number.isFinite(num) ? clamp(num, 0, max) : null;
}

function parseAlpha(token: string | undefined): number | null {
  if (token == null) return 1;
  const trimmed = token.trim().toLowerCase();
  if (trimmed === "none") return 0;
  if (trimmed.endsWith("%")) {
    const num = Number(trimmed.slice(0, -1));
    return Number.isFinite(num) ? clamp(num / 100, 0, 1) : null;
  }
  const num = Number(trimmed);
  return Number.isFinite(num) ? clamp(num, 0, 1) : null;
}

/** Parse hex, rgb(), rgba(), color(srgb ...), or transparent without flattening alpha. */
export function parseCssColor(value: string | null | undefined): Rgba | null {
  if (!value) return null;
  const s = value.trim().toLowerCase();
  if (!s) return null;
  if (s === "transparent") return { ...TRANSPARENT };

  const hex = s.match(/^#([\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/i)?.[1];
  if (hex) {
    if (hex.length === 3 || hex.length === 4) {
      const r = parseInt(hex[0] + hex[0], 16);
      const g = parseInt(hex[1] + hex[1], 16);
      const b = parseInt(hex[2] + hex[2], 16);
      const a = hex.length === 4 ? parseInt(hex[3] + hex[3], 16) / 255 : 1;
      return { r, g, b, a };
    }
    const r = parseInt(hex.slice(0, 2), 16);
    const g = parseInt(hex.slice(2, 4), 16);
    const b = parseInt(hex.slice(4, 6), 16);
    const a = hex.length === 8 ? parseInt(hex.slice(6, 8), 16) / 255 : 1;
    return { r, g, b, a };
  }

  const rgbMatch = s.match(/^rgba?\(([^)]+)\)$/);
  if (rgbMatch) {
    const body = rgbMatch[1].trim();
    const [rgbPart, alphaPart] = body.includes("/") ? body.split("/") : [body, undefined];
    const parts = rgbPart.includes(",")
      ? rgbPart.split(",").map((part) => part.trim()).filter(Boolean)
      : rgbPart.trim().split(/\s+/).filter(Boolean);
    if (parts.length !== 3 && parts.length !== 4) return null;
    const r = parseChannel(parts[0], 255);
    const g = parseChannel(parts[1], 255);
    const b = parseChannel(parts[2], 255);
    const a = parseAlpha(alphaPart ?? parts[3]);
    if (r == null || g == null || b == null || a == null) return null;
    return { r, g, b, a };
  }

  const srgbMatch = s.match(/^color\(\s*srgb\s+([^)]+)\)$/);
  if (srgbMatch) {
    const body = srgbMatch[1].trim();
    const [rgbPart, alphaPart] = body.includes("/") ? body.split("/") : [body, undefined];
    const parts = rgbPart.trim().split(/\s+/).filter(Boolean);
    if (parts.length !== 3) return null;
    const r = parseChannel(parts[0], 1);
    const g = parseChannel(parts[1], 1);
    const b = parseChannel(parts[2], 1);
    const a = parseAlpha(alphaPart);
    if (r == null || g == null || b == null || a == null) return null;
    return { r: r * 255, g: g * 255, b: b * 255, a };
  }

  return null;
}

export function toRgba(value: string | Rgba): Rgba | null {
  return typeof value === "string" ? parseCssColor(value) : { r: value.r, g: value.g, b: value.b, a: clamp(value.a, 0, 1) };
}

/** Porter-Duff source-over compositing in sRGB RGBA without premature flattening. */
export function compositeOver(backdrop: Rgba, layer: Rgba): Rgba {
  const la = clamp(layer.a, 0, 1);
  const ba = clamp(backdrop.a, 0, 1);
  const outA = la + ba * (1 - la);
  if (outA <= 0) return { ...TRANSPARENT };
  const inv = 1 - la;
  return {
    r: (layer.r * la + backdrop.r * ba * inv) / outA,
    g: (layer.g * la + backdrop.g * ba * inv) / outA,
    b: (layer.b * la + backdrop.b * ba * inv) / outA,
    a: outA,
  };
}

/** CSS Color 5 `color-mix(in srgb, a <weightA*100>%, b)` using premultiplied sRGB interpolation. */
export function mixSrgb(a: Rgba, weightA: number, b: Rgba): Rgba {
  const w1 = clamp(weightA, 0, 1);
  const w2 = 1 - w1;
  const outA = w1 * clamp(a.a, 0, 1) + w2 * clamp(b.a, 0, 1);
  if (outA <= 0) return { ...TRANSPARENT };
  const p1 = w1 * clamp(a.a, 0, 1);
  const p2 = w2 * clamp(b.a, 0, 1);
  return {
    r: (a.r * p1 + b.r * p2) / outA,
    g: (a.g * p1 + b.g * p2) / outA,
    b: (a.b * p1 + b.b * p2) / outA,
    a: outA,
  };
}

/** Composite an sRGB foreground over an opaque hex surface. */
export function overlay(surface: string, rgb: [number, number, number], alpha: number): string {
  const base = parseCssColor(surface);
  if (!base) return surface;
  return rgbaToHex(compositeOver({ ...base, a: 1 }, { r: rgb[0], g: rgb[1], b: rgb[2], a: alpha }));
}

function brightenHex(hex: string, factor: number): string {
  const parsed = parseCssColor(hex);
  if (!parsed) return hex;
  return rgbaToHex({
    r: clamp(parsed.r * factor, 0, 255),
    g: clamp(parsed.g * factor, 0, 255),
    b: clamp(parsed.b * factor, 0, 255),
    a: 1,
  });
}

function splitTopLevel(input: string, separator = ","): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote: string | null = null;
  let start = 0;
  for (let i = 0; i < input.length; i++) {
    const ch = input[i];
    if (quote) {
      if (ch === "\\" && i + 1 < input.length) { i++; continue; }
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === "\"" || ch === "'") { quote = ch; continue; }
    if (ch === "(") depth++;
    else if (ch === ")" && depth > 0) depth--;
    else if (ch === separator && depth === 0) {
      out.push(input.slice(start, i).trim());
      start = i + 1;
    }
  }
  const tail = input.slice(start).trim();
  if (tail) out.push(tail);
  return out;
}

const IMAGE_LAYER = /^(url|image-set|-webkit-image-set|cross-fade|-webkit-cross-fade|element|paint)\s*\(/i;
const GRADIENT_LAYER = /^(?:repeating-)?(?:linear|radial|conic)-gradient\s*\(([\s\S]*)\)$/i;
const STOP_POSITION = /^(?:[+-]?(?:\d+\.?\d*|\.\d+)(?:%|px|em|rem|vw|vh|vmin|vmax|ch|ex|cm|mm|in|pt|pc|deg|rad|grad|turn)|0|calc\([\s\S]*\))$/i;
const GRADIENT_PREAMBLE = /^(?:to\s+|from\s+|at\s+|in\s+|circle\b|ellipse\b|closest-|farthest-|[+-]?(?:\d+\.?\d*|\.\d+)(?:deg|rad|grad|turn)$)/i;

function extractStopColorToken(segment: string): string | null {
  const trimmed = segment.trim();
  if (!trimmed || GRADIENT_PREAMBLE.test(trimmed)) return null;
  // Split top-level whitespace tokens (keeping function calls intact).
  const tokens: string[] = [];
  let depth = 0;
  let start = -1;
  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i];
    if (ch === "(") { if (start < 0) start = i; depth++; }
    else if (ch === ")") { depth = Math.max(0, depth - 1); }
    else if (/\s/.test(ch) && depth === 0) {
      if (start >= 0) { tokens.push(trimmed.slice(start, i)); start = -1; }
    } else if (start < 0) {
      start = i;
    }
  }
  if (start >= 0) tokens.push(trimmed.slice(start));
  if (!tokens.length) return null;
  // Drop trailing stop positions (up to two, e.g. `10% 30%`).
  while (tokens.length > 1 && STOP_POSITION.test(tokens[tokens.length - 1])) tokens.pop();
  if (tokens.length !== 1 || STOP_POSITION.test(tokens[0])) return null;
  return tokens[0];
}

function parseGradientStops(layer: string, resolveColor?: (token: string) => Rgba | null): Rgba[] {
  const match = layer.trim().match(GRADIENT_LAYER);
  if (!match) return [];
  const rawStops: Rgba[] = [];
  for (const segment of splitTopLevel(match[1], ",")) {
    const token = extractStopColorToken(segment);
    if (!token) continue;
    const parsed = parseCssColor(token) ?? resolveColor?.(token) ?? null;
    if (parsed) rawStops.push(parsed);
  }
  if (rawStops.length <= 1) return rawStops;
  const withMidpoints: Rgba[] = [];
  for (let i = 0; i < rawStops.length; i++) {
    withMidpoints.push(rawStops[i]);
    if (i + 1 < rawStops.length) {
      withMidpoints.push(mixSrgb(rawStops[i], 0.5, rawStops[i + 1]));
    }
  }
  return withMidpoints;
}

function dedupeRgba(colors: Rgba[], limit = 24): Rgba[] {
  const seen = new Set<string>();
  const out: Rgba[] = [];
  for (const color of colors) {
    const key = `${Math.round(color.r)},${Math.round(color.g)},${Math.round(color.b)},${Math.round(color.a * 100)}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(color);
    if (out.length >= limit) break;
  }
  return out;
}

/**
 * Extract all composited surface colours produced by a CSS background (solid colour,
 * translucent layers, multi-stop gradients, or stacked background layers) over backdrops.
 * Preserves alpha across intermediate layers without flattening prematurely.
 */
export function extractBackgroundSurfaces(
  backgroundColor: string,
  backgroundImage: string,
  backdrops: (string | Rgba)[],
  resolveColor?: (token: string) => Rgba | null,
): SurfaceSpec {
  const baseList = backdrops.map(toRgba).filter((c): c is Rgba => c !== null);
  const bgColor = parseCssColor(backgroundColor) ?? resolveColor?.(backgroundColor) ?? { ...TRANSPARENT };
  const rawLayers = backgroundImage && backgroundImage.trim().toLowerCase() !== "none"
    ? splitTopLevel(backgroundImage, ",")
    : [];

  let hasImage = false;
  let hasTransparency = bgColor.a > 0 && bgColor.a < 0.995;

  // CSS paints background-color at the bottom, then background-image layers from last to first.
  const gradientLayers: Rgba[][] = [];
  for (const layer of rawLayers) {
    const trimmed = layer.trim();
    if (!trimmed || trimmed.toLowerCase() === "none") continue;
    if (IMAGE_LAYER.test(trimmed)) {
      hasImage = true;
      continue;
    }
    const stops = parseGradientStops(trimmed, resolveColor);
    if (stops.length > 0) {
      if (stops.some((stop) => stop.a < 0.995)) hasTransparency = true;
      gradientLayers.push(stops);
    } else if (trimmed.includes("(")) {
      // Unknown CSS image function (e.g. -webkit-gradient or custom paint): treat as image limit.
      hasImage = true;
    }
  }

  let current: Rgba[] = baseList.length > 0 ? baseList : [{ r: 0, g: 0, b: 0, a: 1 }];
  if (bgColor.a > 0) {
    current = dedupeRgba(current.map((base) => compositeOver(base, bgColor)));
  }

  for (let i = gradientLayers.length - 1; i >= 0; i--) {
    const stops = gradientLayers[i];
    const next: Rgba[] = [];
    for (const base of current) {
      for (const stop of stops) {
        next.push(compositeOver(base, stop));
      }
    }
    current = dedupeRgba(next);
  }

  const hexes: string[] = [];
  const seenHex = new Set<string>();
  for (const color of current) {
    const h = rgbaToHex(color);
    if (!seenHex.has(h)) {
      seenHex.add(h);
      hexes.push(h);
    }
  }
  return { surfaces: hexes, hasImage, hasTransparency };
}

/** Pick an accessible opaque foreground over one or more surfaces, reporting whether every surface met the target. */
export function readableForeground(
  preferred: string | Rgba,
  surfaces: string[],
  minimum = 4.5,
): { color: string; meetsAll: boolean } {
  const primary = surfaces[0] ?? "#000000";
  const prefRgba = toRgba(preferred);
  const baseRgba = parseCssColor(primary) ?? { r: 0, g: 0, b: 0, a: 1 };
  const seed = prefRgba
    ? rgbaToHex(prefRgba.a < 0.995 ? compositeOver({ ...baseRgba, a: 1 }, prefRgba) : prefRgba)
    : (typeof preferred === "string" ? preferred : "#ffffff");
  if (surfaces.length === 0) return { color: seed, meetsAll: true };

  const proposed = withContrast(seed, surfaces, minimum);
  if (surfaces.every((bg) => contrastRatio(proposed, bg) >= minimum)) {
    return { color: proposed, meetsAll: true };
  }

  const primarySafe = withContrast(seed, primary, minimum);
  const candidates = Array.from(new Set([proposed, primarySafe, "#000000", "#ffffff"]));
  let best = candidates[0];
  let bestMin = -1;
  let bestPrimary = -1;
  for (const candidate of candidates) {
    const minRatio = Math.min(...surfaces.map((bg) => contrastRatio(candidate, bg)));
    const primaryRatio = contrastRatio(candidate, primary);
    if (minRatio > bestMin + 1e-6 || (Math.abs(minRatio - bestMin) <= 1e-6 && primaryRatio > bestPrimary)) {
      best = candidate;
      bestMin = minRatio;
      bestPrimary = primaryRatio;
    }
  }
  return { color: best, meetsAll: bestMin >= minimum };
}

function resolveSurfaceHexes(surfaces: (string | Rgba)[]): { rgbas: Rgba[]; hexes: string[] } {
  const rgbas = surfaces.map(toRgba).filter((c): c is Rgba => c !== null);
  const hexes = rgbas.map(rgbaToHex);
  return { rgbas, hexes };
}

function dedupeStrings(values: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values) {
    const key = value.toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      out.push(value);
    }
  }
  return out;
}

/** Derived foregrounds for code on normal and merge surfaces; source theme tokens stay untouched. */
export function readableSyntax(
  colors: Record<string, string | Rgba>,
  surfaces: (string | Rgba)[],
  status: (string | Rgba)[],
  accent: string | Rgba,
): Record<string, string> {
  const { rgbas: surfaceRgbas, hexes: surfaceHexes } = resolveSurfaceHexes(surfaces);
  const primaryRgba = surfaceRgbas[0];
  const primary = surfaceHexes[0];
  if (!primaryRgba || !primary) return {};

  const selectedRgba = compositeOver(primaryRgba, { r: 255, g: 206, b: 87, a: 0.16 });
  const variants: string[] = [primary, ...surfaceHexes.slice(1), rgbaToHex(selectedRgba)];

  const accentRgba = toRgba(accent);
  if (accentRgba) {
    for (const amount of [0.08, 0.16, 0.22, 0.3]) {
      variants.push(rgbaToHex(compositeOver(primaryRgba, mixSrgb(accentRgba, amount, TRANSPARENT))));
    }
  }

  for (const base of [primaryRgba, selectedRgba]) {
    variants.push(
      rgbaToHex(compositeOver(base, { r: 150, g: 150, b: 150, a: 0.18 })),
      rgbaToHex(compositeOver(base, { r: 70, g: 130, b: 255, a: 0.18 })),
      rgbaToHex(compositeOver(base, { r: 60, g: 180, b: 110, a: 0.18 })),
    );
  }

  // Keep raw RGBA for status colours so layered diff row + inline word highlights composite accurately.
  for (const value of status) {
    const statusRgba = toRgba(value);
    if (!statusRgba) continue;
    const rowLayer = mixSrgb(statusRgba, 0.13, TRANSPARENT);
    const wordLayer = mixSrgb(statusRgba, 0.38, TRANSPARENT);
    const row = compositeOver(primaryRgba, rowLayer);
    const word = compositeOver(row, wordLayer);
    variants.push(rgbaToHex(row), rgbaToHex(word));
  }

  const uniqueVariants = dedupeStrings(variants);
  return Object.fromEntries(SYNTAX_TONES.filter((tone) => colors[tone]).map((tone) => {
    const raw = colors[tone];
    const seed = readableForeground(raw, [primary], 1).color;
    const proposed = withContrast(seed, uniqueVariants, 4.5);
    // Opposing surfaces may have no single accessible foreground; keep the normal editor readable.
    return [`--readable-sx-${tone}`, withContrast(proposed, primary, 4.5)];
  }));
}

/** Derive text colors without changing the palette colors used for fills, graphs or borders. */
export function readableText(
  colors: Record<string, string | Rgba>,
  surfaces: (string | Rgba)[],
  accent: string | Rgba,
  custom?: CustomControlSurfaces,
): Record<string, string> {
  const { rgbas: surfaceRgbas, hexes: coreSurfaces } = resolveSurfaceHexes(surfaces);
  const panelRgba = surfaceRgbas[0];
  const panel = coreSurfaces[0];
  if (!panelRgba || !panel) return {};

  const accentRgba = toRgba(accent);
  const chosenRgba = compositeOver(panelRgba, { r: 255, g: 206, b: 87, a: 0.16 });
  const chosenHoverRgba = compositeOver(chosenRgba, { r: 255, g: 206, b: 87, a: 0.35 });
  const accentOverlays = accentRgba
    ? [0.08, 0.16, 0.22, 0.28, 0.3].map((amount) => rgbaToHex(compositeOver(panelRgba, mixSrgb(accentRgba, amount, TRANSPARENT))))
    : [];
  const extraGeneral = custom?.extraGeneral ?? [];
  const result: Record<string, string> = {};

  for (const tone of TEXT_TONES) {
    if (!colors[tone]) continue;
    const seed = readableForeground(colors[tone], [panel], 1).color;
    const toneOverlays: string[] = [];
    if (["accent-text", "danger", "warn", "ok"].includes(tone)) {
      const raw = tone === "accent-text" ? accentRgba : toRgba(colors[tone]);
      if (raw) {
        for (const amount of [0.08, 0.16, 0.28]) {
          toneOverlays.push(rgbaToHex(compositeOver(panelRgba, mixSrgb(raw, amount, TRANSPARENT))));
        }
      }
    }
    const allOptions = dedupeStrings([
      ...coreSurfaces,
      ...extraGeneral,
      rgbaToHex(chosenRgba),
      rgbaToHex(chosenHoverRgba),
      ...accentOverlays,
      ...toneOverlays,
    ]);
    let adjusted = withContrast(seed, allOptions, 4.5);
    if (allOptions.some((bg) => contrastRatio(adjusted, bg) < 4.5)) {
      const baseOptions = dedupeStrings([...coreSurfaces, rgbaToHex(chosenRgba), ...accentOverlays, ...toneOverlays]);
      adjusted = withContrast(seed, baseOptions, 4.5);
      if (baseOptions.some((bg) => contrastRatio(adjusted, bg) < 4.5)) {
        adjusted = withContrast(seed, coreSurfaces, 4.5);
        if (coreSurfaces.some((bg) => contrastRatio(adjusted, bg) < 4.5)) {
          adjusted = withContrast(seed, panel, 4.5);
        }
      }
    }
    result[`--readable-${tone}`] = adjusted;
  }

  for (const hex of GRAPH_COLOURS) {
    const value = `#${hex}`;
    const rgb = parseCssColor(value)!;
    const tint = rgbaToHex(compositeOver(panelRgba, { ...rgb, a: 0.18 }));
    result[`--readable-graph-${hex}`] = withContrast(value, dedupeStrings([...coreSurfaces, tint]), 4.5);
  }

  const fg = colors.fg;
  if (fg) {
    const defaultPrimary = accentRgba
      ? [0.08, 0.16].map((amount) => rgbaToHex(compositeOver(panelRgba, mixSrgb(accentRgba, amount, TRANSPARENT))))
      : [panel];
    const primarySurfaces = custom?.primary?.length ? custom.primary : defaultPrimary;
    // Account for `filter: brightness(1.1)` on hovered primary controls when compatible with the resting stops.
    const primaryWithHover = dedupeStrings([...primarySurfaces, ...primarySurfaces.map((bg) => brightenHex(bg, 1.1))]);
    const primaryHoverCandidate = readableForeground(custom?.primaryFg ?? fg, primaryWithHover, 4.5);
    result["--readable-primary-fg"] = primaryHoverCandidate.meetsAll
      ? primaryHoverCandidate.color
      : readableForeground(custom?.primaryFg ?? fg, primarySurfaces, 4.5).color;

    const dangerRgba = colors.danger ? toRgba(colors.danger) : null;
    const defaultDanger = dangerRgba
      ? [rgbaToHex(compositeOver(panelRgba, mixSrgb(dangerRgba, 0.16, TRANSPARENT)))]
      : [panel];
    const dangerSurfaces = custom?.danger?.length ? custom.danger : defaultDanger;
    const dangerWithHover = dedupeStrings([...dangerSurfaces, ...dangerSurfaces.map((bg) => brightenHex(bg, 1.1))]);
    const dangerHoverCandidate = readableForeground(custom?.dangerFg ?? fg, dangerWithHover, 4.5);
    result["--readable-danger-fg"] = dangerHoverCandidate.meetsAll
      ? dangerHoverCandidate.color
      : readableForeground(custom?.dangerFg ?? fg, dangerSurfaces, 4.5).color;

    const buttonSurfaces = custom?.button?.length ? custom.button : coreSurfaces;
    result["--readable-btn-fg"] = readableForeground(fg, buttonSurfaces, 4.5).color;

    const defaultBtnHover = accentRgba
      ? [0.1, 0.22].map((amount) => rgbaToHex(compositeOver(panelRgba, mixSrgb(accentRgba, amount, TRANSPARENT))))
      : coreSurfaces;
    const buttonHoverSurfaces = custom?.buttonHover?.length ? custom.buttonHover : defaultBtnHover;
    result["--readable-btn-hover-fg"] = readableForeground(fg, buttonHoverSurfaces, 4.5).color;

    const defaultSelected = accentRgba
      ? [0.07, 0.2].map((amount) => rgbaToHex(compositeOver(panelRgba, mixSrgb(accentRgba, amount, TRANSPARENT))))
      : [panel];
    const selectedSurfaces = custom?.selected?.length ? custom.selected : defaultSelected;
    result["--readable-sel-fg"] = readableForeground(fg, selectedSurfaces, 4.5).color;
    result["--readable-sel-dim"] = readableForeground(colors["fg-dim"] ?? fg, selectedSurfaces, 4.5).color;

    if (custom?.mark?.length) {
      result["--readable-mark-fg"] = readableForeground(custom.markFg ?? fg, custom.mark, 4.5).color;
    }
  }

  if (accentRgba) {
    const accentHex = rgbaToHex(accentRgba.a < 0.995 ? compositeOver(panelRgba, accentRgba) : accentRgba);
    result["--readable-focus"] = withContrast(accentHex, coreSurfaces, 3);
  }

  return result;
}

const ROOT_ONLY_SELECTOR = /^:root(?::root)*(?:\[[^\]]+\]|\.[a-z0-9_-]+)*$/i;
const WORDMARK_SELECTOR = /\.wordmark\b/i;

/**
 * Inspect a custom theme stylesheet for rules outside `:root` that set foreground colours,
 * images, or opacity directly (which cannot be rewritten safely without per-frame DOM scans).
 */
export function analyzeCustomStylesheet(cssText: string): StylesheetAnalysis {
  const cleaned = cssText.replace(/\/\*[\s\S]*?\*\//g, "");
  let hasElementColorRules = false;
  let hasImageRules = /\b(?:url|image-set|-webkit-image-set|cross-fade|paint)\s*\(/i.test(cleaned);
  let hasOpacityRules = false;

  const rulePattern = /([^{}@]+)\{([^{}]*)\}/g;
  for (const match of cleaned.matchAll(rulePattern)) {
    const selectors = splitTopLevel(match[1], ",");
    const body = match[2];
    const nonRoot = selectors.some((sel) => {
      const s = sel.trim();
      return s.length > 0 && !ROOT_ONLY_SELECTOR.test(s) && !WORDMARK_SELECTOR.test(s);
    });
    if (!nonRoot) continue;
    if (/(?:^|;)\s*color\s*:/i.test(body)) hasElementColorRules = true;
    if (/(?:^|;)\s*opacity\s*:/i.test(body)) hasOpacityRules = true;
    if (/\b(?:url|image-set|-webkit-image-set|cross-fade|paint)\s*\(/i.test(body)) hasImageRules = true;
  }

  return { hasElementColorRules, hasImageRules, hasOpacityRules };
}

export function contrastReportEqual(a: ThemeContrastReport | null, b: ThemeContrastReport | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  const sameList = (x: string[], y: string[]) => x.length === y.length && x.every((item, i) => item === y[i]);
  return sameList(a.imageSurfaces, b.imageSurfaces)
    && sameList(a.mixedGradientSurfaces, b.mixedGradientSurfaces)
    && sameList(a.customForegroundVars, b.customForegroundVars)
    && a.hasCustomCssRules === b.hasCustomCssRules
    && a.hasOpacityRules === b.hasOpacityRules
    && a.translucentWindow === b.translucentWindow;
}

export interface NativeWindowBackground {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Resolve the RGBA tuple synced to the native window and WebView backing surface. */
export function resolveNativeWindowBackground(
  bgRgba: Rgba | null,
  translucentWindow: boolean,
): NativeWindowBackground | null {
  if (!bgRgba) return translucentWindow ? { r: 0, g: 0, b: 0, a: 0 } : null;
  return {
    r: Math.round(clamp(bgRgba.r, 0, 255)),
    g: Math.round(clamp(bgRgba.g, 0, 255)),
    b: Math.round(clamp(bgRgba.b, 0, 255)),
    a: translucentWindow ? 0 : 255,
  };
}

export function hasContrastLimits(report: ThemeContrastReport | null): boolean {
  if (!report) return false;
  return report.imageSurfaces.length > 0
    || report.mixedGradientSurfaces.length > 0
    || report.customForegroundVars.length > 0
    || report.hasCustomCssRules
    || report.hasOpacityRules
    || report.translucentWindow;
}
