// Revisions of a file and the lines a selection in a preview stands for.

/** `file` is the name the file had in that commit. */
export type HistoryEntry = { id: string; subject: string; author: string; date: string; file: string; /** The parents of the commit (a merge has several): the parent choice is known without its details. */ parents?: string[] };
export type LineHistoryEntry = { id: string; subject: string; author: string; date: string; patch: string };
/** Lines of a file, 1-based and inclusive; `old` when they exist only before the change. */
export type LineRange = { start: number; end: number; side: "new" | "old" };

/** The first and last line (0-based) of a selection, from the text before it and the text in it. */
export function selectedLines(before: string, selected: string): [number, number] | null {
  if (!selected.trim()) return null;
  const breaks = (text: string) => text.split("\n").length - 1;
  const first = breaks(before);
  // A selection that ends with a line break does not reach into the next line.
  return [first, first + breaks(selected.replace(/\n$/, ""))];
}

/** The lines of the file that the lines [first, last] of a unified diff show: those after the change, or the removed ones when nothing else is selected. */
export function diffRange(lines: string[], first: number, last: number): LineRange | null {
  let oldLine = 0, newLine = 0, inHunk = false;
  const found = { new: [] as number[], old: [] as number[] };
  for (let i = 0; i <= Math.min(last, lines.length - 1); i++) {
    const line = lines[i], hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) { oldLine = Number(hunk[1]); newLine = Number(hunk[2]); inHunk = true; continue; }
    if (line.startsWith("diff --git ")) inHunk = false;
    if (!inHunk || line.startsWith("\\")) continue;
    const chosen = i >= first;
    if (line.startsWith("+")) { if (chosen) found.new.push(newLine); newLine++; }
    else if (line.startsWith("-")) { if (chosen) found.old.push(oldLine); oldLine++; }
    // Git writes an empty context line as one space; an empty string is only the end of the text.
    else if (line.startsWith(" ")) { if (chosen) found.new.push(newLine); newLine++; oldLine++; }
  }
  const side = found.new.length ? "new" : "old", list = found[side];
  return list.length ? { start: Math.min(...list), end: Math.max(...list), side } : null;
}

/** "12-20", "12,20" or "12" as a range within a file of `count` lines. */
export function parseRange(text: string, count: number): { start: number; end: number } | null {
  const match = /^\s*(\d+)\s*(?:[-,–]\s*(\d+)\s*)?$/.exec(text);
  if (!match) return null;
  const a = Number(match[1]), b = match[2] ? Number(match[2]) : a;
  const start = Math.min(a, b), end = Math.min(Math.max(a, b), count);
  return start >= 1 && start <= end ? { start, end } : null;
}

/** Contents of revisions, loaded once each; the least recently used are dropped. */
export class RevisionCache {
  private readonly entries = new Map<string, Promise<string>>();
  constructor(private readonly load: (entry: HistoryEntry) => Promise<string>, private readonly limit = 24) {}
  get(entry: HistoryEntry): Promise<string> {
    const key = `${entry.id}:${entry.file}`;
    let value = this.entries.get(key);
    if (value) this.entries.delete(key);
    else {
      value = this.load(entry);
      // A failure is asked again; it must not be an unhandled rejection of a prefetch either.
      value.catch(() => { if (this.entries.get(key) === value) this.entries.delete(key); });
    }
    this.entries.set(key, value);
    if (this.entries.size > this.limit) this.entries.delete(this.entries.keys().next().value!);
    return value;
  }
}

/** File history lists matching revisions. Each dot is independent because skipped
 * commits can contain arbitrary ancestry and merges. */
export function fileHistoryRows(entries: readonly HistoryEntry[]): string {
  const field = (value: string) => value.replace(/[\0\r\n]/g, " ");
  return entries.map(({ id, subject, author, date }) =>
    [id, "", "", field(subject), field(author), field(date)].join("\0")).join("\n");
}

export function restoreFileHistoryCursor(ids: readonly string[], saved?: string | null): string | null {
  return saved && ids.includes(saved) ? saved : ids[0] ?? null;
}

/**
 * Whether the actions on the lines shown (history of the selected lines, the context menu entry) may run. They are
 * bound to the revision on screen, so they wait while the scrubber moves to another one: a selection made in the
 * old text must never be asked about the new revision, and the new text is not there yet.
 */
export function lineActionsEnabled(state: { view: string; transition: boolean; scrubDiff?: boolean }): boolean {
  // The change of a revision (the scrubber's diff) is a patch, not the lines of the file: line numbers of it mean nothing.
  return !state.transition && !state.scrubDiff && (state.view === "diff" || state.view === "file");
}

/** What a revision is compared with in the scrubber's diff: its parent (the chosen one of a merge), or nothing. */
export type ScrubBase = { kind: "root"; base: null } | { kind: "parent"; base: string; index: number; count: number };

/**
 * The base of the change shown for a revision. The first parent unless another is chosen (a merge); a revision without a
 * parent is shown against an empty tree, as Git does for a root commit. A choice past the last parent is the last one.
 */
export function scrubBase(parents: readonly string[], chosen = 0): ScrubBase {
  if (parents.length === 0) return { kind: "root", base: null };
  const index = Math.min(Math.max(0, chosen), parents.length - 1);
  return { kind: "parent", base: parents[index], index, count: parents.length };
}

/**
 * The change of a revision against one of its parents. The parents come with the history entry, so only an entry without
 * them asks for its commit. A merge that does not list the file against this parent answers `none` (not a failure).
 */
export async function scrubChange(
  entry: HistoryEntry,
  choice: number,
  io: {
    parentsOf: (id: string) => Promise<string[]>;
    patch: (key: string, load: () => Promise<{ text: string; none: boolean }>) => Promise<{ text: string; none: boolean }>;
    load: (entry: HistoryEntry, base: string | null) => Promise<string>;
  },
): Promise<{ text: string; none: boolean; base: ScrubBase }> {
  const base = scrubBase(entry.parents ?? await io.parentsOf(entry.id), choice);
  const got = await io.patch(`${entry.id}\0${base.base ?? ""}\0${entry.file}`, () => io.load(entry, base.base).then((text) => ({ text, none: false }), (e) => {
    if (String(e).includes("not-in-comparison:")) return { text: "", none: true };
    throw e;
  }));
  return { ...got, base };
}

/** Words for the base of a revision's change. */
export function scrubBaseText(base: ScrubBase): string {
  if (base.kind === "root") return "First commit: compared with an empty tree";
  return base.count > 1 ? `Merge: compared with parent ${base.index + 1} of ${base.count}` : "Compared with its parent";
}

/** Results by key, loaded once each; the least recently used are dropped. A failure is asked again and never kept. */
export class PromiseCache<T> {
  private readonly entries = new Map<string, Promise<T>>();
  constructor(private readonly limit = 32) {}
  get(key: string, load: () => Promise<T>): Promise<T> {
    let value = this.entries.get(key);
    if (value) this.entries.delete(key);
    else {
      value = load();
      value.catch(() => { if (this.entries.get(key) === value) this.entries.delete(key); });
    }
    this.entries.set(key, value);
    if (this.entries.size > this.limit) this.entries.delete(this.entries.keys().next().value!);
    return value;
  }
  get size() { return this.entries.size; }
}
