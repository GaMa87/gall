// "Always open … with …" rules, mirroring OpenRule::find in crates/gall-core/src/storage/settings.rs:
// a rule for the file itself wins, then the one for its extension in the deepest folder
// containing it, then the one for its extension everywhere.
import type { OpenRule } from "../api/types";
import { basename, isInside, isWindowsPath, parent, tildify } from "./path";

export function extOf(path: string): string {
  const n = basename(path);
  const d = n.lastIndexOf(".");
  return d > 0 ? n.slice(d + 1).toLowerCase() : "";
}

export function findRule(rules: OpenRule[], path: string): OpenRule | null {
  // Windows paths are case-insensitive (as in the Rust version).
  const fold = isWindowsPath(path) ? (s: string) => s.toLowerCase() : (s: string) => s;
  const exact = rules.find((r) => fold(r.target) === fold(`path:${path}`));
  if (exact) return exact;
  const ext = extOf(path);
  if (!ext) return null;
  const depth = (r: OpenRule) => (r.dir ? r.dir.split(/[\\/]/).filter(Boolean).length + 1 : 0);
  return rules
    .filter((r) => r.target === `ext:${ext}` && (!r.dir || isInside(fold(path), fold(r.dir))))
    .reduce<OpenRule | null>((best, r) => (!best || depth(r) > depth(best) ? r : best), null);
}

/** Where a Ctrl+Enter choice applies: a rule key plus a label for the palette. */
export interface Scope { target: string; dir?: string; label: string }

/** The scopes offered for `paths`, broadest first: everywhere, the project (git root),
 *  the folder, the file itself. Mixed extensions offer none. */
export function scopesFor(paths: string[], gitRoot: string | null): Scope[] {
  const exts = paths.map(extOf);
  const ext = exts[0] && exts.every((e) => e === exts[0]) ? exts[0] : "";
  const out: Scope[] = [];
  const folder = parent(paths[0]);
  const sameFolder = paths.every((p) => parent(p) === folder);
  if (ext) {
    out.push({ target: `ext:${ext}`, label: `all .${ext} files` });
    if (gitRoot && paths.every((p) => isInside(p, gitRoot)) && gitRoot !== folder)
      out.push({ target: `ext:${ext}`, dir: gitRoot, label: `.${ext} in project “${basename(gitRoot)}”` });
    if (folder && sameFolder) out.push({ target: `ext:${ext}`, dir: folder, label: `.${ext} in “${basename(folder)}”` });
  }
  if (paths.length === 1) out.push({ target: `path:${paths[0]}`, label: `only “${basename(paths[0])}”` });
  return out;
}

export const sameScope = (r: OpenRule, s: Scope) => r.target === s.target && (r.dir ?? undefined) === s.dir;

/** "all .md files", ".md in ~/Projects", "only README.md" (for the settings list). */
export function ruleLabel(r: OpenRule, home: string): string {
  if (r.target.startsWith("path:")) return `only ${tildify(r.target.slice(5), home)}`;
  const ext = r.target.slice(4);
  return r.dir ? `.${ext} in ${tildify(r.dir, home)}` : `all .${ext} files`;
}
