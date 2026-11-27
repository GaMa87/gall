import { fuzzyMatch } from "./fuzzy";
import { remoteTagName } from "./gitBadges";
import type { GitReference, GitRemote, Repository } from "./gitRepository";
import { basename } from "./path";

/** What a new worktree checks out; `id` is the commit the user saw (see `WorktreeSource` in gitadvanced.rs). */
export type WorktreeSource =
  | { kind: "branch"; name: string; id: string }
  | { kind: "remote"; fullName: string; name: string; id: string }
  | { kind: "newBranch"; name: string; id: string }
  | { kind: "commit"; id: string; label: string | null };
export type WorktreeSelection = { kind: "commit"; id: string } | { kind: "reference"; ref: GitReference };

/** The local name of a remote branch (`origin/feature/x` → `feature/x`); remote names may contain slashes. */
export function localBranchName(name: string, remotes: Pick<GitRemote, "name">[]): string {
  const remote = remotes.map((r) => r.name).filter((r) => name.startsWith(r + "/")).sort((a, b) => b.length - a.length)[0];
  return remote ? name.slice(remote.length + 1) : name.slice(name.indexOf("/") + 1);
}

/** Source of the worktree that `W` creates for a selection: a branch is checked out, a remote branch gets its local
 *  tracking branch (or the existing one at the same commit), a commit or tag is checked out detached. */
export function worktreeSource(selection: WorktreeSelection, repository: Pick<Repository, "refs" | "remotes">): WorktreeSource | { error: string; name: string } {
  if (selection.kind === "commit") return { kind: "commit", id: selection.id, label: null };
  const ref = selection.ref;
  if (ref.kind === "branch") return { kind: "branch", name: ref.name, id: ref.id };
  // Tags (also a fetched one kept under `<remote>/tags/`) and `<remote>/HEAD` stand for a commit, not for a branch to track;
  // a real remote branch called `tags/v1` is a branch.
  const fetchedTag = remoteTagName(ref, repository.refs);
  if (ref.kind === "tag" || fetchedTag || /\/HEAD$/.test(ref.name)) return { kind: "commit", id: ref.id, label: fetchedTag ?? ref.name };
  const name = localBranchName(ref.name, repository.remotes);
  const local = repository.refs.find((r) => r.kind === "branch" && r.name === name);
  if (!local) return { kind: "remote", fullName: ref.fullName, name, id: ref.id };
  if (local.id === ref.id) return { kind: "branch", name, id: local.id };
  return { error: "A local branch {name} exists at another commit. Create the worktree from the local branch, or update it first.", name };
}

/** What the folder is named after, for the preview in the form. */
export function worktreeLabel(source: WorktreeSource): string {
  return source.kind === "commit" ? source.label ?? source.id.slice(0, 8) : source.name;
}

/** A worktree as Git's files describe it (see gitworktrees.rs). */
export type WorktreePlace = { path: string; branch: string | null; head: string; main: boolean; current: boolean; locked: boolean; missing: boolean; used: number | null };
export type WorktreeState = { branch: string | null; dirty: number; conflicts: number; ahead: number; behind: number; upstream: boolean };
export type CleanupCandidate = WorktreePlace & { reason: "merged" | "gone" | "missing" };

export const worktreeName = (place: Pick<WorktreePlace, "branch" | "head" | "path">) => place.branch ?? `${basename(place.path)} · ${place.head.slice(0, 8)}`;

/** A colour of its own for every worktree, the same wherever it is shown. */
export function worktreeColor(path: string): string {
  let hash = 0;
  for (const c of path.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase()) hash = (hash * 31 + c.codePointAt(0)!) >>> 0;
  return `hsl(${(hash * 137) % 360} 65% 55%)`;
}

/** Worktrees matching the typed text, best first; without text in the order they were used. */
export function rankPlaces(places: WorktreePlace[], query: string): { place: WorktreePlace }[] {
  const text = query.trim();
  if (!text) return places.map((place) => ({ place }));
  return places.flatMap((place, i) => {
    const scores = [place.branch ?? "", basename(place.path)].map((name) => fuzzyMatch(text, name)?.score ?? -Infinity);
    const score = Math.max(...scores);
    return score === -Infinity ? [] : [{ place, score, i }];
  }).sort((a, b) => b.score - a.score || a.i - b.i);
}

/** Whether `path` is the worktree folder or something in it. */
export function insideWorktree(path: string, folder: string): boolean {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  return norm(path) === norm(folder) || norm(path).startsWith(norm(folder) + "/");
}
