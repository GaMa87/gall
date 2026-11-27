export type GitReference = { name: string; fullName: string; kind: "branch" | "remote" | "tag"; id: string; upstream: string | null; ahead: number; behind: number; upstreamGone: boolean };
export type GitStash = { id: string; selector: string; message: string };
export type GitSubmodule = { name: string; path: string; url: string; id: string; state: string };
export type GitRemote = { name: string; fetchUrl: string; pushUrl: string };
export type Repository = { root: string; head: string; branch: string | null; refs: GitReference[]; stashes: GitStash[]; remotes: GitRemote[]; operation: string | null; operationCommit: { id: string; subject: string; step: [number, number] | null } | null; editStop: boolean; bisectActive: boolean; conflicts: string[]; token: string };
export type GitRequest = { action: string; [key: string]: string | number | boolean | string[] };
import type { GitContext } from "./gitCommands";
export { contextHints, gitCommandForKey, gitCommands, type GitCommand, type GitContext } from "./gitCommands";
export type GitPanel = "sidebar" | "history" | "changes" | "details" | "tools";
/** Which of the four workspace panels a key context belongs to (the sidebar covers all its tabs). */
export function gitPanelOf(context: GitContext): GitPanel {
  if (context === "files") return "changes";
  if (context === "details") return "details";
  if (context === "tools") return "tools";
  return context === "history" ? "history" : "sidebar";
}
/** How to reach the neighbouring panels from the focused one (h/l), plus `[` `]` for sidebar tabs. */
export function gitNavigationHint(context: GitContext): string {
  switch (gitPanelOf(context)) {
    case "history": return "l or Enter details · h sidebar";
    case "details": return "h commits · l preview";
    case "changes": return "h commits · l diff";
    case "tools": return "h commits · Esc close";
    default: return "l commits · [ ] tabs";
  }
}
/** Actions that work on a saved stash entry (saving a new stash is not one of them). */
export const stashEntryActions = ["stashApply", "stashPop", "stashDrop", "stashBranch", "stashRename"];
/** History-rewriting or integrating actions; they make no sense on a stash commit. */
export function rewritesHistory(action: string): boolean {
  return action.startsWith("quick:") || ["reword", "amendSelected", "interactiveRebase", "interactiveRebaseOnto", "rebase", "merge", "revert", "cherryPick", "cherryPickTo", "reset"].includes(action);
}
/** The stash whose commit is the given graph row, so the row can offer the stash actions. */
export function stashAtCommit(stashes: GitStash[], id: string | null | undefined): GitStash | undefined {
  return id ? stashes.find((stash) => stash.id === id) : undefined;
}

export type GitActionState = { commit: boolean; head: boolean; referenceKind?: string; stash: boolean; remote: boolean; conflict: boolean; branch: boolean; remotes: number; operation: string | null; conflicts: number };
export function gitActionEnabled(action: string, state: GitActionState): boolean {
  if (action === "amendSelected") return state.commit && !state.operation && state.conflicts === 0;
  if (action.startsWith("quick:") || ["checkout", "reword", "interactiveRebase", "interactiveRebaseOnto", "merge", "rebase", "revert", "reset", "pick", "cherryPickTo", "savePatch", "copyMenu", "compareBase", "createTag", "worktreeHere", "worktreeNewBranch", "worktreeTry"].includes(action)) return state.commit;
  if (action === "createBranch" || action === "copyHash") return state.commit || state.head;
  if (action === "renameBranch") return state.referenceKind === "branch";
  if (action === "deleteBranch") return state.referenceKind === "branch" || state.referenceKind === "remote";
  if (action === "checkoutRemote") return state.referenceKind === "remote";
  if (action === "pushTag") return state.referenceKind === "tag" && state.remotes > 0 && !state.operation;
  if (action === "deleteRemoteTag") return state.referenceKind === "tag" && state.remotes > 0;
  if (action === "deleteTag") return state.referenceKind === "tag";
  if (action === "copyRef" || action === "showHistory") return !!state.referenceKind;
  if (action === "setUpstream") return (state.referenceKind === "branch" || state.branch) && state.remotes > 0;
  if (stashEntryActions.includes(action)) return state.stash;
  if (action === "stashSave") return state.head && !state.operation;
  if (["renameRemote", "setRemoteUrl", "removeRemote"].includes(action)) return state.remote;
  if (action === "fetch") return state.remotes > 0;
  if (["pull", "push", "forcePush"].includes(action)) return state.remotes > 0 && state.branch && !state.operation;
  if (action.startsWith("resolve") || action === "editConflict") return state.conflict;
  if (action === "continue") return !!state.operation && state.conflicts === 0;
  if (action === "skip") return !!state.operation && state.operation !== "merge";
  if (action === "abort") return !!state.operation;
  return true;
}

export const gitMenuGroups = ["Rewrite history", "Integrate", "Remote", "More"];
/** Submenu that holds an action in the context menu; null keeps it at the top level. */
export function gitMenuGroup(action: string): string | null {
  if (action.startsWith("quick:") || ["reword", "amendSelected", "rebase", "interactiveRebase", "interactiveRebaseOnto"].includes(action)) return "Rewrite history";
  if (["cherryPick", "cherryPickTo", "revert", "reset", "merge"].includes(action)) return "Integrate";
  if (["fetch", "pull", "push", "forcePush", "setUpstream"].includes(action)) return "Remote";
  if (["search", "refresh"].includes(action) || action === "tools" || action.startsWith("tools:")) return "More";
  return null;
}

/** The unresolved files of a running merge, rebase, cherry-pick or revert, for the ⚠ count of the sidebar. */
export type ActiveConflicts = { count: number; first: string; onCurrentRow: boolean };
/**
 * Where the active conflicts are shown: on the row of the current local branch, or, when there is none (a
 * detached rebase, a branch the list does not hold), as an entry of its own. `first` is the file the
 * control opens in the conflict editor. Null when no operation is stopped on a conflict.
 */
export function activeConflicts(repository: Pick<Repository, "operation" | "conflicts" | "branch" | "refs"> | null | undefined): ActiveConflicts | null {
  if (!repository || !repository.operation || repository.conflicts.length === 0) return null;
  const onCurrentRow = repository.branch !== null && repository.refs.some((ref) => ref.kind === "branch" && ref.name === repository.branch);
  return { count: repository.conflicts.length, first: repository.conflicts[0], onCurrentRow };
}
