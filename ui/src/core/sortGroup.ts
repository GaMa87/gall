// Grouping of a listing by type. Mirror of `group_entries` in crates/gall-core/src/view/sort.rs:
// both are checked against sortGroupCases.json, and kinds.json is written from the Rust tables.
import type { FileEntry, GroupMode, GroupOrder } from "../api/types";
import { compare, type DirSizeOf, type SortSpec } from "./sort";
import kindTable from "./kinds.json";

export interface GroupSpec { mode: GroupMode; order: GroupOrder; byKind: boolean }
/** A group of a sorted listing; its entries follow those of the group before it. */
export interface Group {
  /** `dir`, `ext:png`, `dot` (`.bashrc`), `none` (no extension), `kind:image` or `rest`. */
  id: string;
  count: number;
  /** Bytes of its files. */
  size: number;
  latest: number;
  /** How many types it holds: more than 1 only in `rest`. */
  types: number;
}

/** Extensions with a group of their own; the rarest ones beyond them share `rest`, the last. */
export const MAX_TYPE_GROUPS = 200;
export const GROUP_MODES: GroupMode[] = ["none", "folders", "type", "foldersType"];
export const GROUP_ORDERS: GroupOrder[] = ["name", "count", "recent", "size"];
export const foldersFirst = (mode: GroupMode) => mode === "folders" || mode === "foldersType";
export const byType = (mode: GroupMode) => mode === "type" || mode === "foldersType";
/** The mode with folders first switched, the grouping by type kept. */
export const toggleFolders = (mode: GroupMode): GroupMode => ({ none: "folders", folders: "none", type: "foldersType", foldersType: "type" } as const)[mode];

const KINDS = new Map<string, string>();
for (const [kind, list] of Object.entries(kindTable)) for (const ext of list) KINDS.set(ext, kind);

function kindOf(name: string, ext: string): string {
  if (ext) return KINDS.get(ext) ?? "other";
  // Without an extension the whole name counts (Makefile, LICENSE); a file called "png" is no image.
  const kind = KINDS.get(name.replace(/^\.+/, "").toLowerCase());
  return kind === "code" || kind === "document" ? kind : "other";
}

export function groupId(e: FileEntry, byKind: boolean): string {
  if (e.isDir) return "dir";
  if (byKind) return `kind:${kindOf(e.name, e.ext)}`;
  return e.ext ? `ext:${e.ext}` : e.name.startsWith(".") ? "dot" : "none";
}

const text = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
const place = (id: string, first: boolean) => (id === "dir" && first ? 0 : id === "rest" ? 2 : 1);
const nameClass = (id: string) => (id === "dot" ? 1 : id === "none" || id === "kind:other" ? 2 : id === "dir" ? 3 : 0);

type Bucket = { group: Group; entries: FileEntry[] };

/** Sorts a listing group by group; `groups` is null when the mode does not group by type. The group of an
 *  entry is found once, together with the totals, and the sort runs inside each group. */
export function groupEntries(entries: FileEntry[], sort: SortSpec, spec: GroupSpec, dirSize?: DirSizeOf): { entries: FileEntry[]; groups: Group[] | null } {
  if (!byType(spec.mode)) {
    const s = { ...sort, foldersOnTop: foldersFirst(spec.mode) };
    return { entries: [...entries].sort((a, b) => compare(a, b, s, dirSize)), groups: null };
  }
  const seen = new Map<string, Bucket>();
  for (const e of entries) {
    const id = groupId(e, spec.byKind);
    let bucket = seen.get(id);
    if (!bucket) seen.set(id, bucket = { group: { id, count: 0, size: 0, latest: -Infinity, types: 1 }, entries: [] });
    const g = bucket.group;
    g.count++;
    if (!e.isDir) g.size += e.size;
    if (e.modified != null && e.modified > g.latest) g.latest = e.modified;
    bucket.entries.push(e);
  }
  const first = foldersFirst(spec.mode);
  const buckets = mergeTail([...seen.values()]).sort(({ group: a }, { group: b }) =>
    place(a.id, first) - place(b.id, first)
    || (spec.order === "count" ? b.count - a.count : spec.order === "size" ? b.size - a.size : spec.order === "recent" ? (b.latest > a.latest ? 1 : b.latest < a.latest ? -1 : 0) : 0)
    || nameClass(a.id) - nameClass(b.id) || text(a.id, b.id));
  const sorted: FileEntry[] = [];
  for (const bucket of buckets) {
    bucket.entries.sort((a, b) => compare(a, b, sort, dirSize));
    for (const e of bucket.entries) sorted.push(e);
  }
  return { entries: sorted, groups: buckets.map((bucket) => bucket.group) };
}

/** Thousands of extensions would be thousands of headers: the rarest share one group. */
function mergeTail(all: Bucket[]): Bucket[] {
  const types = all.filter((b) => b.group.id.startsWith("ext:"));
  if (types.length <= MAX_TYPE_GROUPS) return all;
  types.sort(({ group: a }, { group: b }) => b.count - a.count || text(a.id, b.id));
  const rest: Bucket = { group: { id: "rest", count: 0, size: 0, latest: -Infinity, types: 0 }, entries: [] };
  for (const { group, entries } of types.splice(MAX_TYPE_GROUPS)) {
    rest.group.count += group.count;
    rest.group.size += group.size;
    rest.group.latest = Math.max(rest.group.latest, group.latest);
    rest.group.types++;
    for (const e of entries) rest.entries.push(e);
  }
  return [...all.filter((b) => !b.group.id.startsWith("ext:")), ...types, rest];
}

// ───────── layout of the list: sections of entries under headers, for the virtualised rows ─────────

export interface Section {
  /** null: the only section of a list without headers. */
  group: Group | null;
  collapsed: boolean;
  /** Its entries in the visible list. */
  start: number;
  count: number;
  top: number;
  /** Height of the header. */
  head: number;
  rows: number;
}
export interface Layout { sections: Section[]; cols: number; itemH: number; height: number; headers: boolean }
/** Where the cursor is: an entry, or the header of a group. */
export type Cursor = { index: number } | { head: string };

/** The entries on screen (those of collapsed groups left out) and the sections they are in.
 *  With `search`, groups that are collapsed open when no open group has an entry. */
export function openGroups(entries: FileEntry[], groups: Group[] | null, collapsed: ReadonlySet<string>, search: boolean): { visible: FileEntry[]; closed: ReadonlySet<string> } {
  const none: ReadonlySet<string> = new Set();
  if (!groups || !collapsed.size) return { visible: entries, closed: none };
  const closed = new Set(groups.filter((g) => collapsed.has(g.id)).map((g) => g.id));
  if (!closed.size || (search && closed.size === groups.length)) return { visible: entries, closed: none };
  const visible: FileEntry[] = [];
  let at = 0;
  for (const g of groups) {
    if (!closed.has(g.id)) for (let i = at; i < at + g.count; i++) visible.push(entries[i]);
    at += g.count;
  }
  return { visible, closed };
}

export function layoutOf(count: number, groups: Group[] | null, closed: ReadonlySet<string>, cols: number, itemH: number, headH: number): Layout {
  if (!groups) {
    const rows = Math.ceil(count / cols);
    return { sections: [{ group: null, collapsed: false, start: 0, count, top: 0, head: 0, rows }], cols, itemH, height: rows * itemH, headers: false };
  }
  let top = 0, start = 0;
  const sections = groups.map((group): Section => {
    const collapsed = closed.has(group.id), n = collapsed ? 0 : group.count;
    const section = { group, collapsed, start, count: n, top, head: headH, rows: Math.ceil(n / cols) };
    top += headH + section.rows * itemH;
    start += n;
    return section;
  });
  return { sections, cols, itemH, height: top, headers: true };
}

/** The last section for which `before` holds (they are in order). */
function lastWhere(sections: Section[], before: (s: Section) => boolean): number {
  let lo = 0, hi = sections.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (before(sections[mid])) lo = mid; else hi = mid - 1;
  }
  return lo;
}
/** The section of a visible entry. A collapsed one starts where the next begins, so the last is the one with entries. */
function sectionAt(l: Layout, index: number): Section {
  let i = lastWhere(l.sections, (s) => s.start <= index);
  while (i > 0 && !l.sections[i].count) i--;
  return l.sections[i];
}
export function placeOf(l: Layout, index: number): { top: number; col: number } {
  const s = sectionAt(l, index), k = index - s.start;
  return { top: s.top + s.head + Math.floor(k / l.cols) * l.itemH, col: k % l.cols };
}
export const headTop = (l: Layout, id: string): number | null => l.sections.find((s) => s.group?.id === id)?.top ?? null;
export const groupAt = (l: Layout, index: number): string | null => (l.headers && l.sections.length ? sectionAt(l, index).group?.id ?? null : null);

/** What is on screen: the entries [from, to) and the sections whose header is in it. */
export function windowOf(l: Layout, scrollTop: number, height: number, extra: number): { from: number; to: number; heads: Section[] } {
  const a = scrollTop - extra, b = scrollTop + height + extra;
  let from = -1, to = 0;
  const heads: Section[] = [];
  if (!l.sections.length) return { from: 0, to: 0, heads };
  for (let i = lastWhere(l.sections, (s) => s.top <= a); i < l.sections.length; i++) {
    const s = l.sections[i];
    if (s.top > b) break;
    if (s.head && s.top + s.head >= a) heads.push(s);
    const body = s.top + s.head;
    const r0 = Math.max(0, Math.floor((a - body) / l.itemH)), r1 = Math.min(s.rows, Math.ceil((b - body) / l.itemH));
    if (r1 <= r0) continue;
    if (from < 0) from = s.start + r0 * l.cols;
    to = Math.min(s.start + s.count, s.start + r1 * l.cols);
  }
  return { from: Math.max(0, from), to, heads };
}

/** The cursor `rows` lines further; headers are lines too, unless `entriesOnly` (extending a selection). */
export function moveCursor(l: Layout, at: Cursor | null, rows: number, entriesOnly = false): Cursor | null {
  const lines = (s: Section) => (s.head ? 1 : 0) + s.rows;
  const total = l.sections.reduce((n, s) => n + lines(s), 0);
  if (!total) return null;
  let line = 0, col = 0;
  if (!at) line = rows > 0 ? -rows : 0; // the first line
  else {
    const target = "head" in at ? l.sections.find((s) => s.group?.id === at.head) : sectionAt(l, at.index);
    for (const s of l.sections) {
      if (s === target) break;
      line += lines(s);
    }
    if ("index" in at && target) {
      const k = at.index - target.start;
      line += (target.head ? 1 : 0) + Math.floor(k / l.cols);
      col = k % l.cols;
    }
  }
  const on = (n: number): Cursor => {
    for (const s of l.sections) {
      if (n >= lines(s)) { n -= lines(s); continue; }
      if (s.head && n === 0) return { head: s.group!.id };
      return { index: Math.min(s.start + (n - (s.head ? 1 : 0)) * l.cols + col, s.start + s.count - 1) };
    }
    throw new Error("no such line");
  };
  const clamp = (n: number) => Math.max(0, Math.min(total - 1, n));
  let to = clamp(line + rows), found = on(to);
  const step = rows < 0 ? -1 : 1;
  while (entriesOnly && "head" in found) {
    to += step;
    if (to < 0 || to >= total) return at;
    found = on(to);
  }
  return found;
}
