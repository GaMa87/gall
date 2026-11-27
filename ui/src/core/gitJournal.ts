/** The operation journal (see gitjournal.rs). */
export type JournalPoint = { reference: string | null; id: string };
export type JournalKind = "commit" | "amend" | "merge" | "rebase" | "cherryPick" | "revert" | "resetSoft" | "resetMixed" | "resetHard" | "rewrite" | "checkout" | "pull" | "absorb";
export type JournalEntry = { id: number; kind: JournalKind; label: string; time: number; before: JournalPoint; after: JournalPoint; snapshot: string | null };
/** `entries` oldest first; `applied` of them are done, the rest can be redone. */
export type JournalView = { entries: JournalEntry[]; applied: number; current: JournalPoint | null; undoBlocked: string | null; redoBlocked: string | null };

export const journalKinds: Record<JournalKind, string> = {
  commit: "Commit", amend: "Amend", merge: "Merge", rebase: "Rebase", cherryPick: "Cherry-pick", revert: "Revert", resetSoft: "Soft reset", resetMixed: "Mixed reset", resetHard: "Hard reset",
  rewrite: "History rewrite", checkout: "Checkout", pull: "Pull", absorb: "Absorb",
};
export const pointName = (point: JournalPoint) => point.reference?.replace(/^refs\/heads\//, "") ?? "HEAD";
const short = (id: string) => id.slice(0, 8) || "—";
/** "main 1a2b3c4d → 5e6f7a8b", or "main → feature" when another branch was checked out. */
export function refMove(from: JournalPoint, to: JournalPoint): string {
  if (from.reference !== to.reference) return `${pointName(from)}${from.reference ? "" : ` ${short(from.id)}`} → ${pointName(to)}${to.reference ? "" : ` ${short(to.id)}`}`;
  return `${pointName(from)} ${short(from.id)} → ${short(to.id)}`;
}
export type JournalNode = { applied: number; entry: JournalEntry | null; state: "done" | "current" | "undone" };
/** The timeline, newest first: a node is the state after its operation, the last one the state before all of them. */
export function journalNodes(view: JournalView): JournalNode[] {
  const nodes: JournalNode[] = view.entries.map((entry, i) => ({ applied: i + 1, entry, state: i + 1 === view.applied ? "current" : i + 1 < view.applied ? "done" : "undone" }));
  if (view.entries.length) nodes.unshift({ applied: 0, entry: null, state: view.applied === 0 ? "current" : "done" });
  return nodes.reverse();
}
/** What a jump does: the operations it goes through and every reference it moves, from where it is to where it ends. */
export function jumpPreview(view: JournalView, applied: number): { undo: number; redo: number; moves: string[] } {
  const steps = applied < view.applied ? view.entries.slice(applied, view.applied).reverse().map((e) => [e.after, e.before] as const) : view.entries.slice(view.applied, applied).map((e) => [e.before, e.after] as const);
  const moves: { from: JournalPoint; to: JournalPoint }[] = [];
  for (const [from, to] of steps) {
    const last = moves[moves.length - 1];
    // Several steps on one branch are one move of it.
    if (last && last.to.reference === from.reference && from.reference === to.reference && last.from.reference === from.reference) last.to = to; else moves.push({ from, to });
  }
  return { undo: Math.max(0, view.applied - applied), redo: Math.max(0, applied - view.applied), moves: moves.map((m) => refMove(m.from, m.to)) };
}
