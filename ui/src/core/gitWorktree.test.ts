import { describe, beforeEach, expect, it, vi } from "vitest";
import type { GitReference } from "./gitRepository";
import { gitActionEnabled, gitCommandForKey, type GitContext } from "./gitRepository";
import { localBranchName, worktreeLabel, worktreeSource } from "./gitWorktree";

const mocks = vi.hoisted(() => ({ confirm: vi.fn(), invoke: vi.fn(), notify: vi.fn(), navigate: vi.fn(), sessionCmd: vi.fn(), openOverlay: vi.fn(), refreshAll: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../state/actions", () => ({ confirm: mocks.confirm, notify: mocks.notify, navigate: mocks.navigate, sessionCmd: mocks.sessionCmd, openOverlay: mocks.openOverlay }));
vi.mock("../state/controllers", () => ({ refreshAll: mocks.refreshAll }));
vi.mock("../state/store", () => ({ getState: () => ({
  session: { activeBrowser: 1 },
  settings: {
    worktreeLocation: "../{repo}-{branch}",
    worktreeLocations: { "/app": "{repo}/w/{branch}" },
    worktreeCommands: { "/app": ["npm", "ci"] },
    worktreeLinks: { "/app": true },
  },
}) }));
import { createWorktree, worktreeLocation } from "../state/gitWorktree";

const ref = (kind: GitReference["kind"], name: string, id = "a".repeat(40)): GitReference => ({ name, fullName: `refs/${kind === "branch" ? "heads" : kind === "tag" ? "tags" : "remotes"}/${name}`, kind, id, upstream: null, ahead: 0, behind: 0, upstreamGone: false });
const remotes = [{ name: "origin", fetchUrl: "", pushUrl: "" }, { name: "team/fork", fetchUrl: "", pushUrl: "" }];
const source = (selected: GitReference, refs: GitReference[] = []) => worktreeSource({ kind: "reference", ref: selected }, { refs, remotes });

beforeEach(() => vi.resetAllMocks());

const quickResult = (path: string, options: { existing?: boolean; copied?: number; linked?: number; warnings?: string[]; log?: string } = {}) =>
  JSON.stringify({ path, existing: false, copied: 0, linked: 0, warnings: [], log: "", ...options });

it("picks the source of a worktree from the selection", () => {
  const id = "a".repeat(40);
  expect(worktreeSource({ kind: "commit", id }, { refs: [], remotes })).toEqual({ kind: "commit", id, label: null });
  expect(source(ref("branch", "feature/x"))).toEqual({ kind: "branch", name: "feature/x", id });
  expect(source(ref("tag", "v1.0"))).toEqual({ kind: "commit", id, label: "v1.0" });
  expect(source(ref("remote", "origin/feature/x"))).toEqual({ kind: "remote", fullName: "refs/remotes/origin/feature/x", name: "feature/x", id });
  expect(source(ref("remote", "team/fork/topic"))).toMatchObject({ kind: "remote", name: "topic" });
  // Entries below refs/remotes that are no branches.
  expect(source(ref("remote", "origin/HEAD"))).toMatchObject({ kind: "commit", label: "origin/HEAD" });
  // `origin/tags/v2` is a fetched tag only when the tag v2 of the same commit exists; otherwise it is a real remote branch.
  expect(source(ref("remote", "origin/tags/v2"), [ref("tag", "v2")])).toMatchObject({ kind: "commit", label: "v2" });
  expect(source(ref("remote", "origin/tags/v2"))).toMatchObject({ kind: "remote", name: "tags/v2" });
  expect(source(ref("remote", "origin/tags/v2"), [ref("tag", "v2", "b".repeat(40))])).toMatchObject({ kind: "remote", name: "tags/v2" });
});
it("uses a local branch of the same commit and refuses one that differs", () => {
  expect(source(ref("remote", "origin/x"), [ref("branch", "x")])).toMatchObject({ kind: "branch", name: "x" });
  expect(source(ref("remote", "origin/x"), [ref("branch", "x", "b".repeat(40))])).toMatchObject({ error: expect.stringContaining("another commit"), name: "x" });
});
it("names the folder after the branch, tag or short hash", () => {
  expect(worktreeLabel({ kind: "branch", name: "a/b", id: "1" })).toBe("a/b");
  expect(worktreeLabel({ kind: "commit", id: "0123456789abcdef", label: null })).toBe("01234567");
  expect(localBranchName("unknown/a/b", remotes)).toBe("a/b");
});
it("creates with W in every panel that has commits and keeps comparing on Ctrl+E", () => {
  for (const context of ["history", "branches", "tags"] as GitContext[]) {
    expect(gitCommandForKey(context, "W")?.action).toBe("worktreeHere");
    expect(gitCommandForKey(context, "N")?.action).toBe("worktreeNewBranch");
    expect(gitCommandForKey(context, "w")?.action).toBe("tools:worktrees");
  }
  expect(gitCommandForKey("history", "Ctrl+e")?.action).toBe("compareBase");
  const none = { commit: false, head: true, stash: false, remote: false, conflict: false, branch: true, remotes: 0, operation: null, conflicts: 0 };
  expect(gitActionEnabled("worktreeHere", none)).toBe(false);
  expect(gitActionEnabled("worktreeHere", { ...none, commit: true })).toBe(true);
});
it("offers to open a created worktree and to take it back", async () => {
  const changed = vi.fn();
  mocks.invoke.mockResolvedValueOnce(quickResult("/app/w/x")).mockResolvedValueOnce("");
  expect(await createWorktree("/app", { kind: "branch", name: "x", id: "1" }, changed)).toBe("/app/w/x");
  expect(mocks.invoke).toHaveBeenNthCalledWith(1, "git_advanced_action", {
    path: "/app", id: expect.stringMatching(/^worktree-/), expected: "",
    request: {
      action: "quickWorktree", source: { kind: "branch", name: "x", id: "1" },
      template: "../{repo}-{branch}", templates: { "/app": "{repo}/w/{branch}" },
      commands: { "/app": ["npm", "ci"] }, links: { "/app": true }, destination: null,
    },
  });
  const [text, kind, actions] = mocks.notify.mock.calls[0];
  expect(text).toContain("/app/w/x");
  expect(kind).toBe("success");
  expect(actions.map((a: { label: string }) => a.label)).toEqual(["Open", "Open in new tab", "Undo"]);
  actions[0].run(); expect(mocks.navigate).toHaveBeenCalledWith(1, "/app/w/x");
  actions[1].run(); expect(mocks.sessionCmd).toHaveBeenCalledWith({ op: "newTab", b: 1, path: "/app/w/x", activate: true });
  expect(changed).not.toHaveBeenCalled();
  actions[2].run();
  await vi.waitFor(() => expect(changed).toHaveBeenCalledOnce());
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, "git_advanced_action", {
    path: "/app", id: expect.stringMatching(/^worktree-/), expected: "",
    request: { action: "undoWorktree", destination: "/app/w/x" },
  });
});
it("reports a refused undo, warnings and an existing worktree", async () => {
  mocks.invoke.mockResolvedValueOnce(quickResult("/w", { copied: 3, linked: 2, warnings: ["a: denied"], log: "setup output" })).mockRejectedValueOnce("changed");
  await createWorktree("/app", { kind: "commit", id: "1", label: null }, () => {});
  expect(mocks.notify.mock.calls[0][0]).toContain("3 ignored files copied, 2 folders linked");
  expect(mocks.notify.mock.calls[1][4]).toBe("a: denied");
  expect(mocks.notify.mock.calls[2][4]).toBe("setup output");
  mocks.notify.mock.calls[0][2][2].run();
  await vi.waitFor(() => expect(mocks.notify).toHaveBeenCalledWith("changed", "warn"));
  mocks.invoke.mockResolvedValueOnce(quickResult("/old", { existing: true }));
  mocks.notify.mockClear();
  await createWorktree("/app", { kind: "branch", name: "x", id: "1" }, () => {});
  expect(mocks.notify.mock.calls[0][2].map((a: { label: string }) => a.label)).toEqual(["Open", "Open in new tab"]);
});
it("brings a change to another worktree only after the check and the confirmation, and never without them", async () => {
  const { sendToWorktree, draggedChange, endChangeDrag, startChangeDrag } = await import("../state/gitWorktree");
  const change = { path: "/app", file: "a.txt", staged: false, lines: [3, 4] };
  const target = { path: "/app-b", label: "feature-b" };
  const check = { token: "t1", copy: false, lines: 2, targetChanged: true };
  mocks.invoke.mockResolvedValueOnce(check).mockResolvedValueOnce(check);
  mocks.confirm.mockResolvedValueOnce(true);
  expect(await sendToWorktree(change, target, "KEYS")).toBe(true);
  expect(mocks.invoke.mock.calls[0]).toEqual(["git_worktree_transfer", { path: "/app", target: "/app-b", file: "a.txt", staged: false, selected: [3, 4], expected: null, apply: false }]);
  expect(mocks.invoke.mock.calls[1]).toEqual(["git_worktree_transfer", { path: "/app", target: "/app-b", file: "a.txt", staged: false, selected: [3, 4], expected: "t1", apply: true }]);
  // The confirmation names both worktrees, the lines, the recovery reference and the key.
  const [title, body] = mocks.confirm.mock.calls[0];
  expect(title).toContain("feature-b");
  for (const part of ["a.txt", "app", "feature-b", "recovery reference", "KEYS", "chosen changed lines"]) expect(body).toContain(part);
  // Cancelling applies nothing.
  mocks.invoke.mockClear(); mocks.invoke.mockResolvedValueOnce(check); mocks.confirm.mockResolvedValueOnce(false);
  expect(await sendToWorktree(change, target, "KEYS")).toBe(false);
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  // A change that does not fit stops at the check, before any question.
  const reason = "The change does not fit the file in the other worktree";
  mocks.invoke.mockClear(); mocks.confirm.mockClear(); mocks.invoke.mockRejectedValueOnce(reason);
  await expect(sendToWorktree(change, target, "KEYS")).rejects.toBe(reason);
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  expect(mocks.confirm).not.toHaveBeenCalled();
  // The dragged change is kept in memory (WebKitGTK drops custom types) and ends with the drag.
  const data = { setData: vi.fn(), effectAllowed: "" };
  startChangeDrag({ dataTransfer: data as unknown as DataTransfer }, change);
  expect(draggedChange()).toEqual(change);
  expect(data.setData).toHaveBeenCalledWith("text/plain", "a.txt");
  endChangeDrag();
  expect(draggedChange()).toBeNull();
});
it("registers a keyboard command for bringing a change to another worktree", () => {
  expect(gitCommandForKey("files", "w")?.action).toBe("file:w");
});
it("previews with the saved templates unless others are given", async () => {
  mocks.invoke.mockResolvedValue({});
  await worktreeLocation("/app", "x");
  expect(mocks.invoke).toHaveBeenLastCalledWith("git_worktree_location", { path: "/app", template: "../{repo}-{branch}", templates: { "/app": "{repo}/w/{branch}" }, branch: "x" });
  await worktreeLocation("/app", "x", "{repo}/{branch}", {});
  expect(mocks.invoke).toHaveBeenLastCalledWith("git_worktree_location", { path: "/app", template: "{repo}/{branch}", templates: {}, branch: "x" });
});

describe("worktrees as places", () => {
  const place = (path: string, branch: string | null): import("./gitWorktree").WorktreePlace => ({ path, branch, head: "0123456789abcdef", main: false, current: false, locked: false, missing: false, used: 1 });
  const places = [place("C:\\repo", "main"), place("C:\\repo\\.worktrees\\login", "feature/login"), place("C:\\wt\\fix", null)];
  it("ranks by branch and folder and keeps the order of use without text", async () => {
    const { rankPlaces, worktreeName } = await import("./gitWorktree");
    expect(rankPlaces(places, " ").map((row) => row.place)).toEqual(places);
    expect(rankPlaces(places, "login")[0].place).toBe(places[1]);
    expect(rankPlaces(places, "fix").map((row) => row.place)).toEqual([places[2]]);
    expect(rankPlaces(places, "zzzz")).toEqual([]);
    expect(places.map(worktreeName)).toEqual(["main", "feature/login", "fix · 01234567"]);
  });
  it("knows what is inside a worktree", async () => {
    const { insideWorktree } = await import("./gitWorktree");
    expect(insideWorktree("C:\\wt\\fix\\src", "C:/wt/fix/")).toBe(true);
    expect(insideWorktree("/wt/fix", "/wt/fix")).toBe(true);
    expect(insideWorktree("/wt/fix-2", "/wt/fix")).toBe(false);
  });
  it("gives a worktree the same colour however its path is written", async () => {
    const { worktreeColor } = await import("./gitWorktree");
    expect(worktreeColor("C:\\wt\\fix")).toBe(worktreeColor("c:/wt/fix/"));
    expect(worktreeColor("/wt/a")).not.toBe(worktreeColor("/wt/b"));
  });
  it("offers the temporary worktree on X", () => {
    for (const context of ["history", "branches", "tags"] as const) expect(gitCommandForKey(context, "X")?.action).toBe("worktreeTry");
  });
});
