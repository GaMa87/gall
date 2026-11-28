import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { belowAnchor, textAnchor, type Anchor } from "../core/anchor";
import { changeSides } from "../core/gitImages";
import { diffRange } from "../core/gitFileHistory";
import { GitAbsorb, type AbsorbRequest } from "./GitAbsorb";
import { GitImageDiff } from "./GitImageDiff";
import { isImage } from "./ImageCompare";
import { useGitKeys } from "../state/gitKeys";
import { GitKeyCap } from "./GitKeyCap";
import * as A from "../state/actions";
import { tt } from "../i18n";
import { api } from "../api/ipc";
import { join } from "../core/path";
import { useStore } from "../state/store";
import { GENERATED_REASON, collapsedRows, generatedSummary, stepVisible } from "../core/outline";
import type { GeneratedReason } from "../api/types";
import { PatchText, formattingOnly, usePatchMarks } from "./PatchMarks";
import "./GitChanges.css";
import { changedBetween, changedInHunk, hunkHeaderFrom } from "../core/patchSelection";
import { CUSTOM_PREFIX } from "../core/gitCustom";
import { gitCommands } from "../core/gitCommands";
import { endChangeDrag, sendToWorktree, startChangeDrag, transferKeyboard } from "../state/gitWorktree";
import { createProgressId } from "../core/gitProgress";
import { GitProgressBar, useGitProgress, useLocalProgress, type GitProgressSeed } from "./GitProgressBar";

type Change = { path: string; original: string | null; index: string; worktree: string };
type Workspace = { root: string; editStop: boolean; merging: boolean; changes: Change[]; token: string; head: string; subject: string; message: string };
type PatchLine = { index: number; text: string; kind: "added" | "deleted" | "context" | "header"; hunk: number | null };
type Patch = { text: string; token: string; selectable: boolean; reason: string | null; lines: PatchLine[] };
type Action = "stage" | "unstage" | "commit" | "amend" | "stageAll" | "unstageAll" | "discard" | "ignore" | "exclude";
/** Unsent commit messages per repository, so leaving the panel (h, [ or a sidebar click) does not lose typing. */
const drafts = new Map<string, string>();
export function GitChanges({ path, onBusy, onChanged, onBack, onClose, onCustom }: { path: string; onBusy: (busy: boolean) => void; onChanged: () => void; onBack?: () => void; onClose?: () => void; onCustom?: (id: string, file: string) => void }) {
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
  const [actionTask, setActionTask] = useState<{ id: string; seed: GitProgressSeed } | null>(null);
  const actionProgress = useGitProgress(actionTask?.id ?? null, actionTask?.seed);
  const loadProgress = useLocalProgress(!state && !error, "Loading changes");
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
  const gitKeys = useGitKeys(), { vim } = gitKeys;
  const rows = [true, false].flatMap((staged) => (state?.changes ?? []).flatMap<Change & { staged: boolean }>((c) =>
    staged ? c.index !== " " && c.index !== "?" ? [{ ...c, staged }] : []
      : c.worktree !== " " || c.index === "?" ? [{ ...c, staged }] : []));
  const current = rows[selected];
  const stagedCount = rows.filter((r) => r.staged).length;
  const unstagedCount = rows.length - stagedCount;
  // Generated files (lockfiles, build output, .gitattributes, extra patterns) are drawn collapsed by default; they stay in the list and act as any other.
  const [generatedFiles, setGeneratedFiles] = useState<ReadonlyMap<string, GeneratedReason>>(new Map());
  const [revealed, setRevealed] = useState({ staged: false, unstaged: false });
  const generatedPatternsKey = (useStore((s) => s.settings?.generatedPatterns) ?? []).join("\n");
  const repoRoot = state?.root ?? path;
  const pathKey = (state?.changes ?? []).map((c) => c.path).join("\n");
  useEffect(() => {
    let live = true;
    const paths = [...new Set((state?.changes ?? []).map((c) => c.path))];
    Promise.all(paths.map((file) => api.generatedClassify(join(repoRoot, file), "").then((g) => [file, g] as const).catch(() => [file, null] as const)))
      .then((found) => { if (live) setGeneratedFiles(new Map(found.flatMap(([file, g]) => (g ? [[file, g.reason] as const] : [])))); });
    return () => { live = false; };
  }, [repoRoot, pathKey, generatedPatternsKey]);
  const collapsed = collapsedRows(rows, generatedFiles, revealed);
  // A selection that lands on a collapsed row moves to the first drawn one (the file stays in the list).
  useEffect(() => { const first = collapsed.indexOf(false); if (collapsed[selected] && first >= 0) setSelected(first); }, [collapsed.join(","), selected]);
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
  // What really changed on each patch line (structural diff); the patch shows without it.
  const marks = usePatchMarks(current?.path, patch?.text);
  useEffect(() => { root.current?.querySelector(".gc-file.selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);
  useEffect(() => { diffRef.current?.querySelector(".gc-diff-line.current")?.scrollIntoView({ block: "nearest" }); }, [line]);
  const act = async (action: Action, target = current) => {
    if (gate.current || !state || (!["commit", "amend", "stageAll", "unstageAll"].includes(action) && !target)) return;
    if ((action === "stage" && target?.staged) || (action === "unstage" && !target?.staged)) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    try {
      if (action === "discard" && !await A.confirm(tt("Discard unstaged changes?"), tt("The unstaged contents of “{path}” will be removed. Staged changes and other files remain unchanged. A hidden recovery copy (reference under refs/gall/recovery, not a stash) will be retained.", { path: target!.path }), tt("Discard file changes"), true)) return;
      if (action === "amend" && !await A.confirm(tt("Replace the latest commit?"), tt("Amend {hash} — {subject}\n\nThis rewrites the commit using the current staged changes and message. Avoid amending a shared commit. Gall retains the original commit as a recovery reference.", { hash: state.head.slice(0, 12), subject: state.subject }), tt("Amend commit"), true)) return;
      const phaseMap: Record<Action, string> = { stage: "Staging changes", unstage: "Unstaging changes", stageAll: "Staging changes", unstageAll: "Unstaging changes", commit: "Committing changes", amend: "Amending commit", discard: "Discarding changes", ignore: "Updating Git index", exclude: "Updating Git index" };
      const progressId = createProgressId("index");
      setActionTask({ id: progressId, seed: { operation: "index", phase: phaseMap[action], detail: target?.path } });
      await invoke("git_index_action", { path, action, file: target?.path ?? null, message, expected: state.token, progressId });
      if (action === "commit" || action === "amend") { writeMessage(""); setAmend(false); }
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load().catch(() => {}); }
    finally { setActionTask(null); gate.current = false; setBusy(false); onBusy(false); }
  };
  const changedIn = (from: number, to: number) => changedBetween(patch?.lines ?? [], from, to);
  const hunkLines = (hunk: number | null | undefined) => changedInHunk(patch?.lines ?? [], hunk);
  /** The lines an action works on: the chosen ones, else (no choice made) the whole hunk under the cursor. */
  const targetLines = () => selectedLines.size ? [...selectedLines] : hunkLines(patch?.lines[line]?.hunk);
  const [rangeFrom, setRangeFrom] = useState<number | null>(null);
  const [svgImage, setSvgImage] = useState(false);
  const [absorb, setAbsorb] = useState<AbsorbRequest | null>(null);
  /** `F`: the staged file, or in the diff the chosen lines, go into a commit that is chosen next. */
  const fixupInto = (inDiff: boolean, target?: string) => {
    if (!current) return;
    if (!current.staged) { setError(tt("Stage the changes first; only staged changes are put into a commit.")); return; }
    const lines = inDiff && patch?.selectable ? targetLines() : [];
    const range = lines.length && patch ? diffRange(patch.lines.map((entry) => entry.text), Math.min(...lines), Math.max(...lines)) : null;
    setAbsorb({ file: current.path, range, target });
  };
  // A staged hunk dropped on a commit of the history.
  useEffect(() => {
    const drop = (event: Event) => {
      const { file, range, target } = (event as CustomEvent<{ file: string; range: AbsorbRequest["range"]; target: string }>).detail;
      setAbsorb({ file, range, target });
    };
    window.addEventListener("gall-git-fixup", drop);
    return () => window.removeEventListener("gall-git-fixup", drop);
  }, []);
  const lastClick = useRef<number | null>(null);
  useEffect(() => setRangeFrom(null), [current?.path, current?.staged]);
  const moveLine = (to: number) => {
    const next = Math.max(0, Math.min(to, (patch?.lines.length ?? 1) - 1)); setLine(next);
    if (rangeFrom !== null) setSelectedLines(new Set(changedIn(rangeFrom, next)));
  };
  /** `]` and `[`: the first line of the next or previous hunk (its `@@` header). */
  const jumpHunk = (direction: 1 | -1) => {
    const target = hunkHeaderFrom(patch?.lines ?? [], line, direction);
    if (target !== null) moveLine(target);
  };
  const runSelection = async (command: "git_apply_selection" | "git_discard_selection", lines: number[]) => {
    if (gate.current || !patch?.selectable || !current || !lines.length) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    const progressId = createProgressId("sel");
    setActionTask({ id: progressId, seed: { operation: "index", phase: command === "git_discard_selection" ? "Discarding changes" : current.staged ? "Unstaging changes" : "Staging changes", detail: current.path } });
    try {
      if (command === "git_discard_selection") await invoke(command, { path, file: current.path, selected: lines, expected: patch.token, progressId });
      else await invoke(command, { path, file: current.path, staged: current.staged, selected: lines, expected: patch.token, progressId });
      setRangeFrom(null);
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load().catch(() => {}); }
    finally { setActionTask(null); gate.current = false; setBusy(false); onBusy(false); }
  };
  const applySelection = () => runSelection("git_apply_selection", targetLines());
  const discardSelection = async () => {
    const lines = targetLines();
    if (!lines.length || !current || current.staged || !patch?.selectable) return;
    if (!await A.confirm(tt("Discard selected lines?"), tt("{count} changed lines of “{path}” will be removed from the working file. The index and other lines remain unchanged. A hidden recovery copy (reference under refs/gall/recovery, not a stash) will be retained.", { count: lines.length, path: current.path }), tt("Discard lines"), true)) return;
    await runSelection("git_discard_selection", lines);
  };
  /** `S`: the selected file, or in the diff the chosen lines, go to a stash of their own. */
  const stashPart = async (inDiff: boolean) => {
    if (gate.current || !current || !state) return;
    const lines = inDiff && patch?.selectable && !current.staged ? targetLines() : [];
    if (inDiff && !lines.length) { setError(patch?.reason ?? tt("Choose unstaged lines to stash.")); return; }
    const name = await A.promptText(lines.length ? tt("Stash {count} changed lines of “{path}”", { count: lines.length, path: current.path }) : tt("Stash “{path}”", { path: current.path }), "", "Save stash");
    if (name === null || gate.current) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    const progressId = createProgressId("stash");
    setActionTask({ id: progressId, seed: { operation: "stash", phase: "Saving stash", detail: current.path } });
    try {
      if (lines.length) await invoke("git_stash_selection", { path, file: current.path, selected: lines, message: name, expected: patch!.token, progressId });
      else {
        const { token } = await invoke<{ token: string }>("git_repository", { path });
        await invoke("git_repository_action", { path, request: { action: "stashSave", message: name, keepIndex: false, trackedOnly: false, files: [current.path] }, expected: token, progressId });
      }
      setRangeFrom(null);
      A.notify(tt("Stashed. Press z for the stashes."), "success");
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load().catch(() => {}); }
    finally { setActionTask(null); gate.current = false; setBusy(false); onBusy(false); }
  };
  /** Drag a file between the Staged and Unstaged lists. */
  const dropFile = (event: React.DragEvent, staged: boolean) => {
    const raw = event.dataTransfer.getData("text/x-gall-change"); if (!raw) return;
    event.preventDefault();
    const [from, file] = [raw.startsWith("1:"), raw.slice(2)];
    if (from === staged) return;
    const row = rows.find((r) => r.staged === from && r.path === file);
    if (row) void act(staged ? "stage" : "unstage", row);
  };
  const toggleLines = (indices: number[]) => setSelectedLines((previous) => {
    const next = new Set(previous); const remove = indices.every((index) => next.has(index));
    indices.forEach((index) => { if (remove) next.delete(index); else next.add(index); });
    return next;
  });
  // The commands of the user for a file, with the keys they have now.
  const customFiles = gitCommands("files", gitKeys.shortcuts).filter((item) => item.action.startsWith(CUSTOM_PREFIX));
  const fileActions = (anchor: Anchor) => {
    A.showMenuAt(anchor, [
      { label: tt("Absorb staged changes into their commits…"), disabled: busy || !stagedCount, run: () => setAbsorb({}) },
      { label: tt("Put staged file into a commit…"), disabled: busy || !current?.staged, run: () => fixupInto(false) },
      { label: tt("Discard unstaged file changes…"), disabled: busy || !current || current.staged, run: () => void act("discard") },
      { label: tt("Ignore file in .gitignore"), disabled: busy || current?.index !== "?", run: () => void act("ignore") },
      { label: tt("Exclude file locally"), disabled: busy || current?.index !== "?", run: () => void act("exclude") },
      ...(customFiles.length ? [{ separator: true } as const, ...customFiles.map((item) => ({ label: item.label, keyCap: item.key || undefined, disabled: busy || !current, run: () => command.current(item.action) }))] : []),
    ]);
  };
  /** `w`: brings the chosen lines (in the diff) or the whole file to another worktree, chosen in a menu of the others. */
  const sendAway = async (anchor: Anchor) => {
    if (!current) return;
    const inDiff = !!diffRef.current?.contains(document.activeElement) && !!patch?.selectable;
    const lines = inDiff ? targetLines() : [];
    const change = { path, file: current.path, staged: current.staged, lines: lines.length ? lines : undefined };
    try {
      const listed = await invoke<{ repository: string | null; worktrees: { path: string; branch: string | null; bare: boolean; prunable: boolean }[] }>("git_advanced", { path, commit: null });
      const others = listed.worktrees.filter((tree) => !tree.bare && !tree.prunable && tree.path !== listed.repository);
      if (!others.length) { A.notify(tt("This repository has no other worktree.")); return; }
      A.showMenuAt(anchor, others.map((tree) => {
        const label = tree.branch ?? tree.path.split(/[\\/]/).filter(Boolean).pop() ?? tree.path;
        return { label: tt("Bring to {target}…", { target: label }), run: () => void bring(change, { path: tree.path, label }) };
      }));
    } catch (e) { setError(String(e)); }
  };
  const bring = async (change: Parameters<typeof sendToWorktree>[0], target: { path: string; label: string }) => {
    if (gate.current) return;
    gate.current = true; setBusy(true); onBusy(true); setError("");
    try { await sendToWorktree(change, target, transferKeyboard(vim)); }
    catch (e) { setError(String(e)); }
    finally { gate.current = false; setBusy(false); onBusy(false); }
  };
  const canAmend = !!state?.head && !state.merging;
  const amending = amend && canAmend;
  const canCommit = !!state && !!message.trim() && (amending || (!state.editStop && (stagedCount > 0 || state.merging)));
  const commitNow = () => { if (canCommit && !busy) void act(amending ? "amend" : "commit"); };
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
    else if (key === "A" && canAmend) toggleAmend(!amend);
    else if (key === "d" && current && !current.staged) void act("discard");
    else if (key === "i" && current) {
      const row = root.current?.querySelector(".gc-file.selected");
      fileActions(textAnchor(row, row));
    }
    else if (key === "u") void act("unstage");
    else if (key === "S") void stashPart(!!diffRef.current?.contains(document.activeElement));
    else if (key === "b") { if (stagedCount) setAbsorb({}); else setError(tt("Nothing is staged.")); }
    else if (key === "F") fixupInto(!!diffRef.current?.contains(document.activeElement));
    else if (key === "Ctrl+Enter") commitNow();
    else if (key === "r") void load().catch((e) => setError(String(e)));
    else if (key === "e" && current) A.editPaths([`${state?.root ?? path}/${current.path}`]);
    else if (key.startsWith(CUSTOM_PREFIX) && current) onCustom?.(key.slice(CUSTOM_PREFIX.length), current.path);
    else if (key === "w" && current) { const row = root.current?.querySelector(".gc-file.selected"); void sendAway(textAnchor(row, row)); }
  };
  return <div className="git-changes gc-complete" ref={root} tabIndex={-1} onKeyDown={(e) => {
    const target = e.target as HTMLElement, inDiff = !!target.closest(".gc-patch");
    const { key, on } = gitKeys.press(inDiff ? ["diff", "files"] : ["files"], e);
    // Committing works from every part of the panel, the message included, with and without Vim keys.
    if (key === "Ctrl+Enter") { e.preventDefault(); e.stopPropagation(); commitNow(); return; }
    if (target.closest("input,textarea,select")) {
      if (e.key === "Escape" && target === messageRef.current) { e.preventDefault(); e.stopPropagation(); root.current?.focus(); }
      return;
    }
    if (e.key === "Escape" && rangeFrom !== null && inDiff) { e.preventDefault(); e.stopPropagation(); setRangeFrom(null); return; }
    if (e.key === "Escape") {
      // Like the commit details: close this panel and return to the commit list, never the whole workspace.
      if (onClose) { e.preventDefault(); e.stopPropagation(); onClose(); }
      return;
    }
    e.stopPropagation();
    if (e.key === "ArrowDown" || (vim && key === "j")) { e.preventDefault(); if (inDiff) moveLine(line + 1); else setSelected((s) => stepVisible(collapsed, s, 1)); }
    else if (e.key === "ArrowUp" || (vim && key === "k")) { e.preventDefault(); if (inDiff) moveLine(line - 1); else setSelected((s) => stepVisible(collapsed, s, -1)); }
    else if (inDiff && key === "]") { e.preventDefault(); jumpHunk(1); }
    else if (inDiff && key === "[") { e.preventDefault(); jumpHunk(-1); }
    else if (inDiff && key === "v" && patch?.selectable) { e.preventDefault(); if (rangeFrom === null) { setRangeFrom(line); setSelectedLines(new Set(changedIn(line, line))); } else setRangeFrom(null); }
    else if (inDiff && key === "x" && patch?.selectable && ["added", "deleted"].includes(patch.lines[line]?.kind)) { e.preventDefault(); toggleLines([line]); }
    else if (inDiff && key === "d" && !current?.staged) { e.preventDefault(); void discardSelection(); }
    else if (inDiff && (key === " " ? patch?.selectable : key === "Enter") && !target.closest("button")) { e.preventDefault(); void applySelection(); }
    else if (vim && key === "l") { e.preventDefault(); diffRef.current?.focus(); }
    else if (vim && key === "h") { e.preventDefault(); if (inDiff) root.current?.focus(); else onBack?.(); }
    else if (key === " " && !inDiff && !target.closest("button:not(.gc-file)")) { e.preventDefault(); command.current(" "); }
    else if (on && key === "u") { e.preventDefault(); if (inDiff) { if (current?.staged) void applySelection(); } else command.current("u"); }
    else if (on && customFiles.some((item) => item.defaults.includes(key))) { e.preventDefault(); command.current(customFiles.find((item) => item.defaults.includes(key))!.action); }
    else if (on && ["a", "A", "d", "i", "r", "e", "c", "w", "S", "b", "F"].includes(key)) { e.preventDefault(); command.current(key); }
  }}>
    {absorb && <GitAbsorb key={JSON.stringify([path, absorb])} path={path} request={absorb} onBusy={onBusy} onDone={() => { setAbsorb(null); void load().catch((e) => setError(String(e))); onChanged(); root.current?.focus(); }} onClose={() => { setAbsorb(null); root.current?.focus(); }} />}
    <div className="gc-tools"><button disabled={busy || !current || current.staged} onClick={() => void act("stage")}>{tt("Stage file")}<GitKeyCap scope="files" k=" " /></button><button disabled={busy || !current?.staged} onClick={() => void act("unstage")}>{tt("Unstage file")}<GitKeyCap scope="files" k="u" /></button><button disabled={busy || !unstagedCount} onClick={() => void act("stageAll")}>{tt("Stage all")}{unstagedCount > 0 && <GitKeyCap scope="files" k="a" />}</button><button disabled={busy || !stagedCount} onClick={() => void act("unstageAll")}>{tt("Unstage all")}{!unstagedCount && <GitKeyCap scope="files" k="a" />}</button><button disabled={busy || !current} onClick={(e) => fileActions(belowAnchor(e.currentTarget))}>{tt("File actions…")}<GitKeyCap scope="files" k="i" /></button><button disabled={busy} onClick={() => load().catch((e) => setError(String(e)))}>{tt("Refresh")}<GitKeyCap scope="files" k="r" /></button></div>
    {state?.editStop && <p className="gc-edit-stop">{tt("Interactive edit stop: stage changes, amend this commit, then Continue in the operation panel.")}</p>}
    {error && <p className="gc-error" role="alert">{error}</p>}
    {!state ? (loadProgress ? <GitProgressBar progress={loadProgress} compact /> : <p>{tt("Changes could not be loaded. Use Refresh to retry.")}</p>) : <div className="gc-body"><div className="gc-files">{rows.length === 0 ? <p>{tt("Working tree clean")}</p> : rows.map((row, i) => <div key={`${row.staged}:${row.path}`}>
      {(i === 0 || rows[i - 1].staged !== row.staged) && <div className="gc-group" onDragOver={(e) => e.preventDefault()} onDrop={(e) => dropFile(e, row.staged)}>{row.staged ? `${tt("Staged")} · ${stagedCount}` : `${tt("Unstaged")} · ${unstagedCount}`}</div>}
      {!collapsed[i] && <button disabled={busy} draggable title={row.staged ? tt("Drag to Unstaged to unstage this file, onto a commit to put it into it, or onto a worktree in the sidebar to bring it there") : tt("Drag to Staged to stage this file, or onto a worktree in the sidebar to bring it there")} onDragStart={(e) => { e.dataTransfer.setData("text/x-gall-change", `${row.staged ? 1 : 0}:${row.path}`); startChangeDrag(e, { path, file: row.path, staged: row.staged }); }} onDragEnd={endChangeDrag} onDragOver={(e) => e.preventDefault()} onDrop={(e) => dropFile(e, row.staged)} className={`gc-file ${selected === i ? "selected" : ""}`} onFocus={() => setSelected(i)} onClick={() => { setSelected(i); root.current?.focus(); }}><span className={`gc-state ${row.staged ? "staged" : ""}`}>{row.staged ? tt("Staged") : tt("Unstaged")}</span><span title={row.path}>{row.original ? `${row.original} → ` : ""}{row.path}</span></button>}
      {(i === rows.length - 1 || rows[i + 1].staged !== row.staged) && (() => { const g = generatedSummary(rows, generatedFiles, row.staged); const open = row.staged ? revealed.staged : revealed.unstaged; return g.count > 0 ? <div className="gc-generated" role="status"><span>{open ? tt("{count} generated files shown", { count: g.count }) : tt("{count} generated files collapsed ({reasons})", { count: g.count, reasons: g.reasons.map((r) => tt(GENERATED_REASON[r])).join(", ") })}</span> <button className="gg-link" onClick={() => setRevealed((r) => ({ ...r, [row.staged ? "staged" : "unstaged"]: !open }))}>{open ? tt("hide them") : tt("show them")}</button></div> : null; })()}
    </div>)}</div><div className="gc-diff"><div className="gc-selection-tools"><button disabled={busy || !patch?.selectable || !selectedLines.size} onClick={() => void applySelection()}>{current?.staged ? tt("Unstage selection") : tt("Stage selection")}</button>{current && !current.staged && <button disabled={busy || !patch?.selectable || !selectedLines.size} onClick={() => void discardSelection()}>{tt("Discard selection")}</button>}<small>{patch?.selectable ? tt("{count} selected lines · select a hunk or individual changes", { count: selectedLines.size }) : patch?.reason ?? ""}</small></div>{current && isImage(current.path) && (!/\.svg$/i.test(current.path) || svgImage) ? <div className="gc-patch gc-image" ref={(el) => { (diffRef as React.RefObject<HTMLElement | null>).current = el; }} tabIndex={0} aria-label={tt("Selected image")}>{/\.svg$/i.test(current.path) && <button className="gid-toggle" onClick={() => setSvgImage(false)}>{tt("Show as text")}</button>}<GitImageDiff path={path} sides={changeSides(current)} revision={state} /></div> : <>{current && /\.svg$/i.test(current.path) && <button className="gid-toggle" onClick={() => setSvgImage(true)}>{tt("Show as image")}</button>}<pre ref={diffRef} tabIndex={0} className="gc-patch" aria-label={tt("Selected file diff")}>{loadingPatch ? tt("Loading diff…") : current ? patch?.lines.length ? patch.lines.map((entry) => <span draggable={patch.selectable && entry.text.startsWith("@@ ") && !busy} onDragStart={(e) => { if (!patch.selectable || !entry.text.startsWith("@@ ") || !current) { e.preventDefault(); return; } startChangeDrag(e, { path, file: current.path, staged: current.staged, lines: patch.lines.filter((l) => l.hunk === entry.hunk && ["added", "deleted"].includes(l.kind)).map((l) => l.index), range: diffRange(patch.lines.map((l) => l.text), entry.index, Math.max(...patch.lines.filter((l) => l.hunk === entry.hunk).map((l) => l.index))) }); }} onDragEnd={endChangeDrag} title={formattingOnly(marks, entry.index) ? tt("Only the formatting of this line differs") : undefined} className={`gc-diff-line ${entry.kind} ${formattingOnly(marks, entry.index) ? "fmt" : ""} ${line === entry.index ? "current" : ""} ${selectedLines.has(entry.index) ? "chosen" : ""}`} key={entry.index}>
      {patch.selectable && ["added", "deleted"].includes(entry.kind) ? <input type="checkbox" aria-label={tt("Select changed line {line}", { line: entry.index + 1 })} checked={selectedLines.has(entry.index)} disabled={busy} onFocus={() => setLine(entry.index)} onClick={(e) => { if (e.shiftKey && lastClick.current !== null) { e.preventDefault(); setSelectedLines(new Set([...selectedLines, ...changedIn(lastClick.current, entry.index)])); } lastClick.current = entry.index; }} onChange={() => toggleLines([entry.index])} /> : patch.selectable && entry.text.startsWith("@@ ") ? <button className="gc-hunk" disabled={busy} onClick={() => toggleLines(patch.lines.filter((l) => l.hunk === entry.hunk && ["added", "deleted"].includes(l.kind)).map((l) => l.index))} aria-label={tt("Select hunk {hunk}", { hunk: (entry.hunk ?? 0) + 1 })} title={tt("Select or deselect all changed lines in this hunk")}>±</button> : <span className="gc-line-spacer" />}
      <span><PatchText text={entry.text || " "} spans={marks?.[entry.index]} /></span>{"\n"}
    </span>) : tt("No textual difference") : tt("Select a file to review its changes")}</pre></>}</div></div>}
    <div className="gc-commit"><div className="gc-message"><textarea ref={messageRef} aria-label={tt("Commit message")} placeholder={tt("Commit message (Enter = new line, Ctrl+Enter commits)")} value={message} onChange={(e) => writeMessage(e.target.value)} disabled={busy} /><label className="gc-amend"><input type="checkbox" checked={amending} disabled={busy || !canAmend} onChange={(e) => toggleAmend(e.target.checked)} />{tt("Amend latest commit")}{amending && state ? ` · ${state.head.slice(0, 8)}` : ""}</label></div><div><button disabled={busy || !canCommit} onClick={commitNow}>{amending ? tt("Amend commit…") : state?.merging ? tt("Commit merge") : tt("Commit staged")}</button><small>{tt("Only staged changes · unsigned · hooks disabled · Ctrl+Enter commits")}</small></div></div>
    <div className="gc-key-help">{vim ? `${gitKeys.hints("files").map((hint) => `${hint.name === "Space" ? tt("Space") : hint.name} ${tt(hint.label)}`).join(" · ")} · ${tt("j/k files · l diff · ? keys")}` : tt("Arrow keys navigate · Tab selects controls · Space toggles selected changes · Ctrl+Enter commits")}</div>
    {busy && (actionProgress ? <GitProgressBar progress={actionProgress} compact /> : <p role="status">{tt("Updating Git index…")}</p>)}
  </div>;
}
