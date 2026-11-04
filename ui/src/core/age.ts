// Mirror of crates/gall-core/src/view/age.rs (F-05).
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

export interface Age {
  label: string;
  heat: number;
}

export function age(modified: number, now: number): Age {
  const d = Math.max(0, now - modified);
  let label: string;
  if (d < HOUR) label = `${Math.max(1, Math.floor(d / MIN))}'`;
  else if (d < DAY) label = `${Math.floor(d / HOUR)}h${Math.floor((d % HOUR) / MIN)}'`;
  else if (d < MONTH) label = `${Math.floor(d / DAY)}d`;
  else if (d < YEAR) label = `${Math.floor(d / MONTH)}M`;
  else label = `${Math.floor(d / YEAR)}Y`;
  const max = 5 * YEAR;
  const heat = 1 - Math.min(1, Math.log(d / MIN + 1) / Math.log(max / MIN + 1));
  return { label, heat };
}

/** The age in words, for where the compact label (23', 22h13', 1d, 3M, 2Y) is explained:
 *  "23 minutes", "22 hours 13 minutes", "1 day", "3 months", "2 years". */
export function ageWords(modified: number, now: number): string {
  const d = Math.max(0, now - modified);
  const n = (v: number, unit: string) => `${v} ${unit}${v === 1 ? "" : "s"}`;
  if (d < HOUR) return n(Math.max(1, Math.floor(d / MIN)), "minute");
  if (d < DAY) {
    const m = Math.floor((d % HOUR) / MIN);
    return n(Math.floor(d / HOUR), "hour") + (m ? ` ${n(m, "minute")}` : "");
  }
  if (d < MONTH) return n(Math.floor(d / DAY), "day");
  if (d < YEAR) return n(Math.floor(d / MONTH), "month");
  return n(Math.floor(d / YEAR), "year");
}

/**
 * Discrete age tiers so the colour reads at a glance (hot = recent):
 * <1h red · <24h orange · <7d yellow · <30d green · <1y blue · older grey.
 */
export const AGE_TIERS = [
  { max: HOUR, hue: 0, label: "last hour" },
  { max: DAY, hue: 26, label: "today" },
  { max: 7 * DAY, hue: 48, label: "this week" },
  { max: MONTH, hue: 142, label: "this month" },
  { max: YEAR, hue: 214, label: "this year" },
  { max: Infinity, hue: -1, label: "older" },
] as const;

export function ageTier(modified: number, now: number): number {
  const d = Math.max(0, now - modified);
  return AGE_TIERS.findIndex((t) => d < t.max);
}

/**
 * Muted badge colours for a tier: a translucent tint of the tier hue with text in the
 * same hue, so every tier stays readable. Saturation/lightness come from settings
 * (defaults 0.55 / 0.5) and shift the text colour.
 */
export function ageColor(tier: number, saturation: number, lightness: number): { bg: string; fg: string; border: string } {
  const t = AGE_TIERS[tier] ?? AGE_TIERS[AGE_TIERS.length - 1];
  if (t.hue < 0) return { bg: "transparent", fg: "var(--fg-faint)", border: "var(--border)" };
  const s = Math.round(30 + saturation * 70); // 0.55 → ~68%
  const light = document.documentElement.dataset.theme === "light";
  const l = light ? Math.round(18 + lightness * 30) : Math.round(48 + lightness * 30); // dark: 0.5 → 63%
  return {
    bg: `hsl(${t.hue} ${s}% 50% / ${light ? 0.14 : 0.16})`,
    fg: `hsl(${t.hue} ${s}% ${l}%)`,
    border: `hsl(${t.hue} ${s}% 50% / 0.28)`,
  };
}
