import { fuzzyMatch } from "./fuzzy";
import type { LineRange } from "./gitFileHistory";

/** See gitabsorb.rs. */
export type AbsorbHunk = { index: number; file: string; oldStart: number; oldCount: number; newStart: number; newCount: number; lines: string[]; target: string | null; certain: boolean; reason: string | null; movable: boolean };
export type StackCommit = { id: string; subject: string };
export type AbsorbProposal = { token: string; hunks: AbsorbHunk[]; stack: StackCommit[]; reason: string | null };

/** The hunks of a file that the chosen lines touch; without lines all of the file. */
export function touchedHunks(hunks: AbsorbHunk[], file: string, range: LineRange | null): AbsorbHunk[] {
  return hunks.filter((hunk) => {
    if (hunk.file !== file || !hunk.movable) return false;
    if (!range) return true;
    const [start, count] = range.side === "new" ? [hunk.newStart, hunk.newCount] : [hunk.oldStart, hunk.oldCount];
    return count > 0 && start <= range.end && start + count - 1 >= range.start;
  });
}

/** Commits whose subject or ID matches, best first; without text newest first. */
export function rankCommits(stack: StackCommit[], query: string): StackCommit[] {
  const text = query.trim();
  if (!text) return stack;
  return stack.flatMap((commit, i) => {
    const score = commit.id.startsWith(text.toLowerCase()) ? Infinity : fuzzyMatch(text, commit.subject)?.score ?? -Infinity;
    return score === -Infinity ? [] : [{ commit, score, i }];
  }).sort((a, b) => (b.score === a.score ? a.i - b.i : b.score - a.score)).map(({ commit }) => commit);
}
