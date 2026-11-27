import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getState } from "../state/store";
import * as A from "../state/actions";
import { tt } from "../i18n";
import { gitPanelKey } from "../core/gitCommands";
import { api } from "../api/ipc";
import { eventAnchor } from "../core/anchor";
import { compareWorktrees, draggedChange, endChangeDrag, sendToWorktree, transferKeyboard, useWorktreeStates, worktreeMenu, type ChangeDrag } from "../state/gitWorktree";
import { WorktreeBadges, WorktreeDot } from "./GitWorktrees";
import type { MenuItem } from "../state/store";

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
  const [first, setFirst] = useState<string | null>(null);
  const [dropOn, setDropOn] = useState<string | null>(null);
  // Asked after the list is on screen, one row after the other.
  const states = useWorktreeStates(useMemo(() => (state?.worktrees ?? []).filter((tree) => !tree.bare && !tree.prunable).map((tree) => tree.path), [state]), reloadKey);
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
      if (!await A.confirm(tt("Remove worktree"), tt("Remove this clean working folder and its worktree registration:\n{path}\n\nThe branch and commits are retained. Local changes prevent removal.", { path: tree.path }), tt("Remove"), true)) return;
      try { await run({ action: "removeWorktree", destination: tree.path }, state.token); }
      catch (e) {
        if (!String(e).includes(IGNORED)) throw e;
        const again = await A.confirm(tt("Delete ignored files too?"), tt("{path}\n\nThis folder contains untracked or ignored files, for example node_modules, build output or caches. Removing the worktree deletes them permanently.\n\nChanges to tracked files and new files that are not ignored still block the removal.", { path: tree.path }), tt("Delete and remove"), true);
        if (!again) return;
        const fresh = await load();
        if (!fresh) return;
        await run({ action: "removeWorktreeWithIgnored", destination: tree.path }, fresh.token);
      }
      onChanged();
    } catch (e) { setError(String(e)); }
    finally { onBusy(false); await load(); }
  };
  const act = async (request: Record<string, string>) => {
    if (!state || busy) return;
    onBusy(true); setError("");
    try { const note = await run(request, state.token); if (note.trim()) A.notify(note.trim()); onChanged(); }
    catch (e) { setError(String(e)); }
    finally { onBusy(false); await load(); }
  };
  const move = async (tree: Worktree) => {
    if (!state || busy || tree.bare || tree.locked || tree.path === state.repository) return;
    const to = await A.promptText(tt("Move worktree to a new absolute folder"), tree.path, "Move");
    if (to?.trim() && to.trim() !== tree.path) await act({ action: "moveWorktree", destination: tree.path, to: to.trim() });
  };
  const repair = async () => {
    const folder = await A.promptText(tt("Repair: the folder where the worktree is now"), "", "Repair");
    if (folder?.trim()) await act({ action: "repairWorktree", destination: folder.trim() });
  };
  const compare = (tree: Worktree) => {
    if (tree.bare || tree.prunable) return;
    const other = state?.worktrees.find((item) => item.path === first);
    if (!other) { setFirst(tree.path); A.notify(tt("Choose the second worktree to compare with {name}", { name: tree.branch ?? folderName(tree.path) })); return; }
    setFirst(null);
    if (other.path !== tree.path) void compareWorktrees(path, other, tree);
  };
  /** A changed file or hunk dragged out of Working Changes is brought to another worktree, after the confirmation. */
  const accepts = (tree: Worktree) => !!draggedChange() && !tree.bare && !tree.prunable && tree.path !== state?.repository;
  const bring = async (change: ChangeDrag, tree: Worktree) => {
    if (busy) return;
    onBusy(true); setError("");
    try { if (await sendToWorktree(change, { path: tree.path, label: tree.branch ?? folderName(tree.path) }, transferKeyboard(vim))) onChanged(); }
    catch (e) { setError(String(e)); }
    finally { onBusy(false); await load(); }
  };
  const cleanUp = () => A.openOverlay({ type: "worktrees", path, cleanup: true });
  const menu = (tree: Worktree): MenuItem[] => [
    ...(tree.bare || tree.prunable ? [] : worktreeMenu(tree.path)),
    { separator: true },
    { label: first && first !== tree.path ? tt("Compare with {name}", { name: folderName(first) }) : tt("Compare with another worktree…"), disabled: tree.bare || tree.prunable || first === tree.path, run: () => compare(tree) },
    { label: tt("Move worktree…"), disabled: busy || tree.bare || tree.locked || tree.path === state?.repository, run: () => void move(tree) },
    { label: tt("Repair a moved worktree…"), disabled: busy, run: () => void repair() },
    { label: tt("Clean up worktrees…"), disabled: busy, run: cleanUp },
    { label: tt("Remove worktree…"), danger: true, disabled: busy || tree.bare || tree.locked || tree.path === state?.repository, run: () => void remove(tree) },
  ];
  const keys = (e: React.KeyboardEvent, tree: Worktree) => {
    if (busy) return;
    const { key, on } = gitPanelKey(["worktrees"], e, getState().settings?.shortcuts ?? {}, vim);
    if (key === " " || key === "Enter") { e.preventDefault(); e.stopPropagation(); open(tree); }
    else if (on && key === "d") { e.preventDefault(); e.stopPropagation(); void remove(tree); }
    else if (on && key === "n") { e.preventDefault(); e.stopPropagation(); onCreate(); }
    else if (on && ["c", "g", "r", "t", "e"].includes(key)) {
      e.preventDefault(); e.stopPropagation();
      if (key === "c") compare(tree);
      else if (key === "g") cleanUp();
      else if (key === "r") void move(tree);
      else if (tree.bare || tree.prunable) return;
      else if (key === "t") api.openTerminal(tree.path).catch(A.fail);
      else A.openOverlay({ type: "openWith", paths: [tree.path], mode: "editor" });
    }
  };
  if (!state) return <p className="gg-empty">{error || tt("Loading worktrees…")}</p>;
  return <>
    {error && <p className="gg-empty gg-worktree-error" role="alert">{error}</p>}
    {state.worktrees.map((tree) => {
      const current = tree.path === state.repository;
      return <div key={tree.path} className={`gg-worktree-item ${first === tree.path ? "compare-first" : ""}`} onDragOver={(e) => { if (accepts(tree)) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; setDropOn(tree.path); } else if (dropOn) setDropOn(null); }} onDragLeave={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropOn(null); }} onDrop={(e) => { const change = draggedChange(); setDropOn(null); if (!change || !accepts(tree)) return; e.preventDefault(); endChangeDrag(); void bring(change, tree); }} onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); setSelected(tree.path); A.showMenuAt(eventAnchor(e), menu(tree)); }}>
        <button className={`gg-side-row gg-worktree-main ${current ? "current" : ""} ${selected === tree.path ? "selected" : ""} ${dropOn === tree.path ? "gg-drop-target" : ""}`} disabled={busy} title={tree.path} onFocus={() => setSelected(tree.path)} onClick={() => setSelected(tree.path)} onDoubleClick={() => open(tree)} onKeyDown={(e) => keys(e, tree)}>
          <span><WorktreeDot path={tree.path} /> {tree.branch ?? (tree.bare ? tt("Bare repository") : tt("Detached HEAD"))}{current && <em className="gg-current-pill">{tt("Current")}</em>}{tree.locked && <em className="gg-current-pill">{tt("Locked")}</em>}{tree.prunable && <em className="gg-current-pill">{tt("Prunable")}</em>}</span>
          <small>{folderName(tree.path)} · {tree.head.slice(0, 8)} {!tree.bare && !tree.prunable && <WorktreeBadges state={states.get(tree.path)} />}</small>
        </button>
        <span className="gg-worktree-actions">
          <button disabled={busy || tree.bare} onClick={() => open(tree)}>{tt("Open")}</button>
          <button disabled={busy || tree.bare || tree.locked || current} title={tree.locked ? tt("Unlock it in Repository tools first") : current ? tt("The current worktree cannot be removed here") : undefined} onClick={() => void remove(tree)}>{tt("Remove…")}</button>
        </span>
      </div>;
    })}
  </>;
}
