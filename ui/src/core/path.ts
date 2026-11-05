// Path helpers for the UI. Paths are native strings from the backend.

export const isWindowsPath = (p: string) => /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("\\\\");

export function sepOf(p: string): string {
  return isWindowsPath(p) ? "\\" : "/";
}

/** Splits a native path into its ancestor chain (root first), like nav::path::ancestors. */
export function ancestors(p: string): string[] {
  const sep = sepOf(p);
  if (sep === "/") {
    const parts = p.split("/").filter(Boolean);
    const out = ["/"];
    let cur = "";
    for (const part of parts) {
      cur += "/" + part;
      out.push(cur);
    }
    return out;
  }
  const parts = p.split(/[\\/]/).filter(Boolean);
  const out: string[] = [];
  // UNC: \\server\share is the root, like a drive letter.
  const unc = /^[\\/]{2}[^\\/]/.test(p);
  if (unc) {
    if (parts.length < 2) return [p];
    parts.splice(0, 2, `\\\\${parts[0]}\\${parts[1]}`);
  }
  let cur = "";
  parts.forEach((part, i) => {
    cur = i === 0 ? part + "\\" : cur.endsWith("\\") ? cur + part : cur + "\\" + part;
    out.push(cur);
  });
  return out;
}

/** Joins a `/`-separated relative path (as folder compare reports them) onto a native root. */
export function joinRel(root: string, rel: string): string {
  const sep = sepOf(root);
  return join(root, sep === "/" ? rel : rel.replace(/\//g, sep));
}

/** `~`-shortened path for display when it lies inside `home`. */
export function tildify(p: string, home: string | undefined | null): string {
  if (!home || home === "/" || !isInside(p, home)) return p;
  return "~" + p.slice(home.replace(/[\\/]$/, "").length);
}

export function basename(p: string): string {
  if (p === "/" || /^[a-zA-Z]:\\?$/.test(p)) return p;
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

export function parent(p: string): string | null {
  const a = ancestors(p);
  return a.length > 1 ? a[a.length - 2] : null;
}

export function join(dir: string, name: string): string {
  const sep = sepOf(dir);
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

export function isInside(child: string, dir: string): boolean {
  if (child === dir) return true;
  const sep = sepOf(dir);
  return child.startsWith(dir.endsWith(sep) ? dir : dir + sep);
}

/** Folders to go back to when leaving the drive at `mount`: the history (oldest first)
 *  newest first, without those on the drive and without repeats. */
export function backOutside(back: string[], mount: string): string[] {
  const out: string[] = [];
  for (const p of [...back].reverse()) if (!isInside(p, mount) && !out.includes(p)) out.push(p);
  return out;
}

/** Splits "name.ext" into [stem, ".ext"]; dot-files keep their whole name. */
export function splitExt(name: string): [string, string] {
  const i = name.lastIndexOf(".");
  return i > 0 ? [name.slice(0, i), name.slice(i)] : [name, ""];
}

/** Inside a WSL distribution (`\\wsl.localhost\Ubuntu\home\u\…`, also `\\wsl$\…`), its
 *  Linux home (`\\wsl.localhost\Ubuntu\home\u`; `…\root` for root, `…\home` outside
 *  both), so "Home" searches there and not in the Windows home. Null elsewhere. */
export function linuxHome(p: string): string | null {
  const m = /^([\\/]{2}(?:\?[\\/]UNC[\\/])?wsl(?:\.localhost|\$)[\\/][^\\/]+)([\\/].*)?$/i.exec(p);
  if (!m) return null;
  const root = m[1].replace(/\//g, "\\");
  const h = /^[\\/](home[\\/][^\\/]+|root)(?=[\\/]|$)/i.exec(m[2] ?? "");
  return h ? `${root}\\${h[1].replace("/", "\\")}` : `${root}\\home`;
}


/** A Quick Open query (`ext:pdf`, `'exact`, `^start`, `end$`, `!not`, several terms, path
 *  terms) as one pattern for the wildcard name search (F3, `*` and `?`), for Alt+I:
 *  "rep ext:pdf" → "*rep*.pdf", "^src/main" → "main". Excluding terms are left out. */
export function quickToWildcard(q: string): string {
  let ext = "";
  const terms: string[] = [];
  for (const raw of q.trim().split(/\s+/)) {
    if (!raw || raw.startsWith("!")) continue;
    const e = /^ext:(.+)$/i.exec(raw);
    if (e) { ext = e[1].replace(/^\./, ""); continue; }
    let t = raw.replace(/^['^]+/, "").replace(/\$$/, "");
    t = t.split(/[\\/]/).filter(Boolean).pop() ?? "";
    if (t) terms.push(t);
  }
  if (!ext && terms.length <= 1) return terms[0] ?? "";
  return `*${terms.join("*")}${terms.length ? "*" : ""}${ext ? `.${ext}` : ""}`.replace(/\*+/g, "*");
}
