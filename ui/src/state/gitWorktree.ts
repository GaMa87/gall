import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { api } from "../api/ipc";
import { tt } from "../i18n";
import { tildify } from "../core/path";
import { insideWorktree, type CleanupCandidate, type WorktreePlace, type WorktreeSource, type WorktreeState } from "../core/gitWorktree";
import type { LineRange } from "../core/gitFileHistory";
import * as A from "./actions";
import { refreshAll } from "./controllers";
import { getState, subscribe, type MenuItem } from "./store";
import { gitCommands, gitShownKey } from "../core/gitCommands";
import { platformLabel } from "../core/keymap";

export type WorktreeLocation = { main: string; path: string | null; error: string | null; inside: boolean };
type QuickResult = { path: string; existing: boolean; copied: number; linked: number; warnings: string[]; log: string };
const jobId = () => `worktree-${Date.now()}-${Math.random().toString(36).slice(2)}`;

/** Where a worktree would go; `template` and `own` default to what is saved. Starts no Git process. */
export function worktreeLocation(path: string, branch: string, template?: string, own?: Record<string, string>): Promise<WorktreeLocation> {
  const settings = getState().settings;
  return invoke<WorktreeLocation>("git_worktree_location", { path, template: template ?? settings?.worktreeLocation ?? "", templates: own ?? settings?.worktreeLocations ?? {}, branch });
}

/** Creates a worktree without a form and offers to open it or to take it back. `changed` runs after an undo. */
export async function createWorktree(path: string, source: WorktreeSource, changed: () => void, destination: string | null = null, onJobId?: (id: string) => void): Promise<string> {
  const settings = getState().settings;
  const request = { action: "quickWorktree", source, template: settings?.worktreeLocation ?? "", templates: settings?.worktreeLocations ?? {}, commands: settings?.worktreeCommands ?? {}, links: settings?.worktreeLinks ?? {}, destination };
  const id = jobId();
  onJobId?.(id);
  const made = JSON.parse(await invoke<string>("git_advanced_action", { path, id, request, expected: "" })) as QuickResult;
  const open = (newTab: boolean) => {
    A.openOverlay(null);
    const b = getState().session?.activeBrowser ?? 0;
    if (newTab) A.sessionCmd({ op: "newTab", b, path: made.path, activate: true }); else A.navigate(b, made.path);
  };
  const shown = tildify(made.path, getState().info?.home);
  const actions = [{ label: "Open", run: () => open(false) }, { label: "Open in new tab", run: () => open(true) }];
  if (made.existing) { A.notify(tt("This branch already has a worktree: {path}", { path: shown }), "info", actions); return made.path; }
  const undo = async () => {
    try {
      const note = await invoke<string>("git_advanced_action", { path, id: jobId(), request: { action: "undoWorktree", destination: made.path }, expected: "" });
      A.notify(tt("Worktree removed: {path}", { path: shown }), "success", undefined, undefined, note.trim() || undefined);
    } catch (e) { A.notify(String(e), "warn"); }
    refreshAll(); changed();
  };
  const text = made.copied && made.linked
    ? tt("Worktree created: {path} ({copied} ignored files copied, {linked} folders linked)", { path: shown, copied: made.copied, linked: made.linked })
    : made.copied
      ? tt("Worktree created: {path} ({count} ignored files copied)", { path: shown, count: made.copied })
      : made.linked
        ? tt("Worktree created: {path} ({count} ignored folders linked)", { path: shown, count: made.linked })
        : tt("Worktree created: {path}", { path: shown });
  A.notify(text, "success", [...actions, { label: "Undo", run: () => void undo() }]);
  if (made.warnings.length) A.notify(tt("The worktree was created, but not everything worked."), "warn", undefined, undefined, made.warnings.join("\n"));
  if (made.log) A.notify(tt("Command after creating the worktree:"), "info", undefined, undefined, made.log);
  return made.path;
}

/** Opens a worktree in the browser in use, the other one or a new tab. */
export function openWorktree(path: string, where: "here" | "other" | "tab") {
  A.openOverlay(null);
  const b = A.activeBrowser();
  if (where === "tab") void A.sessionCmd({ op: "newTab", b, path, activate: true });
  else if (where === "other") A.openInOther(b, path);
  else A.navigate(b, path);
}

/** Where else a worktree can be opened. */
export function worktreeMenu(path: string): MenuItem[] {
  return [
    { label: tt("Open"), icon: "open", run: () => openWorktree(path, "here") },
    { label: tt("Open in new tab"), run: () => openWorktree(path, "tab") },
    { label: tt("Open in the other browser"), run: () => openWorktree(path, "other") },
    { separator: true },
    { label: tt("Open in terminal"), icon: "terminal", run: () => { api.openTerminal(path).catch(A.fail); }, ctrl: { label: tt("Terminal here with…"), run: () => A.terminalWith(path) } },
    { label: tt("Open in editor"), icon: "rename", run: () => A.openOverlay({ type: "openWith", paths: [path], mode: "editor" }) },
    { separator: true },
    { label: tt("Copy path"), run: () => A.copyPaths([], path) },
  ];
}

/** The states of the worktrees on screen, asked one after the other and kept while they stay there. */
export function useWorktreeStates(paths: string[], reloadKey?: unknown): Map<string, WorktreeState | null> {
  const [states, setStates] = useState<Map<string, WorktreeState | null>>(new Map());
  const known = useRef<{ key: unknown; asked: Set<string> }>({ key: undefined, asked: new Set() });
  useEffect(() => {
    let alive = true;
    if (known.current.key !== reloadKey) known.current = { key: reloadKey, asked: new Set() };
    const { asked } = known.current;
    void (async () => {
      for (const path of paths) {
        if (!alive) return;
        if (asked.has(path)) continue;
        asked.add(path);
        const value = await invoke<WorktreeState>("git_worktree_state", { path }).catch(() => null);
        // Asked for nothing: the next list asks again.
        if (!alive) { asked.delete(path); return; }
        setStates((old) => new Map(old).set(path, value));
      }
    })();
    return () => { alive = false; };
  }, [paths, reloadKey]);
  return states;
}

type Advanced = { token: string; worktrees: { path: string }[] };
const advanced = (path: string) => invoke<Advanced>("git_advanced", { path, commit: null });
const advancedAction = (path: string, request: Record<string, unknown>, expected: string) => invoke<string>("git_advanced_action", { path, id: jobId(), request, expected });

/** Removes finished worktrees one by one; a failure leaves the others. A changed one is stored first, nothing is forced. */
export async function cleanUpWorktrees(
  path: string,
  picked: CleanupCandidate[],
  changed: (path: string) => boolean,
  ignored: boolean,
  onProgress?: (info: { done: number; total: number; detail: string }) => void,
) {
  const failed: { path: string; message: string }[] = [], notes: string[] = [];
  let removed = 0;
  const missing = picked.filter((c) => c.reason === "missing");
  const steps = [...picked.filter((c) => c.reason !== "missing"), ...missing.slice(0, 1)];
  const total = steps.length;
  onProgress?.({ done: 0, total, detail: "" });
  for (let i = 0; i < steps.length; i++) {
    const candidate = steps[i];
    onProgress?.({ done: i, total, detail: folderLabel(candidate.path) });
    try {
      const { token, worktrees } = await advanced(path);
      if (candidate.reason === "missing") { await advancedAction(path, { action: "pruneWorktrees" }, token); removed += missing.length; continue; }
      // Git's own spelling of the folder: what it lists is what it removes.
      const destination = worktrees.find((tree) => insideWorktree(tree.path, candidate.path) && insideWorktree(candidate.path, tree.path))?.path;
      if (!destination) throw new Error(tt("The worktree is no longer listed"));
      const request = changed(candidate.path) ? { action: "removeWorktreeWithSnapshot", destination, ignored } : { action: ignored ? "removeWorktreeWithIgnored" : "removeWorktree", destination };
      const note = await advancedAction(path, request, token);
      if (note.trim()) notes.push(`${candidate.path}: ${note.trim()}`);
      removed++;
    } catch (e) { failed.push({ path: candidate.path, message: e instanceof Error ? e.message : String(e) }); }
    onProgress?.({ done: i + 1, total, detail: folderLabel(candidate.path) });
  }
  refreshAll();
  return { removed, failed, notes };
}

// ───────── scratch worktrees: removed again when the last tab in them is closed, if nothing changed ─────────

const scratch = new Map<string, string>();
let watching = false;
const openTabs = () => (getState().session?.browsers ?? []).flatMap((browser) => browser.tabs.map((tab) => tab.path));

function watchScratch() {
  if (watching) return;
  watching = true;
  let session = getState().session;
  subscribe(() => {
    if (getState().session === session) return;
    session = getState().session;
    if (!scratch.size || !session) return;
    const tabs = openTabs();
    for (const [folder, repository] of scratch) {
      if (tabs.some((tab) => insideWorktree(tab, folder))) continue;
      scratch.delete(folder);
      void leaveScratch(folder, repository);
    }
  });
}

/** The undo removes it only when nothing changed in it since it was made. */
async function leaveScratch(folder: string, repository: string) {
  const shown = tildify(folder, getState().info?.home);
  try {
    const note = await advancedAction(repository, { action: "undoWorktree", destination: folder }, "");
    A.notify(tt("Temporary worktree removed: {path}", { path: shown }), "success", undefined, undefined, note.trim() || undefined);
  } catch (e) {
    A.notify(tt("The temporary worktree was kept: {path}", { path: shown }), "warn", [{ label: "Open", run: () => openWorktree(folder, "here") }, { label: "Clean up…", run: () => A.openOverlay({ type: "worktrees", path: repository, cleanup: true }) }], undefined, String(e));
  }
  refreshAll();
}

/** Checks a commit out in a temporary worktree and opens it in a new tab. */
export async function tryInWorktree(path: string, id: string, label: string) {
  const settings = getState().settings;
  const request = { action: "quickWorktree", source: { kind: "commit", id, label: `try-${label}` }, template: settings?.worktreeLocation ?? "", templates: settings?.worktreeLocations ?? {}, destination: null };
  const made = JSON.parse(await advancedAction(path, request, "")) as QuickResult;
  if (!made.existing) {
    scratch.set(made.path, path);
    watchScratch();
  }
  openWorktree(made.path, "tab");
  A.notify(tt("Temporary worktree: {path}\nIt is removed when no tab is in it any more, if nothing changed in it.", { path: tildify(made.path, getState().info?.home) }), "success");
  if (made.warnings.length) A.notify(tt("The worktree was created, but not everything worked."), "warn", undefined, undefined, made.warnings.join("\n"));
  if (made.log) A.notify(tt("Command after creating the worktree:"), "info", undefined, undefined, made.log);
}

/** Compares two worktrees: the folders, or for one inside the other (where a sync would copy it into itself) their commits. */
export async function compareWorktrees(repository: string, a: Pick<WorktreePlace, "path" | "head">, b: Pick<WorktreePlace, "path" | "head">) {
  if (!insideWorktree(a.path, b.path) && !insideWorktree(b.path, a.path)) { A.compareFolders(a.path, b.path); return; }
  if (a.head === b.head) { A.notify(tt("Both worktrees are at the same commit, and one is inside the other: their folders cannot be compared."), "warn"); return; }
  A.notify(tt("One worktree is inside the other, so their commits are compared, not their folders."));
  A.openOverlay({ type: "gitGraph", path: repository, compare: [a.head, b.head] });
}

/** What is being dragged out of Working Changes: kept in memory, like the file drags (WebKitGTK drops custom types). */
export type ChangeDrag = { path: string; file: string; staged: boolean; lines?: number[]; /** The lines of a dragged hunk in the file. */ range?: LineRange | null };
type Transfer = { token: string; copy: boolean; lines: number; targetChanged: boolean };
let changeDrag: ChangeDrag | null = null;
export const draggedChange = () => changeDrag;
export const endChangeDrag = () => { changeDrag = null; };
export function startChangeDrag(event: { dataTransfer: DataTransfer }, value: ChangeDrag) {
  changeDrag = value;
  event.dataTransfer.setData("text/plain", value.file);
  event.dataTransfer.effectAllowed = "copyMove";
}
/** The sentence of the confirmation that names the key doing the same as a drop. */
export function transferKeyboard(vim: boolean): string {
  const command = gitCommands("files", getState().settings?.shortcuts ?? {}).find((item) => item.action === "file:w");
  return tt("Keyboard equivalent: {command} ({key})", { command: command ? tt(command.label) : "file:w", key: (command && platformLabel(gitShownKey(command, vim))) || tt("Find a Git command…") });
}
const folderLabel = (value: string) => value.split(/[\\/]/).filter(Boolean).pop() ?? value;
/**
 * Brings a changed file, or the chosen lines of it, to another worktree: Git checks that it fits first, the confirmation
 * names both worktrees, and the other worktree's own changes are saved as a recovery reference before anything is applied.
 * Returns false when the user cancels; an error (it does not fit, the file is there) is thrown with Git's reason.
 */
export async function sendToWorktree(change: ChangeDrag, target: { path: string; label: string }, keyboard: string): Promise<boolean> {
  const request = { path: change.path, target: target.path, file: change.file, staged: change.staged, selected: change.lines ?? null };
  const found = await invoke<Transfer>("git_worktree_transfer", { ...request, expected: null, apply: false });
  const source = folderLabel(change.path), what = change.lines ? tt("the {count} chosen changed lines", { count: found.lines }) : tt("all {count} changed lines", { count: found.lines });
  const note = found.targetChanged ? `\n\n${tt("{target} has local changes of its own. Gall saves them as a recovery reference first.", { target: target.label })}` : "";
  const body = found.copy
    ? tt("Copy the new file “{file}” from {source} to {target}. Nothing is at that path there, so nothing is overwritten.\n\n{keyboard}", { file: change.file, source, target: target.label, keyboard })
    : tt("Apply {what} of “{file}” from {source} to {target}. The working file there changes and nothing is staged; Git has checked that it fits. {source} keeps its change.{note}\n\n{keyboard}", { what, file: change.file, source, target: target.label, note, keyboard });
  if (!await A.confirm(tt("Bring the change to {target}?", { target: target.label }), body, tt("Bring change"))) return false;
  await invoke<Transfer>("git_worktree_transfer", { ...request, expected: found.token, apply: true });
  A.notify(tt("Brought “{file}” to {target}", { file: change.file, target: target.label }), "success");
  return true;
}