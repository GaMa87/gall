// Pure rules of the interactive rebase editor: autosquash discovery, the plan of a squashed range and exec lines.
// No IPC: everything works on the plan that `git_rebase_plan` already returned.

export type RebaseAction = "pick" | "reword" | "squash" | "fixup" | "drop" | "edit" | "exec";
/** The message is asked for when it is needed (`commitMessages`). */
export type PlanCommit = { id: string; subject: string; message?: string };
/** The base of a plan from the root commit; the backend knows the same word (`ROOT_BASE`). */
export const ROOT_BASE = "root";
/** An exec line has no commit: its ID is empty on the wire and a local key in the editor. */
export const EXEC_PREFIX = "exec:";
export const MAX_EXEC = 4096;

export function isExec(entry: { action: RebaseAction }): boolean {
  return entry.action === "exec";
}
/** What the backend accepts as a command: one non-empty line (a line break would add plan lines). */
export function execProblem(command: string): "empty" | "line" | "long" | null {
  if (!command.trim()) return "empty";
  if (/[\r\n\0]/.test(command)) return "line";
  if (command.length > MAX_EXEC) return "long";
  return null;
}

const prefix = /^(fixup|squash|amend)! /;
/** The subject a `fixup!`, `squash!` or `amend!` commit points at, with repeated prefixes removed; null for other commits. */
export function autosquashTarget(subject: string): { kind: "fixup" | "squash" | "amend"; target: string } | null {
  const first = prefix.exec(subject);
  if (!first) return null;
  let target = subject.slice(first[0].length);
  while (prefix.test(target)) target = target.replace(prefix, "");
  return target ? { kind: first[1] as "fixup" | "squash" | "amend", target } : null;
}

export type Autosquash = {
  order: string[];
  actions: Record<string, "fixup" | "squash">;
  /** Commits whose message an `amend!` commit replaces: the ID of the latest such commit. */
  rewords: Record<string, string>;
  count: number;
};
/** The message an `amend!` commit gives its target: everything after its own subject. Empty when it has none. */
export function amendMessage(message: string): string {
  const at = message.indexOf("\n");
  return at < 0 ? "" : message.slice(at).trim();
}
/**
 * What `git rebase --autosquash` does with the plan (oldest first): every `fixup!` or `squash!` commit moves right
 * behind the latest earlier commit whose subject starts with the text after the prefix (or whose ID starts with it)
 * and becomes a fixup or squash. A commit without a target stays where it is. An `amend!` commit is folded like a
 * fixup, and its target is reworded with its message. Null when nothing would change.
 */
export function autosquash(commits: PlanCommit[]): Autosquash | null {
  const after = new Map<string, string[]>(); // target id -> commits that follow it, in plan order
  const actions: Record<string, "fixup" | "squash"> = {}, rewords: Record<string, string> = {};
  const moved = new Set<string>();
  commits.forEach((commit, index) => {
    const found = autosquashTarget(commit.subject);
    if (!found) return;
    // The latest earlier commit that was not itself folded into a group (a `fixup! fixup! x` points at `x`).
    let target: PlanCommit | undefined;
    for (let i = index - 1; i >= 0; i--) {
      const earlier = commits[i];
      if (moved.has(earlier.id)) continue;
      if (earlier.subject.startsWith(found.target) || earlier.id.startsWith(found.target)) { target = earlier; break; }
    }
    if (!target) return;
    after.set(target.id, [...(after.get(target.id) ?? []), commit.id]);
    actions[commit.id] = found.kind === "squash" ? "squash" : "fixup";
    if (found.kind === "amend") rewords[target.id] = commit.id;
    moved.add(commit.id);
  });
  if (!moved.size) return null;
  const order: string[] = [];
  for (const commit of commits) {
    if (moved.has(commit.id)) continue;
    order.push(commit.id);
    order.push(...(after.get(commit.id) ?? []));
  }
  return { order, actions, rewords, count: moved.size };
}
/** The part of a long list to draw: `count` items from `first`, with the room above and below for the others. */
export function listWindow(total: number, scrollTop: number, height: number, itemHeight: number, extra = 30): { first: number; last: number; above: number; below: number } {
  if (total <= 200 || itemHeight <= 0) return { first: 0, last: total, above: 0, below: 0 };
  const first = Math.max(0, Math.min(Math.floor(scrollTop / itemHeight) - extra, total - 1));
  const last = Math.min(total, first + Math.ceil(height / itemHeight) + 2 * extra);
  return { first, last, above: first * itemHeight, below: (total - last) * itemHeight };
}
/** Only the ID order matters to a caller that wants to know whether the plan differs from the original. */
export function sameOrder(commits: { id: string }[], order: string[]): boolean {
  return commits.length === order.length && commits.every((commit, index) => commit.id === order[index]);
}

export type SquashPlan = { entries: { id: string; action: "pick" | "squash"; message: string }[]; first: string };
/**
 * The plan that squashes the chosen commits into one: the oldest of them stays a pick, the others become squashes and
 * the last one carries the combined message. The rest of the plan (older and newer commits) is picked unchanged.
 * Null when the chosen commits are not one run of the plan, since squashing them would have to reorder commits.
 */
export function squashRange(commits: PlanCommit[], chosen: ReadonlySet<string>, message: string): SquashPlan | null {
  const positions = commits.map((commit, index) => (chosen.has(commit.id) ? index : -1)).filter((index) => index >= 0);
  if (positions.length < 2 || positions.length !== chosen.size) return null;
  if (positions[positions.length - 1] - positions[0] + 1 !== positions.length) return null;
  const first = commits[positions[0]].id, last = commits[positions[positions.length - 1]].id;
  return {
    first,
    entries: commits.map((commit) => ({ id: commit.id, action: !chosen.has(commit.id) || commit.id === first ? "pick" : "squash", message: commit.id === last ? message : "" })),
  };
}
/** The message a squash starts with: the messages of the chosen commits, oldest first, as `git rebase` would combine them. */
export function combinedMessage(commits: PlanCommit[], chosen: ReadonlySet<string>): string {
  return commits.filter((commit) => chosen.has(commit.id)).map((commit) => (commit.message ?? "").trim()).join("\n\n");
}
