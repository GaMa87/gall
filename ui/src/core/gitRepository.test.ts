import { expect, it } from "vitest";
import { gitHints } from "./gitCommands";
import { activeConflicts, contextHints, gitActionEnabled, gitCommandForKey, gitCommands, gitMenuGroup, gitNavigationHint, gitPanelOf, rewritesHistory, stashAtCommit, stashEntryActions, type GitContext } from "./gitRepository";
it("uses lazygit actions for each focused panel without a prefix", () => {
  expect(gitCommandForKey("files", " ")?.action).toBe("file: ");
  expect(gitCommandForKey("files", "s")?.action).toBe("stashSave");
  expect(gitCommandForKey("branches", "r")?.action).toBe("rebase");
  expect(gitCommandForKey("stashes", "g")?.action).toBe("stashPop");
  expect(gitCommandForKey("history", "g")?.action).toBe("reset");
  expect(gitCommandForKey("history", "p")?.action).toBe("pull");
  expect(gitCommandForKey("history", "P")?.action).toBe("push");
});
it("registers a keyboard command for every drop in the graph and sidebar (cherry-pick, merge, rebase)", () => {
  // The drop confirmations name these registry commands as their keyboard equivalents.
  expect(gitCommands("history").some((c) => c.action === "cherryPick")).toBe(true);
  expect(gitCommands("history").some((c) => c.action === "cherryPickTo")).toBe(true);
  expect(gitCommands("branches").some((c) => c.action === "merge")).toBe(true);
  expect(gitCommands("branches").some((c) => c.action === "rebase")).toBe(true);
});
it("keeps commit-panel keys identical to lazygit and leaves rebase/merge keys to the branch panel", () => {
  // Lazygit commits panel: s squash, f fixup, r reword, d drop, i interactive rebase,
  // t revert, T tag, g reset, n new branch. Space and v choose commits, C and V cherry-pick them directly, c checks out.
  const lazygit: Record<string, string> = { s: "quick:squash", f: "quick:fixup", r: "reword", d: "quick:drop", i: "interactiveRebase", t: "revert", T: "createTag", g: "reset", n: "createBranch" };
  for (const [key, action] of Object.entries(lazygit)) expect(gitCommandForKey("history", key)?.action).toBe(action);
  for (const [key, action] of Object.entries({ " ": "pick", v: "pickRange", C: "cherryPick", V: "cherryPickTo", c: "checkout" })) expect(gitCommandForKey("history", key)?.action).toBe(action);
  // `b` is bisect options and `w` worktree options in lazygit (never a rebase); rebase/merge are branch keys.
  expect(gitCommandForKey("history", "b")?.action).toBe("tools:bisect");
  expect(gitCommandForKey("history", "w")?.action).toBe("tools:worktrees");
  for (const key of ["M", "I"]) expect(gitCommandForKey("history", key)).toBeUndefined();
  // `y` opens the copy-attribute menu on a commit; the Gall-only tools opener is `x` everywhere.
  expect(gitCommandForKey("history", "y")?.action).toBe("copyMenu");
  expect(gitCommandForKey("history", "a")?.action).toBe("applyPatch");
  expect(gitCommandForKey("branches", "y")?.action).toBe("copyRef");
  for (const context of ["history", "branches", "tags", "stashes", "remotes"] as GitContext[]) expect(gitCommandForKey(context, "x")?.action).toBe("tools");
  expect(gitCommandForKey("branches", "r")?.action).toBe("rebase");
  expect(gitCommandForKey("branches", "M")?.action).toBe("merge");
  expect(gitActionEnabled("rebase", { commit: true, head: true, stash: false, remote: false, conflict: false, branch: true, remotes: 1, operation: null, conflicts: 0 })).toBe(true);
});
it("only offers operations applicable to the selected reference and operation", () => {
  const state = { commit: false, head: true, stash: false, remote: false, conflict: false, branch: true, remotes: 1, operation: null, conflicts: 0 };
  expect(gitActionEnabled("reword", state)).toBe(false);
  expect(gitActionEnabled("renameBranch", { ...state, referenceKind: "tag" })).toBe(false);
  expect(gitActionEnabled("deleteBranch", { ...state, referenceKind: "remote" })).toBe(true);
  expect(gitActionEnabled("deleteBranch", { ...state, referenceKind: "tag" })).toBe(false);
  expect(gitActionEnabled("deleteTag", { ...state, referenceKind: "tag" })).toBe(true);
  expect(gitActionEnabled("renameBranch", { ...state, referenceKind: "branch" })).toBe(true);
  expect(gitActionEnabled("stashApply", state)).toBe(false);
  expect(gitActionEnabled("skip", { ...state, operation: "merge" })).toBe(false);
  expect(gitActionEnabled("continue", { ...state, operation: "rebase", conflicts: 1 })).toBe(false);
  expect(gitActionEnabled("continue", { ...state, operation: "rebase", conflicts: 0 })).toBe(true);
});
it("finds the stash that a graph row belongs to", () => {
  const stashes = [{ id: "aaa", selector: "stash@{0}", message: "On Dev: one" }, { id: "bbb", selector: "stash@{1}", message: "On Dev: two" }];
  expect(stashAtCommit(stashes, "bbb")?.selector).toBe("stash@{1}");
  expect(stashAtCommit(stashes, "ccc")).toBeUndefined();
  expect(stashAtCommit(stashes, null)).toBeUndefined();
  for (const action of ["quick:squash", "quick:fixup", "quick:drop", "quick:moveEarlier", "reword", "revert", "cherryPick", "reset", "rebase", "merge", "interactiveRebase"]) expect(rewritesHistory(action)).toBe(true);
  for (const action of ["stashDrop", "stashPop", "stashApply", "copyMenu", "createBranch", "createTag", "refresh"]) expect(rewritesHistory(action)).toBe(false);
  // Renaming a stash (`r`, WN-74) is an action on a saved entry like the others.
  expect(stashEntryActions).toEqual(["stashApply", "stashPop", "stashDrop", "stashBranch", "stashRename"]);
});
it("shows unambiguous contextual menus with a shared shortcut to the stashes", () => {
  for (const context of ["history", "files", "branches", "stashes", "remotes", "conflicts", "details"] as GitContext[]) {
    const keys = gitCommands(context).flatMap((c) => c.keys);
    expect(new Set(keys).size).toBe(keys.length);
    expect(gitCommandForKey(context, "z")).toMatchObject({ action: "showStashes", shared: true });
  }
  expect(gitCommandForKey("branches", "R")?.action).toBe("renameBranch");
  expect(gitCommandForKey("files", "R")?.action).toBe("refresh");
});

it("footer key hints only advertise keys that really exist in that panel", () => {
  for (const context of Object.keys(contextHints) as GitContext[]) {
    expect(contextHints[context].length).toBeGreaterThan(0);
    // Details keys j/d/v are handled locally by the details panel and are not Git commands.
    if (context === "details" || context === "tools" || context === "worktrees") continue;
    for (const [key] of contextHints[context]) expect(gitCommandForKey(context, key), `${context} ${key}`).toBeDefined();
  }
  expect(gitHints("history")).toContainEqual({ key: "r", label: "reword" });
  expect(gitHints("branches")).toContainEqual({ key: "r", label: "rebase" });
  expect(gitHints("branches")).toContainEqual({ key: " ", label: "checkout" });
  expect(gitNavigationHint("history")).toContain("l or Enter details");
  expect(gitNavigationHint("branches")).toContain("l commits");
  expect(gitNavigationHint("details")).toContain("h commits");
  expect(gitMenuGroup("tools:bisect")).toBe("More");
  expect(gitMenuGroup("tools")).toBe("More");
});

it("maps every key context to exactly one visible panel", () => {
  expect(gitPanelOf("history")).toBe("history");
  expect(gitPanelOf("files")).toBe("changes");
  expect(gitPanelOf("details")).toBe("details");
  expect(gitPanelOf("tools")).toBe("tools");
  for (const context of ["branches", "tags", "stashes", "remotes", "worktrees", "conflicts"] as GitContext[]) expect(gitPanelOf(context)).toBe("sidebar");
});

it("groups rarely used Git actions into context menu submenus", () => {
  expect(gitMenuGroup("quick:fixup")).toBe("Rewrite history");
  expect(gitMenuGroup("rebase")).toBe("Rewrite history");
  expect(gitMenuGroup("merge")).toBe("Integrate");
  expect(gitMenuGroup("reset")).toBe("Integrate");
  expect(gitMenuGroup("push")).toBe("Remote");
  expect(gitMenuGroup("checkout")).toBeNull();
  expect(gitMenuGroup("createBranch")).toBeNull();
});

const branchRef = (name: string) => ({ name, fullName: `refs/heads/${name}`, kind: "branch" as const, id: "a".repeat(40), upstream: null, ahead: 0, behind: 0, upstreamGone: false });
it("shows the active conflicts on the current branch row, or as an entry of their own when there is no such row", () => {
  const base = { operation: "rebase", conflicts: ["a.txt", "dir/b.txt"], branch: "topic" as string | null, refs: [branchRef("main"), branchRef("topic")] };
  expect(activeConflicts(base)).toEqual({ count: 2, first: "a.txt", onCurrentRow: true });
  // A detached rebase has no current local branch; so does a branch the list does not hold.
  expect(activeConflicts({ ...base, branch: null })).toEqual({ count: 2, first: "a.txt", onCurrentRow: false });
  expect(activeConflicts({ ...base, branch: "gone" })?.onCurrentRow).toBe(false);
  // A tag or remote of that name is not a local branch.
  expect(activeConflicts({ ...base, refs: [{ ...branchRef("topic"), kind: "remote" as const }] })?.onCurrentRow).toBe(false);
});
it("has no active conflicts without an operation or without unresolved files", () => {
  const base = { operation: null as string | null, conflicts: ["a.txt"], branch: "main" as string | null, refs: [branchRef("main")] };
  expect(activeConflicts(base)).toBeNull();
  expect(activeConflicts({ ...base, operation: "merge", conflicts: [] })).toBeNull();
  expect(activeConflicts(null)).toBeNull();
  expect(activeConflicts(undefined)).toBeNull();
});
