// Runtime of the translations: the current language, `t()` and a hook that re-renders on change.
// English is bundled; other catalogs load on demand (`locales/<code>.json`).
import { useSyncExternalStore } from "react";
import en from "./locales/en.json";
import { platformLabel } from "../core/keymap";
import { own, resolveLocale, translate, type Catalog, type OmarchyLang, type Resolved, type Vars } from "./core";

const loaders = import.meta.glob<{ default: Catalog }>("./locales/*.json");
/** Language codes with a catalog, English first. */
export const AVAILABLE: string[] = ["en", ...Object.keys(loaders).map((p) => p.match(/\/([^/]+)\.json$/)![1]).filter((c) => c !== "en")];

const base = en as Catalog;
const catalogs = new Map<string, Catalog>([["en", base]]);

// Texts translated by their English wording (tables of settings, dialogs): `texts/<code>.json` maps
// the English text to the translation. A missing entry shows the English text.
const textLoaders = import.meta.glob<{ default: Record<string, string> }>("./texts/*.json");
const texts = new Map<string, Record<string, string>>();
let current: Resolved = { code: "en", format: "en-US", dir: "ltr" };
let rev = 0;
const listeners = new Set<() => void>();

/** Translates `key`; `{name}` in the text is replaced from `vars`, plural entries are chosen by `vars.count`. */
export function t(key: string, vars?: Vars): string {
  // Texts name shortcuts with Ctrl and Alt; macOS writes them as Cmd and Option.
  return platformLabel(translate(catalogs.get(current.code), base, current.code, key, vars));
}

/** Translates an English text that is its own key; `{name}` is replaced from `vars`. */
export function tt(text: string, vars?: Vars): string {
  const out = own(texts.get(current.code), text) ?? text;
  return platformLabel(vars ? out.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m)) : out);
}

/** The BCP 47 tag numbers and dates are formatted with. */
export const formatLocale = () => current.format;
export const currentLanguage = () => current.code;

/**
 * Switches to the language the setting and the desktop ask for (see `resolveLocale`), loading its
 * catalog first, and sets `lang` and `dir` on the document (right-to-left languages mirror the layout).
 */
let applySeq = 0;
export async function applyLanguage(setting: string, omarchy: OmarchyLang | null | undefined, system: readonly string[] = navigator.languages ?? []) {
  const mine = ++applySeq;
  const next = resolveLocale(setting, omarchy, system, AVAILABLE);
  if (!catalogs.has(next.code)) {
    try {
      catalogs.set(next.code, (await loaders[`./locales/${next.code}.json`]()).default);
    } catch (e) {
      console.warn(`no catalog for ${next.code}`, e);
      next.code = "en";
    }
  }
  const tl = textLoaders[`./texts/${next.code}.json`];
  if (tl && !texts.has(next.code)) {
    try { texts.set(next.code, (await tl()).default); } catch (e) { console.warn(`no texts for ${next.code}`, e); }
  }
  // A newer call (the user changed the language again while a catalog was loading) wins.
  if (mine !== applySeq) return;
  if (next.code === current.code && next.format === current.format && next.dir === current.dir) return;
  current = next;
  if (typeof document !== "undefined") {
    document.documentElement.lang = next.code;
    document.documentElement.dir = next.dir;
  }
  rev++;
  listeners.forEach((l) => l());
}

/** Re-renders the component when the language changes; returns `t`. Put it in the root so the whole tree updates. */
export function useT(): typeof t {
  useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => rev,
  );
  return t;
}

/** The name of a language in itself ("Slovenščina"), for the language picker. */
export function languageName(code: string): string {
  try {
    const n = new Intl.DisplayNames([code], { type: "language" }).of(code);
    return n ? n.charAt(0).toLocaleUpperCase(code) + n.slice(1) : code;
  } catch {
    return code;
  }
}
