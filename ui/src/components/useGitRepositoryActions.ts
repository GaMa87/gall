import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import { tt } from "../i18n";
import { forecastText } from "../state/gitForecast";
import { stashEntryActions, type GitReference, type GitRemote, type GitRequest, type GitStash, type Repository } from "../core/gitRepository";
import { createProgressId } from "../core/gitProgress";
import { useGitProgress, type GitProgressSeed } from "./GitProgressBar";

/** Zero represents an ordinary commit. A cancelled parent selection returns null. */
export async function chooseMainline(path: string, commit: string): Promise<number | null> {
  const { parents } = await invoke<{ parents: string[] }>("git_commit_details", { path, commit, base: null });
  if (parents.length < 2) return 0;
  const choice = await A.dialog({
    title: tt("Choose the mainline parent"),
    message: tt("Use this parent as the baseline for the merge commit’s changes."),
    buttons: [
      { id: "cancel", label: tt("Cancel") },
      ...parents.map((parent, index) => ({
        id: String(index + 1),
        label: tt("Parent {number} · {commit}", { number: index + 1, commit: parent.slice(0, 12) }),
        key: index < 9 ? String(index + 1) : undefined,
      })),
    ],
  });
  return !choice || choice.button === "cancel" ? null : Number(choice.button);
}

export type GitTarget =
  | { kind: "commit"; id: string }
  | { kind: "reference"; ref: GitReference }
  | { kind: "stash"; stash: GitStash }
  | { kind: "remote"; remote: GitRemote }
  | { kind: "conflict"; file: string }
  | { kind: "working" };

export function targetCommit(target: GitTarget | null): string | null {
  return target?.kind === "commit" ? target.id : target?.kind === "reference" ? target.ref.id : target?.kind === "stash" ? target.stash.id : null;
}

export function useGitRepositoryActions(path: string, repository: Repository | null, reload: () => Promise<void>, checkout: (target: string, branch: boolean, fresh?: boolean) => Promise<void>, onHistoryChange: (head: string) => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [output, setOutput] = useState("");
  const [job, setJob] = useState<string | null>(null);
  const [progressTask, setProgressTask] = useState<{ id: string; seed: GitProgressSeed } | null>(null);
  const activeJob = useRef<string | null>(null);
  const active = useRef(false);
  const mounted = useRef(true);
  const progress = useGitProgress(progressTask?.id ?? null, progressTask?.seed);
  const beginProgress = (id: string, operation: string, phase: string, cancellable = false, detail?: string) => {
    if (mounted.current) setProgressTask({ id, seed: { operation, phase, cancellable, detail } });
    return id;
  };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; if (activeJob.current) void invoke("git_network_cancel", { id: activeJob.current }); }; }, []);
  const refreshHistory = async () => { await reload(); const state = await invoke<Repository>("git_repository", { path }); if (mounted.current && state.head) onHistoryChange(state.head); };
  const perform = async (action: string, target: GitTarget | null) => {
    if (active.current || !repository) return;
    const commit = targetCommit(target);
    const reference = target?.kind === "reference" ? target.ref : null;
    const remote = target?.kind === "remote" ? target.remote : repository.remotes.find((r) => repository.refs.find((ref) => ref.name === repository.branch && ref.kind === "branch")?.upstream?.startsWith(r.name + "/")) ?? repository.remotes[0];
    const stash = target?.kind === "stash" ? target.stash : null;
    const name = reference?.name ?? "";
    if (action === "copyHash" || action === "copyRef") { await invoke("clip_set_text", { text: action === "copyRef" ? name : commit ?? repository.head }).catch((e) => setError(String(e))); return; }
    if (action === "terminal") { await api.openTerminal(repository.root).catch((e) => setError(String(e))); return; }
    let retryForced = false;
    active.current = true; setBusy(true); setError("");
    try {
      let request: GitRequest | null = null;
      if (action.startsWith("quick:") && commit) {
        const operation = action.slice(6);
        const label = ({ fixup: "Fixup into its parent", squash: "Squash into its parent", drop: "Drop this commit", moveEarlier: "Move this commit earlier", moveLater: "Move this commit later" } as Record<string, string>)[operation];
        const mergeDrop = operation === "drop" && (await invoke<{ parents: string[] }>("git_commit_details", { path, commit, base: null })).parents.length > 1;
        const description = mergeDrop
          ? tt("Drop merge {hash} and the commits it introduced. Later commits will be replayed. The original history is kept for recovery.", { hash: commit.slice(0, 12) })
          : `${label}: ${commit.slice(0, 12)} on ${repository.branch ?? "detached HEAD"}. Descendant IDs will change. The old HEAD is saved for recovery. Commit or stash working changes first.`;
        if (await A.confirm("Rewrite selected commit?", description, "Rewrite commit", true)) {
          const progressId = beginProgress(createProgressId("edit"), "rewrite", "Rewriting history");
          const result = await invoke<string>("git_commit_edit", { path, target: commit, action: operation, expected: repository.token, progressId });
          if (mounted.current) setOutput(result); await refreshHistory();
        }
        return;
      }
      const targetLabel = reference ? `${reference.name} (${reference.id.slice(0, 8)})` : commit?.slice(0, 12) ?? "";
      if (action === "amendSelected" && commit) {
        const confirmed = await A.confirm(
          tt("Amend selected commit?"),
          tt("Include the staged changes in {hash}. The original history is kept for recovery.", { hash: commit.slice(0, 12) }),
          tt("Amend commit"),
          true,
        );
        if (confirmed) {
          const progressId = beginProgress(createProgressId("amend"), "amend", "Amending commit");
          const result = await invoke<string>("git_amend_staged", { path, target: commit, expected: repository.token, progressId });
          if (mounted.current) setOutput(result);
          await refreshHistory();
        }
        return;
      }
      if (action === "reword" && commit) {
        const details = await invoke<{ message: string }>("git_commit_details", { path, commit, base: null });
        const answer = await A.dialog({ title: "Edit commit message", message: `${commit.slice(0, 12)} · ${repository.branch ?? "Detached HEAD"}\nDescendant commit IDs may change. The original history remains available for recovery.`, input: { value: details.message.trimEnd(), multiline: true }, buttons: [{ id: "cancel", label: "Cancel" }, { id: "save", label: "Save message", primary: true }] });
        const message = answer?.button === "save" ? answer.value : null;
        if (message?.trim() && message.trim() !== details.message.trim()) {
          const progressId = beginProgress(createProgressId("reword"), "reword", "Rewording commit");
          const result = await invoke<string>("git_reword", { path, target: commit, message, expected: repository.token, progressId });
          if (mounted.current) setOutput(result); await refreshHistory();
        }
        return;
      }
      if (action === "checkout") {
        if (reference?.kind === "branch") await checkout(name, true);
        else if (commit) await checkout(commit, false);
        return;
      }
      if (action === "checkoutRemote" && reference?.kind === "remote") {
        const short = reference.name.slice(reference.name.indexOf("/") + 1);
        const local = repository.refs.find((r) => r.kind === "branch" && r.name === short);
        if (local) {
          if (local.id !== reference.id && !await A.confirm("Local branch differs", `${short} already exists at ${local.id.slice(0, 8)}, while ${reference.name} is at ${reference.id.slice(0, 8)}. Switch to the existing local branch ${short}?`, "Switch")) return;
          await checkout(short, true); return;
        }
        if (!await A.confirm("Create tracking branch?", `Create local branch ${short} at ${reference.id.slice(0, 8)} tracking ${reference.name}, then switch to it.`, "Create & switch")) return;
        await invoke<string>("git_repository_action", { path, request: { action: "createBranch", name: short, target: reference.id }, expected: repository.token });
        const next = await invoke<Repository>("git_repository", { path });
        await invoke<string>("git_repository_action", { path, request: { action: "setUpstream", branch: short, upstream: reference.name }, expected: next.token });
        await reload(); await checkout(short, true, true);
        return;
      }
      if (action === "refresh") { await reload(); return; }
      if (action === "editConflict" && target?.kind === "conflict") { A.editPaths([`${repository.root}/${target.file}`]); return; }
      if (action === "createBranch") {
        const value = await A.promptText(`New branch at ${targetLabel || "HEAD"}`, "", "Create branch");
        if (value?.trim()) request = { action, name: value.trim(), target: commit ?? repository.head };
      } else if (action === "renameBranch" && reference?.kind === "branch") {
        const value = await A.promptText(`Rename branch ${name}`, name, "Rename branch");
        if (value?.trim() && value.trim() !== name) request = { action, name, newName: value.trim() };
      } else if (action === "deleteBranch" && reference?.kind === "branch") {
        if (await A.confirm("Delete local branch?", `Delete ${targetLabel}? Git will refuse a branch that is not merged or is checked out in a worktree.`, "Delete branch", true)) request = { action, name };
      } else if (action === "createTag") {
        const value = await A.promptText(`Tag ${targetLabel || "HEAD"}`, "", "Next");
        if (value?.trim()) {
          const message = await A.promptText("Tag description (empty creates a lightweight tag)", "", "Create tag");
          if (message !== null) request = { action, name: value.trim(), target: commit ?? repository.head, message };
        }
      } else if (action === "deleteTag" && reference?.kind === "tag") {
        if (await A.confirm("Delete local tag?", `Delete ${targetLabel}? Remote tags are unchanged.`, "Delete tag", true)) request = { action, name };
      } else if (action === "setUpstream") {
        const branch = reference?.kind === "branch" ? reference.name : repository.branch;
        if (!branch) throw new Error("Select a local branch first.");
        const refs = repository.refs.filter((r) => r.kind === "remote");
        const choice = await A.dialog({ title: `Upstream for ${branch}`, message: "Select the remote branch that this local branch should track.", buttons: [{ id: "cancel", label: "Cancel" }, ...refs.map((ref) => ({ id: ref.fullName, label: ref.name }))] });
        if (choice && choice.button !== "cancel") request = { action, branch, upstream: refs.find((r) => r.fullName === choice.button)!.name };
      } else if (action === "stashSave") {
        const choice = await A.dialog({ title: tt("Stash local changes"), input: { value: "", placeholder: tt("Name (optional)") }, buttons: [{ id: "cancel", label: "Cancel" }, { id: "tracked", label: tt("Without new files") }, { id: "keep", label: tt("Keep staged changes") }, { id: "ok", label: tt("Stash everything"), primary: true }] });
        if (choice && choice.button !== "cancel") request = { action, message: choice.value, keepIndex: choice.button === "keep", trackedOnly: choice.button === "tracked", files: [] };
      } else if (action === "stashBranch" && stash) {
        const name = await A.promptText(tt("Branch from {stash}, at the commit it was made on", { stash: stash.selector }), "", "Create branch");
        if (name?.trim()) request = { action, id: stash.id, name: name.trim() };
      } else if (action === "stashRename" && stash) {
        const label = await A.promptText(tt("Rename stash…"), stash.message.replace(/^(?:On |WIP on )[^:]+: /, ""), tt("Rename"));
        if (!label?.trim()) return;
        request = { action, id: stash.id, name: label };
      } else if (stashEntryActions.includes(action) && stash) {
        const verb = action === "stashApply" ? "Apply" : action === "stashPop" ? "Pop" : "Drop";
        if (await A.confirm(`${verb} stash?`, `${stash.selector} · ${stash.message}\n${stash.id}\n\n${action === "stashDrop" ? "Remove this stash from the list. A recovery reference is retained." : "Restore this stash into the working tree. A conflicting pop keeps the stash."}`, verb, action === "stashDrop")) request = { action, id: stash.id };
      } else if (action === "merge") {
        if (!commit) throw new Error("Select a commit or reference first.");
        const forecast = await forecastText(path, repository.head, commit);
        const choice = await A.dialog({
          title: tt("Merge commit?"),
          message: [commit.slice(0, 12), forecast].filter(Boolean).join("\n\n"),
          buttons: [
            { id: "cancel", label: tt("Cancel") },
            { id: "merge", label: tt("Merge"), key: "m" },
            { id: "noFastForward", label: tt("Create a merge commit"), key: "n" },
            { id: "fastForwardOnly", label: tt("Fast-forward only"), key: "f" },
            { id: "squash", label: tt("Squash merge"), key: "s" },
          ],
        });
        if (!choice || choice.button === "cancel") return;
        request = choice.button === "merge"
          ? { action, target: commit }
          : { action: "mergeWithMode", target: commit, mode: choice.button };
      } else if (["rebase", "revert"].includes(action)) {
        const id = commit;
        if (!id) throw new Error("Select a commit or reference first.");
        const mainline = action === "revert" ? await chooseMainline(path, id) : 0;
        if (mainline == null) return;
        if (mainline) {
          request = { action: "integrateMerge", target: id, mainline, revert: true };
        }
        const verb = action[0].toUpperCase() + action.slice(1);
        const forecast = action === "revert" ? "" : await forecastText(path, repository.head, id);
        if (!request && await A.confirm(`${verb} commit?`, `${forecast ? `${forecast}\n\n` : ""}${verb} ${id.slice(0, 12)} ${action === "rebase" ? "as the new base of" : "into"} ${repository.branch ?? "detached HEAD"}.\n\nCommit or stash local changes first. Conflicts remain available in the Conflicts panel. Recovery references preserve rewritten commits. Commits are unsigned and hooks are disabled.`, verb, action === "rebase")) request = { action, target: id };
      } else if (action === "reset" && commit) {
        const result = await A.dialog({ title: `Reset ${repository.branch ?? "HEAD"} to ${targetLabel}`, message: "s  Soft: keeps the index and working files.\nm  Mixed: also unstages changes.\nh  Hard: replaces tracked working files after saving recovery data; untracked files that the target would overwrite are refused.\nEsc  Cancel", buttons: [{ id: "cancel", label: "Cancel" }, { id: "soft", label: "Soft", key: "s" }, { id: "mixed", label: "Mixed", key: "m" }, { id: "hard", label: "Hard…", danger: true, key: "h" }] });
        if (result && result.button !== "cancel") {
          if (result.button !== "hard" || await A.confirm("Replace tracked working files?", `Hard reset to ${targetLabel} will remove tracked local edits from the working folder and check out initialized submodules at the commits it records. Gall saves the old HEAD and tracked changes, also those of submodules, for recovery.`, "Hard reset", true)) request = { action, target: commit, mode: result.button };
        }
      } else if (["continue", "skip", "abort"].includes(action)) {
        if (!repository.operation) throw new Error("No Git operation is in progress.");
        if (action === "continue" || await A.confirm(`${action === "abort" ? "Abort" : "Skip a step of"} ${repository.operation}?`, action === "abort" ? "Git restores the operation's original state. Resolution edits may be removed." : "Git discards this operation's current conflicting step and continues with the next commit.", action === "abort" ? "Abort operation" : "Skip step", true)) request = { action };
      } else if (action.startsWith("resolve") && target?.kind === "conflict") {
        const side = action === "resolveOurs" ? "ours" : action === "resolveTheirs" ? "theirs" : "mark";
        if (await A.confirm("Resolve conflict?", side === "mark" ? `Stage your current resolution of ${target.file}? Check that conflict markers have been removed.` : `Replace ${target.file} with the ${side} Git side and stage it? During rebase, ours is the rebased destination and theirs is the commit being replayed.`, "Resolve file", side !== "mark")) request = { action: "resolve", file: target.file, side };
      } else if (action === "addRemote") {
        const name = await A.promptText("Remote name", "origin", "Next");
        if (name?.trim()) { const url = await A.promptText(`URL for ${name}`, "", "Add remote"); if (url?.trim()) request = { action, name: name.trim(), url: url.trim() }; }
      } else if (action === "setRemoteUrl" && remote) {
        const url = await A.promptText(`Fetch URL for ${remote.name}`, remote.fetchUrl, "Save URL");
        if (url?.trim()) request = { action, name: remote.name, url: url.trim() };
      } else if (action === "renameRemote" && remote) {
        const name = await A.promptText(`Rename remote ${remote.name}`, remote.name, "Rename remote");
        if (name?.trim()) request = { action, name: remote.name, newName: name.trim() };
      } else if (action === "removeRemote" && remote) {
        if (await A.confirm("Remove remote?", `Remove ${remote.name} and its tracking references? The remote repository itself is unchanged.`, "Remove remote", true)) request = { action, name: remote.name };
      } else if (["fetch", "pull", "push", "forcePush", "pushTag", "deleteRemoteTag"].includes(action) || (action === "deleteBranch" && reference?.kind === "remote")) {
        // A remote branch is deleted on its own remote (`<remote>/<branch>`), not on the remote of the current branch.
        let owner = action === "deleteBranch" ? repository.remotes.find((r) => name.startsWith(r.name + "/")) : remote;
        const deletingTag = action === "deleteRemoteTag";
        const tagAction = action === "pushTag" || deletingTag;
        if (tagAction) {
          if (reference?.kind !== "tag") return;
          const choice = await A.dialog({
            title: deletingTag ? tt("Delete remote tag {tag}?", { tag: name }) : tt("Push tag {tag}?", { tag: name }),
            message: deletingTag ? tt("Choose the remote from which to delete this tag.") : tt("Choose the remote that should receive this tag."),
            buttons: [{ id: "cancel", label: tt("Cancel"), primary: true }, ...repository.remotes.map((item) => ({
              id: "remote:" + item.name, label: item.name + " · " + item.pushUrl, danger: deletingTag,
            }))],
          });
          if (!choice || choice.button === "cancel") return;
          owner = repository.remotes.find((item) => choice.button === "remote:" + item.name);
        }
        if (!owner) throw new Error(action === "deleteBranch" ? "The remote of this branch is not configured." : "Add a remote first.");
        const branchName = action === "deleteBranch" ? name.slice(owner.name.length + 1) : tagAction ? name : null;
        if (action === "deleteBranch" && (branchName === "HEAD" || /^tags\//.test(branchName ?? ""))) throw new Error("Select a remote branch to delete.");
        if (action === "deleteBranch" && !await A.confirm("Delete remote branch?", `Delete branch ${branchName} on ${owner.name}?\n${owner.pushUrl}\n\nThis removes the branch from the remote repository for everyone. Local branches are unchanged. Hooks are disabled.`, "Delete remote branch", true)) return;
        let lease: string | null = null;
        let networkAction = deletingTag ? "deleteTag" : action;
        if (action === "pull") {
          const choice = await A.dialog({
            title: tt("Pull from {remote}", { remote: owner.name }),
            message: owner.fetchUrl,
            buttons: [
              { id: "cancel", label: tt("Cancel") },
              { id: "pull", label: tt("Use Git pull settings"), key: "p" },
              { id: "pullMerge", label: tt("Merge"), key: "m" },
              { id: "pullRebase", label: tt("Rebase"), key: "r" },
            ],
          });
          if (!choice || choice.button === "cancel") return;
          networkAction = choice.button;
        }
        if (action === "forcePush") {
          const upstream = repository.refs.find((r) => r.name === repository.branch && r.kind === "branch")?.upstream ?? `${owner.name}/${repository.branch}`;
          lease = repository.refs.find((r) => r.kind === "remote" && r.name === upstream)?.id ?? null;
          if (!lease) throw new Error("Fetch the upstream before force-with-lease.");
        }
        if (action === "forcePush") {
          if (!await A.confirm("Force push with lease", `Your branch has diverged from the remote branch. Press Esc to cancel, or Enter to force push with lease.\n\n${owner.name} · ${owner.pushUrl}\nThe lease expects the remote branch at ${lease}; the push fails if someone else has pushed since.`, "Force push with lease", true)) return;
        } else if (action === "push" && !await A.confirm(`Push ${repository.branch ?? "detached HEAD"}?`, `Publish to ${owner.name}\n${owner.pushUrl}\n\nThis changes the remote branch. Hooks are disabled.`, "Push")) return;
        const id = crypto.randomUUID(); activeJob.current = id; setJob(id); setOutput("");
        const netPhase = networkAction === "fetch" ? "Fetching from remote" : networkAction.startsWith("pull") ? "Pulling from remote" : "Pushing to remote";
        beginProgress(id, networkAction, netPhase, true, owner.name);
        const unlisten = await listen<{ id: string; text: string }>("git-network-progress", (event) => { if (event.payload.id === id && mounted.current) setOutput((text) => (text + event.payload.text).slice(-12000)); });
        try { const text = await invoke<string>("git_network", { path, id, remote: owner.name, action: networkAction, expected: repository.token, lease, branchName }); if (mounted.current) setOutput(text); }
        finally { unlisten(); activeJob.current = null; if (mounted.current) { setJob(null); setProgressTask(null); } }
        await reload(); return;
      }
      if (request) {
        const reqPhase: Record<string, string> = { merge: "Merging", mergeWithMode: "Merging", rebase: "Rebasing", revert: "Reverting", integrateMerge: "Cherry-picking", reset: "Resetting working tree", stashSave: "Saving stash", stashApply: "Applying stash", stashPop: "Popping stash", stashBranch: "Creating branch from stash", continue: "Continuing operation", skip: "Skipping step", abort: "Aborting operation" };
        const progressId = beginProgress(createProgressId("repo"), request.action, reqPhase[request.action] ?? "Running Git operation");
        const result = await invoke<string>("git_repository_action", { path, request, expected: repository.token, progressId }); if (mounted.current) setOutput(result); if (["merge", "mergeWithMode", "integrateMerge", "rebase", "revert", "reset", "continue", "abort", "skip"].includes(request.action)) await refreshHistory(); else await reload();
      }
    } catch (e) {
      const text = String(e);
      if (action === "deleteBranch" && reference?.kind === "branch" && /not fully merged/i.test(text)) {
        // Like lazygit: offer `git branch -D`, naming the tip so the branch can be recreated.
        try {
          if (await A.confirm("Branch is not fully merged", `${name} has commits that no other branch contains. Deleting it leaves them reachable only through the reflog.\n\nTip: ${reference.id}\nRecreate it with: git branch ${name} ${reference.id.slice(0, 12)}`, "Force delete", true)) {
            const result = await invoke<string>("git_repository_action", { path, request: { action, name, force: true }, expected: repository.token });
            if (mounted.current) setOutput(result);
          }
        } catch (forced) { if (mounted.current) setError(String(forced)); }
        await reload().catch(() => {});
        return;
      }
      const rejected = action === "push" && /rejected|non-fast-forward|fetch first|failed to push/i.test(text);
      const missing = /No such file or directory \(os error 2\)/.test(text) ? `A file or folder Git needed no longer exists, so the operation stopped. Reload the workspace (the working tree or rebase state may have changed outside Gall) and try again.\n\n${text}` : text;
      if (rejected) {
        // Like lazygit: the force-with-lease confirmation explains why; Enter accepts it.
        retryForced = true;
        if (mounted.current) setOutput(text);
      } else if (mounted.current) setError(missing);
      await reload().catch(() => {});
    }
    finally { active.current = false; if (mounted.current) { setBusy(false); setProgressTask(null); } }
    if (retryForced && mounted.current) await perform("forcePush", target);
  };
  return { perform, refreshHistory, busy, error, setError, output, setOutput, job, progress, cancel: () => job && void invoke("git_network_cancel", { id: job }) };
}
