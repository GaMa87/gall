// Blame as it arrives from gitblame.rs, line by line.
export type BlameCommit = { id: string; author: string; time: number; subject: string; previous: [string, string] | null };
export type BlameRange = { commit: string; line: number; count: number; original: number; file: string };
export type BlamePart = { commits: BlameCommit[]; ranges: BlameRange[] };
/** One line: the commit it comes from and where it was there. */
export type BlameLine = { commit: BlameCommit; original: number; file: string };
export type BlameMode = "line" | "all";

export const uncommitted = (commit: BlameCommit) => /^0+$/.test(commit.id);

/** Lines by index (0-based); a hole has not arrived yet. */
export class BlameLines {
  readonly lines: (BlameLine | undefined)[] = [];
  private readonly commits = new Map<string, BlameCommit>();
  add(part: BlamePart): void {
    for (const commit of part.commits) this.commits.set(commit.id, commit);
    for (const range of part.ranges) {
      const commit = this.commits.get(range.commit);
      if (!commit) continue;
      for (let i = 0; i < range.count; i++) this.lines[range.line - 1 + i] = { commit, original: range.original + i, file: range.file };
    }
  }
}

/** The line of the file (0-based) for every line of a unified diff; null for removed lines and headers. */
export function diffLineNumbers(lines: string[]): (number | null)[] {
  let at = 0, inHunk = false;
  return lines.map((line) => {
    const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) { at = Number(hunk[1]) - 1; inHunk = true; return null; }
    if (line.startsWith("diff --git ")) inHunk = false;
    return inHunk && (line.startsWith("+") || line.startsWith(" ")) ? at++ : null;
  });
}

/** Keeps the latest blames; those of commits never change. */
export class BlameCache<T> {
  private readonly entries = new Map<string, T>();
  constructor(private readonly limit = 12) {}
  get(key: string): T | undefined {
    const value = this.entries.get(key);
    if (value !== undefined) { this.entries.delete(key); this.entries.set(key, value); }
    return value;
  }
  set(key: string, value: T): void {
    this.entries.delete(key);
    this.entries.set(key, value);
    if (this.entries.size > this.limit) this.entries.delete(this.entries.keys().next().value!);
  }
}
