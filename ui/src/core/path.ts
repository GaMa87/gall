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
