export type GitReference = { name: string; fullName: string; kind: "branch" | "remote" | "tag"; id: string; upstream: string | null; ahead: number; behind: number };
export type GitStash = { id: string; selector: string; message: string };
export type GitSubmodule = { name: string; path: string; url: string; id: string; state: string };
export type GitRemote = { name: string; fetchUrl: string; pushUrl: string };
export type Repository = { root: string; head: string; branch: string | null; refs: GitReference[]; stashes: GitStash[]; remotes: GitRemote[]; operation: string | null; operationCommit: { id: string; subject: string; step: [number, number] | null } | null; editStop: boolean; bisectActive: boolean; conflicts: string[]; token: string };
export type GitRequest = { action: string; [key: string]: string };
export type GitContext = "history" | "files" | "branches" | "tags" | "stashes" | "remotes" | "conflicts" | "details" | "tools";
export type GitCommand = { key: string; label: string; action: string; danger?: boolean };
const common: GitCommand[] = [
  { key: "f", label: "Fetch remote", action: "fetch" },
  { key: "p", label: "Pull (fast-forward only)", action: "pull" },
  { key: "P", label: "Push branch", action: "push" },
  { key: "R", label: "Refresh workspace", action: "refresh" },
  { key: "m", label: "Continue / skip / abort", action: "operation" },
  { key: "x", label: "Repository tools…", action: "tools" },
];
const commands: Record<GitContext, GitCommand[]> = {
  history: [
    { key: "f", label: "Fixup into preceding commit…", action: "quick:fixup" },
    { key: "s", label: "Squash into preceding commit…", action: "quick:squash" },
    { key: "d", label: "Drop selected commit…", action: "quick:drop", danger: true },
    { key: "Ctrl+j", label: "Move commit earlier…", action: "quick:moveEarlier" },
    { key: "Ctrl+k", label: "Move commit later…", action: "quick:moveLater" },
    { key: "r", label: "Reword commit…", action: "reword" },
    { key: "i", label: "Interactive rebase from this commit…", action: "interactiveRebase" },
    { key: " ", label: "Checkout selected commit…", action: "checkout" },
    { key: "n", label: "Create branch here…", action: "createBranch" },
    { key: "T", label: "Tag commit…", action: "createTag" },
    { key: "C", label: "Copy commit for cherry-pick", action: "copyCommit" },
    { key: "V", label: "Cherry-pick copied commit…", action: "cherryPick" },
    { key: "t", label: "Revert commit…", action: "revert" },
    { key: "g", label: "Reset to commit…", action: "reset" },
    { key: "W", label: "Mark comparison base / compare", action: "compareBase" },
    { key: "y", label: "Copy commit attribute…", action: "copyMenu" },
    { key: "b", label: "Bisect options…", action: "tools:bisect" },
    { key: "w", label: "Worktree options…", action: "tools:worktrees" },
    { key: "/", label: "Search loaded history", action: "search" },
  ],
  files: [
    { key: " ", label: "Stage / unstage selected file", action: "file: " },
    { key: "a", label: "Stage / unstage all files", action: "file:a" },
    { key: "c", label: "Commit message", action: "file:c" },
    { key: "A", label: "Amend last commit…", action: "file:A", danger: true },
    { key: "d", label: "Discard selected changes…", action: "file:d", danger: true },
    { key: "i", label: "Ignore / exclude selected file…", action: "file:i" },
    { key: "e", label: "Edit selected file", action: "file:e" },
    { key: "s", label: "Stash working changes…", action: "stashSave" },
    { key: "r", label: "Refresh files", action: "file:r" },
  ],
  branches: [
    { key: " ", label: "Checkout selected reference…", action: "checkout" },
    { key: "n", label: "Create branch here…", action: "createBranch" },
    { key: "c", label: "Checkout remote branch as local…", action: "checkoutRemote" },
    { key: "R", label: "Rename branch…", action: "renameBranch" },
    { key: "d", label: "Delete merged local branch…", action: "deleteBranch", danger: true },
    { key: "r", label: "Rebase current branch onto selection…", action: "rebase" },
    { key: "M", label: "Merge selection into current branch…", action: "merge" },
    { key: "u", label: "Set upstream…", action: "setUpstream" },
    { key: "T", label: "Create tag here…", action: "createTag" },
    { key: "g", label: "Reset to selected reference…", action: "reset" },
    { key: "w", label: "Worktree options…", action: "tools:worktrees" },
    { key: "y", label: "Copy reference name", action: "copyRef" },
  ],
  tags: [
    { key: " ", label: "Checkout tagged commit…", action: "checkout" },
    { key: "h", label: "Show history of this tag", action: "showHistory" },
    { key: "n", label: "Create tag at HEAD…", action: "createTag" },
    { key: "d", label: "Delete local tag…", action: "deleteTag", danger: true },
    { key: "g", label: "Reset to tagged commit…", action: "reset" },
    { key: "y", label: "Copy tag name", action: "copyRef" },
  ],
  stashes: [
    { key: " ", label: "Apply stash…", action: "stashApply" },
    { key: "g", label: "Pop stash…", action: "stashPop" },
    { key: "d", label: "Drop stash…", action: "stashDrop", danger: true },
    { key: "s", label: "Save working changes…", action: "stashSave" },
  ],
  remotes: [
    { key: "n", label: "Add remote…", action: "addRemote" },
    { key: "e", label: "Edit fetch URL…", action: "setRemoteUrl" },
    { key: "r", label: "Rename remote…", action: "renameRemote" },
    { key: "d", label: "Remove remote…", action: "removeRemote", danger: true },
  ],
  conflicts: [
    { key: "e", label: "Edit conflicted file", action: "editConflict" },
    { key: "o", label: "Use current Git side (ours)…", action: "resolveOurs" },
    { key: "t", label: "Use incoming Git side (theirs)…", action: "resolveTheirs" },
    { key: " ", label: "Mark file resolved…", action: "resolveMark" },
  ],
  // The Tools panel handles its own tab keys (w s b p n r); only the shared keys exist here.
  tools: [],
  details: [
    { key: "Enter", label: "Focus file diff", action: "detail:Enter" },
    { key: "y", label: "Copy selected file path", action: "detail:y" },
    { key: "b", label: "Blame selected file", action: "detail:b" },
    { key: "f", label: "File history", action: "detail:f" },
  ],
};
/** Short key hints for the footer bar, like lazygit's options bar. Every key must exist in `commands[context]`, except in `details` and `tools`, whose panels handle their own keys (a test enforces the rest). */
export const contextHints: Record<GitContext, [key: string, label: string][]> = {
  history: [["r", "reword"], ["f", "fixup"], ["s", "squash"], ["d", "drop"], ["i", "interactive"], ["y", "copy"], ["x", "tools"], [" ", "checkout"]],
  files: [[" ", "stage"], ["a", "stage all"], ["c", "commit"], ["d", "discard"], ["e", "edit"], ["s", "stash"]],
  branches: [[" ", "checkout"], ["r", "rebase"], ["M", "merge"], ["n", "new"], ["R", "rename"], ["d", "delete"]],
  tags: [[" ", "checkout"], ["n", "new"], ["d", "delete"], ["g", "reset"]],
  stashes: [[" ", "apply"], ["g", "pop"], ["d", "drop"], ["s", "save"]],
  remotes: [["n", "add"], ["e", "edit"], ["r", "rename"], ["d", "remove"]],
  conflicts: [["e", "edit"], ["o", "ours"], ["t", "theirs"], [" ", "mark resolved"]],
  details: [["j", "next file"], ["d", "diff"], ["v", "file"], ["f", "history"], ["b", "blame"], ["y", "copy path"]],
  tools: [["w", "worktrees"], ["s", "submodules"], ["b", "bisect"], ["p", "patches"], ["n", "notes"], ["r", "repository"]],
};
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
export function gitContextHint(context: GitContext): string {
  return contextHints[context].map(([key, label]) => `${key === " " ? "Space" : key} ${label}`).join(" · ");
}
export function gitCommands(context: GitContext): GitCommand[] {
  const local = commands[context];
  return [...local, ...common.filter((item) => !local.some((other) => other.key === item.key))];
}
export function gitCommandForKey(context: GitContext, key: string): GitCommand | undefined {
  return gitCommands(context).find((item) => item.key === key);
}

export type GitActionState = { commit: boolean; head: boolean; referenceKind?: string; stash: boolean; remote: boolean; conflict: boolean; branch: boolean; remotes: number; operation: string | null; conflicts: number };
export function gitActionEnabled(action: string, state: GitActionState): boolean {
  if (action.startsWith("quick:") || ["checkout", "reword", "interactiveRebase", "interactiveRebaseOnto", "merge", "rebase", "revert", "reset", "copyCommit", "copyMenu", "compareBase", "createTag"].includes(action)) return state.commit;
  if (action === "createBranch" || action === "copyHash") return state.commit || state.head;
  if (["renameBranch", "deleteBranch"].includes(action)) return state.referenceKind === "branch";
  if (action === "checkoutRemote") return state.referenceKind === "remote";
  if (action === "deleteTag") return state.referenceKind === "tag";
  if (action === "copyRef" || action === "showHistory") return !!state.referenceKind;
  if (action === "setUpstream") return (state.referenceKind === "branch" || state.branch) && state.remotes > 0;
  if (["stashApply", "stashPop", "stashDrop"].includes(action)) return state.stash;
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
  if (action.startsWith("quick:") || ["reword", "rebase", "interactiveRebase", "interactiveRebaseOnto"].includes(action)) return "Rewrite history";
  if (["copyCommit", "cherryPick", "revert", "reset", "merge"].includes(action)) return "Integrate";
  if (["fetch", "pull", "push", "forcePush", "setUpstream"].includes(action)) return "Remote";
  if (["search", "refresh"].includes(action) || action === "tools" || action.startsWith("tools:")) return "More";
  return null;
}
