// Launch requests: `gall <path>`, `--select`, `--compare`, `--git`, a second instance, and
// org.freedesktop.FileManager1 calls (ShowFolders / ShowItems / ShowItemProperties) reach a
// window as `cli-args` / `filemanager-action` events. Requests that arrive before the window
// is ready are queued by the backend and drained here right after `windowReady`.
import { useLayoutEffect } from "react";
import { api, events } from "../api/ipc";
import type { CliArgs, FileManagerAction } from "../api/types";
import { parent } from "../core/path";
import { tt } from "../i18n";
import * as A from "./actions";
import { controller } from "./controllers";

/** Newer launch requests supersede the folder-selection wait of older ones. */
let launchSeq = 0;
const POLL_MS = 50;
const POLL_LIMIT_MS = 4000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Waits for `dir` to be listed in browser `b`, then selects `paths` (focusing the first).
 * Gives up when a newer request arrived, the browser moved elsewhere, or after 4 s.
 * Resolves with the entry of the first path, if it is there.
 */
export async function selectWhenListed(b: number, dir: string, paths: string[], seq: number) {
  const first = paths[0];
  for (let waited = 0; waited <= POLL_LIMIT_MS; waited += POLL_MS) {
    if (seq !== launchSeq) return null;
    const c = controller(b);
    if (c && c.path() === dir) {
      const entry = c.entries().find((e) => e.path === first);
      if (entry) {
        c.select(paths.filter((p) => c.entries().some((e) => e.path === p)), first);
        return entry;
      }
    }
    await sleep(POLL_MS);
  }
  return null;
}

async function isDir(path: string): Promise<boolean | null> {
  const r = await api.pathResolve(path).catch(() => null);
  return r && r.exists ? r.isDir : null;
}

let finishPending = false;
let closeWaitWindow: (() => void) | null = null;
let closePending = false;

export async function finishCompareWait(outcome: "accepted" | "cancelled" | "error", content?: string, message?: string) {
  if (finishPending) return;
  finishPending = true;
  try { await api.finishCompareWait(outcome, content, message); }
  catch (error) { finishPending = false; A.fail(error); }
}

function requestCompareWaitClose() {
  if (closeWaitWindow) closeWaitWindow();
  else closePending = true;
}

/** Native close uses the same discard guard as the control in the active handoff view. */
export function useCompareWaitClose(wait: boolean | undefined, close: () => void) {
  useLayoutEffect(() => {
    if (!wait) return;
    closeWaitWindow = close;
    if (closePending) {
      closePending = false;
      queueMicrotask(requestCompareWaitClose);
    }
    return () => { closeWaitWindow = null; };
  }, [wait, close]);
}

function errorMessage(error: unknown) {
  return String(error instanceof Error ? error.message : error);
}

async function openCompare([left, right]: [string, string], wait = false) {
  try {
    const [l, r] = await Promise.all([isDir(left), isDir(right)]);
    const reject = (message: string) => {
      if (wait) throw new Error(message);
      return A.notify(message, "warn");
    };
    if (l === null || r === null) return reject(`Cannot compare: “${l === null ? left : right}” does not exist`);
    if (l && r) return A.compareFolders(left, right, wait);
    if (l !== r) return reject("Cannot compare a file with a folder");
    A.compare({ kind: "file", path: left }, { kind: "file", path: right }, wait);
  } catch (error) {
    if (!wait) throw error;
    await finishCompareWait("error", undefined, errorMessage(error));
  }
}

async function chooseCompareFiles(seed: string | null) {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const left = seed ?? await open({ title: tt("Choose the first file to compare"), multiple: false, directory: false });
  if (typeof left !== "string") return;
  const right = await open({ title: tt("Choose the second file to compare"), multiple: false, directory: false, defaultPath: parent(left) ?? undefined });
  if (typeof right === "string") await openCompare([left, right]);
}

/** Writes the request into Gall.log (diagnostics, and what the E2E suite looks for). */
function logLaunch(a: CliArgs) {
  const parts = [
    a.path1 && `folder=${a.path1}`, a.path2 && `folder2=${a.path2}`,
    ...(a.paths ?? []).filter((p) => p !== a.path1 && p !== a.path2).map((p) => `extra=${p}`),
    ...(a.selectPaths ?? []).map((p) => `select=${p}`),
    a.compare && `compare=${a.compare[0]} <-> ${a.compare[1]}`, a.gitRepo && `git=${a.gitRepo}`,
    a.merge && `merge=${a.merge.local} <-> ${a.merge.remote}`, a.comparePicker && "compareFiles",
    a.newTab && "newTab", a.newWindow && "newWindow",
  ].filter(Boolean);
  if (parts.length) api.uiLog("info", `launch: ${parts.join(" ")}`).catch(() => {});
}

/** Applies a launch request to this window. */
export async function applyLaunch(a: CliArgs) {
  const seq = ++launchSeq;
  logLaunch(a);
  if (a.merge && a.wait) {
    const { base, local, remote, output } = a.merge;
    return A.openOverlay({ type: "standaloneMerge", base: base ?? undefined, left: local, right: remote, waitOutput: output });
  }
  if (a.comparePicker) return chooseCompareFiles(a.compareSeed);
  if (a.compare) return openCompare(a.compare, a.wait);
  if (a.gitRepo) return A.openGitWorkspace(a.gitRepo);
  const select = a.selectPaths ?? [];
  const folder = a.path1 ?? (select[0] ? parent(select[0]) : null);
  if (folder) {
    if (a.newTab) A.sessionCmd({ op: "newTab", b: 0, path: folder });
    else A.navigate(0, folder);
  }
  if (a.path2) A.navigate(1, a.path2);
  for (const extra of (a.paths ?? []).filter((p) => p !== a.path1 && p !== a.path2)) {
    A.sessionCmd({ op: "newTab", b: 0, path: extra, activate: false });
  }
  if (folder) A.focusBrowser(0);
  if (folder && select.length) await selectWhenListed(A.activeBrowser(), folder, select, seq);
}

/** `ShowItemProperties`: the folder and selection come with the `cli-args` of the same request. */
async function applyFileManager(f: FileManagerAction) {
  api.uiLog("info", `filemanager: ${f.kind} ${JSON.stringify(f)}`).catch(() => {});
  if (f.kind !== "showItemProperties") return;
  const seq = launchSeq;
  const entry = await selectWhenListed(A.activeBrowser(), f.folder, [f.target], seq);
  if (entry) A.openOverlay({ type: "properties", entry });
  // A newer launch request replaced this one: that is no "not found".
  else if (seq === launchSeq) A.notify(`“${f.target}” was not found in ${f.folder}`, "warn");
}

let listening: Promise<unknown> | null = null;

/** Registers the launch listeners (once per page). */
export function listenForLaunches() {
  listening ??= Promise.all([
    events.cliArgs((a) => void applyLaunch(a).catch(A.fail)),
    events.fileManagerAction((f) => void applyFileManager(f).catch(A.fail)),
    events.compareWaitClose(requestCompareWaitClose),
  ]);
  return listening;
}

/**
 * Marks this window ready and applies the launch requests that were queued meanwhile. The order
 * matters: requests before `windowReady` are queued (drained here), later ones arrive as events.
 */
export async function windowReadyAndDrain() {
  await listenForLaunches().catch(() => {});
  const staged = await api.windowReady().catch(() => false);
  if (staged) {
    requestAnimationFrame(() => requestAnimationFrame(() => {
      void api.windowFrameReady().catch(() => {});
    }));
  }
  try {
    const cli = await api.cliTakePending();
    const fm = await api.filemanagerTakePending();
    for (const a of cli) await applyLaunch(a);
    for (const f of fm) await applyFileManager(f);
  } catch (e) {
    A.fail(e);
  }
}
