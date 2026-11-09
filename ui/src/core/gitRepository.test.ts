import { expect, it } from "vitest";
import { contextHints, gitActionEnabled, gitCommandForKey, gitCommands, gitContextHint, gitMenuGroup, gitNavigationHint, gitPanelOf, type GitContext } from "./gitRepository";
it("uses lazygit actions for each focused panel without a prefix", () => {
  expect(gitCommandForKey("files", " ")?.action).toBe("file: ");
  expect(gitCommandForKey("files", "s")?.action).toBe("stashSave");
  expect(gitCommandForKey("branches", "r")?.action).toBe("rebase");
  expect(gitCommandForKey("stashes", "g")?.action).toBe("stashPop");
  expect(gitCommandForKey("history", "g")?.action).toBe("reset");
  expect(gitCommandForKey("history", "p")?.action).toBe("pull");
  expect(gitCommandForKey("history", "P")?.action).toBe("push");
});
it("keeps commit-panel keys identical to lazygit and leaves rebase/merge keys to the branch panel", () => {
  // Lazygit commits panel: s squash, f fixup, r reword, d drop, i interactive rebase,
  // C/V cherry-pick, t revert, T tag, g reset, n new branch, space checkout.
  const lazygit: Record<string, string> = { s: "quick:squash", f: "quick:fixup", r: "reword", d: "quick:drop", i: "interactiveRebase", C: "copyCommit", V: "cherryPick", t: "revert", T: "createTag", g: "reset", n: "createBranch", " ": "checkout" };
  for (const [key, action] of Object.entries(lazygit)) expect(gitCommandForKey("history", key)?.action).toBe(action);
  // `b` is bisect options and `w` worktree options in lazygit (never a rebase); rebase/merge are branch keys.
  expect(gitCommandForKey("history", "b")?.action).toBe("tools:bisect");
  expect(gitCommandForKey("history", "w")?.action).toBe("tools:worktrees");
  for (const key of ["M", "I"]) expect(gitCommandForKey("history", key)).toBeUndefined();
  // `y` opens the copy-attribute menu on a commit; the Gall-only tools opener is `x` everywhere.
  expect(gitCommandForKey("history", "y")?.action).toBe("copyMenu");
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
  expect(gitActionEnabled("deleteBranch", { ...state, referenceKind: "remote" })).toBe(false);
  expect(gitActionEnabled("deleteTag", { ...state, referenceKind: "tag" })).toBe(true);
  expect(gitActionEnabled("renameBranch", { ...state, referenceKind: "branch" })).toBe(true);
  expect(gitActionEnabled("stashApply", state)).toBe(false);
  expect(gitActionEnabled("skip", { ...state, operation: "merge" })).toBe(false);
  expect(gitActionEnabled("continue", { ...state, operation: "rebase", conflicts: 1 })).toBe(false);
  expect(gitActionEnabled("continue", { ...state, operation: "rebase", conflicts: 0 })).toBe(true);
});
it("shows unambiguous contextual menus and reserves lower-case typing for inputs", () => {
  for (const context of ["history", "files", "branches", "stashes", "remotes", "conflicts", "details"] as GitContext[]) {
    const keys = gitCommands(context).map((c) => c.key);
    expect(new Set(keys).size).toBe(keys.length);
    expect(gitCommandForKey(context, "z")).toBeUndefined();
  }
  expect(gitCommandForKey("branches", "R")?.action).toBe("renameBranch");
  expect(gitCommandForKey("files", "R")?.action).toBe("refresh");
});

it("footer key hints only advertise keys that really exist in that panel", () => {
  for (const context of Object.keys(contextHints) as GitContext[]) {
    expect(contextHints[context].length).toBeGreaterThan(0);
    // Details keys j/d/v are handled locally by the details panel and are not Git commands.
    if (context === "details" || context === "tools") continue;
    for (const [key] of contextHints[context]) expect(gitCommandForKey(context, key), `${context} ${key}`).toBeDefined();
  }
  expect(gitContextHint("history")).toContain("r reword");
  expect(gitContextHint("branches")).toContain("r rebase");
  expect(gitContextHint("branches")).toContain("Space checkout");
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
  for (const context of ["branches", "tags", "stashes", "remotes", "conflicts"] as GitContext[]) expect(gitPanelOf(context)).toBe("sidebar");
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
