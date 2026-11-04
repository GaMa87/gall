// Batch rename: new names from a template, find & replace and a case change, and the
// checks shown next to each row before anything is renamed. Pure functions (tested).
import { splitExt } from "./path";

export type CaseMode = "keep" | "lower" | "upper" | "title";

export interface RenameRules {
  /** Name template. Tokens: {name} (name without extension), {ext}, {n} / {n:3}
   *  (counter, padded), {date} / {time} (modified), {folder} (parent folder name). */
  template: string;
  find: string;
  replace: string;
  regex: boolean;
  matchCase: boolean;
  caseMode: CaseMode;
  start: number;
  step: number;
  /** Rules apply to the extension too (else it is kept as it is). */
  withExt: boolean;
}

export const DEFAULT_RULES: RenameRules = {
  template: "{name}", find: "", replace: "", regex: false, matchCase: false, caseMode: "keep", start: 1, step: 1, withExt: false,
};

export interface RenameItem {
  name: string;
  isDir: boolean;
  /** Modified, ms since the epoch. */
  modified: number | null;
  /** Name of the folder the item is in. */
  folder: string;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

function titleCase(s: string): string {
  return s.toLowerCase().replace(/(^|[\s_\-.(\[])(\p{L})/gu, (_, sep: string, c: string) => sep + c.toUpperCase());
}

function applyCase(s: string, mode: CaseMode): string {
  switch (mode) {
    case "lower": return s.toLowerCase();
    case "upper": return s.toUpperCase();
    case "title": return titleCase(s);
    default: return s;
  }
}

/** Expands the template tokens for item `i`. */
function expand(template: string, it: RenameItem, stem: string, ext: string, i: number, r: RenameRules): string {
  return template.replace(/\{(name|ext|n|date|time|folder)(?::(\d{1,2}))?\}/g, (_, tok: string, width?: string) => {
    switch (tok) {
      case "name": return stem;
      case "ext": return ext.replace(/^\./, "");
      case "n": return String(r.start + i * r.step).padStart(width ? Number(width) : 1, "0");
      case "folder": return it.folder;
      case "date": {
        if (it.modified == null) return "";
        const d = new Date(it.modified);
        return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
      }
      case "time": {
        if (it.modified == null) return "";
        const d = new Date(it.modified);
        return `${pad2(d.getHours())}.${pad2(d.getMinutes())}.${pad2(d.getSeconds())}`;
      }
      default: return "";
    }
  });
}

/** The new name of every item, or an error (an invalid regular expression). */
export function newNames(items: RenameItem[], r: RenameRules): { names: string[]; error: string | null } {
  let re: RegExp | null = null;
  if (r.find) {
    try {
      const src = r.regex ? r.find : r.find.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      re = new RegExp(src, r.matchCase ? "g" : "gi");
    } catch (e) {
      return { names: items.map((it) => it.name), error: (e as Error).message };
    }
  }
  const names = items.map((it, i) => {
    const [stem, ext] = it.isDir || r.withExt ? [it.name, ""] : splitExt(it.name);
    const origExt = it.isDir ? "" : splitExt(it.name)[1];
    let base = expand(r.template || "{name}", it, stem, origExt, i, r);
    // A literal replacement keeps "$" as typed; a regex one supports $1, $<name>.
    if (re) base = r.regex ? base.replace(re, r.replace) : base.replace(re, () => r.replace);
    return applyCase(base, r.caseMode) + ext;
  });
  return { names, error: null };
}

/** Names typed one per line ("edit as text"); null when the line count does not match. */
export function namesFromText(text: string, count: number): string[] | null {
  const lines = text.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n");
  return lines.length === count ? lines : null;
}

export type RenameIssue = "empty" | "invalid" | "duplicate" | "exists";

export const ISSUE_TEXT: Record<RenameIssue, string> = {
  empty: "empty name",
  invalid: "not allowed in a name",
  duplicate: "same name twice",
  exists: "already exists",
};

/**
 * What is wrong with each new name: empty, characters a file name cannot have, the same
 * name twice in the batch, or the name of another item in the folder that is not being
 * renamed. `others`: names in the folder outside the batch. `windows`: also the
 * characters and names Windows forbids. `ignoreCase`: the file system ignores case.
 */
export function renameIssues(
  oldNames: string[], names: string[], others: Iterable<string>, opts: { windows: boolean; ignoreCase: boolean },
): (RenameIssue | null)[] {
  const key = (n: string) => (opts.ignoreCase ? n.toLowerCase() : n);
  const outside = new Set([...others].map(key));
  for (const n of oldNames) outside.delete(key(n));
  const count = new Map<string, number>();
  for (const n of names) count.set(key(n), (count.get(key(n)) ?? 0) + 1);
  return names.map((n) => {
    if (!n.trim()) return "empty";
    if (n === "." || n === ".." || n.includes("/") || n.includes("\0")) return "invalid";
    if (opts.windows && (/[<>:"\\|?*\x00-\x1f]/.test(n) || /[ .]$/.test(n) || /^(con|prn|aux|nul|com\d|lpt\d)(\..*)?$/i.test(n))) return "invalid";
    if ((count.get(key(n)) ?? 0) > 1) return "duplicate";
    if (outside.has(key(n))) return "exists";
    return null;
  });
}
