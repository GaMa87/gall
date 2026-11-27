import { describe, expect, it } from "vitest";
import { checkCatalog, outdatedKeys, pluralCategory, resolveLocale, sourceHash, translate, type Catalog } from "./core";
import en from "./locales/en.json";
import syncManifest from "./sync-manifest.json";
import requiredTextKeys from "./required-text-keys.json";
import { ACTION_LABELS } from "../core/keymap";

const texts = import.meta.glob<{ default: Record<string, string> }>("./texts/*.json", { eager: true });
const catalogs = import.meta.glob<{ default: Catalog }>("./locales/*.json", { eager: true });
const rawCatalogs = import.meta.glob<string>(["./texts/*.json", "./locales/*.json"], { query: "?raw", import: "default", eager: true });
// Every wording any text catalog or the required list knows: all of them must cover all of it,
// so English plus Slovenian, German and Spanish are complete, not "common labels".
const textSourceKeys = new Set<string>([
  ...Object.values(texts).flatMap((m) => Object.keys(m.default)),
  ...requiredTextKeys,
]);

describe("plural categories (CLDR)", () => {
  it("knows the Slovenian dual and paucal forms", () => {
    const cats = [1, 2, 3, 4, 5, 21, 22, 101, 0].map((n) => pluralCategory("sl", n));
    expect(cats).toEqual(["one", "two", "few", "few", "other", "other", "other", "one", "other"]);
  });
  it("English has one and other", () => {
    expect([0, 1, 2].map((n) => pluralCategory("en", n))).toEqual(["other", "one", "other"]);
  });
  it("falls back to English rules for a bad tag", () => {
    expect(pluralCategory("not a locale!!", 1)).toBe("one");
  });
});

describe("translate", () => {
  const base: Catalog = { a: "Hello {name}", n: { one: "{count} file", other: "{count} files" }, only: "English only" };
  const sl: Catalog = { a: "Živjo {name}", n: { one: "{count} datoteka", two: "{count} datoteki", few: "{count} datoteke", other: "{count} datotek" } };
  it("fills placeholders and keeps unknown ones", () => {
    expect(translate(sl, base, "sl", "a", { name: "Ana" })).toBe("Živjo Ana");
    expect(translate(sl, base, "sl", "a")).toBe("Živjo {name}");
  });
  it("chooses the plural form of the language", () => {
    const f = (n: number) => translate(sl, base, "sl", "n", { count: n });
    expect([1, 2, 3, 5, 11, 101, 102].map(f)).toEqual(["1 datoteka", "2 datoteki", "3 datoteke", "5 datotek", "11 datotek", "101 datoteka", "102 datoteki"]);
    expect(translate(base, base, "en", "n", { count: 1 })).toBe("1 file");
    expect(translate(base, base, "en", "n", { count: 0 })).toBe("0 files");
  });
  it("falls back to English, then to the key", () => {
    expect(translate(sl, base, "sl", "only")).toBe("English only");
    expect(translate(sl, base, "sl", "missing.key")).toBe("missing.key");
    expect(translate(undefined, base, "xx", "a", { name: "B" })).toBe("Hello B");
  });
});

describe("resolveLocale", () => {
  const available = ["en", "sl", "de", "es", "pt-BR"];
  it("uses an explicit setting first", () => {
    expect(resolveLocale("sl", { code: "en", formatLocale: "en-US", direction: "ltr" }, ["en-US"], available).code).toBe("sl");
    expect(resolveLocale("xx", null, ["sl-SI"], available).code).toBe("en");
    expect(resolveLocale("de-DE", null, [], available).code).toBe("de");
    expect(resolveLocale("es", null, [], available).code).toBe("es");
  });
  it("auto prefers Omarchy and takes its format locale and direction", () => {
    const r = resolveLocale("auto", { code: "sl", formatLocale: "sl-SI", direction: "ltr" }, ["en-US"], available);
    expect(r).toEqual({ code: "sl", format: "sl-SI", dir: "ltr" });
    const ar = resolveLocale("auto", { code: "ar", formatLocale: "ar-AE", direction: "rtl" }, ["en-US"], available);
    expect(ar).toEqual({ code: "en", format: "ar-AE", dir: "rtl" });
  });
  it("auto falls back to the system languages and matches regional tags", () => {
    expect(resolveLocale("auto", null, ["de-DE", "sl-SI"], available).code).toBe("de");
    expect(resolveLocale("auto", null, ["es-MX", "de-DE"], available).code).toBe("es");
    expect(resolveLocale("auto", null, ["sl-SI"], available).code).toBe("sl");
    expect(resolveLocale("auto", null, ["pt-BR"], available).code).toBe("pt-BR");
    expect(resolveLocale("auto", null, ["pt-PT"], available).code).toBe("pt-BR");
    expect(resolveLocale("auto", null, ["he-IL"], available)).toMatchObject({ code: "en", dir: "rtl" });
    expect(resolveLocale("auto", null, [], available)).toEqual({ code: "en", format: "en-US", dir: "ltr" });
  });
});

describe("catalogs", () => {
  it("every catalog has exactly the English keys, matching placeholders and the plural forms its language needs", () => {
    for (const [path, mod] of Object.entries(catalogs)) {
      const code = path.match(/\/([^/]+)\.json$/)![1];
      if (code === "en") continue;
      expect(checkCatalog(en as Catalog, mod.default, code), code).toEqual([]);
    }
  });
  it("no locale is outdated: English text changed since its translation was reviewed", () => {
    for (const path of Object.keys(catalogs)) {
      const code = path.match(/\/([^/]+)\.json$/)![1];
      if (code === "en") continue;
      const stale = outdatedKeys(en as Catalog, (syncManifest as Record<string, Record<string, string>>)[code]);
      expect(stale, `${code} is outdated for ${stale.length} key(s). Update ui/src/i18n/locales/${code}.json, then run: npm --prefix ui run i18n:sync -- ${code}`).toEqual([]);
    }
  });
  it("outdatedKeys lists changed, new and unrecorded English keys", () => {
    const base: Catalog = { a: "Hello", b: "Bye", c: { one: "x", other: "y" } };
    const reviewed = { a: sourceHash("Hello"), b: sourceHash("Goodbye"), c: sourceHash({ one: "x", other: "y" }) };
    expect(outdatedKeys(base, reviewed)).toEqual(["b"]);
    expect(outdatedKeys({ ...base, d: "new" }, reviewed)).toEqual(["b", "d"]);
    expect(outdatedKeys(base, undefined)).toEqual(["a", "b", "c"]);
  });
  it("flags what is wrong", () => {
    const base: Catalog = { a: "Hi {x}", n: { one: "{count} a", other: "{count} b" } };
    expect(checkCatalog(base, { a: "Hi", n: { other: "x" } }, "sl")).toEqual(["placeholder mismatch: a", "plural one missing: n", "plural two missing: n", "plural few missing: n"]);
    expect(checkCatalog(base, { a: "Hi {y}", n: { one: "1", two: "2", few: "3", other: "x" }, z: "?" }, "sl")).toEqual(["placeholder mismatch: a", "unknown: z"]);
    expect(checkCatalog(base, {}, "sl")).toEqual(["missing: a", "missing: n"]);
  });
  it("text catalogs keep the placeholders of the English wording and have no empty entries", () => {
    const ph = (x: string) => [...x.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join();
    for (const [path, mod] of Object.entries(texts)) {
      for (const [k, v] of Object.entries(mod.default)) {
        expect(v.trim(), `${path}: ${k}`).not.toBe("");
        expect(ph(v), `${path}: ${k}`).toBe(ph(k));
      }
    }
  });
  it.each(["sl", "de", "es"])("%s text catalog covers every UI text with matching placeholders", (code) => {
    const catalog = texts[`./texts/${code}.json`]?.default;
    expect(catalog, code).toBeDefined();
    expect(Object.keys(catalog!).sort(), code).toEqual([...textSourceKeys].sort());
    const ph = (x: string) => [...x.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join();
    for (const key of textSourceKeys) {
      expect(catalog![key].trim(), `${code}: ${key}`).not.toBe("");
      expect(ph(catalog![key]), `${code}: ${key}`).toBe(ph(key));
    }
  });
  it("every command label of the palette and shortcut list is translated in Slovenian", () => {
    const sl = texts["./texts/sl.json"].default;
    expect(Object.values(ACTION_LABELS).filter((l) => !(l in sl))).toEqual([]);
  });
  it("a key that names an Object.prototype member is not found in a catalog", () => {
    expect(translate({}, {}, "en", "constructor")).toBe("constructor");
    expect(translate({ a: "x" }, { a: "x" }, "en", "toString")).toBe("toString");
  });
  it("text and locale catalogs have no duplicate JSON keys", () => {
    for (const [rel, raw] of Object.entries(rawCatalogs)) {
      const seen = new Set<string>();
      const duplicates: string[] = [];
      for (const line of raw.split(/\r?\n/)) {
        const match = /^  "((?:\\.|[^"\\])*)"\s*:/.exec(line);
        if (!match) continue;
        const key = JSON.parse(`"${match[1]}"`) as string;
        if (seen.has(key)) duplicates.push(key);
        else seen.add(key);
      }
      expect(duplicates, rel).toEqual([]);
    }
  });
});
