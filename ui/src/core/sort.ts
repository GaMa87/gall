// Mirror of crates/gall-core/src/view/sort.rs — keep the two in sync.
import type { FileEntry, SortKey } from "../api/types";

export interface SortSpec {
  key: SortKey;
  descending: boolean;
  foldersOnTop: boolean;
  foldersByName: boolean;
}

const isDigit = (c: string) => c >= "0" && c <= "9";

/** Natural, case-insensitive comparison: "file2" < "file10". */
export function naturalCmp(a: string, b: string): number {
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const x = a[i];
    const y = b[j];
    if (isDigit(x) && isDigit(y)) {
      let ei = i;
      while (ei < a.length && isDigit(a[ei])) ei++;
      let ej = j;
      while (ej < b.length && isDigit(b[ej])) ej++;
      const na = a.slice(i, ei);
      const nb = b.slice(j, ej);
      const ta = na.replace(/^0+/, "");
      const tb = nb.replace(/^0+/, "");
      if (ta.length !== tb.length) return ta.length - tb.length;
      if (ta !== tb) return ta < tb ? -1 : 1;
      if (na.length !== nb.length) return na.length - nb.length;
      i = ei;
      j = ej;
    } else {
      const lx = x.toLowerCase();
      const ly = y.toLowerCase();
      if (lx !== ly) return lx < ly ? -1 : 1;
      i++;
      j++;
    }
  }
  if (i < a.length) return 1;
  if (j < b.length) return -1;
  return a < b ? -1 : a > b ? 1 : 0;
}

const num = (v: number | null) => v ?? -Infinity;

function byKey(a: FileEntry, b: FileEntry, key: SortKey): number {
  const n = () => naturalCmp(a.name, b.name);
  switch (key) {
    case "name":
      return n();
    case "ext":
      return a.ext < b.ext ? -1 : a.ext > b.ext ? 1 : n();
    case "modified":
      return num(a.modified) - num(b.modified) || n();
    case "created":
      return num(a.created) - num(b.created) || n();
    case "size":
      return a.size - b.size || n();
  }
}

/** Calculated total size of a folder, when known (UI only: the Rust mirror has none). */
export type DirSizeOf = (path: string) => number | undefined;

export function compare(a: FileEntry, b: FileEntry, s: SortSpec, dirSize?: DirSizeOf): number {
  if (s.foldersOnTop && a.isDir !== b.isDir) return a.isDir ? -1 : 1;
  const bothDirs = a.isDir && b.isDir;
  // Folders sort by size once their sizes are calculated (unknown ones first, by name).
  if (bothDirs && s.key === "size" && !s.foldersByName && dirSize) {
    const x = dirSize(a.path) ?? -1, y = dirSize(b.path) ?? -1;
    const o = x - y || naturalCmp(a.name, b.name);
    return s.descending ? -o : o;
  }
  const key: SortKey = bothDirs && (s.foldersByName || s.key === "size") ? "name" : s.key;
  const o = byKey(a, b, key);
  return s.descending && !(bothDirs && s.foldersByName) ? -o : o;
}

export function sortEntries(v: FileEntry[], s: SortSpec, dirSize?: DirSizeOf): FileEntry[] {
  return [...v].sort((a, b) => compare(a, b, s, dirSize));
}
