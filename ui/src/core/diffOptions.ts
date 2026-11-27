import type { DiffOptions, Granularity, WhitespaceMode } from "../api/types";

// Options of the text comparison: defaults, the rules for saved settings and small helpers.

export const defaultOpts: DiffOptions = {
  ignoreWhitespace: false,
  whitespace: "none",
  granularity: "word",
  ignoreComments: false,
  ignoreLines: [],
  ignoreText: [],
  ignoreCase: false,
  ignoreEol: true,
  minimal: false,
  structural: true,
  movesEdited: false,
};
const WHITESPACE_MODES: WhitespaceMode[] = ["none", "leading", "trailing", "all"];
const GRANULARITIES: Granularity[] = ["line", "word", "char"];
const MAX_RULES = 16;
const MAX_RULE_LENGTH = 512;

/** The rules of a text box: one per line, empty lines dropped, within the limits of the core. */
export function parseRules(text: string): string[] {
  return text.split("\n").map((l) => l.trim()).filter((l) => l !== "").slice(0, MAX_RULES).map((l) => l.slice(0, MAX_RULE_LENGTH));
}

/** Saved options, tolerating older versions: unknown keys are dropped, `ignoreWhitespace: true` becomes `whitespace: "all"`. */
export function migrateOpts(saved: Record<string, unknown>): DiffOptions {
  const o = { ...defaultOpts };
  for (const k of ["ignoreCase", "ignoreEol", "minimal", "structural", "ignoreComments", "movesEdited"] as const) if (typeof saved[k] === "boolean") o[k] = saved[k] as boolean;
  if (WHITESPACE_MODES.includes(saved.whitespace as WhitespaceMode)) o.whitespace = saved.whitespace as WhitespaceMode;
  else if (saved.ignoreWhitespace === true) o.whitespace = "all";
  if (GRANULARITIES.includes(saved.granularity as Granularity)) o.granularity = saved.granularity as Granularity;
  for (const k of ["ignoreLines", "ignoreText"] as const) {
    const v = saved[k];
    if (Array.isArray(v)) o[k] = v.filter((x): x is string => typeof x === "string").slice(0, MAX_RULES).map((x) => x.slice(0, MAX_RULE_LENGTH));
  }
  return o;
}

/** How many of the filters that hide differences are on. */
export const activeFilters = (o: DiffOptions) =>
  [o.whitespace !== undefined && o.whitespace !== "none", o.ignoreComments === true, (o.ignoreLines?.length ?? 0) > 0, (o.ignoreText?.length ?? 0) > 0].filter(Boolean).length;
