import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getState } from "../state/store";
import * as A from "../state/actions";

type Worktree = { path: string; head: string; branch: string | null; bare: boolean; locked: boolean; prunable: boolean };
type State = { repository: string | null; token: string; worktrees: Worktree[] };
type Props = { path: string; reloadKey: unknown; busy: boolean; vim: boolean; onBusy: (busy: boolean) => void; onChanged: () => void; onCreate: () => void };

const IGNORED = "Untracked or ignored files are present";
const folderName = (value: string) => value.split(/[\\/]/).filter(Boolean).pop() ?? value;

/** The Worktrees tab of the Git sidebar: every worktree of the repository, with open and remove. Create, lock and unlock stay in Repository tools. */
export function GitWorktreeList({ path, reloadKey, busy, vim, onBusy, onChanged, onCreate }: Props) {
  const [loaded, setLoaded] = useState<{ path: string; value: State } | null>(null);
  const [failure, setFailure] = useState<{ path: string; message: string } | null>(null);
  const request = useRef(0), currentPath = useRef(path);
  currentPath.current = path;
  const state = loaded?.path === path ? loaded.value : null;
  const error = failure?.path === path ? failure.message : "";
  const setError = (message: string) => setFailure({ path, message });
  const [selected, setSelected] = useState("");
  const load = useCallback(async () => {
    if (currentPath.current !== path) return null;
    const ticket = ++request.current;
    try {
      const value = await invoke<State>("git_advanced", { path, commit: null });
      if (ticket !== request.current || currentPath.current !== path) return null;
      setLoaded({ path, value }); setFailure(null); return value;
    } catch (e) {
      if (ticket === request.current && currentPath.current === path) setFailure({ path, message: String(e) });
      return null;
    }
  }, [path]);
  useEffect(() => { void load(); return () => { request.current++; }; }, [load, reloadKey]);
  const open = (tree: Worktree) => { if (tree.bare) return; A.openOverlay(null); A.navigate(getState().session?.activeBrowser ?? 0, tree.path); };
  const run = (request: Record<string, string>, token: string | undefined) => invoke<string>("git_advanced_action", { path, request, expected: token, id: `worktree-${Date.now()}-${Math.random().toString(36).slice(2)}` });
  const remove = async (tree: Worktree) => {
    if (!state || busy || tree.bare || tree.locked || tree.path === state.repository) return;
    onBusy(true); setError("");
    try {
      if (!await A.confirm("Remove worktree", `Remove this clean working folder and its worktree registration:\n${tree.path}\n\nThe branch and commits are retained. Local changes prevent removal.`, "Remove", true)) return;
      try { await run({ action: "removeWorktree", destination: tree.path }, state.token); }
      catch (e) {
        if (!String(e).includes(IGNORED)) throw e;
        const again = await A.confirm("Delete ignored files too?", `${tree.path}\n\nThis folder contains untracked or ignored files, for example node_modules, build output or caches. Removing the worktree deletes them permanently.\n\nChanges to tracked files and new files that are not ignored still block the removal.`, "Delete and remove", true);
        if (!again) return;
        const fresh = await load();
        if (!fresh) return;
        await run({ action: "removeWorktreeWithIgnored", destination: tree.path }, fresh.token);
      }
      onChanged();
    } catch (e) { setError(String(e)); }
    finally { onBusy(false); await load(); }
  };
  const keys = (e: React.KeyboardEvent, tree: Worktree) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (busy) return;
    if (e.key === " " || e.key === "Enter") { e.preventDefault(); e.stopPropagation(); open(tree); }
    else if (vim && e.key === "d") { e.preventDefault(); e.stopPropagation(); void remove(tree); }
    else if (vim && e.key === "n") { e.preventDefault(); e.stopPropagation(); onCreate(); }
  };
  if (!state) return <p className="gg-empty">{error || "Loading worktrees…"}</p>;
  return <>
    {error && <p className="gg-empty gg-worktree-error" role="alert">{error}</p>}
    {state.worktrees.map((tree) => {
      const current = tree.path === state.repository;
      return <div key={tree.path} className="gg-worktree-item">
        <button className={`gg-side-row gg-worktree-main ${current ? "current" : ""} ${selected === tree.path ? "selected" : ""}`} disabled={busy} title={tree.path} onFocus={() => setSelected(tree.path)} onClick={() => setSelected(tree.path)} onDoubleClick={() => open(tree)} onKeyDown={(e) => keys(e, tree)}>
          <span>{tree.branch ?? (tree.bare ? "Bare repository" : "Detached HEAD")}{current && <em className="gg-current-pill">Current</em>}{tree.locked && <em className="gg-current-pill">Locked</em>}{tree.prunable && <em className="gg-current-pill">Prunable</em>}</span>
          <small>{folderName(tree.path)} · {tree.head.slice(0, 8)}</small>
        </button>
        <span className="gg-worktree-actions">
          <button disabled={busy || tree.bare} onClick={() => open(tree)}>Open</button>
          <button disabled={busy || tree.bare || tree.locked || current} title={tree.locked ? "Unlock it in Repository tools first" : current ? "The current worktree cannot be removed here" : undefined} onClick={() => void remove(tree)}>Remove…</button>
        </span>
      </div>;
    })}
  </>;
}
