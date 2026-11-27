// Records that a locale is up to date with the English catalog.
//   npm --prefix ui run i18n:sync -- de es sl   (or `all`)
// Writes src/i18n/sync-manifest.json: per locale and key, a hash of the English value the
// translation was last reviewed against. `i18n.test.ts` fails when English changes afterwards.
// Run it only after the translations were actually updated (see AGENTS.md "Translations").
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const dir = fileURLToPath(new URL("../src/i18n/", import.meta.url));
const manifestPath = `${dir}sync-manifest.json`;

// Keep identical to `sourceHash` in src/i18n/core.ts (FNV-1a over the JSON text).
function sourceHash(value) {
  const text = JSON.stringify(value);
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

const en = JSON.parse(readFileSync(`${dir}locales/en.json`, "utf8"));
const all = readdirSync(`${dir}locales`).map((f) => f.replace(/\.json$/, "")).filter((c) => c !== "en");
const wanted = process.argv.slice(2);
if (!wanted.length) {
  console.error(`usage: i18n-sync.mjs <locale...|all>   locales: ${all.join(", ")}`);
  process.exit(2);
}
const codes = wanted.includes("all") ? all : wanted;
for (const c of codes) if (!all.includes(c)) { console.error(`unknown locale: ${c}`); process.exit(2); }

let manifest = {};
try { manifest = JSON.parse(readFileSync(manifestPath, "utf8")); } catch { /* first run */ }
for (const c of codes) {
  manifest[c] = Object.fromEntries(Object.keys(en).sort().map((k) => [k, sourceHash(en[k])]));
}
const sorted = Object.fromEntries(Object.keys(manifest).sort().map((c) => [c, manifest[c]]));
writeFileSync(manifestPath, `${JSON.stringify(sorted, null, 2)}\n`);
console.log(`i18n sync manifest updated for: ${codes.join(", ")}`);
