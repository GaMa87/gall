import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../state/store";
import * as A from "../state/actions";
import "./GitChanges.css";

type Change = { path: string; original: string | null; index: string; worktree: string };
type Workspace = { root: string; editStop: boolean; changes: Change[]; token: string; head: string; subject: string; message: string };
type PatchLine = { index: number; text: string; kind: "added" | "deleted" | "context" | "header"; hunk: number | null };
type Patch = { text: string; token: string; selectable: boolean; reason: string | null; lines: PatchLine[] };
type Action = "stage" | "unstage" | "commit" | "amend" | "stageAll" | "unstageAll" | "discard" | "ignore" | "exclude";
/** Unsent commit messages per repository, so leaving the panel (h, [ or a sidebar click) does not lose typing. */
const drafts = new Map<string, string>();
export function GitChanges({ path, onBusy, onChanged, onBack, onClose }: { path: string; onBusy: (busy: boolean) => void; onChanged: () => void; onBack?: () => void; onClose?: () => void }) {
  const [state, setState] = useState<Workspace | null>(null);
  const [error, setError] = useState("");
  const [patch, setPatch] = useState<Patch | null>(null);
  const [loadingPatch, setLoadingPatch] = useState(false);
  const [selected, setSelected] = useState(0);
  const [line, setLine] = useState(0);
  const [selectedLines, setSelectedLines] = useState<Set<number>>(new Set());
  const [message, setMessage] = useState(() => drafts.get(path) ?? "");
  const [amend, setAmend] = useState(false);
  // An amended message belongs to that commit, not to the repository's draft: it is never stored.
  const writeMessage = (value: string) => { setMessage(value); if (amend) return; if (value) drafts.set(path, value); else drafts.delete(path); };
  const [busy, setBusy] = useState(false);
  const gate = useRef(false);
  const loadTicket = useRef(0);
  const root = useRef<HTMLDivElement>(null);
  const messageRef = useRef<HTMLTextAreaElement>(null);
  const diffRef = useRef<HTMLPreElement>(null);
  const command = useRef<(key: string) => void>(() => {});
  useEffect(() => {
    const element = root.current;
    const listener = (event: Event) => command.current((event as CustomEvent<string>).detail);
    element?.addEventListener("gall-git-command", listener);
    return () => element?.removeEventListener("gall-git-command", listener);
  }, []);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const rows = [true, false].flatMap((staged) => (state?.changes ?? []).flatMap<Change & { staged: boolean }>((c) =>
    staged ? c.index !== " " && c.index !== "?" ? [{ ...c, staged }] : []
      : c.worktree !== " " || c.index === "?" ? [{ ...c, staged }] : []));
  const current = rows[selected];
  const stagedCount = rows.filter((r) => r.staged).length;
  const unstagedCount = rows.length - stagedCount;
  const load = async () => {
    const ticket = ++loadTicket.current;
    const value = await invoke<Workspace>("git_workspace", { path });
    if (ticket === loadTicket.current) { setState(value); setSelected(0); }
  };
  useEffect(() => { let active = true; setState(null); setError(""); setAmend(false); setMessage(drafts.get(path) ?? ""); invoke<Workspace>("git_workspace", { path }).then((v) => { if (active) { setState(v); if (v.editStop) { setAmend(true); setMessage(v.message); } } }).catch((e) => { if (active) setError(String(e)); }); root.current?.focus(); return () => { active = false; loadTicket.current++; }; }, [path]);
  useEffect(() => {
    let active = true;
    setPatch(null); setSelectedLines(new Set()); setLine(0);
    if (!current) { setLoadingPatch(false); return; }
    setLoadingPatch(true);
    invoke<Patch>("git_patch_selection", { path, file: current.path, staged: current.staged }).then((value) => { if (active) setPatch(value); }).catch((e) => { if (active) setError(String(e)); }).finally(() => { if (active) setLoadingPatch(false); });
    return () => { active = false; };
  }, [path, current?.path, current?.staged, state]);
  useEffect(() => { root.current?.querySelector(".gc-file.selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);
  useEffect(() => { diffRef.current?.querySelector(".gc-diff-line.current")?.scrollIntoView({ block: "nearest" }); }, [line]);
  const act = async (action: Action) => {
    if (gate.current || !state || (!["commit", "amend", "stageAll", "unstageAll"].includes(action) && !current)) return;
    if ((action === "stage" && current?.staged) || (action === "unstage" && !current?.staged)) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    try {
      if (action === "discard" && !await A.confirm("Discard unstaged changes?", `The unstaged contents of “${current.path}” will be removed. Staged changes and other files remain unchanged. A recovery stash named “Gall recovery before file discard” will be retained.`, "Discard file changes", true)) return;
      if (action === "amend" && !await A.confirm("Replace the latest commit?", `Amend ${state.head.slice(0, 12)} — ${state.subject}\n\nThis rewrites the commit using the current staged changes and message. Avoid amending a shared commit. Gall retains the original commit as a recovery reference.`, "Amend commit", true)) return;
      await invoke("git_index_action", { path, action, file: current?.path ?? null, message, expected: state.token });
      if (action === "commit" || action === "amend") { writeMessage(""); setAmend(false); }
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load().catch(() => {}); }
    finally { gate.current = false; setBusy(false); onBusy(false); }
  };
  const applySelection = async () => {
    if (gate.current || !patch?.selectable || !current || !selectedLines.size) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    try {
      await invoke("git_apply_selection", { path, file: current.path, staged: current.staged, selected: [...selectedLines], expected: patch.token });
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load().catch(() => {}); }
    finally { gate.current = false; setBusy(false); onBusy(false); }
  };
  const toggleLines = (indices: number[]) => setSelectedLines((previous) => {
    const next = new Set(previous); const remove = indices.every((index) => next.has(index));
    indices.forEach((index) => { if (remove) next.delete(index); else next.add(index); });
    return next;
  });
  const fileActions = (element: HTMLElement) => {
    const box = element.getBoundingClientRect();
    A.showMenu(box.left, box.bottom, [
      { label: "Discard unstaged file changes…", disabled: busy || !current || current.staged, run: () => void act("discard") },
      { label: "Ignore file in .gitignore", disabled: busy || current?.index !== "?", run: () => void act("ignore") },
      { label: "Exclude file locally", disabled: busy || current?.index !== "?", run: () => void act("exclude") },
    ]);
  };
  const canCommit = !!state && !!message.trim() && (amend || (!!stagedCount && !state.editStop));
  const commitNow = () => { if (canCommit && !busy) void act(amend ? "amend" : "commit"); };
  const toggleAmend = (value: boolean) => {
    setAmend(value);
    if (value) { if (!message) setMessage(state?.message ?? ""); }
    else if (!state?.editStop) setMessage(drafts.get(path) ?? "");
  };
  command.current = (key) => {
    if (busy) return;
    if (key === " ") void act(current?.staged ? "unstage" : "stage");
    else if (key === "a") void act(unstagedCount ? "stageAll" : "unstageAll");
    else if (key === "c") messageRef.current?.focus();
    else if (key === "A" && state?.head) toggleAmend(!amend);
    else if (key === "d" && current && !current.staged) void act("discard");
    else if (key === "i" && root.current) fileActions(root.current.querySelector<HTMLButtonElement>(".gc-tools button:nth-child(5)") ?? root.current);
    else if (key === "r") void load().catch((e) => setError(String(e)));
    else if (key === "e" && current) A.editPaths([`${state?.root ?? path}/${current.path}`]);
  };
  return <div className="git-changes gc-complete" ref={root} tabIndex={-1} onKeyDown={(e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !(e.target as HTMLElement).closest("textarea")) { e.preventDefault(); e.stopPropagation(); commitNow(); return; }
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    if (e.key === "Escape") {
      // Like the commit details: close this panel and return to the commit list, never the whole workspace.
      if (onClose) { e.preventDefault(); e.stopPropagation(); onClose(); }
      return;
    }
    e.stopPropagation();
    const inDiff = !!(e.target as HTMLElement).closest(".gc-patch");
    if (e.key === "ArrowDown" || (vim && e.key === "j")) { e.preventDefault(); if (inDiff) setLine((s) => Math.min(s + 1, (patch?.lines.length ?? 1) - 1)); else setSelected((s) => Math.min(s + 1, Math.max(0, rows.length - 1))); }
    else if (e.key === "ArrowUp" || (vim && e.key === "k")) { e.preventDefault(); if (inDiff) setLine((s) => Math.max(0, s - 1)); else setSelected((s) => Math.max(0, s - 1)); }
    else if (e.key === " " && inDiff && patch?.selectable && ["added", "deleted"].includes(patch.lines[line]?.kind)) { e.preventDefault(); toggleLines([line]); }
    else if (e.key === "Enter" && inDiff) { e.preventDefault(); void applySelection(); }
    else if (vim && e.key === "l") { e.preventDefault(); diffRef.current?.focus(); }
    else if (vim && e.key === "h") { e.preventDefault(); if (inDiff) root.current?.focus(); else onBack?.(); }
    else if (e.key === " " && !inDiff && !(e.target as HTMLElement).closest("button:not(.gc-file)")) { e.preventDefault(); command.current(" "); }
    else if (vim && e.key === "u") { e.preventDefault(); if (inDiff) { if (current?.staged) void applySelection(); } else void act("unstage"); }
    else if (vim && ["a", "A", "d", "i", "r", "e"].includes(e.key)) { e.preventDefault(); command.current(e.key); }
    else if (vim && e.key === "c") { e.preventDefault(); messageRef.current?.focus(); }
  }}>
    <div className="gc-tools"><button disabled={busy || !current || current.staged} onClick={() => void act("stage")}>Stage file</button><button disabled={busy || !current?.staged} onClick={() => void act("unstage")}>Unstage file</button><button disabled={busy || !unstagedCount} onClick={() => void act("stageAll")}>Stage all</button><button disabled={busy || !stagedCount} onClick={() => void act("unstageAll")}>Unstage all</button><button disabled={busy || !current} onClick={(e) => fileActions(e.currentTarget)}>File actions…</button><button disabled={busy} onClick={() => load().catch((e) => setError(String(e)))}>Refresh</button></div>
    {state?.editStop && <p className="gc-edit-stop">Interactive edit stop: stage changes, amend this commit, then Continue in the operation panel.</p>}
    {error && <p className="gc-error" role="alert">{error}</p>}
    {!state ? <p>{error ? "Changes could not be loaded. Use Refresh to retry." : "Loading changes…"}</p> : <div className="gc-body"><div className="gc-files">{rows.length === 0 ? <p>Working tree clean</p> : rows.map((row, i) => <div key={`${row.staged}:${row.path}`}>
      {(i === 0 || rows[i - 1].staged !== row.staged) && <div className="gc-group">{row.staged ? `Staged · ${stagedCount}` : `Unstaged · ${unstagedCount}`}</div>}
      <button disabled={busy} className={`gc-file ${selected === i ? "selected" : ""}`} onFocus={() => setSelected(i)} onClick={() => { setSelected(i); root.current?.focus(); }}><span className={`gc-state ${row.staged ? "staged" : ""}`}>{row.staged ? "Staged" : "Unstaged"}</span><span title={row.path}>{row.original ? `${row.original} → ` : ""}{row.path}</span></button>
    </div>)}</div><div className="gc-diff"><div className="gc-selection-tools"><button disabled={busy || !patch?.selectable || !selectedLines.size} onClick={() => void applySelection()}>{current?.staged ? "Unstage selection" : "Stage selection"}</button><small>{patch?.selectable ? `${selectedLines.size} selected lines · select a hunk or individual changes` : patch?.reason ?? ""}</small></div><pre ref={diffRef} tabIndex={0} className="gc-patch" aria-label="Selected file diff">{loadingPatch ? "Loading diff…" : current ? patch?.lines.length ? patch.lines.map((entry) => <span className={`gc-diff-line ${entry.kind} ${line === entry.index ? "current" : ""} ${selectedLines.has(entry.index) ? "chosen" : ""}`} key={entry.index}>
      {patch.selectable && ["added", "deleted"].includes(entry.kind) ? <input type="checkbox" aria-label={`Select changed line ${entry.index + 1}`} checked={selectedLines.has(entry.index)} disabled={busy} onFocus={() => setLine(entry.index)} onChange={() => toggleLines([entry.index])} /> : patch.selectable && entry.text.startsWith("@@ ") ? <button className="gc-hunk" disabled={busy} onClick={() => toggleLines(patch.lines.filter((l) => l.hunk === entry.hunk && ["added", "deleted"].includes(l.kind)).map((l) => l.index))} aria-label={`Select hunk ${(entry.hunk ?? 0) + 1}`}>±</button> : <span className="gc-line-spacer" />}
      <span>{entry.text || " "}</span>{"\n"}
    </span>) : "No textual difference" : "Select a file to review its changes"}</pre></div></div>}
    <div className="gc-commit"><div className="gc-message"><textarea ref={messageRef} aria-label="Commit message" placeholder="Commit message (Enter = new line, Ctrl+Enter commits)" value={message} onChange={(e) => writeMessage(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); e.stopPropagation(); commitNow(); } }} disabled={busy} /><label className="gc-amend"><input type="checkbox" checked={amend} disabled={busy || !state?.head} onChange={(e) => toggleAmend(e.target.checked)} />Amend latest commit{amend && state ? ` · ${state.head.slice(0, 8)}` : ""}</label></div><div><button disabled={busy || !message.trim() || (!amend && (!stagedCount || state?.editStop))} onClick={() => void act(amend ? "amend" : "commit")}>{amend ? "Amend commit…" : "Commit staged"}</button><small>Only staged changes · unsigned · hooks disabled · Ctrl+Enter commits</small></div></div>
    <div className="gc-key-help">{vim ? "j/k files · Space stage/unstage · l diff · Enter apply selection (in diff) · Ctrl+Enter commit · a stage all · c message · A amend · d discard · ? commands" : "Arrow keys navigate · Tab selects controls · Space toggles selected changes · Ctrl+Enter commits"}</div>
    {busy && <p role="status">Updating Git index…</p>}
  </div>;
}
