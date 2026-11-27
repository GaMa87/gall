// Translation core: plain functions with no browser or bundler dependencies, so they are unit tested.
// A catalog maps a key to a string, or to one string per CLDR plural category
// ({ one, two, few, other, … }); `{name}` in a string is replaced by a variable.

export type Entry = string | Partial<Record<Intl.LDMLPluralRule, string>>;
export type Catalog = Record<string, Entry>;
export type Vars = Record<string, string | number>;

/** Scripts written right to left (primary language subtags). */
const RTL = new Set(["ar", "he", "fa", "ur", "ps", "sd", "yi", "ckb", "dv"]);

const rules = new Map<string, Intl.PluralRules>();

/** CLDR plural category of `n` in `locale` (Slovenian has `two` and `few`). */
export function pluralCategory(locale: string, n: number): Intl.LDMLPluralRule {
  let r = rules.get(locale);
  if (!r) {
    try {
      r = new Intl.PluralRules(locale);
    } catch {
      r = new Intl.PluralRules("en");
    }
    rules.set(locale, r);
  }
  return r.select(n);
}

/** An entry of `c` itself: a key such as "constructor" must not find `Object.prototype` members. */
export function own<T>(c: Record<string, T> | undefined, key: string): T | undefined {
  return c && Object.hasOwn(c, key) ? c[key] : undefined;
}

function fill(text: string, vars?: Vars): string {
  return vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text;
}

/**
 * The text for `key`: from `catalog`, else from `fallback` (English), else the key itself.
 * A plural entry is chosen by `vars.count` in `locale`, falling back to `other`.
 */
export function translate(catalog: Catalog | undefined, fallback: Catalog, locale: string, key: string, vars?: Vars): string {
  const entry = own(catalog, key) ?? own(fallback, key);
  if (entry === undefined) return key;
  if (typeof entry === "string") return fill(entry, vars);
  const count = typeof vars?.count === "number" ? vars.count : Number(vars?.count ?? NaN);
  const form = Number.isFinite(count) ? (entry[pluralCategory(locale, count)] ?? entry.other) : entry.other;
  return fill(form ?? key, vars);
}

export interface Resolved {
  /** Catalog to use ("en" when the language has none). */
  code: string;
  /** BCP 47 tag for Intl formatting. */
  format: string;
  dir: "ltr" | "rtl";
}

export interface OmarchyLang {
  code: string;
  formatLocale: string;
  direction: "ltr" | "rtl";
}

/** `sl-SI` -> `sl`, `pt-BR` stays `pt-BR` when a catalog exists for it. */
function match(tag: string, available: readonly string[]): string | null {
  const t = tag.replace(/_/g, "-");
  const exact = available.find((a) => a.toLowerCase() === t.toLowerCase());
  if (exact) return exact;
  const primary = t.split("-")[0].toLowerCase();
  return available.find((a) => a.toLowerCase() === primary) ?? available.find((a) => a.toLowerCase().split("-")[0] === primary) ?? null;
}

/**
 * Which language to show. `setting` is "auto" or a code; auto prefers Omarchy's language, then the
 * system languages of the webview. Formatting follows Omarchy's `formatLocale` when it is the
 * source, else the matched language tag.
 */
export function resolveLocale(setting: string, omarchy: OmarchyLang | null | undefined, system: readonly string[], available: readonly string[]): Resolved {
  const dirOf = (tag: string, explicit?: "ltr" | "rtl") => explicit ?? (RTL.has(tag.split("-")[0].toLowerCase()) ? "rtl" : "ltr");
  if (setting && setting !== "auto") {
    const code = match(setting, available) ?? "en";
    return { code, format: code === "en" ? "en-US" : code, dir: dirOf(setting) };
  }
  if (omarchy) {
    const code = match(omarchy.code, available) ?? "en";
    return { code, format: omarchy.formatLocale || omarchy.code, dir: dirOf(omarchy.code, omarchy.direction) };
  }
  for (const tag of system) {
    const code = match(tag, available);
    if (code) return { code, format: tag, dir: dirOf(tag) };
  }
  return { code: "en", format: system[0] || "en-US", dir: dirOf(system[0] || "en") };
}

/** Placeholder names `{name}` of a string. */
const placeholders = (s: string) => new Set([...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]));

/**
 * Problems in `catalog` compared with the English `base`: keys missing or unknown, placeholders
 * the English text does not have, and plural entries lacking a category the language needs
 * (e.g. Slovenian `two` and `few`).
 */
/** FNV-1a hash of an English catalog value; `scripts/i18n-sync.mjs` computes the same one. */
export function sourceHash(value: unknown): string {
  const text = JSON.stringify(value);
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

/**
 * Keys whose English text changed since the locale was last marked as reviewed (`reviewed` maps
 * key → hash recorded by `npm --prefix ui run i18n:sync`). Keys never recorded count as outdated.
 */
export function outdatedKeys(base: Catalog, reviewed: Record<string, string> | undefined): string[] {
  return Object.keys(base).filter((k) => reviewed?.[k] !== sourceHash(base[k]));
}

export function checkCatalog(base: Catalog, catalog: Catalog, locale: string): string[] {
  const problems: string[] = [];
  const needed = [...new Set([1, 2, 3, 5, 0, 21, 101].map((n) => pluralCategory(locale, n)))].filter((c) => c !== "other");
  for (const key of Object.keys(base)) {
    const b = base[key];
    const c = catalog[key];
    if (c === undefined) {
      problems.push(`missing: ${key}`);
      continue;
    }
    if (typeof b === "string" && typeof c !== "string") problems.push(`not a plain string: ${key}`);
    if (typeof b !== "string" && typeof c === "string") problems.push(`not a plural entry: ${key}`);
    const known = placeholders((typeof b === "string" ? [b] : Object.values(b)).join(" "));
    const texts = typeof c === "string" ? [c] : (Object.values(c) as string[]);
    if (texts.some((text) => {
      const actual = placeholders(text);
      // A plain string must keep every placeholder; a plural form may drop {count} ("one" → "a single file").
      return [...actual].some((p) => !known.has(p)) || (typeof c === "string" && [...known].some((p) => !actual.has(p)));
    })) problems.push(`placeholder mismatch: ${key}`);
    if (typeof c !== "string") {
      for (const cat of needed) if (!c[cat]) problems.push(`plural ${cat} missing: ${key}`);
      if (!c.other) problems.push(`plural other missing: ${key}`);
    }
  }
  for (const key of Object.keys(catalog)) if (!(key in base)) problems.push(`unknown: ${key}`);
  return problems;
}
