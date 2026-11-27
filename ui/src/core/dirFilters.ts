// The filters of a folder comparison as the user types them, and as the core takes them: include and exclude globs,
// inclusive size bounds and inclusive modification dates. Pure, so it is unit-tested; the window only shows the fields.
import type { DirCmpOptions } from "../api/types";

export interface FilterForm { include: string; exclude: string; minSize: string; maxSize: string; after: string; before: string }
export type FilterFields = Pick<DirCmpOptions, "include" | "exclude" | "minSize" | "maxSize" | "modifiedAfter" | "modifiedBefore">;
export const EMPTY_FORM: FilterForm = { include: "", exclude: "", minSize: "", maxSize: "", after: "", before: "" };

const UNITS: Record<string, number> = { "": 1, b: 1, k: 1024, kb: 1024, kib: 1024, m: 1024 ** 2, mb: 1024 ** 2, mib: 1024 ** 2, g: 1024 ** 3, gb: 1024 ** 3, gib: 1024 ** 3 };

/** "100", "10k", "1.5 MB" as a number of bytes (1024 per unit), or null when it is not a size. */
export function parseSize(text: string): number | null {
  const m = /^\s*(\d+(?:\.\d+)?)\s*([a-z]*)\s*$/i.exec(text);
  if (!m) return null;
  const unit = UNITS[m[2].toLowerCase()];
  if (unit === undefined) return null;
  const bytes = Math.round(Number(m[1]) * unit);
  return Number.isSafeInteger(bytes) ? bytes : null;
}

/** "2025-01-31" as seconds since the epoch at the start (or, with `end`, the last second) of that day, local time. */
export function parseDate(text: string, end: boolean): number | null {
  const m = /^\s*(\d{4})-(\d{2})-(\d{2})\s*$/.exec(text);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const date = end ? new Date(y, mo - 1, d, 23, 59, 59) : new Date(y, mo - 1, d, 0, 0, 0);
  // new Date rolls 2025-02-31 over to March: that is not the date that was typed.
  if (date.getFullYear() !== y || date.getMonth() !== mo - 1 || date.getDate() !== d) return null;
  return Math.floor(date.getTime() / 1000);
}

/** Globs separated by commas or line breaks, empty ones dropped. */
export function parseGlobs(text: string): string[] {
  return text.split(/[\n,]/).map((g) => g.trim()).filter((g) => g !== "");
}

export type Parsed = { ok: true; fields: FilterFields } | { ok: false; error: string };

/** The core's fields from the form, or the first thing that is wrong with it (a range that cannot match is wrong too). */
export function parseFilters(form: FilterForm): Parsed {
  const size = (text: string, name: string): number | null | string => {
    if (text.trim() === "") return null;
    const n = parseSize(text);
    return n === null ? `${name} is not a size (for example 500, 10k, 2 MB)` : n;
  };
  const min = size(form.minSize, "Smallest size"), max = size(form.maxSize, "Largest size");
  if (typeof min === "string") return { ok: false, error: min };
  if (typeof max === "string") return { ok: false, error: max };
  if (min !== null && max !== null && min > max) return { ok: false, error: "The smallest size is larger than the largest" };
  const date = (text: string, end: boolean, name: string): number | null | string => {
    if (text.trim() === "") return null;
    const t = parseDate(text, end);
    return t === null ? `${name} is not a date (write it as 2025-01-31)` : t;
  };
  const after = date(form.after, false, "Modified from"), before = date(form.before, true, "Modified until");
  if (typeof after === "string") return { ok: false, error: after };
  if (typeof before === "string") return { ok: false, error: before };
  if (after !== null && before !== null && after > before) return { ok: false, error: "The first date is after the last" };
  return { ok: true, fields: { include: parseGlobs(form.include), exclude: parseGlobs(form.exclude), minSize: min, maxSize: max, modifiedAfter: after, modifiedBefore: before } };
}

const two = (n: number) => String(n).padStart(2, "0");
const dateText = (t: number | null | undefined) => { if (t == null) return ""; const d = new Date(t * 1000); return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`; };

/** The form that shows the fields (sizes in bytes). */
export function formOf(o: Partial<FilterFields>): FilterForm {
  return {
    include: (o.include ?? []).join(", "), exclude: (o.exclude ?? []).join(", "),
    minSize: o.minSize == null ? "" : String(o.minSize), maxSize: o.maxSize == null ? "" : String(o.maxSize),
    after: dateText(o.modifiedAfter), before: dateText(o.modifiedBefore),
  };
}

/** How many of the filters narrow the comparison. */
export function activeFilterCount(o: Partial<FilterFields>): number {
  return [(o.include?.length ?? 0) > 0, (o.exclude?.length ?? 0) > 0, o.minSize != null, o.maxSize != null, o.modifiedAfter != null, o.modifiedBefore != null].filter(Boolean).length;
}
