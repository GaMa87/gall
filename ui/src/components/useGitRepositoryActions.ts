import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import type { GitReference, GitRemote, GitRequest, GitStash, Repository } from "../core/gitRepository";

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
  const [copied, setCopied] = useState<string | null>(null);
  const activeJob = useRef<string | null>(null);
  const active = useRef(false);
  const mounted = useRef(true);
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
    if (action === "copyCommit") { if (commit) { setCopied(commit); A.notify(`Copied ${commit.slice(0, 8)} for cherry-pick. Use V to apply.`); } return; }
    if (action === "terminal") { await api.openTerminal(repository.root).catch((e) => setError(String(e))); return; }
    active.current = true; setBusy(true); setError("");
    try {
      let request: GitRequest | null = null;
      if (action.startsWith("quick:") && commit) {
        const operation = action.slice(6);
        const label = ({ fixup: "Fixup into its parent", squash: "Squash into its parent", drop: "Drop this commit", moveEarlier: "Move this commit earlier", moveLater: "Move this commit later" } as Record<string, string>)[operation];
        if (await A.confirm("Rewrite selected commit?", `${label}: ${commit.slice(0, 12)} on ${repository.branch ?? "detached HEAD"}. Descendant IDs will change. The old HEAD is saved for recovery. Commit or stash working changes first.`, "Rewrite commit", true)) {
          const result = await invoke<string>("git_commit_edit", { path, target: commit, action: operation, expected: repository.token });
          if (mounted.current) setOutput(result); await refreshHistory();
        }
        return;
      }
      const targetLabel = reference ? `${reference.name} (${reference.id.slice(0, 8)})` : commit?.slice(0, 12) ?? "";
      if (action === "reword" && commit) {
        const details = await invoke<{ message: string }>("git_commit_details", { path, commit, base: null });
        const answer = await A.dialog({ title: "Edit commit message", message: `${commit.slice(0, 12)} · ${repository.branch ?? "Detached HEAD"}\nDescendant commit IDs may change. The original history remains available for recovery.`, input: { value: details.message.trimEnd(), multiline: true }, buttons: [{ id: "cancel", label: "Cancel" }, { id: "save", label: "Save message", primary: true }] });
        const message = answer?.button === "save" ? answer.value : null;
        if (message?.trim() && message.trim() !== details.message.trim()) {
          const result = await invoke<string>("git_reword", { path, target: commit, message, expected: repository.token });
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
        const message = await A.promptText("Stash tracked and untracked changes", "", "Save stash");
        if (message !== null) request = { action, message };
      } else if (["stashApply", "stashPop", "stashDrop"].includes(action) && stash) {
        const verb = action === "stashApply" ? "Apply" : action === "stashPop" ? "Pop" : "Drop";
        if (await A.confirm(`${verb} stash?`, `${stash.selector} · ${stash.message}\n${stash.id}\n\n${action === "stashDrop" ? "Remove this stash from the list. A recovery reference is retained." : "Restore this stash into the working tree. A conflicting pop keeps the stash."}`, verb, action === "stashDrop")) request = { action, id: stash.id };
      } else if (["merge", "rebase", "cherryPick", "revert"].includes(action)) {
        const id = action === "cherryPick" ? copied ?? commit : commit;
        if (!id) throw new Error("Select a commit or reference first.");
        const verb = action === "cherryPick" ? "Cherry-pick" : action[0].toUpperCase() + action.slice(1);
        if (await A.confirm(`${verb} commit?`, `${verb} ${id.slice(0, 12)} ${action === "rebase" ? "as the new base of" : "into"} ${repository.branch ?? "detached HEAD"}.\n\nCommit or stash local changes first. Conflicts remain available in the Conflicts panel. Recovery references preserve rewritten commits. Commits are unsigned and hooks are disabled.`, verb, action === "rebase")) request = { action, target: id };
      } else if (action === "reset" && commit) {
        const result = await A.dialog({ title: `Reset ${repository.branch ?? "HEAD"} to ${targetLabel}`, message: "Soft keeps the index and working files. Mixed also unstages changes. Hard replaces tracked working files after saving recovery data; untracked collisions are refused.", buttons: [{ id: "cancel", label: "Cancel" }, { id: "soft", label: "Soft" }, { id: "mixed", label: "Mixed" }, { id: "hard", label: "Hard…", danger: true }] });
        if (result && result.button !== "cancel") {
          if (result.button !== "hard" || await A.confirm("Replace tracked working files?", `Hard reset to ${targetLabel} will remove tracked local edits from the working folder. Gall saves the old HEAD and tracked changes for recovery.`, "Hard reset", true)) request = { action, target: commit, mode: result.button };
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
      } else if (["fetch", "pull", "push", "forcePush"].includes(action)) {
        if (!remote) throw new Error("Add a remote first.");
        let lease: string | null = null;
        if (action === "forcePush") {
          const upstream = repository.refs.find((r) => r.name === repository.branch && r.kind === "branch")?.upstream ?? `${remote.name}/${repository.branch}`;
          lease = repository.refs.find((r) => r.kind === "remote" && r.name === upstream)?.id ?? null;
          if (!lease) throw new Error("Fetch the upstream before force-with-lease.");
        }
        if (action !== "fetch" && !await A.confirm(`${action === "pull" ? "Pull" : "Push"} ${repository.branch ?? "detached HEAD"}?`, `${action === "pull" ? "Fast-forward from" : "Publish to"} ${remote.name}\n${action === "pull" ? remote.fetchUrl : remote.pushUrl}\n\n${action === "forcePush" ? `Force-with-lease expects ${lease}. This can replace remote history.` : action === "pull" ? "Diverged history is refused. Use merge or rebase after fetching." : "This changes the remote branch. Hooks are disabled."}`, action === "pull" ? "Pull" : action === "forcePush" ? "Force-with-lease" : "Push", action === "forcePush")) return;
        const id = crypto.randomUUID(); activeJob.current = id; setJob(id); setOutput("");
        const unlisten = await listen<{ id: string; text: string }>("git-network-progress", (event) => { if (event.payload.id === id && mounted.current) setOutput((text) => (text + event.payload.text).slice(-12000)); });
        try { const text = await invoke<string>("git_network", { path, id, remote: remote.name, action, expected: repository.token, lease }); if (mounted.current) setOutput(text); }
        finally { unlisten(); activeJob.current = null; if (mounted.current) setJob(null); }
        await reload(); return;
      }
      if (request) { const result = await invoke<string>("git_repository_action", { path, request, expected: repository.token }); if (mounted.current) setOutput(result); if (["merge", "rebase", "cherryPick", "revert", "reset", "continue", "abort", "skip"].includes(request.action)) await refreshHistory(); else await reload(); }
    } catch (e) {
      const text = String(e);
      const rejected = action === "push" && /rejected|non-fast-forward|fetch first|failed to push/i.test(text);
      const missing = /No such file or directory \(os error 2\)/.test(text) ? `A file or folder Git needed no longer exists, so the operation stopped. Reload the workspace (the working tree or rebase state may have changed outside Gall) and try again.\n\n${text}` : text;
      if (mounted.current) setError(rejected ? `Push rejected: the remote branch has commits your branch does not contain (it diverged, for example after a rebase or reset).\nUse Actions → Force push with explicit lease… to overwrite it safely, or Pull to integrate the remote commits first.\n\n${text}` : missing);
      await reload().catch(() => {});
    }
    finally { active.current = false; if (mounted.current) setBusy(false); }
  };
  return { perform, busy, error, setError, output, setOutput, job, cancel: () => job && void invoke("git_network_cancel", { id: job }) };
}
