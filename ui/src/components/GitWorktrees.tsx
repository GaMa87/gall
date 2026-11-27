import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { age, ageWords } from "../core/age";
import { eventAnchor } from "../core/anchor";
import { HoverGuard } from "../core/hover";
import { basename, tildify } from "../core/path";
import { platformLabel } from "../core/keymap";
import { rankPlaces, worktreeColor, worktreeName, type CleanupCandidate, type WorktreePlace, type WorktreeState } from "../core/gitWorktree";
import { cleanUpWorktrees, openWorktree, useWorktreeStates, worktreeMenu } from "../state/gitWorktree";
import * as A from "../state/actions";
import { getState } from "../state/store";
import { tt } from "../i18n";
import { GitProgressBar, useLocalProgress } from "./GitProgressBar";
import "./GitPalette.css";
import "./GitWorktrees.css";

export function WorktreeDot({ path, title }: { path: string; title?: string }) {
  return <i className="wt-dot" style={{ background: worktreeColor(path) }} title={title} aria-hidden={title ? undefined : true} />;
}

const stateText = (state: WorktreeState) => [
  tt("{count} changed", { count: state.dirty }),
  state.conflicts ? tt("{count} in conflict", { count: state.conflicts }) : "",
  state.upstream ? tt("{ahead} ahead, {behind} behind", { ahead: state.ahead, behind: state.behind }) : tt("no upstream"),
].filter(Boolean).join(" · ");

/** What is known about a worktree, short: changed items, commits ahead and behind, conflicts. */
export function WorktreeBadges({ state }: { state: WorktreeState | null | undefined }) {
  if (state === undefined) return <span className="wt-badges dim">…</span>;
  if (!state) return <span className="wt-badges dim" title={tt("The state is not available")}>?</span>;
  return <span className="wt-badges" title={stateText(state)}>
    {state.conflicts > 0 && <em className="bad">!{state.conflicts}</em>}
    {state.dirty > 0 ? <em className="dirty">●{state.dirty}</em> : <em className="ok">✓</em>}
    {state.ahead > 0 && <em>↑{state.ahead}</em>}{state.behind > 0 && <em>↓{state.behind}</em>}
  </span>;
}

/** Jump between the worktrees of a repository (Ctrl+Alt+W): the most recently used first, typed text filters. */
export function WorktreeSwitcher({ path }: { path: string }) {
  const [places, setPlaces] = useState<WorktreePlace[] | null>(null), [error, setError] = useState("");
  const [query, setQuery] = useState(""), [index, setIndex] = useState<number | null>(null);
  const input = useRef<HTMLInputElement>(null), list = useRef<HTMLDivElement>(null), guard = useRef<HoverGuard | null>(null);
  const loadProgress = useLocalProgress(!places && !error, "Loading worktrees");
  useEffect(() => {
    let alive = true;
    invoke<WorktreePlace[]>("git_worktree_places", { path }).then((value) => alive && setPlaces(value)).catch((e) => alive && setError(String(e)));
    input.current?.focus();
    return () => { alive = false; };
  }, [path]);
  const rows = useMemo(() => rankPlaces(places ?? [], query), [places, query]);
  // Without a choice the first other worktree is the one wanted: one key to get there, as with Alt+Tab.
  const first = Math.max(0, rows.findIndex((row) => !row.place.current && !row.place.missing));
  const active = Math.min(index ?? first, Math.max(0, rows.length - 1));
  // Only what is on screen is asked for, one after the other.
  const states = useWorktreeStates(useMemo(() => rows.slice(0, 30).filter((row) => !row.place.missing).map((row) => row.place.path), [rows]));
  useEffect(() => { list.current?.querySelector(".on")?.scrollIntoView({ block: "nearest" }); }, [active, rows]);
  const close = () => A.openOverlay(null);
  const go = (at: number, where: "here" | "other" | "tab") => { const row = rows[at]; if (row && !row.place.missing) openWorktree(row.place.path, where); };
  const now = Date.now(), home = getState().info?.home;
  return <div className="backdrop" onMouseDown={close}>
    <div className="gp wt-switcher" role="dialog" aria-label={tt("Worktrees")} onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => {
      e.stopPropagation();
      if (e.nativeEvent.isComposing) return;
      const down = e.key === "ArrowDown" || (e.ctrlKey && (e.key === "n" || e.key === "j")), up = e.key === "ArrowUp" || (e.ctrlKey && (e.key === "p" || e.key === "k"));
      if (e.key === "Escape") { e.preventDefault(); close(); }
      else if (down || up) { e.preventDefault(); if (rows.length) setIndex((active + (down ? 1 : -1) + rows.length) % rows.length); }
      else if (e.key === "Enter") { e.preventDefault(); go(active, e.ctrlKey || e.metaKey ? "tab" : e.altKey || e.shiftKey ? "other" : "here"); }
    }}>
      <div className="gp-input"><input ref={input} role="combobox" aria-expanded="true" aria-controls="wt-list" aria-activedescendant={rows.length ? `wt-${active}` : undefined} aria-label={tt("Search worktrees")} placeholder={tt("Branch or folder of a worktree…")} spellCheck={false} value={query} onChange={(e) => { setQuery(e.target.value); setIndex(null); }} /></div>
      <div className="gp-list" id="wt-list" role="listbox" ref={list}>
        {rows.map(({ place }, i) => <div key={place.path} id={`wt-${i}`} role="option" aria-selected={i === active} aria-disabled={place.missing} className={`gp-row ${i === active ? "on" : ""} ${place.missing ? "off" : ""}`}
          onMouseMove={(e) => { if ((guard.current ??= new HoverGuard(false)).moved(e) && i !== active) setIndex(i); }} onClick={(e) => go(i, e.ctrlKey || e.metaKey ? "tab" : e.altKey || e.shiftKey ? "other" : "here")}
          onContextMenu={(e) => { e.preventDefault(); if (!place.missing) A.showMenuAt(eventAnchor(e), worktreeMenu(place.path)); }}>
          <WorktreeDot path={place.path} />
          <span className="gp-label"><b>{worktreeName(place)}</b> <small title={place.path}>{tildify(place.path, home)}</small></span>
          {place.current && <em className="gg-current-pill">{tt("Current")}</em>}{place.locked && <em className="gg-current-pill">{tt("Locked")}</em>}
          {place.missing ? <small className="gp-scope">{tt("Folder is missing")}</small> : <WorktreeBadges state={states.get(place.path)} />}
          {place.used != null && <small className="gp-scope" title={tt("Last change: {value} ago", { value: ageWords(place.used, now) })}>{age(place.used, now).label}</small>}
        </div>)}
        {places && !rows.length && <p className="gp-empty">{tt("No matching worktree")}</p>}
        {!places && (loadProgress ? <GitProgressBar progress={loadProgress} compact /> : <p className="gp-empty" role={error ? "alert" : undefined}>{error || tt("Loading worktrees…")}</p>)}
      </div>
      <div className="gp-foot"><span>{platformLabel(tt("Enter opens · Ctrl+Enter in a new tab · Alt+Enter in the other browser"))}</span></div>
    </div>
  </div>;
}

const REASONS: Record<CleanupCandidate["reason"], string> = { merged: "Merged", gone: "Gone on the remote", missing: "Folder is missing" };

/** Worktrees that look finished, with one confirmation for all. Changed ones are stored before they are removed. */
export function WorktreeCleanup({ path }: { path: string }) {
  const [found, setFound] = useState<CleanupCandidate[] | null>(null), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  const [cleanStep, setCleanStep] = useState<{ done: number; total: number; detail: string } | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set()), [ignored, setIgnored] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const loadProgress = useLocalProgress(!found && !error, "Loading worktrees");
  const cleanProgress = useLocalProgress(busy && !!cleanStep, "Removing worktrees", {
    operation: "worktreeCleanup",
    done: cleanStep?.done ?? null,
    total: cleanStep?.total ?? null,
    unit: "worktrees",
    detail: cleanStep?.detail ?? null,
  });
  const load = useCallback(async () => {
    try {
      const value = await invoke<CleanupCandidate[]>("git_worktree_cleanup", { path });
      setFound(value);
      return value;
    } catch (e) { setError(String(e)); return null; }
  }, [path]);
  useEffect(() => { void load(); root.current?.focus(); }, [load]);
  const states = useWorktreeStates(useMemo(() => (found ?? []).filter((c) => c.reason !== "missing").map((c) => c.path), [found]));
  // Merged and clean ones are chosen; nothing with local changes is, the user decides about those.
  const decided = useRef(new Set<string>());
  useEffect(() => {
    const add = (found ?? []).filter((c) => !decided.current.has(c.path) && (c.reason === "missing" || states.has(c.path)));
    if (!add.length) return;
    add.forEach((c) => decided.current.add(c.path));
    const safe = add.filter((c) => c.reason === "missing" || (c.reason === "merged" && states.get(c.path)?.dirty === 0));
    if (safe.length) setChosen((old) => new Set([...old, ...safe.map((c) => c.path)]));
  }, [found, states]);
  const toggle = (p: string) => setChosen((old) => { const next = new Set(old); if (!next.delete(p)) next.add(p); return next; });
  const close = () => { if (!busy) A.openOverlay(null); };
  const missing = (found ?? []).filter((c) => c.reason === "missing");
  const clean = async () => {
    const picked = (found ?? []).filter((c) => chosen.has(c.path));
    if (!picked.length || busy) return;
    const changed = picked.filter((c) => c.reason !== "missing" && states.get(c.path)?.dirty !== 0).length;
    // Git forgets all missing worktrees at once.
    const lines = picked.map((c) => tildify(c.path, getState().info?.home));
    setBusy(true); setError("");
    try {
      const text = [lines.join("\n"), tt("Branches and commits are kept."), changed ? tt("{count} have local changes: these are stored first (refs/gall/recovery).", { count: changed }) : "", ignored ? tt("Ignored files in them are deleted permanently.") : ""].filter(Boolean).join("\n\n");
      if (!await A.confirm(tt("Remove {count} worktrees?", { count: picked.length }), text, tt("Remove"), true)) return;
      const result = await cleanUpWorktrees(path, picked, (p) => states.get(p)?.dirty !== 0, ignored, setCleanStep);
      if (result.removed) A.notify(tt("{count} worktrees removed", { count: result.removed }), "success", undefined, undefined, result.notes.join("\n") || undefined);
      if (result.failed.length) setError(result.failed.map((f) => `${basename(f.path)}: ${f.message}`).join("\n"));
      setChosen(new Set()); decided.current = new Set();
      await load();
    } finally { setCleanStep(null); setBusy(false); root.current?.focus(); }
  };
  return <div className="backdrop" onMouseDown={close}>
    <div className="gp wt-cleanup" ref={root} tabIndex={-1} role="dialog" aria-label={tt("Clean up worktrees")} onMouseDown={(e) => e.stopPropagation()} onKeyDown={(e) => {
      e.stopPropagation();
      if (e.key === "Escape") { e.preventDefault(); close(); }
      else if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "j" || e.key === "k") {
        e.preventDefault();
        const boxes = [...root.current!.querySelectorAll<HTMLElement>("input, button")], at = boxes.indexOf(document.activeElement as HTMLElement);
        boxes[Math.max(0, Math.min(boxes.length - 1, at + (e.key === "ArrowDown" || e.key === "j" ? 1 : -1)))]?.focus();
      }
    }}>
      <div className="wt-title"><strong>{tt("Clean up worktrees")}</strong><button aria-label={tt("Close")} title={tt("Close (Esc)")} onClick={close}>✕</button></div>
      <div className="gp-list">
        {(found ?? []).map((c) => <label key={c.path} className="gp-row">
          <input type="checkbox" disabled={busy} checked={chosen.has(c.path)} onChange={() => c.reason === "missing" ? setChosen((old) => {
            // They are forgotten together, so they are chosen together.
            const next = new Set(old), on = !old.has(c.path);
            missing.forEach((m) => { if (on) next.add(m.path); else next.delete(m.path); });
            return next;
          }) : toggle(c.path)} />
          <WorktreeDot path={c.path} />
          <span className="gp-label"><b>{worktreeName(c)}</b> <small title={c.path}>{tildify(c.path, getState().info?.home)}</small></span>
          {c.reason !== "missing" && <WorktreeBadges state={states.get(c.path)} />}
          <small className="gp-scope">{tt(REASONS[c.reason])}</small>
        </label>)}
        {found && !found.length && <p className="gp-empty">{tt("No worktree is merged, gone on the remote or missing.")}</p>}
        {!found && !error && (loadProgress ? <GitProgressBar progress={loadProgress} compact /> : <p className="gp-empty">{tt("Loading worktrees…")}</p>)}
      </div>
      {cleanProgress && <GitProgressBar progress={cleanProgress} compact />}
      {error && <p className="wt-error" role="alert">{error}</p>}
      <div className="wt-foot">
        <label><input type="checkbox" disabled={busy} checked={ignored} onChange={(e) => setIgnored(e.target.checked)} /> {tt("Also delete ignored files (node_modules, build output)")}</label>
        <button className="btn danger" disabled={busy || !chosen.size} onClick={() => void clean()}>{busy ? tt("Removing…") : tt("Remove {count}…", { count: chosen.size })}</button>
      </div>
    </div>
  </div>;
}
