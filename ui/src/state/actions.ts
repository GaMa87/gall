// Application actions: everything that changes state or touches the backend.
import { archiveOf, createFormatFor, isBrowsableArchive, type ArchiveFormat } from "../core/archive";
import { api, events, setMediaBase, type FavCmd, type MarkCmd, type SessionCmd, type WorkspaceCmd } from "../api/ipc";
import type { ConflictInfo, ConflictPolicy, DiffSource, Drive, FileEntry, FlowDirection, FlowOp, Layout, PairMode, Settings, TaskKind, Volume, WorkspaceSnapshot } from "../api/types";
import { activePair as findActivePair, flowSourceTabIsActive, nextPairMode, resolveFlowDispatch } from "../core/pairs";
import { createSerialQueue } from "./serialQueue";
import { syncActiveWorkspaceSummary } from "../core/workspaces";
import { eventAnchor, type Anchor } from "../core/anchor";
import { commitSides } from "../core/gitImages";
import { ownsClipboard } from "../core/clipOwner";
import { coalesced } from "../core/coalesce";
import { etaSeconds, updateRate, type Rate } from "../core/eta";
import { ancestors, backOutside, basename, isInside, join, parent, splitExt, tildify } from "../core/path";
import { controller, emitFolderChanged, refreshAll } from "./controllers";
import { bumpGitStatus, bumpSearchHidden } from "./hooks";
import { getState, monitorShown, setState, sidebarShown, type DialogSpec, type FolderSize, type MenuItem, type NoteAction, type NoteKind, type Overlay, type UiState } from "./store";
import { formatSize } from "../core/format";
import type { ImportChoice } from "../core/favoritesImport";
import { keyboardSettings } from "../core/keymap";
import { effectiveHidden, hiddenDivergence, planApplyHiddenAll, planToggleHiddenAll, planToggleHiddenBoard, type HiddenOverrides, type HiddenTogglePlan } from "../core/hiddenScope";
import { isMissingError as isMissingOperationError, operationError, volumeMountError } from "../core/operationError";
import { PREVIOUS_MARK, type MarkOp } from "../core/vim";
import { applyLaunch } from "./launch";
import { pickOpenEntries } from "./pick";
import { keyedActionsFor, runKeyedAction } from "../core/actionsMenu";
import { syncViewport } from "./desktop";
import { platformLabel } from "../core/keymap";
import { tt, t } from "../i18n";

let noteSeq = 1;
/** Smoothed speed per running task, for the time-left estimate. */
const taskRates = new Map<number, Rate>();

// ───────────── notifications (W-09) ─────────────

/** When each shown notification goes away (ms since the epoch); paused while hovered. */
const noteDue = new Map<number, number>();
let noteTimer = 0;
let notesHeld = false;

function scheduleNotes() {
  clearTimeout(noteTimer);
  if (notesHeld || !noteDue.size) return;
  const next = Math.min(...noteDue.values());
  noteTimer = window.setTimeout(() => {
    const now = Date.now();
    for (const [id, due] of noteDue) if (due <= now + 20) dismiss(id);
    scheduleNotes();
  }, Math.max(50, next - Date.now()));
}

export function notify(text: string, kind: NoteKind = "info", actions?: NoteAction | NoteAction[], reportError?: string, details?: string, sticky = false): number {
  const [action, ...more] = Array.isArray(actions) ? actions : [actions];
  const n = { id: noteSeq++, kind, text, at: Date.now(), action, more: more.filter((item) => !!item), reportError, details };
  setState((s) => {
    const last = s.notificationLog[0];
    const repeated = !action && !last?.action && last?.text === text && last.kind === kind
      && last.reportError === n.reportError && n.at - last.at < (s.settings?.notificationMs ?? 6000);
    const history = repeated ? s.notificationLog.slice(1) : s.notificationLog;
    return {
      notifications: [...s.notifications.filter((o) => o.text !== text || o.kind !== kind), n].slice(-5),
      notificationLog: [n, ...history.slice(0, 99)],
    };
  });
  // Errors stay longer (and in the notification log, bell); sticky progress toasts stay until dismissed by caller.
  if (!sticky) {
    const ms = getState().settings?.notificationMs ?? 6000;
    const life = kind === "error" ? Math.max(20_000, ms * 4) : kind === "warn" || action ? ms * 1.5 : ms;
    noteDue.set(n.id, Date.now() + life);
  }
  // Those pushed out of the five shown are forgotten.
  const shown = new Set(getState().notifications.map((o) => o.id));
  for (const id of noteDue.keys()) if (!shown.has(id)) noteDue.delete(id);
  scheduleNotes();
  return n.id;
}

export function dismiss(id: number) {
  noteDue.delete(id);
  setState((s) => ({ notifications: s.notifications.filter((n) => n.id !== id) }));
  // The last one gone under the pointer: no mouseleave will come.
  if (!getState().notifications.length && !Object.keys(getState().tasks).length) notesHeld = false;
}

/** The pointer is over the notifications: they wait (an Undo button must not run away);
 *  afterwards each gets at least a few more seconds. */
export function holdNotes(hold: boolean) {
  if (notesHeld === hold) return;
  notesHeld = hold;
  if (!hold) {
    const min = Date.now() + 2500;
    for (const [id, due] of noteDue) if (due < min) noteDue.set(id, min);
  }
  scheduleNotes();
}

/** Displays a failure and keeps its technical details in the application log. */
function showFailure(err: unknown, context?: string, action?: NoteAction) {
  const raw = String(err);
  const feedback = operationError(raw, getState().info?.platform);
  const text = context && feedback.kind === "error" && !feedback.text.startsWith("Cannot ")
    ? `${context}: ${feedback.text}` : feedback.text;
  const detail = context ? `${context}: ${raw}` : raw;
  notify(text, feedback.kind, action, feedback.reportable ? detail : undefined);
  if (feedback.kind !== "info") api.uiLog(feedback.reportable ? "error" : "warn", detail).catch(() => {});
}

export function fail(err: unknown) {
  showFailure(err);
}

export function isMissingError(raw: string): boolean {
  return isMissingOperationError(raw, getState().info?.platform);
}

/** Browser `b` shows `path`, which no longer exists (deleted, renamed, drive gone):
 *  it goes to the nearest folder above that still exists. */
export async function leaveMissing(b: number, path: string) {
  const chain = ancestors(path).reverse();
  for (let i = 1; i < chain.length; i++) {
    const anc = chain[i];
    const r = await api.pathResolve(anc).catch(() => null);
    if (r?.exists && r.isDir) {
      if (activePath(b) !== path) return; // the user went elsewhere meanwhile
      const gone = chain[i - 1];
      // An unplugged or unmounted drive: back to where the browser was before it, as when
      // ejecting (not into /run/media/<user>); its folders stay in the history.
      if (isDriveMount(gone)) {
        await leaveMountIn(b, gone);
        notify(`“${basename(gone)}” is not connected`, "info");
        return;
      }
      // Going there refreshes the history, which drops the deleted folder.
      navigate(b, anc);
      notify(`“${basename(path)}” no longer exists; showing “${basename(anc) || anc}”`, "info");
      return;
    }
  }
}

/** A drive's mount point (listed, or where removable drives are mounted), also once gone. */
function isDriveMount(path: string): boolean {
  const mountParent = /^(\/run\/media\/[^/]+|\/media(\/[^/]+)?|\/mnt|\/Volumes)$/;
  return getState().drives.some((d) => d.mount === path) || mountParent.test(parent(path) ?? "");
}


export function readableError(raw: string): string {
  return operationError(raw, getState().info?.platform).text;
}

// ───────────── overlays / dialogs / menus ─────────────

function isWaitingComparison(o: Overlay | null | undefined) {
  const view = o?.type === "dialog" ? o.under : o;
  return (view?.type === "diff" || view?.type === "dircmp") ? !!view.wait
    : view?.type === "standaloneMerge" && view.waitOutput !== undefined;
}

export function openOverlay(o: Overlay | null) {
  const current = getState().overlay;
  // An unrelated UI action (for example an update notification) must not replace
  // the only view that can answer an external caller.
  if (isWaitingComparison(current) && !isWaitingComparison(o)) return;
  setState({ overlay: o, menu: null });
}

export function dialog(spec: Omit<DialogSpec, "resolve">): Promise<{ button: string; value: string; checked?: boolean } | null> {
  // Asked from a window (compare view, trash, settings): that window stays open under the
  // dialog and is back once it is answered, instead of being closed (losing its state).
  // A dialog still waiting is cancelled, so whoever awaits it is not left hanging.
  const prev = getState().overlay;
  if (prev?.type === "dialog") prev.dialog.resolve(null);
  const cur = getState().overlay;
  const under = cur?.type === "dialog" ? cur.under ?? null : cur;
  return new Promise((resolve) => {
    const d: DialogSpec = {
      ...spec,
      resolve: (r) => {
        const o = getState().overlay;
        if (o?.type === "dialog" && o.dialog === d) openOverlay(under);
        resolve(r);
      },
    };
    openOverlay({ type: "dialog", dialog: d, under });
  });
}

export async function confirm(title: string, message: string, okLabel = "OK", danger = false): Promise<boolean> {
  const r = await dialog({ title, message, buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: okLabel, primary: true, danger }] });
  return r?.button === "ok";
}

/**
 * Git refuses a repository owned by another user (a WSL folder opened from Windows, a network
 * share). After a confirmation, adds exactly the path Git names to its global safe.directory.
 * Returns false when Git does not refuse the folder, so the caller can say it is no repository.
 */
export async function trustGitFolder(path: string): Promise<boolean> {
  const repo = await api.gitUntrusted(path).catch(() => null);
  if (!repo) return false;
  const ok = await confirm(tt("Trust this Git repository?"),
    tt("Git refuses to read this repository because it belongs to another user (for example a WSL folder opened from Windows, or a network share):\n\n{repo}\n\nTrusting adds it to Git's global safe.directory list. Only do this for repositories you trust: their configuration can run commands.", { repo }), "Trust");
  if (!ok) return true;
  try {
    await api.gitTrust(path);
    bumpGitStatus();
    notify(tt("Git repository trusted"));
  } catch (e) { showFailure(e, tt("Could not trust the repository")); }
  return true;
}

export async function promptText(title: string, value: string, okLabel = "OK", select?: [number, number]): Promise<string | null> {
  const r = await dialog({ title, input: { value, select }, buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: okLabel, primary: true }] });
  return r && r.button !== "cancel" ? r.value : null;
}

let menuToken = 0;

/** Opens a menu at an anchor (the element a key acts on, or a point). The placement rule is in core/anchor.ts. */
export function showMenuAt(anchor: Anchor, items: MenuItem[]): number {
  const token = ++menuToken;
  setState({ menu: { ...anchor, items } });
  return token;
}

/** Updates the currently open menu only if it is still the one opened with `token`. */
export function updateOpenMenu(token: number, updater: (items: MenuItem[]) => MenuItem[]) {
  if (token !== menuToken) return;
  const cur = getState().menu;
  if (!cur) return;
  setState({ menu: { ...cur, items: updater(cur.items) } });
}

/** Opens a menu for a click or a right click: at the pointer, or at the element when a key made the event. */
export function showMenuFor(event: Parameters<typeof eventAnchor>[0], items: MenuItem[]): number {
  return showMenuAt(eventAnchor(event), items);
}

// ───────────── bootstrap ─────────────

export async function bootstrap() {
  const began = performance.now();
  // Startup timing goes to the log: slow calls show up there.
  const timed = <T,>(name: string, p: Promise<T>): Promise<T> => {
    const t = performance.now();
    return p.finally(() => {
      const ms = performance.now() - t;
      if (ms > 50) api.uiLog("info", `startup: ${name} took ${Math.round(ms)} ms`).catch(() => {});
    });
  };
  // The history and the drives are not needed to show the folders, and both can be slow
  // (the history looks at every folder in it, a network share that does not answer
  // included; the drives ask every mount): they are asked for at once, but the window
  // does not wait for the history at all and for the drives only briefly, so the
  // sidebar is normally complete when it appears.
  const history = timed("history", api.historyGet()).then((history) => setState({ history })).catch(() => {});
  const drives = timed("drives", api.listDrives()).then((drives) => setState({ drives })).catch(() => {});
  void history;
  // The listeners are registered while the state loads (one message each): events of
  // the first folders must find them.
  const listening = listen();
  const [info, settings, wsSnap, favorites, marks, args] = await Promise.all([
    timed("appInfo", api.appInfo()), timed("settings", api.getSettings()), timed("workspaces", api.workspacesGet()),
    timed("favorites", api.favoritesGet()), timed("marks", api.marksGet()), timed("args", api.startupArgs()),
  ]);
  const fetched = performance.now();
  await Promise.race([drives, new Promise((done) => setTimeout(done, 150))]);
  const withDrives = performance.now();
  await listening;
  api.uiLog("info", `startup: state fetched ${Math.round(fetched - began)} ms, drives +${Math.round(withDrives - fetched)} ms, listeners +${Math.round(performance.now() - withDrives)} ms`).catch(() => {});
  setMediaBase(info.mediaBase);
  setState({
    info,
    settings: keyboardSettings(settings),
    session: wsSnap.session,
    workspaces: wsSnap.workspaces,
    activeWorkspace: wsSnap.active,
    favorites,
    marks,
    sidebarVisible: settings.sidebarVisible ?? true,
  });
  scheduleGravityRefresh();
  if (args.pick) {
    setState({ pick: args.pick, pickChoices: Object.fromEntries(args.pick.choices.map((c) => [c.id, c.initial])) });
    if (args.pick.title) document.title = args.pick.title;
  }
  void applyLaunch(args).catch(fail);
  // First start: the welcome setup (not during self-tests, which start with fresh settings).
  // Not in a picker window: it answers another program and must not ask for a setup first.
  if (info.firstRun && !args.selftest && !args.pick && !args.wait && !args.comparePicker) openOverlay({ type: "welcome" });
  if (args.selftest) setTimeout(() => import("../selftest").then((m) => (args.selftestMode === "demo" ? m.runDemo() : args.selftestMode === "design" ? m.runDesignShots() : args.selftestMode === "audit" ? m.runUiAudit() : m.runSelfTest())), 1500);
  // Backend warnings and errors from before the UI listened: never only in the log file.
  // Not awaited: the window appears without waiting for them.
  void api.logBacklog().then((lines) => { for (const l of lines) notify(l.msg, l.level, undefined, l.level === "error" ? l.msg : undefined); }).catch(() => {});
  // Poll drives and cloud only while this window is visible and focused so unfocused
  // background windows stay completely quiet, catching up immediately when refocused.
  const visible = () => typeof document === "undefined" || !document.hidden;
  const focused = () => visible() && (typeof document === "undefined" || typeof document.hasFocus !== "function" || document.hasFocus());
  let lastFocusRefresh = 0;
  const catchUp = () => {
    if (!visible()) return;
    const now = Date.now();
    if (now - lastFocusRefresh < 2000) return;
    lastFocusRefresh = now;
    void refreshDrives();
  };
  setInterval(() => { if (focused()) void refreshDrives(); }, 15_000);
  refreshCloud();
  // Clients can be installed or signed in while Gall runs.
  setInterval(() => { if (focused()) void refreshCloud(); }, 120_000);
  document.addEventListener("visibilitychange", () => {
    if (visible()) {
      catchUp();
      void refreshCloud();
    }
  });
  window.addEventListener("focus", catchUp);
}

/** Registers the backend listeners, all at once. */
function listen(): Promise<unknown> {
  return Promise.all([
    events.fsChanged(({ path, storm }) => {
      if (storm) setStormNotice(path);
      invalidateFolderSizes(path);
      emitFolderChanged(path, storm);
    }),
    events.task((ev) => {
      if (ev.state === "progress") {
        // Bytes when known (copy/move), else items (delete).
        const byBytes = ev.totalBytes > 0;
        const now = Date.now();
        const r = updateRate(taskRates.get(ev.id), now, byBytes ? ev.doneBytes : ev.doneItems);
        taskRates.set(ev.id, r);
        const eta = etaSeconds(r, byBytes ? ev.totalBytes : ev.totalItems, now);
        setState((s) => ({ tasks: { ...s.tasks, [ev.id]: { ...ev, eta } } }));
        return;
      }
      taskRates.delete(ev.id);
      const replaceReq = replaceRequests.get(ev.id);
      replaceRequests.delete(ev.id);
      const cutPaths = cutPastes.get(ev.id);
      cutPastes.delete(ev.id);
      // Not when a permanent replace is about to be offered: that retries the move.
      const retrying = !!replaceReq && ev.state === "failed" && String(ev.error).startsWith("no-trash:");
      if (cutPaths && ev.state !== "done" && !retrying) void restoreCut(cutPaths);
      setState((s) => {
        const t = { ...s.tasks };
        delete t[ev.id];
        return { tasks: t };
      });
      const verb: Record<TaskKind, string> = { copy: "Copied", move: "Moved", delete: "Deleted", compress: "Created", extract: "Extracted", duplicate: "Duplicated" };
      const operation: Record<TaskKind, string> = { copy: "Copy", move: "Move", delete: "Delete", compress: "Compression", extract: "Extraction", duplicate: "Duplication" };
      const undoFor = undoAction(ev.undoId);
      if (ev.state === "done" && ev.items === 0) {
        notify(ev.skipped ? tt("Skipped {value}", { value: ev.name ? `“${ev.name}”` : items(ev.skipped) }) : tt("{operation} finished. No items changed.", { operation: tt(operation[ev.kind]) }));
      } else if (ev.state === "done" && (ev.kind === "compress" || (ev.kind === "extract" && ev.items === 1))) {
        notify(ev.kind === "compress" ? tt("Created “{name}”", { name: ev.name ?? "" }) : tt("Extracted to “{name}”", { name: ev.name ?? "" }), "success", undoFor);
      } else if (ev.state === "done") {
        const what = ev.name && ev.items === 1 ? `“${ev.name}”` : items(ev.items);
        const to = ev.to && ev.kind !== "delete" ? tt(" to {to}", { to: ev.to }) : "";
        notify(`${tt(verb[ev.kind])} ${what}${to}${ev.skipped ? tt(", skipped {count}", { count: ev.skipped }) : ""}`, "success",
          ev.kind !== "delete" ? undoFor : undefined);
      } else if (ev.state === "failed" && retrying && replaceReq) {
        void offerPermanentReplace(replaceReq, ev.error);
      } else if (ev.state === "failed") {
        showFailure(ev.error, tt("{operation} failed", { operation: tt(operation[ev.kind]) }), undoFor);
      } else {
        notify(tt("{operation} cancelled", { operation: tt(operation[ev.kind]) }), "info", undoFor);
      }
      refreshUndo();
      invalidateAllFolderSizes();
      // A folder deleted or moved away leaves the history (the backend drops gone folders).
      if (ev.kind === "delete" || ev.kind === "move") {
        api.historyGet().then((history) => setState({ history })).catch(() => {});
      }
    }),
    events.folderSizeProgress(applyFolderSizeProgress),
    // Backend warnings and errors: never only in the log file.
    events.backendLog((l) => notify(l.msg, l.level, undefined, l.level === "error" ? l.msg : undefined)),
  ]);
}

let stormShown = 0;
function setStormNotice(path: string) {
  if (Date.now() - stormShown > 30_000) {
    stormShown = Date.now();
    notify(tt("Something is changing files very rapidly in {path}; refreshes are throttled.", { path }), "warn");
  }
}

let drivesInFlight = false;

export async function refreshDrives() {
  if (drivesInFlight) return;
  drivesInFlight = true;
  try {
    const [drives, volumes] = await Promise.all([api.listDrives(), api.otherVolumes().catch(() => getState().volumes)]);
    const old = getState().drives;
    if (JSON.stringify(old) !== JSON.stringify(drives)) {
      setState({ drives });
      // A drive that went away (unmounted elsewhere, pulled out): browsers leave it.
      for (const d of old) if (!drives.some((n) => n.mount === d.mount)) void leaveMount(d.mount);
      // A cloud client may have mounted or removed its drive (Google Drive).
      refreshCloud();
    }
    if (JSON.stringify(getState().volumes) !== JSON.stringify(volumes)) setState({ volumes });
  } catch {
    /* ignore transient */
  } finally {
    drivesInFlight = false;
  }
}

/** The kernel refuses an NTFS volume that Windows left marked dirty. Offers to clear the flag
 *  (`ntfsfix -d`, asks for the administrator password) and mount it. True when handled. */
async function repairAndMountNtfs(v: Volume, name: string): Promise<boolean> {
  const r = await dialog({
    title: tt("Repair and mount {name}?", { name }),
    message: tt("{name} could not be mounted. Linux refuses an NTFS volume that Windows left marked as unclean (a crash, a forced power-off or Fast Startup). Gall can clear that flag and mount it; you will be asked for the administrator password. Do this only if Windows was shut down completely. Running chkdsk in Windows is the thorough repair.", { name }),
    buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Repair and mount", primary: true }],
  });
  if (!r || r.button !== "ok") return true;
  const progress = notify(tt("Repairing {name}…", { name }));
  try {
    const mount = await api.volumeRepairMount(v.id);
    await refreshDrives();
    notify(tt("Mounted {name}", { name }), "success");
    if (mount) navigate(activeBrowser(), mount);
  } catch (e) {
    api.uiLog("warn", `repair mount ${v.id}: ${String(e)}`).catch(() => {});
    notify(String(e), "warn");
  } finally {
    dismiss(progress);
  }
  return true;
}

/** Powers a removable drive off from "Not mounted", so the stick can be unplugged safely. */
export async function powerOffVolume(v: Volume) {
  try {
    await api.volumePowerOff(v.id);
    notify(tt("“{value}” can be unplugged now", { value: v.label }), "success");
  } catch (e) {
    fail(e);
  }
  await refreshDrives();
}

/** Mounts a volume from "Not mounted" (asking for the passphrase if it is encrypted)
 *  and opens it in the active browser. */
export async function mountVolume(v: Volume) {
  const name = `“${v.label}”`;
  let progressNotification: number | undefined;
  try {
    let mount: string | null = null;
    if (!v.locked) {
      progressNotification = notify(tt(v.fs === "wsl" ? "Starting {name}…" : "Mounting {name}…", { name }));
      mount = await api.volumeMount(v.id);
    } else if (getState().info?.platform === "windows") {
      await api.volumeUnlock(v.id, ""); // Windows asks in its own BitLocker dialog
      setTimeout(refreshDrives, 3000);
      return;
    } else {
      // First check whether the desktop keyring already holds the saved passphrase
      // (zero Gall prompt when unlocked previously in Dolphin/KWallet or GNOME Files/Keyring).
      let needPrompt = false;
      try {
        mount = await api.volumeUnlock(v.id, "");
      } catch (e) {
        const err = String(e);
        if (err === "Passphrase required" || err === "Wrong passphrase") {
          needPrompt = true;
        } else {
          throw e;
        }
      }
      if (needPrompt) {
        let message = v.removable
          ? `${v.label} is encrypted. Enter its passphrase to unlock and mount it.`
          : `${v.label} is an encrypted internal partition. Enter its encryption passphrase (your system may then also ask for your administrator password to authorize unlocking it).`;
        let remember = false;
        for (;;) {
          const r = await dialog({
            title: tt("Unlock {name}", { name }),
            message,
            input: { value: "", password: true, placeholder: "Passphrase" },
            checkbox: { label: "Remember passphrase in system keyring", checked: remember },
            buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Unlock", primary: true }],
          });
          if (!r || r.button !== "ok" || !r.value) return;
          remember = !!r.checked;
          try {
            mount = await api.volumeUnlock(v.id, r.value, remember);
            break;
          } catch (e) {
            const err = String(e);
            if (err === "Wrong passphrase") {
              message = `${err}. Try again.`;
              continue;
            }
            throw e;
          }
        }
      }
    }
    await refreshDrives();
    notify(tt(v.locked ? "Unlocked {name}" : v.fs === "wsl" ? "Started {name}" : "Mounted {name}", { name }), "success");
    if (mount) navigate(activeBrowser(), mount);
  } catch (e) {
    const feedback = volumeMountError(String(e), v.fs);
    if (feedback) {
      if (progressNotification !== undefined) dismiss(progressNotification);
      if (v.fs.toLowerCase() === "ntfs" && !v.locked && (await repairAndMountNtfs(v, name))) return;
      notify(tt(feedback.text, { name: v.label, fs: v.fs || "unknown" }), feedback.kind, undefined, undefined, feedback.details);
      api.uiLog("warn", `mount ${v.id}: ${String(e)}`).catch(() => {});
    } else {
      fail(e);
    }
  } finally {
    if (progressNotification !== undefined) dismiss(progressNotification);
  }
}

/**
 * Browsers showing a folder on the drive mounted at `mount` go back to where they were
 * before it (the latest folder in their history outside it that still exists), else to
 * the home folder: before ejecting or locking it (an open folder can keep it busy), and
 * when it disappears.
 */
export async function leaveMount(mount: string) {
  for (const b of [0, 1]) await leaveMountIn(b, mount);
}

/** {@link leaveMount} for browser `b`. */
async function leaveMountIn(b: number, mount: string) {
  const home = getState().info?.home ?? "/";
  const br = getState().session?.browsers[b];
  const tab = br?.tabs[br.active];
  if (!tab || !isInside(tab.path, mount) || isInside(home, mount)) return;
  let target = home;
  for (const p of backOutside(tab.history.back, mount)) {
    const r = await api.pathResolve(p).catch(() => null);
    if (r?.exists && r.isDir) { target = r.path; break; }
  }
  navigate(b, target);
}

/** Ejects a removable drive; browsers on it go back first. */
export async function ejectDrive(d: Drive) {
  // Stopping a WSL distribution ends every program in it, editors with unsaved work too.
  if (d.fs === "wsl" && !(await confirm(tt("Stop {short}?", { short: d.short }),
    tt("Every program running in the WSL distribution “{short}” is closed; unsaved work in them is lost.", { short: d.short }), "Stop", true))) return;
  // Linux only unmounts when ejecting; ask whether the stick should also be powered off so it
  // can be unplugged. Enter picks the default, the full power-off.
  let powerOff = false;
  if (d.fs !== "wsl" && getState().info?.platform === "linux") {
    const r = await dialog({
      title: tt("Remove {value}", { value: d.label || d.short }),
      message: tt("Power the drive off so it can be unplugged, or only unmount it? An unmounted drive stays under “Not mounted” and can be mounted again."),
      buttons: [
        { id: "cancel", label: "Cancel" },
        { id: "unmount", label: "Unmount only" },
        { id: "poweroff", label: "Power off", primary: true },
      ],
    });
    if (!r || r.button === "cancel") return;
    powerOff = r.button === "poweroff";
  }
  await leaveMount(d.mount);
  try {
    await api.eject(d.mount, powerOff);
    notify(d.fs === "wsl" ? tt("Stopped “{short}” (WSL)", { short: d.short })
      : powerOff ? tt("“{value}” can be unplugged now", { value: d.label || d.short })
      : tt("Ejected “{value}”", { value: d.label || d.short }), "success");
  } catch (e) {
    fail(e);
  }
  await refreshDrives();
}

/** Unmounts an unlocked encrypted drive and locks it again. */
export async function lockDrive(d: Drive) {
  try {
    await leaveMount(d.mount);
    await api.driveLock(d.mount);
    notify(tt("Locked “{value}”", { value: d.label || d.short }), "success");
    await refreshDrives();
  } catch (e) {
    fail(e);
  }
}

let cloudInFlight = false;

/** Looks up the cloud folders again (start-up, drive changes, periodically while focused). */
export async function refreshCloud() {
  if (cloudInFlight) return;
  cloudInFlight = true;
  try {
    const cloud = await api.listCloudFolders();
    if (JSON.stringify(getState().cloud) !== JSON.stringify(cloud)) setState({ cloud });
  } catch {
    /* ignore transient */
  } finally {
    cloudInFlight = false;
  }
}

// ───────────── settings ─────────────

/** Writes the settings to the backend; a field that changes with every key (text, a
 *  colour being picked) waits for a pause, because each write is a file synced to disk
 *  and makes every browser ask for its folder's view again. */
const settingsSave = coalesced(async (next: Settings) => {
  try {
    await api.setSettings(next);
    setState({ settingsRev: getState().settingsRev + 1 });
  } catch (e) {
    fail(e);
  }
}, 300);
if (typeof window !== "undefined") window.addEventListener("pagehide", () => void settingsSave.flush());

/** Changes settings. The UI has them at once; with `whileTyping` they are saved after a
 *  short pause, else now (and the promise waits for it). */
export async function updateSettings(patch: Partial<Settings>, whileTyping = false) {
  const s = getState().settings;
  if (!s) return;
  const next = keyboardSettings(s, patch);
  const changedKeys = next.keyboardProfile !== s.keyboardProfile || next.leaderKey !== s.leaderKey || next.leaderKeyBasic !== s.leaderKeyBasic;
  setState({ settings: next, ...(changedKeys ? { keySeq: [] } : {}) });
  // Changing the global showHidden default re-syncs any per-browser override.
  if (patch.showHidden !== undefined && getState().session?.browsers.some((br) => br.showHidden != null)) {
    void sessionCmd({ op: "hidden", b: null, on: null });
  }
  if (patch.liveTabPairs === false) {
    const pairs = getState().session?.pairs ?? [];
    for (const p of pairs) void sessionCmd({ op: "unpair", pairId: p.id });
    if (getState().hoveredPairId !== null) setState({ hoveredPairId: null });
  }
  if (patch.pairGravity === false) {
    if (getState().gravityGhost[0] || getState().gravityGhost[1]) setState({ gravityGhost: [null, null] });
  } else if (patch.pairGravity === true) {
    scheduleGravityRefresh();
  }
  if (whileTyping) settingsSave.later(next);
  else await settingsSave.now(next);
}

/** Forgets the sort and view kept for every folder (Settings › View). */
export async function forgetFolderViews() {
  if (!(await confirm(tt("Forget folder views?"), tt("Every folder goes back to the default sort and view."), "Forget", true))) return;
  try {
    await api.clearPrefs();
    setState({ settingsRev: getState().settingsRev + 1 });
    notify(tt("Folder views forgotten"));
  } catch (e) {
    fail(e);
  }
}

/** Factory reset (Settings › Advanced): every setting back to its default. */
export async function resetAllSettings() {
  if (!(await confirm(tt("Reset all settings?"),
    tt("Every setting, shortcut, folder colour, remembered program and folder view goes back to the default. Favourites, colour tags, history and open tabs stay."),
    "Reset", true))) return;
  try {
    await settingsSave.flush();
    const settings = await api.resetSettings();
    setState({ settings: keyboardSettings(settings), keySeq: [], settingsRev: getState().settingsRev + 1 });
    notify(tt("Settings reset to defaults"));
    // Back to first-start settings: the welcome setup again (it replaces the settings window).
    openOverlay({ type: "welcome" });
  } catch (e) {
    fail(e);
  }
}

// ───────────── session / navigation ─────────────

const enqueueWorkspaceMutation = createSerialQueue();

export async function sessionCmd(cmd: SessionCmd): Promise<boolean> {
  const s = getState();
  const workspaceId = s.workspaces[s.activeWorkspace]?.id;
  try {
    return await enqueueWorkspaceMutation(async () => {
      const session = await api.sessionCmd(cmd, workspaceId);
      const current = getState();
      if (workspaceId != null && current.workspaces[current.activeWorkspace]?.id !== workspaceId) return false;
      setState((state) => ({
        session,
        workspaces: syncActiveWorkspaceSummary(state.workspaces, state.activeWorkspace, session),
      }));
      if (cmd.op === "navigate" || cmd.op === "back" || cmd.op === "forward" || cmd.op === "up") {
        api.historyGet().then((history) => setState({ history })).catch(() => {});
      }
      scheduleGravityRefresh();
      return true;
    });
  } catch (e) {
    fail(e);
    return false;
  }
}

function applyWorkspaceSnapshot(snap: WorkspaceSnapshot) {
  const prev = getState();
  const prevId = prev.workspaces[prev.activeWorkspace]?.id ?? null;
  const nextId = snap.workspaces[snap.active]?.id ?? null;
  const switched = prevId !== null && nextId !== null && prevId !== nextId;
  setState({
    session: snap.session,
    workspaces: snap.workspaces,
    activeWorkspace: snap.active,
    ...(switched ? { focused: [null, null], worktreeOf: [], keySeq: [], menu: null, hoveredPairId: null, gravityGhost: [null, null] } : {}),
  });
  if (switched) {
    setTimeout(() => controller(snap.session.activeBrowser)?.focus(), 0);
    scheduleGravityRefresh();
  }
}

export async function workspaceCmd(cmd: WorkspaceCmd) {
  const s = getState();
  const sourceWorkspaceId = s.workspaces[s.activeWorkspace]?.id;
  const expectedWorkspaceIds = s.workspaces.map((workspace) => workspace.id);
  try {
    await enqueueWorkspaceMutation(async () => {
      const snap = await api.workspaceCmd(cmd, sourceWorkspaceId, cmd.op === "create" ? undefined : expectedWorkspaceIds);
      applyWorkspaceSnapshot(snap);
    });
  } catch (e) {
    fail(e);
  }
}

export function switchWorkspace(idx: number) {
  const s = getState();
  if (idx < 0 || idx >= s.workspaces.length || idx === s.activeWorkspace) return;
  void workspaceCmd({ op: "switch", idx });
}

export function cycleWorkspace(delta: 1 | -1) {
  const s = getState();
  if (s.workspaces.length < 2) return;
  const next = (s.activeWorkspace + delta + s.workspaces.length) % s.workspaces.length;
  void workspaceCmd({ op: "switch", idx: next });
}

export function createWorkspace(opts?: { name?: string | null; accent?: string | null; cloneCurrent?: boolean }) {
  void workspaceCmd({ op: "create", ...opts });
}

export function duplicateWorkspace(idx: number) {
  void workspaceCmd({ op: "duplicate", idx });
}

export async function promptRenameWorkspace(idx: number) {
  const ws = getState().workspaces[idx];
  if (!ws) return;
  const name = await promptText(t("workspace.renameTitle"), ws.name ?? "", t("common.save"));
  if (name !== null) void workspaceCmd({ op: "rename", idx, name: name.trim() || null });
}

export function setWorkspaceAccent(idx: number, accent: string | null) {
  void workspaceCmd({ op: "accent", idx, accent });
}

export function reorderWorkspace(from: number, to: number) {
  if (from === to) return;
  void workspaceCmd({ op: "reorder", from, to });
}

export function closeWorkspace(idx: number) {
  if (getState().workspaces.length < 2) return;
  void workspaceCmd({ op: "close", idx });
}

// ───────────── live tab pairs (WN-180) ─────────────

export function pairTabs(
  leftIdx: number,
  rightIdx: number,
  mode: PairMode = "linked",
  flowDir?: FlowDirection,
  flowOp?: FlowOp,
) {
  void sessionCmd({ op: "pairTabs", leftIdx, rightIdx, mode, flowDir, flowOp });
}

export function pairActiveTabs(mode: PairMode = "linked", flowDir?: FlowDirection, flowOp?: FlowOp) {
  const s = getState().session;
  if (!s || s.browsers.length < 2) return;
  pairTabs(s.browsers[0].active, s.browsers[1].active, mode, flowDir, flowOp);
}

export function unpairTabs(pairId: number) {
  if (getState().hoveredPairId === pairId) setState({ hoveredPairId: null });
  void sessionCmd({ op: "unpair", pairId });
}

export function setHoveredPair(pairId: number | null) {
  if (getState().hoveredPairId !== pairId) setState({ hoveredPairId: pairId });
}

export function setPairMode(pairId: number, mode: PairMode, flowDir?: FlowDirection, flowOp?: FlowOp) {
  void sessionCmd({ op: "pairMode", pairId, mode, flowDir, flowOp });
}

export function reanchorPair(pairId: number) {
  void sessionCmd({ op: "pairReanchor", pairId });
  notify(t("pair.reanchored"), "info");
}

export function toggleActivePair() {
  if (getState().settings?.liveTabPairs === false) return;
  const s = getState().session;
  if (!s || s.browsers.length < 2) return;
  const cur = findActivePair(s);
  if (cur) {
    unpairTabs(cur.id);
    return;
  }
  // If both panes are in different linked Git worktrees, default straight to Tandem walk.
  const wt = getState().worktreeOf;
  const defaultMode: PairMode = wt[0] && wt[1] && wt[0] !== wt[1] ? "tandem" : "linked";
  pairActiveTabs(defaultMode);
}

export function cycleActivePairMode(pairId?: number) {
  const s = getState().session;
  if (!s) return;
  const pair = pairId != null ? s.pairs?.find((p) => p.id === pairId) : findActivePair(s);
  if (!pair) {
    pairActiveTabs("linked");
    return;
  }
  const next = nextPairMode(pair.mode);
  setPairMode(pair.id, next, pair.flowDir, pair.flowOp);
}

export function flipActiveFlowDirection(pairId?: number) {
  const s = getState().session;
  if (!s) return;
  const pair = pairId != null ? s.pairs?.find((p) => p.id === pairId) : findActivePair(s);
  if (!pair) return;
  const nextDir: FlowDirection = pair.flowDir === "rightToLeft" ? "leftToRight" : "rightToLeft";
  setPairMode(pair.id, "flow", nextDir, pair.flowOp);
}

export async function sendAcrossFlow(pairId?: number) {
  const s = getState().session;
  if (!s) return;
  const pair = pairId != null ? s.pairs?.find((p) => p.id === pairId) : findActivePair(s);
  const plan = resolveFlowDispatch(s, pair);
  if (!plan) {
    notify(t("pair.noActiveFlow"), "warn");
    return;
  }
  const sourceBrowser = s.browsers[plan.srcBrowser];
  const sourceTab = sourceBrowser?.tabs[sourceBrowser.active];
  const sourceController = controller(plan.srcBrowser);
  if (
    !pair
    || !flowSourceTabIsActive(s, pair)
    || !sourceTab
    || !sourceController
    || sourceController.path() !== sourceTab.path
  ) {
    notify(t("pair.noActiveFlow"), "warn");
    return;
  }
  const items = sourceController.selection();
  if (!items.length) {
    notify(t("pair.selectToSend"), "warn");
    return;
  }
  await transfer(plan.op, items.map((i) => i.path), plan.dstPath);
}

export async function createTandemFolder(path: string, onDone?: () => void) {
  try {
    await api.ensureDir(path);
    refreshAll();
    onDone?.();
    notify(t("pair.folderCreated", { name: basename(path) || path }), "success");
  } catch (e) {
    fail(e);
  }
}

export async function openTandemParent(b: number, missingPath: string, root: string) {
  const chain = ancestors(missingPath).reverse();
  for (let i = 1; i < chain.length; i++) {
    const anc = chain[i];
    const r = await api.pathResolve(anc).catch(() => null);
    if (r?.exists && r.isDir) {
      await sessionCmd({ op: "navigateSolo", b, path: anc });
      return;
    }
  }
  if (root && root !== missingPath) {
    await sessionCmd({ op: "navigateSolo", b, path: root });
  }
}

// ───────────── pair gravity (WN-181) ─────────────

let gravityTimer: number | null = null;
let gravitySeq = 0;

/** Schedules an asynchronous, debounced Pair Gravity lookup for the opposite TabBar.
 *  Zero synchronous I/O during navigation or first paint. */
export function scheduleGravityRefresh() {
  if (typeof window === "undefined") return;
  if (gravityTimer !== null) window.clearTimeout(gravityTimer);
  gravityTimer = window.setTimeout(() => {
    gravityTimer = null;
    void refreshGravityNow();
  }, 140);
}

async function refreshGravityNow() {
  const s = getState();
  const session = s.session;
  if (s.settings?.pairGravity === false || !session || session.layout === "classic" || session.browsers.length < 2 || s.pick) {
    if (s.gravityGhost[0] || s.gravityGhost[1]) setState({ gravityGhost: [null, null] });
    return;
  }
  const srcB = session.activeBrowser === 1 ? 1 : 0;
  const dstB = 1 - srcB;
  const srcTab = session.browsers[srcB]?.tabs[session.browsers[srcB].active];
  if (!srcTab || findActivePair(session) || session.browsers[dstB]?.collapsed) {
    if (s.gravityGhost[0] || s.gravityGhost[1]) setState({ gravityGhost: [null, null] });
    return;
  }
  const srcPath = srcTab.path;
  if (!srcPath || archiveOf(srcPath)) {
    if (s.gravityGhost[0] || s.gravityGhost[1]) setState({ gravityGhost: [null, null] });
    return;
  }
  const oppositePaths = session.browsers[dstB].tabs.map((tab) => tab.path);
  const seq = ++gravitySeq;
  const suggestion = await api.gravitySuggest(srcPath, oppositePaths).catch(() => null);
  if (seq !== gravitySeq) return;
  // Re-verify active state did not move while the async lookup ran.
  const latest = getState().session;
  const latestTab = latest?.browsers[srcB]?.tabs[latest.browsers[srcB].active];
  if (!latest || !latestTab || latestTab.path !== srcPath || findActivePair(latest)) {
    return;
  }
  const next: [typeof suggestion, typeof suggestion] = dstB === 0 ? [suggestion, null] : [null, suggestion];
  setState({ gravityGhost: next });
}

export async function acceptGravityGhost(targetBrowser?: number, pair = true) {
  const s = getState();
  const session = s.session;
  if (!session || session.browsers.length < 2) return;
  const dstB = targetBrowser ?? (s.gravityGhost[1] ? 1 : s.gravityGhost[0] ? 0 : 1 - session.activeBrowser);
  const ghost = s.gravityGhost[dstB];
  if (!ghost) return;
  setState({ gravityGhost: [null, null] });
  try {
    const workspaceId = s.workspaces[s.activeWorkspace]?.id;
    if (!(await sessionCmd({ op: "newTab", b: dstB, path: ghost.path, activate: true }))) return;
    const current = getState();
    if (workspaceId != null && current.workspaces[current.activeWorkspace]?.id !== workspaceId) return;
    const updated = current.session;
    if (!updated) return;
    if (pair) {
      const leftIdx = updated.browsers[0].active;
      const rightIdx = updated.browsers[1].active;
      await sessionCmd({ op: "pairTabs", leftIdx, rightIdx, mode: ghost.suggestedMode });
    }
  } catch (e) {
    fail(e);
  }
}

export async function dismissGravityGhost(targetBrowser?: number) {
  const s = getState();
  const dstB = targetBrowser ?? (s.gravityGhost[1] ? 1 : s.gravityGhost[0] ? 0 : -1);
  if (dstB < 0) return;
  const ghost = s.gravityGhost[dstB];
  if (!ghost) return;
  setState({ gravityGhost: [null, null] });
  await api.gravityDismiss(ghost.sourcePath, ghost.path).catch(() => {});
}

export function activeBrowser(): number {
  return getState().session?.activeBrowser ?? 0;
}

export function otherBrowser(b: number): number {
  return 1 - b;
}

/** The other browser when it can be the target of "… to other browser": on screen, or
 *  only collapsed (it comes back, so the result is seen). Null in Classic or a narrow
 *  window, where there is no other browser to look at. */
export function otherTarget(b: number): number | null {
  const o = 1 - b;
  const s = getState();
  if (s.shownBrowsers.includes(o)) return o;
  if (s.session?.layout !== "classic" && s.session?.browsers[o]?.collapsed && s.shownBrowsers.length === 1 && s.shownBrowsers[0] === b) {
    sessionCmd({ op: "collapse", b: o, collapsed: false });
    return o;
  }
  return null;
}

/** Whether an "… other browser" item makes sense now (see [`otherTarget`]); no side effects. */
export function hasOtherTarget(b: number): boolean {
  const o = 1 - b;
  const s = getState();
  return s.shownBrowsers.includes(o) || (s.session?.layout !== "classic" && !!s.session?.browsers[o]?.collapsed && s.shownBrowsers[0] === b);
}

/** "Open in other browser": a new tab there, or here when there is no other browser on screen. */
export function openInOther(b: number, path: string, activate = true) {
  const o = otherTarget(b);
  sessionCmd({ op: "newTab", b: o ?? b, path, activate });
}

/** Says that "… to other browser" has nothing to go to. */
export function noOtherBrowser(what: string) {
  notify(`${what} needs a second browser: switch to a two-browser layout (or widen the window)`, "warn");
}

export function activePath(b: number): string {
  const s = getState().session;
  if (!s) return "/";
  const br = s.browsers[b];
  return br.tabs[br.active].path;
}

/** Resolves special favourite URIs (trash) to real folders. */
export function resolveSpecial(path: string): string {
  if (path.startsWith("trash:")) return getState().info?.trashDir ?? path;
  return path;
}

export function navigate(b: number, path: string) {
  // The trash opens in its own window (restore, delete for good), on every system: the
  // Windows Recycle Bin is not a folder, and Linux's Trash/files has no restore.
  if (path.startsWith("trash:")) {
    openOverlay({ type: "trash" });
    return;
  }
  sessionCmd({ op: "navigate", b, path: resolveSpecial(path) });
}

/**
 * Shows a folder in Gall (Settings › Themes folder, About › settings location, …): closes
 * the overlay, opens it in browser `b` (default: the active one; a hidden browser comes
 * back) and focuses that browser.
 */
export function showFolder(path: string, b?: number) {
  const s = getState().session;
  if (!s) return;
  const t = s.layout === "classic" ? s.activeBrowser : (b ?? s.activeBrowser);
  openOverlay(null);
  if (s.browsers[t]?.collapsed) sessionCmd({ op: "collapse", b: t, collapsed: false });
  navigate(t, path);
  focusBrowser(t);
}

/** Is the preview & details pane of browser `b` shown? Its own choice, else the setting. */
export function detailsShown(s: Pick<UiState, "session" | "settings">, b: number): boolean {
  return s.session?.browsers[b]?.details ?? s.settings?.showDetailsPane ?? false;
}

/** Miller columns (true) or breadcrumbs in browser `b`: its own choice for the current
 *  layout, else the layout's default (Standard: the setting; Columns: columns; Classic:
 *  breadcrumbs). */
export function columnsShown(s: Pick<UiState, "session" | "settings">, b: number): boolean {
  const layout = s.session?.layout ?? "standard";
  const own = s.session?.browsers[b]?.columns?.[layout];
  if (own != null) return own;
  return layout === "standard" ? s.settings?.showNavColumns ?? false : layout === "columns";
}

/** Switches columns / breadcrumbs of one browser in the current layout; remembered with the session. */
export function toggleColumns(b: number, on = !columnsShown(getState(), b)) {
  sessionCmd({ op: "columns", b, on });
}

/** Toggles the preview & details pane of one browser only. */
export function toggleDetails(b: number) {
  sessionCmd({ op: "details", b, on: !detailsShown(getState(), b) });
}

function hiddenOverrides(s: Pick<UiState, "session">): HiddenOverrides {
  return [s.session?.browsers[0]?.showHidden ?? null, s.session?.browsers[1]?.showHidden ?? null];
}

function isSingleBoard(s: Pick<UiState, "session" | "shownBrowsers">): boolean {
  return s.session?.layout === "classic" || s.shownBrowsers.length < 2;
}

/** Are hidden files shown in browser `b`? Its own override, else the global setting. */
export function hiddenShown(s: Pick<UiState, "session" | "settings">, b: number): boolean {
  return effectiveHidden(s.settings?.showHidden ?? false, s.session?.browsers[b]?.showHidden);
}

/** Non-null only in dual-board view when browser `b` overrides the global hidden-files state. */
export function hiddenDiverged(s: Pick<UiState, "session" | "settings" | "shownBrowsers">, b: number): boolean | null {
  return hiddenDivergence(s.settings?.showHidden ?? false, hiddenOverrides(s), b, isSingleBoard(s));
}

function commitHiddenPlan(plan: HiddenTogglePlan) {
  // Update session overrides first when global did not change, or alongside updateSettings
  // (which clears overrides automatically whenever global showHidden changes).
  if (plan.globalChanged) {
    void updateSettings({ showHidden: plan.nextGlobal });
  } else if (plan.sessionOp) {
    void sessionCmd({ op: "hidden", ...plan.sessionOp });
  }
}

/** Toggles hidden files for browser `b` (self-healing back to global sync when both match). */
export function toggleHidden(b: number) {
  const s = getState();
  if (!s.settings) return;
  commitHiddenPlan(planToggleHiddenBoard(s.settings.showHidden, hiddenOverrides(s), b, isSingleBoard(s)));
}

/** Toggles hidden files across both browsers and re-syncs them with the global default. */
export function toggleHiddenAll() {
  const s = getState();
  if (!s.settings) return;
  commitHiddenPlan(planToggleHiddenAll(s.settings.showHidden, hiddenOverrides(s)));
}

/** Sets hidden files to `on` across both browsers and re-syncs them. */
export function applyHiddenAll(on: boolean) {
  const s = getState();
  if (!s.settings) return;
  commitHiddenPlan(planApplyHiddenAll(s.settings.showHidden, hiddenOverrides(s), on));
}

/** Swaps the folders of the two browsers; each keeps its own pane layout. */
export function swapBrowsers() {
  const s = getState().session;
  if (!s || s.layout === "classic") return;
  sessionCmd({ op: "swap" });
}

export function focusBrowser(b: number) {
  if (getState().session?.activeBrowser !== b) sessionCmd({ op: "focus", b });
}

export function setLayout(layout: Layout) {
  sessionCmd({ op: "layout", layout });
}

export function nextLayout() {
  const order: Layout[] = ["standard", "columns", "classic"];
  const cur = getState().session?.layout ?? "standard";
  setLayout(order[(order.indexOf(cur) + 1) % order.length]);
}

export async function goToTyped(b: number, input: string) {
  const r = await api.pathResolve(input, activePath(b)).catch((e) => (fail(e), null));
  if (!r) return;
  if (!r.exists) return notify(`“${r.path}” does not exist`, "warn");
  if (r.isDir) navigate(b, r.path);
  else {
    const p = parent(r.path);
    if (p) navigate(b, p);
    setTimeout(() => controller(b)?.select([r.path], r.path), 150);
  }
}

// ───────────── vim folder marks ─────────────

/** Mark key as shown to the user: ' (the previous folder) reads as '' like in Vim. */
const markName = (key: string) => (key === PREVIOUS_MARK ? "''" : key);

async function markCmd(cmd: MarkCmd): Promise<boolean> {
  try {
    setState({ marks: await api.marksCmd(cmd) });
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

/** Does the marked folder still exist? Inside an archive the archive file must. */
async function markReachable(path: string): Promise<boolean> {
  if (path.startsWith("trash:")) return true;
  const arc = archiveOf(path);
  const r = await api.pathResolve(arc ?? path).catch(() => null);
  return !!r?.exists && (arc ? true : r.isDir);
}

/** The actions.d actions for the active selection with their keys in the leader group `x` (which-key). */
export function leaderActions() {
  const b = activeBrowser();
  return keyedActionsFor(controller(b)?.selection() ?? [], activePath(b));
}

/** Leader, x, then a key: runs that actions.d action on the selection (or the folder). The selection is
 *  read now, before the list is fetched, so a selection change meanwhile cannot redirect the action. */
export async function runLeaderAction(key: string) {
  const b = activeBrowser();
  const ran = await runKeyedAction(key, controller(b)?.selection() ?? [], activePath(b), fail);
  if (!ran) notify(tt("No custom action on that key for this selection"), "info");
}

/** Runs a folder-mark key sequence (vim keys: m{x}, '{x}, `{x}, dm{x}). */
export function runMark(op: MarkOp, key: string) {
  if (op === "set") void setMark(key);
  else if (op === "delete") void deleteMark(key);
  else void jumpToMark(key, op === "jumpItem");
}

/** m{x}: remembers the active browser's folder and its focused item under `key`. */
export async function setMark(key: string) {
  const b = activeBrowser();
  const path = activePath(b);
  const item = controller(b)?.focused()?.path ?? null;
  const prev = getState().marks.marks[key];
  if (!(await markCmd({ op: "set", key, path, item }))) return;
  const home = getState().info?.home;
  if (prev && prev.path !== path) {
    notify(tt("Mark {key}: {value} (was {value2})", { key, value: tildify(path, home), value2: tildify(prev.path, home) }), "info", {
      label: "Undo", run: () => void markCmd({ op: "set", key, path: prev.path, item: prev.item }),
    });
  } else notify(tt("Mark {key}: {value}", { key, value: tildify(path, home) }));
}

/** '{x} / `{x}: opens the marked folder in the active browser; `{x} also selects the item
 *  that was focused when the mark was set. The folder left behind becomes mark '. */
export async function jumpToMark(key: string, withItem: boolean) {
  const m = getState().marks.marks[key];
  const home = getState().info?.home;
  if (!m) {
    notify(key === PREVIOUS_MARK ? tt("No earlier folder to go back to yet") : tt("Mark {key} is not set (m {key2} marks this folder)", { key, key2: key }), "warn");
    return;
  }
  const b = activeBrowser();
  const from = activePath(b);
  const fromItem = controller(b)?.focused()?.path ?? null;
  const tab = () => { const br = getState().session?.browsers[b]; return br?.tabs[br.active]?.id; };
  const startTab = tab();
  // The item is checked too: a select of a missing item would wait and fire if it came back.
  const [reachable, item] = await Promise.all([
    markReachable(m.path),
    withItem && m.item ? api.pathResolve(m.item).then((r) => r.exists, () => false) : Promise.resolve(false),
  ]);
  // A slow check (a disconnected network share): the user went on meanwhile, so do nothing.
  if (activePath(b) !== from || tab() !== startTab) return;
  if (!reachable) {
    notify(tt("Mark {value}: {value2} is not available", { value: markName(key), value2: tildify(m.path, home) }), "warn", {
      label: tt("Delete mark"), run: () => void deleteMark(key),
    });
    return;
  }
  if (from !== m.path) {
    navigate(b, m.path);
    void markCmd({ op: "set", key: PREVIOUS_MARK, path: from, item: fromItem });
  }
  if (withItem && m.item) {
    if (item) controller(b)?.select([m.item], m.item);
    else notify(tt("Mark {value}: {value2} is no longer in that folder", { value: markName(key), value2: basename(m.item) }), "warn");
  }
}

/** dm{x}: forgets a mark (with undo). */
export async function deleteMark(key: string) {
  const prev = getState().marks.marks[key];
  if (!prev) {
    notify(tt("Mark {value} is not set", { value: markName(key) }), "warn");
    return;
  }
  if (!(await markCmd({ op: "remove", key }))) return;
  notify(tt("Mark {value} deleted", { value: markName(key) }), "info", {
    label: "Undo", run: () => void markCmd({ op: "set", key, path: prev.path, item: prev.item }),
  });
}

// ───────────── favourites ─────────────

/**
 * Imports the ticked places, one new group per source. One notification offers "Undo import" for the whole batch
 * (it removes exactly the places this import added). Returns whether anything was added.
 */
export async function importFavorites(choices: ImportChoice[]): Promise<boolean> {
  try {
    const { favorites, receipt } = await api.favoritesImportApply(choices);
    setState({ favorites });
    const added = receipt.groups.reduce((count, group) => count + group.paths.length, 0);
    if (added === 0) {
      notify(tt("Nothing new to import: everything ticked is already in your favorites."));
      return false;
    }
    notify(tt("Imported favorites: {count}", { count: added }), "success", {
      label: tt("Undo import"),
      run: () => {
        api.favoritesImportUndo(receipt)
          .then((restored) => { setState({ favorites: restored }); notify(tt("Import undone")); })
          .catch(fail);
      },
    });
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

export async function favCmd(cmd: FavCmd) {
  try {
    setState({ favorites: await api.favoritesCmd(cmd) });
  } catch (e) {
    fail(e);
  }
}

// ───────────── compare ─────────────

const fileSrc = (path: string): DiffSource => ({ kind: "file", path });

export function compare(a: DiffSource, b: DiffSource, wait?: boolean) {
  // Documents (.docx / .pdf) open directly inside DiffView's DocumentCompare mode so extraction
  // can be cancelled immediately by closing the window, with one-click text or byte fallback.
  openOverlay({ type: "diff", a, b, wait });
}

/** Partner for `entry` in the other browser: its focused file, else a file with the same name. */
export async function otherPanelPartner(b: number, entry: FileEntry): Promise<string | null> {
  const o = otherBrowser(b);
  if (o === b || getState().session?.layout === "classic") return null;
  const f = controller(o)?.focused() ?? getState().focused[o];
  if (f && !f.isDir && f.path !== entry.path) return f.path;
  const same = join(activePath(o), entry.name);
  if (same === entry.path) return null;
  const r = await api.pathResolve(same).catch(() => null);
  return r?.exists && !r.isDir ? r.path : null;
}

/**
 * Opens the standalone merge of three files. `paths` are the selected files: three are base, local and remote in
 * the order they are listed, two are local and remote (no base); anything else opens the window empty.
 */
export function openStandaloneMerge(paths: string[] = []) {
  const [a, b, c] = paths;
  openOverlay(paths.length === 3 ? { type: "standaloneMerge", base: a, left: b, right: c } : paths.length === 2 ? { type: "standaloneMerge", left: a, right: b } : { type: "standaloneMerge" });
}

/** The merge of the files selected in a browser (up to three). */
export function mergeSelection(b: number) {
  const files = (controller(b)?.selection() ?? []).filter((e) => !e.isDir);
  if (files.length > 3) notify(tt("Using the first three selected files for the merge."), "warn");
  openStandaloneMerge(files.slice(0, 3).map((e) => e.path));
}

export function compareFolders(left: string, right: string, wait?: boolean) {
  const reject = (message: string) => { if (wait) throw new Error(message); return notify(message, "warn"); };
  if (left === right) return reject(tt("Both sides are the same folder"));
  // A folder and one inside it: a sync would copy the folder into itself or trash its own files.
  if (isInside(left, right) || isInside(right, left))
    return reject(`“${basename(isInside(left, right) ? left : right)}” is inside the other folder: compare two separate folders`);
  void api.gravityRecord(left, right, 1.5, false).catch(() => {});
  openOverlay({ type: "dircmp", left, right, wait });
}

/** The folders shown in both browsers (browser 1 on the left). */
export function comparePanelFolders(b: number) {
  const o = otherBrowser(b);
  if (o === b || getState().session?.layout === "classic") return notify(tt("Compare folders needs two panels"), "warn");
  compareFolders(activePath(0), activePath(1));
}

/** One folder: the folder with the same name in the other panel, else the other panel's folder. */
export async function compareFolderWithOther(b: number, dir: FileEntry) {
  const o = otherBrowser(b);
  if (o === b) return notify(tt("Compare folders needs two panels"), "warn");
  const same = join(activePath(o), dir.name);
  const r = await api.pathResolve(same).catch(() => null);
  const other = r?.exists && r.isDir && r.path !== dir.path ? r.path : activePath(o);
  b === 0 ? compareFolders(dir.path, other) : compareFolders(other, dir.path);
}

/** The "Compare" command: two selected files, or the focused file vs. the other browser. */
export async function compareSelection(b: number) {
  const c = controller(b);
  const selDirs = (c?.selection() ?? []).filter((e) => e.isDir);
  if (selDirs.length >= 2 && selDirs.length === (c?.selection() ?? []).length) return compareFolders(selDirs[0].path, selDirs[1].path);
  if (selDirs.length === 1 && (c?.selection() ?? []).length === 1) return compareFolderWithOther(b, selDirs[0]);
  const files = (c?.selection() ?? []).filter((e) => !e.isDir);
  if (files.length > 2) return openOverlay({ type: "matrix", paths: files.slice(0, 30).map((f) => f.path) });
  if (files.length === 2) return compare(fileSrc(files[0].path), fileSrc(files[1].path));
  const one = files[0] ?? (c?.focused()?.isDir === false ? c.focused() : null);
  if (!one) return notify(tt("Select two files to compare (or one, to compare with the other panel)"), "warn");
  const partner = (await otherPanelPartner(b, one)) ?? (await pickCompareFile(b, one));
  if (!partner) return;
  b === 0 ? compare(fileSrc(one.path), fileSrc(partner)) : compare(fileSrc(partner), fileSrc(one.path));
}

/** No partner in the other panel: let the user choose one, starting in the other panel's folder. */
async function pickCompareFile(b: number, one: FileEntry): Promise<string | null> {
  const o = otherBrowser(b);
  const start = o !== b && getState().session?.layout !== "classic" ? activePath(o) : parent(one.path);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const p = await open({ title: tt("Compare {name} with…", { name: one.name }), defaultPath: start ?? undefined, multiple: false, directory: false });
  return typeof p === "string" && p !== one.path ? p : null;
}

export async function compareWithClipboard(path: string) {
  const c = await api.clipInspect().catch(() => null);
  if (c?.kind === "text") return compare(fileSrc(path), { kind: "text", label: "Clipboard", text: c.text });
  if (c?.kind === "url") return compare(fileSrc(path), { kind: "text", label: "Clipboard", text: c.url });
  if (c?.kind === "paths" && c.paths.length === 1) return compare(fileSrc(path), fileSrc(c.paths[0]));
  notify(tt("The clipboard holds no text or single file to compare with"), "warn");
}

let gitFileComparisonRequest = 0;

export async function compareGitFile(path: string, revision: "head" | "lastChange" = "head") {
  const id = ++gitFileComparisonRequest;
  const overlay = getState().overlay;
  const browser = activeBrowser();
  const folder = activePath(browser);
  const current = () => id === gitFileComparisonRequest && getState().overlay === overlay && activeBrowser() === browser && activePath(browser) === folder;
  try {
    if (revision === "lastChange") {
      const { history } = await api.gitLastFileChange(path);
      if (!current()) return;
      const lastTouch = history.entries[0];
      if (!lastTouch) {
        notify(tt("No file history at this revision."), "warn");
        return;
      }
      const details = await api.gitCommitDetails(path, lastTouch.id);
      if (!current()) return;
      const file = details.files.find((file) => file.path === lastTouch.file)
        ?? details.files.find((file) => file.original === lastTouch.file);
      if (!file) {
        notify(tt("No file changes between these revisions."), "warn");
        return;
      }
      const [before, after] = commitSides(file, details.id, details.base);
      const source = (side: typeof before, absent: string): DiffSource => side.kind === "commit"
        ? { kind: "gitBlob", root: details.root, commit: side.id, file: side.file, lastTouch: side.id === lastTouch.id ? lastTouch : null }
        : { kind: "text", label: absent, text: "" };
      compare(source(before, tt("Absent before this commit")), source(after, tt("Absent after this commit")));
      return;
    }
    const { left, right, lastTouch } = await api.gitWorktreeComparison(path);
    if (!current()) return;
    compare(
      left ? { ...left, isHead: true, lastTouch } : { kind: "text", label: tt("Absent from HEAD"), text: "" },
      right ?? { kind: "text", label: tt("Absent from the worktree"), text: "" },
    );
  } catch (error) {
    if (current()) fail(error);
  }
}

/** "Compare with…" submenu for one file. `inRepo` comes from the pane (Git root of its folder). */
export function compareMenu(b: number, entry: FileEntry, inRepo: boolean): MenuItem {
  const o = otherBrowser(b);
  const focusedOther = o !== b ? controller(o)?.focused() : null;
  const other = focusedOther && !focusedOther.isDir && focusedOther.path !== entry.path ? focusedOther.name : null;
  return {
    label: tt("Compare with"),
    icon: "compare",
    submenu: [
      { label: other ? tt("Other panel: {other}", { other }) : tt("Other panel (same name)"), shortcut: "Ctrl+Shift+D", run: async () => {
          const partner = (await otherPanelPartner(b, entry)) ?? (await pickCompareFile(b, entry));
          if (!partner) return;
          b === 0 ? compare(fileSrc(entry.path), fileSrc(partner)) : compare(fileSrc(partner), fileSrc(entry.path));
        } },
      { label: "Clipboard", run: () => compareWithClipboard(entry.path) },
      { label: inRepo ? tt("Git HEAD (my changes)") : tt("Git HEAD (not in a repository)"), disabled: !inRepo, run: () => compareGitFile(entry.path) },
      { label: tt("Show the last Git change to this file"), disabled: !inRepo, run: () => compareGitFile(entry.path, "lastChange") },
      { label: tt("Open file history"), disabled: !inRepo, run: () => openOverlay({ type: "gitGraph", path: parent(entry.path) ?? entry.path, focusFile: entry.path }) },
      { label: tt("Git commit…"), disabled: !inRepo, run: () => openOverlay({ type: "gitCommits", path: entry.path }) },
      { separator: true },
      { label: tt("Choose file…"), run: async () => {
          const { open } = await import("@tauri-apps/plugin-dialog");
          const p = await open({ defaultPath: parent(entry.path) ?? undefined, multiple: false, directory: false });
          if (typeof p === "string") compare(fileSrc(entry.path), fileSrc(p));
        } },
    ],
  };
}

// ───────────── folder colours ─────────────

/** Palette for folder icons (Catppuccin Mocha accents, readable on dark and light). */
export const FOLDER_COLORS: [string, string][] = [
  ["Red", "#f38ba8"], ["Orange", "#fab387"], ["Yellow", "#f9e2af"], ["Green", "#a6e3a1"],
  ["Teal", "#94e2d5"], ["Blue", "#89b4fa"], ["Purple", "#cba6f7"], ["Pink", "#f5c2e7"], ["Grey", "#9399b2"],
];

export function setFolderColor(paths: string[], color: string | null) {
  const s = getState().settings;
  if (!s) return;
  const next = { ...s.folderColors };
  for (const p of paths) {
    if (color) next[p] = color;
    else delete next[p];
  }
  updateSettings({ folderColors: next });
}

/** "Folder colour" submenu for the given folders. */
export function folderColorMenu(paths: string[]): MenuItem {
  const cur = paths.length === 1 ? getState().settings?.folderColors[paths[0]] : undefined;
  return {
    label: tt("Folder colour"),
    icon: "folder",
    submenu: [
      { label: "Default", checked: !cur, run: () => setFolderColor(paths, null) },
      { separator: true },
      ...FOLDER_COLORS.map(([label, c]) => ({ label, swatch: c, checked: cur === c, run: () => setFolderColor(paths, c) })),
      { separator: true },
      {
        label: tt("Custom…"),
        swatch: cur && !FOLDER_COLORS.some(([, c]) => c === cur) ? cur : undefined,
        run: async () => {
          const v = await promptText(tt("Folder colour (e.g. #ff8800 or rebeccapurple; empty = default)"), cur ?? "", "Apply");
          if (v === null) return;
          const t = v.trim();
          if (!t) return setFolderColor(paths, null);
          if (!CSS.supports("color", t)) return notify(tt("Not a valid colour: {t}", { t }), "warn");
          setFolderColor(paths, t);
        },
      },
    ],
  };
}

/** Sidebar group that "pin" adds to; created at the top on first use. */
export const PIN_GROUP = "Pinned";

/** Where `path` appears in the favourites, as (group id, index, alias). */
function pinSpots(path: string) {
  return getState().favorites.groups.flatMap((g) =>
    g.items.flatMap((f, index) => (resolveSpecial(f.path) === path ? [{ id: g.id, index, alias: f.alias }] : [])),
  );
}

export function isPinned(path: string): boolean {
  return pinSpots(path).length > 0;
}

/** Hides files and folders from Quick Open, find in files and the name search via
 *  the `.gallignore` next to each (gitignore syntax; works outside git too). */
export async function hideFromSearch(paths: string[]) {
  const home = getState().info?.home;
  const added: string[] = [];
  const files = new Set<string>();
  for (const p of paths) {
    try {
      const file = await api.hideFromSearch(p);
      if (file) { added.push(p); files.add(file); }
    } catch (err) {
      fail(err);
    }
  }
  bumpSearchHidden();
  if (!added.length) {
    if (paths.length) notify(paths.length === 1 ? `“${basename(paths[0])}” is already hidden from search` : tt("Already hidden from search"), "info");
    return;
  }
  const what = added.length === 1 ? `“${basename(added[0])}”` : `${added.length} items`;
  const where = files.size === 1 ? ` (in ${tildify([...files][0], home)})` : "";
  notify(`${what} hidden from search${where}`, "success", {
    label: "Undo",
    run: () => void Promise.all(added.map((p) => api.showInSearch(p))).then(bumpSearchHidden).catch(fail),
  });
}

/** Shows an item hidden by a `.gallignore` in search again (removes its line, or adds a ! exception). */
export async function showInSearch(path: string) {
  const home = getState().info?.home;
  try {
    const out = await api.showInSearch(path);
    bumpSearchHidden();
    const name = `“${basename(path)}”`;
    switch (out.kind) {
      case "removed":
        notify(`${name} shows in search again`, "success", { label: "Undo", run: () => void hideFromSearch([path]) });
        break;
      case "excepted":
        notify(`${name} shows in search again (a pattern hides it, so ${tildify(out.file, home)} got an exception for it)`, "success", {
          label: tt("Edit list"),
          run: () => void api.openPath(out.file).catch(fail),
        });
        break;
      case "insideHidden":
        notify(`${name} is inside “${basename(out.folder)}”, which is hidden from search. Show that folder first.`, "warn", {
          label: tt("Show it"),
          run: () => void showInSearch(out.folder),
        });
        break;
      case "notHidden":
        notify(`${name} is not hidden by a .gallignore`, "info");
        break;
    }
  } catch (err) {
    fail(err);
  }
}

/** Pin a folder to the sidebar, or unpin it from every group it is in (with undo). */
export async function togglePin(path: string) {
  try {
    const spots = pinSpots(path);
    if (spots.length) {
      // Remove from the back so earlier indices stay valid.
      for (const s of [...spots].sort((a, b) => b.index - a.index)) {
        setState({ favorites: await api.favoritesCmd({ op: "removeItem", id: s.id, index: s.index }) });
      }
      notify(tt("Unpinned {value}", { value: basename(path) || path }), "info", {
        label: "Undo",
        run: async () => {
          for (const s of [...spots].sort((a, b) => a.index - b.index)) {
            await api.favoritesCmd({ op: "addItem", id: s.id, path, index: s.index });
            if (s.alias) await api.favoritesCmd({ op: "alias", id: s.id, index: s.index, alias: s.alias });
          }
          setState({ favorites: await api.favoritesGet() });
        },
      });
      return;
    }
    let g = getState().favorites.groups.find((x) => x.name === PIN_GROUP);
    if (!g) {
      let favs = await api.favoritesCmd({ op: "addGroup", name: PIN_GROUP });
      const id = favs.groups[favs.groups.length - 1].id;
      for (let i = favs.groups.length - 1; i > 0; i--) favs = await api.favoritesCmd({ op: "moveGroup", id, delta: -1 });
      g = favs.groups.find((x) => x.id === id)!;
    }
    if (g.collapsed) await api.favoritesCmd({ op: "collapse", id: g.id, collapsed: false });
    setState({ favorites: await api.favoritesCmd({ op: "addItem", id: g.id, path }) });
    notify(tt("Pinned {value} to the sidebar", { value: basename(path) || path }), "success");
  } catch (e) {
    fail(e);
  }
}

// ───────────── file operations ─────────────

export async function refreshUndo() {
  const undoLabel = await api.undoPeek().catch(() => null);
  setState((s) => ({ undoLabel, undoRev: s.undoRev + 1 }));
}

/** Undo the most recent operation in folder `dir` (the panel's undo button). */
export async function undoIn(dir: string) {
  await runUndo((permanent) => api.undoIn(dir, permanent), tt("Nothing to undo in this folder"));
}

export async function undo() {
  await runUndo((permanent) => api.undo(permanent));
}

/** Undo the specific file operation tied to a notification card. */
export async function undoRecord(id: number) {
  await runUndo((permanent) => api.undoById(id, permanent), tt("This action can no longer be undone"));
}

/** Undo one or more specific file operations (e.g. a two-sided compare save) in reverse order. */
export async function undoRecords(ids: number[]) {
  const ordered = [...ids].reverse();
  if (ordered.length === 1) return undoRecord(ordered[0]);
  let undoneCount = 0;
  let lastDescription: string | null = null;
  for (const id of ordered) {
    try {
      const description = await api.undoById(id, false);
      if (description !== null) {
        undoneCount++;
        lastDescription = description;
      }
    } catch (e) {
      if (!String(e).startsWith("no-trash: ")) {
        fail(e);
        break;
      }
      if (await confirm(tt("Undo without the trash?"),
        `${readableError(String(e))} Undo deletes them for good instead; they cannot be restored.`, tt("Delete for good"), true)) {
        try {
          const description = await api.undoById(id, true);
          if (description !== null) {
            undoneCount++;
            lastDescription = description;
          }
        } catch (err) {
          fail(err);
          break;
        }
      } else {
        break;
      }
    }
  }
  if (undoneCount === 0) notify(tt("This action can no longer be undone"));
  else {
    notify(undoneCount === 1 && lastDescription ? lastDescription : tt("Undid {count} saved files", { count: undoneCount }), "success");
    refreshAll();
  }
  refreshUndo();
  invalidateAllFolderSizes();
}

/** Builds a notification Undo action bound to a specific undo journal record ID. */
export function undoAction(undoId?: number | null): NoteAction | undefined {
  return undoId != null ? { label: "Undo", run: () => void undoRecord(undoId) } : undefined;
}

/** Runs an undo. When the trash refuses what the operation created (a copy on a network
 *  drive), asks to delete it for good instead: otherwise that undo would fail every time
 *  and block Ctrl+Z for everything older. */
async function runUndo(call: (permanent: boolean) => Promise<string | null>, empty = tt("Nothing to undo")) {
  const apply = async (permanent: boolean) => {
    const description = await call(permanent);
    if (description === null) notify(empty);
    else {
      notify(description, "success");
      refreshAll();
    }
  };
  try {
    await apply(false);
  } catch (e) {
    if (!String(e).startsWith("no-trash: ")) fail(e);
    else if (await confirm(tt("Undo without the trash?"),
      `${readableError(String(e))} Undo deletes them for good instead; they cannot be restored.`, tt("Delete for good"), true)) {
      await apply(true).catch(fail);
    }
  }
  refreshUndo();
  // Also after a failure: part of the operation may be undone.
  invalidateAllFolderSizes();
}

type ConflictChoice = { policy: ConflictPolicy; merge: boolean };

/** Asks what to do with items that already exist in the destination. Folders that exist
 *  on both sides can be merged; then a second question decides for the files that exist
 *  in both. Null when cancelled. */
async function askConflict(info: ConflictInfo): Promise<ConflictChoice | null> {
  const conflicts = info.paths;
  if (!conflicts.length) return { policy: "keepBoth", merge: false };
  const home = getState().info?.home ?? "";
  // One item: its name in the title (like the copy/move/delete messages) and the folder below.
  const one = conflicts.length === 1;
  const list = (ps: string[], more: number) => ps.map(basename).join(", ") + (more > 0 ? `, … (+${more})` : "");
  const allFolders = info.folders === conflicts.length;
  const r = await dialog({
    title: `${named(conflicts)} already exist${one ? "s" : ""}`,
    message: (one ? `in ${tildify(parent(conflicts[0]) ?? conflicts[0], home)}` : list(conflicts.slice(0, 5), conflicts.length - 5))
      + (info.folders ? `\n${allFolders ? (one ? tt("It is a folder on both sides") : tt("They are folders on both sides")) : `${items(info.folders)} are folders on both sides`}: Merge puts the contents together.` : ""),
    buttons: [
      { id: "cancel", label: "Cancel", key: "c" },
      { id: "skip", label: "Skip", key: "s" },
      { id: "overwrite", label: "Replace", danger: true, key: "r", hint: "old one to the trash" },
      ...(info.folders ? [{ id: "merge", label: "Merge", key: "m", hint: "folders", primary: true }] : []),
      { id: "keepBoth", label: tt("Keep both"), primary: !info.folders, key: "k" },
    ],
  });
  if (!r || r.button === "cancel") return null;
  if (r.button !== "merge") return { policy: r.button as ConflictPolicy, merge: false };
  if (!info.filesWhenMerged) return { policy: "keepBoth", merge: true };
  const n = info.filesWhenMerged;
  const f = await dialog({
    title: `${n === 1 ? "1 file exists" : `${n} files exist`} in both`,
    message: list(info.sample, n - info.sample.length),
    buttons: [
      { id: "cancel", label: "Cancel", key: "c" },
      { id: "skip", label: "Skip", key: "s", hint: "keep existing" },
      { id: "overwriteOlder", label: tt("Replace older"), key: "o", hint: "if newer" },
      { id: "overwrite", label: "Replace", danger: true, key: "r", hint: "old to the trash" },
      { id: "keepBoth", label: tt("Keep both"), primary: true, key: "k" },
    ],
  });
  if (!f || f.button === "cancel") return null;
  return { policy: f.button as ConflictPolicy, merge: true };
}

/** Copy or move with conflict resolution (O-03, O-11). */
/** Packs `paths` into a new archive `name` in `dest` (background task). */
export async function compress(paths: string[], dest: string, name: string, format: ArchiveFormat) {
  if (!paths.length) return;
  const outer = archiveOf(dest);
  if (outer) dest = parent(outer) ?? dest; // from inside an archive: next to it
  const sources = await realPaths(paths).catch((e) => (fail(e), null));
  if (!sources) return;
  await api.startTask({ kind: "compress", sources, dest, archiveName: name, format }).catch(fail);
}

/** Asks for the archive name; the extension picks the format (.zip when it names none Gall writes). */
export async function compressAs(paths: string[], dest: string, base: string) {
  const v = await promptText(tt("Compress to"), `${base}.zip`, "Compress", [0, base.length]);
  if (!v?.trim()) return;
  let name = v.trim();
  let format = createFormatFor(name);
  if (!format) { name += ".zip"; format = "zip"; }
  await compress(paths, dest, name, format);
}

/** Unpacks each archive into `dest` (background task); `trashArchive` then moves it to the trash. */
export async function extract(paths: string[], dest: string, mode: "smart" | "folder" = "smart", trashArchive = false) {
  if (!paths.length) return;
  // An archive inside an archive: extract next to the outer one, never "into" it.
  const outer = archiveOf(dest);
  if (outer) {
    dest = parent(outer) ?? dest;
    trashArchive = false;
  }
  const sources = await realPaths(paths).catch((e) => (fail(e), null));
  if (!sources) return;
  await api.startTask({ kind: "extract", sources, dest, extractMode: mode, trashArchive }).catch(fail);
}

/** Archives are browsed read-only: says so (once per attempt) and returns true for a
 *  destination folder or items inside one. */
export function refuseInArchive(dirOrItems: string | string[], what = "change"): boolean {
  const inside = typeof dirOrItems === "string" ? archiveOf(dirOrItems) : dirOrItems.map((p) => archiveOf(parent(p) ?? "")).find(Boolean);
  if (!inside) return false;
  notify(platformLabel(tt("Can't {what} inside “{value}”: archives open read-only. Copy items out (y y / Ctrl+C) or extract it.", { what, value: basename(inside) })), "warn");
  return true;
}

const PASSWORD_ERR = "archive-password:";

/** If `err` says an archive needs its password (or got a wrong one): asks for it and
 *  hands it to the backend, which keeps it in memory until Gall exits. True = retry. */
export async function askArchivePassword(err: unknown): Promise<boolean> {
  const s = String(err);
  const at = s.indexOf(PASSWORD_ERR);
  if (at < 0) return false;
  const rest = s.slice(at + PASSWORD_ERR.length);
  const wrong = rest.startsWith("wrong:");
  const archive = rest.slice(rest.indexOf(":") + 1).trim();
  const r = await dialog({
    title: wrong ? tt("Wrong password") : tt("Password required"),
    message: wrong ? tt("That password does not open “{value}”. Try again.", { value: basename(archive) }) : `“${basename(archive)}” is encrypted.`,
    input: { value: "", password: true, placeholder: "Password" },
    buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Unlock", primary: true }],
  });
  if (!r || r.button === "cancel" || !r.value) return false;
  await api.archivePassword(archive, r.value);
  return true;
}

/** Runs `fn`, asking for an archive password and retrying while that is what it needs. */
export async function withArchivePassword<T>(fn: () => Promise<T>): Promise<T> {
  for (;;) {
    try {
      return await fn();
    } catch (e) {
      if (!(await askArchivePassword(e))) throw e;
    }
  }
}

/** Items inside archives are extracted to a cache first (only those), so copying,
 *  the clipboard and dragging work with real files. Other paths are unchanged. */
export async function realPaths(paths: string[]): Promise<string[]> {
  if (!paths.some((p) => archiveOf(parent(p) ?? ""))) return paths;
  const n = setTimeout(() => notify(tt("Extracting {value} from the archive…", { value: named(paths) })), 400);
  try {
    return await withArchivePassword(() => api.archiveMaterialize(paths));
  } finally {
    clearTimeout(n);
  }
}

/** Pastes of cut items, by task id: the clipboard is emptied at once (a second paste must
 *  not move them again); when the move fails or is cancelled, what is still at its old
 *  place goes back on the clipboard. */
const cutPastes = new Map<number, string[]>();

/** Did the system clipboard take the files of our last copy or cut? Where it did not
 *  (no clipboard, or a write that failed), it still shows something older, which must
 *  not be read as "copied later in another program". */
let clipShared = false;
async function shareClip(paths: string[], cut: boolean) {
  await api.clipSetFiles(paths, cut);
  clipShared = ownsClipboard(paths, await api.clipInspect().catch(() => ({ kind: "empty" as const })));
}

async function restoreCut(paths: string[]) {
  if (getState().clipboard.paths.length) return; // something else was cut or copied since
  const left = await api.findExisting(paths).catch(() => []);
  if (!left.length || getState().clipboard.paths.length) return;
  await shareClip(left, true).catch(() => {});
  setState({ clipboard: { paths: left, cut: true } });
  notify(`${named(left)} not moved: still on the clipboard`, "info");
}

type TransferRequest = { kind: "copy" | "move"; sources: string[]; dest: string; merge: boolean };
/** Copies and moves started with "Replace", by task id: offered again as a permanent
 *  replace when the trash refuses the replaced items (network shares, read-only trash). */
const replaceRequests = new Map<number, TransferRequest>();
// "Replace older" is not offered as a permanent replace: that would ignore the dates.
const replacing = (p: ConflictPolicy) => p === "overwrite";

async function offerPermanentReplace(req: TransferRequest, error: string) {
  const ok = await confirm(
    tt("Replace permanently?"),
    `${readableError(error)} Replace without the trash? The replaced items will be deleted and cannot be restored.`,
    tt("Replace permanently"), true);
  if (!ok) return notify(tt(req.kind === "move" ? "Move stopped; nothing more was replaced" : "Copy stopped; nothing more was replaced"), "warn");
  await api.startTask({ ...req, conflict: "overwritePermanently" }).catch(fail);
}

export async function transfer(kind: "copy" | "move", sources: string[], dest: string) {
  if (!sources.length) return;
  if (refuseInArchive(dest, kind === "move" ? "move items" : "paste")) return;
  // Out of an archive nothing can be removed from it: a move (default drag) copies.
  if (kind === "move" && sources.some((s) => archiveOf(parent(s) ?? ""))) kind = "copy";
  try {
    sources = await realPaths(sources);
  } catch (e) {
    return fail(e);
  }
  if (sources.some((s) => isInside(dest, s) && s !== dest)) return notify(tt(kind === "copy" ? "Cannot copy a folder into itself" : "Cannot move a folder into itself"), "warn");
  if (kind === "move" && sources.every((s) => parent(s) === dest)) return;
  try {
    const choice = await askConflict(await api.conflictInfo(sources, dest));
    if (!choice) return;
    const id = await api.startTask({ kind, sources, dest, conflict: choice.policy, merge: choice.merge });
    if (replacing(choice.policy)) replaceRequests.set(id, { kind, sources, dest, merge: choice.merge });
    const srcFolder = parent(sources[0]);
    if (srcFolder && srcFolder !== dest) {
      void api.gravityRecord(srcFolder, dest, 1.0, false).catch(() => {});
    }
  } catch (e) {
    fail(e);
  }
}

export async function trashPaths(paths: string[]) {
  if (!paths.length) return;
  if (refuseInArchive(paths, "delete")) return;
  try {
    const undoId = await api.trash(paths);
    refreshAll();
    notify(tt("Moved {value} to trash", { value: named(paths) }), "info", undoAction(undoId));
    refreshUndo();
    invalidateAllFolderSizes();
  } catch (e) {
    refreshAll();
    refreshUndo(); // part of the items may be in the trash
    invalidateAllFolderSizes();
    const msg = String(e);
    if (!msg.startsWith("trash:")) return fail(e);
    // No trash here (network share, read-only trash, …): offer to delete for good.
    const left = await api.findExisting(paths).catch(() => paths);
    if (!left.length) return fail(e);
    const ok = await confirm(tt("Can't move to the trash"),
      `${msg.replace(/^trash:\s*/, "")}. Delete ${named(left)} permanently instead? This cannot be undone.`,
      tt("Delete permanently"), true);
    if (ok) await api.startTask({ kind: "delete", sources: left }).catch(fail);
  }
}

export async function deletePermanently(paths: string[]) {
  if (!paths.length) return;
  if (refuseInArchive(paths, "delete")) return;
  if (!(await confirm(tt("Delete permanently?"), `${named(paths)} will be deleted and cannot be restored.`, "Delete", true))) return;
  try {
    await api.startTask({ kind: "delete", sources: paths });
  } catch (e) {
    fail(e);
  }
}

/** F2 with several items selected: the batch rename window. */
export function batchRename(b: number, entries: FileEntry[]) {
  if (!entries.length || refuseInArchive(entries.map((e) => e.path), "rename")) return;
  openOverlay({ type: "batchRename", b, entries });
}

/** F2 rename popup; F2 inside cycles stem → extension → all (O-04). */
export async function renameEntry(b: number, entry: FileEntry) {
  if (refuseInArchive([entry.path], "rename")) return;
  const [stem, ext] = splitExt(entry.name);
  const select: [number, number] = entry.isDir || !ext ? [0, entry.name.length] : [0, stem.length];
  const r = await dialog({
    title: "Rename",
    input: {
      value: entry.name,
      select,
      toggleSelections: entry.isDir || !ext ? [] : [[0, stem.length], [stem.length + 1, entry.name.length], [0, entry.name.length]],
    },
    buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Rename", primary: true }],
  });
  if (!r || r.button === "cancel" || r.value === entry.name || !r.value.trim()) return;
  const newName = r.value.trim();
  if (!entry.isDir && ext && !newName.includes(".")) {
    if (!(await confirm(tt("Remove extension?"), `"${newName}" has no extension. The file may no longer open with the right program.`, "Rename"))) return;
  }
  try {
    const p = await api.rename(entry.path, newName);
    // The old name leaves the history (the backend drops gone folders).
    if (entry.isDir) api.historyGet().then((history) => setState({ history })).catch(() => {});
    // Folder colours are keyed by path: carry the folder's (and its subfolders') colours over.
    const colors = getState().settings?.folderColors ?? {};
    if (entry.isDir && Object.keys(colors).some((k) => isInside(k, entry.path))) {
      const next: Record<string, string> = {};
      for (const [k, v] of Object.entries(colors)) {
        next[isInside(k, entry.path) ? p + k.slice(entry.path.length) : k] = v;
      }
      updateSettings({ folderColors: next });
    }
    controller(b)?.select([p], p);
    refreshUndo();
  } catch (e) {
    fail(e);
  }
}

/** `base`, or "base (2)", "base (3)" … : the first name not taken in `dir` (ignoring
 *  case on Windows and macOS, whose file systems do; Linux has "new folder" and
 *  "New folder" side by side). */
export async function freeName(dir: string, base: string): Promise<string> {
  const fold = getState().info?.platform === "linux" ? (n: string) => n : (n: string) => n.toLowerCase();
  const taken = new Set((await api.listDir(dir, { showHidden: true, hideTemp: false }).catch(() => [])).map((e) => fold(e.name)));
  if (!taken.has(fold(base))) return base;
  for (let n = 2; ; n++) if (!taken.has(fold(`${base} (${n})`))) return `${base} (${n})`;
}

export async function newFolder(b: number) {
  const dir = activePath(b);
  if (refuseInArchive(dir, "create folders")) return;
  const sel = controller(b)?.selection() ?? [];
  // A free name to start with: "New folder (2)" when "New folder" is taken.
  const name = await freeName(dir, tt("New folder"));
  const r = await dialog({
    title: tt("New folder"),
    input: { value: name, select: [0, name.length] },
    buttons: [
      { id: "cancel", label: "Cancel" },
      ...(sel.length ? [{ id: "moveIn", label: tt("Create & move selection inside"), hint: "Shift+Enter" }] : []),
      { id: "open", label: tt("Create & open"), hint: "Ctrl+Enter" },
      { id: "ok", label: "Create", primary: true },
    ],
  });
  if (!r || r.button === "cancel" || !r.value.trim()) return;
  try {
    const { path: p, undoId } = await api.createDirWithUndo(dir, r.value.trim());
    if (r.button === "open") navigate(b, p);
    else if (r.button === "moveIn") await api.startTask({ kind: "move", sources: sel.map((e) => e.path), dest: p, conflict: "keepBoth" });
    else controller(b)?.select([p], p);
    if (r.button !== "moveIn") notify(tt("Created folder “{value}”", { value: basename(p) }), "success", undoAction(undoId));
    refreshUndo();
  } catch (e) {
    fail(e);
  }
}

export async function createFromTemplate(b: number, dir: string, template: string | undefined, name: string, open: boolean) {
  try {
    const { path: p, undoId } = await api.createFileWithUndo(dir, name, template);
    if (open) await api.openPath(p);
    controller(b)?.select([p], p);
    notify(tt("Created “{value}”", { value: basename(p) }), "success", undoAction(undoId));
    refreshUndo();
  } catch (e) {
    fail(e);
  }
}

function isNoDefaultAppError(err: unknown): boolean {
  return /no default application/i.test(String(err));
}

export async function openEntries(b: number, entries: FileEntry[]) {
  if (!entries.length) return;
  if (pickOpenEntries(b, entries)) return;
  const resolved: FileEntry[] = [];
  for (const entry of entries) {
    if (entry.ext !== "lnk") {
      resolved.push(entry);
      continue;
    }
    try {
      resolved.push(await withArchivePassword(() => api.shortcutResolve(entry.path)) ?? entry);
    } catch (e) {
      fail(e);
    }
  }
  entries = resolved;
  if (!entries.length) return;
  const platform = getState().info?.platform;
  const dirs = entries.filter((e) => e.isDir);
  const files = entries.filter((e) => !e.isDir);
  if (files.length >= 10 && !(await confirm(tt("Open many files?"), tt("Open {length} files at once?", { length: files.length }), "Open"))) return;
  // macOS: `.app` bundles are directories on disk, but Finder launches them on Open
  // (Show package contents navigates inside).
  if (platform === "macos" && dirs.length === 1 && !files.length && dirs[0].name.toLowerCase().endsWith(".app")) {
    api.openPath(dirs[0].path).catch(fail);
    return;
  }
  if (dirs.length === 1 && !files.length) return navigate(b, dirs[0].path);
  // An archive opens like a folder (Open with… still hands it to another program).
  if (entries.length === 1 && isBrowsableArchive(files[0].name)) return navigate(b, files[0].path);
  for (const d of dirs) sessionCmd({ op: "newTab", b, path: d.path, activate: false });
  const toOpen: FileEntry[] = [];
  for (const f of files) {
    // A link to nothing: the system opener would fail out of sight.
    if (f.isSymlink && !archiveOf(parent(f.path) ?? "")) {
      const r = await api.pathResolve(f.path).catch(() => null);
      if (!r?.exists) {
        notify(`“${f.name}” is a link to something that does not exist (or to itself)`, "warn");
        continue;
      }
    }
    if (f.ext === "2do" || f.ext === "2dx") {
      const info = await api.previewInfo(f.path).catch(() => null);
      openOverlay({ type: "note", dir: activePath(b), path: f.path, text: info?.text ?? "" });
      continue;
    }
    toOpen.push(f);
  }
  if (!toOpen.length) return;
  const handleOpenError = (e: unknown) => {
    if (isNoDefaultAppError(e)) {
      notify(tt("No default application for {item} — choose one", { item: named(toOpen.map((f) => f.path)) }), "info");
      openOverlay({ type: "openWith", paths: toOpen.map((f) => f.path), mode: "file" });
      return;
    }
    fail(e);
  };
  if (toOpen.length === 1) {
    withArchivePassword(() => api.openPath(toOpen[0].path)).catch(handleOpenError);
  } else {
    withArchivePassword(() => api.openPaths(toOpen.map((f) => f.path))).catch(handleOpenError);
  }
}

/** Navigates to a symlink or Windows shortcut target's folder and selects the target (Explorer "Open file location" / Finder "Show Original"). */
export async function openTargetLocation(b: number, entry: FileEntry) {
  try {
    let targetPath: string | null = null;
    if (entry.ext === "lnk") {
      const r = await withArchivePassword(() => api.shortcutResolve(entry.path));
      targetPath = r?.path ?? null;
    } else {
      const r = await api.pathResolve(entry.path);
      targetPath = r.exists ? r.path : null;
    }
    if (!targetPath) {
      notify(tt("Target of “{name}” could not be found", { name: entry.name }), "warn");
      return;
    }
    const dir = parent(targetPath);
    if (!dir) return;
    await navigate(b, dir);
    controller(b)?.select([targetPath], targetPath);
  } catch (e) {
    fail(e);
  }
}

/** "Open with…" palette for files (folders: choose a terminal). Ctrl+Enter there remembers the choice. */
export function openWith(entries: FileEntry[]) {
  if (!entries.length) return;
  const files = entries.filter((e) => !e.isDir);
  if (files.length) openOverlay({ type: "openWith", paths: files.map((f) => f.path), mode: "file" });
  else openOverlay({ type: "openWith", paths: [entries[0].path], mode: "terminal" });
}

/** What Edit opens: the files, else the folders (an editor such as VS Code opens a
 *  folder as a project). */
function editTargets(entries: FileEntry[]): string[] {
  const files = entries.filter((e) => !e.isDir).map((e) => e.path);
  return files.length ? files : entries.filter((e) => e.isDir).map((e) => e.path);
}

/** Edit (F4 / e): the files (or folders) in the chosen editor; the first time, ask (Ctrl+Enter remembers). */
export function editFiles(entries: FileEntry[]) {
  editPaths(editTargets(entries));
}

/** Edit with… (Shift+F4): choose the editor; Ctrl+Enter makes it the one Edit uses. */
export function editWith(entries: FileEntry[]) {
  const paths = editTargets(entries);
  if (paths.length) openOverlay({ type: "openWith", paths, mode: "editor" });
}

/** Opens `paths` (files or a folder) in the editor of the settings, else asks for one. */
export function editPaths(paths: string[]) {
  if (!paths.length) return;
  const editor = getState().settings?.editor.trim();
  if (editor) api.openWithRun(paths, editor).catch(fail);
  else openOverlay({ type: "openWith", paths, mode: "editor" });
}

/** "Terminal here with…": choose the terminal for `dir` (Ctrl+Enter makes it the default). */
export function terminalWith(dir: string) {
  openOverlay({ type: "openWith", paths: [dir], mode: "terminal" });
}

export async function copyToClipboard(entries: FileEntry[], cut: boolean) {
  if (!entries.length) return;
  let paths = entries.map((e) => e.path);
  if (cut && refuseInArchive(paths, "cut")) return;
  try {
    // From an archive: the clipboard gets extracted copies, so any program can paste them.
    paths = await realPaths(paths);
    await shareClip(paths, cut);
  } catch (e) {
    return fail(e);
  }
  setState({ clipboard: { paths, cut } });
  movedClip = null;
  // Nothing is copied yet: say what is on the clipboard and what pasting will do,
  // so this does not read like the "Copied …" that a finished paste shows.
  notify(`${named(paths)} on the clipboard; paste to ${cut ? "move" : "copy"}`, "info");
}

/** "1 item", "3 items". */
export function items(n: number): string {
  return t("count.items", { count: n });
}

/** What a message is about: the quoted name of a single item (“a.txt”), else the count (3 items). */
export function named(paths: string[]): string {
  return paths.length === 1 ? `“${basename(paths[0])}”` : items(paths.length);
}

/** The items of the last paste of cut ones (one path per line), while the system
 *  clipboard may still list them. */
let movedClip: string | null = null;
const lines = (t: string) => t.split(/\r?\n/).map((l) => l.trim()).filter(Boolean).join("\n");

/** Ctrl+V: files from our clipboard, external paths, or text/image/URL → new file (O-10). */
export async function paste(b: number) {
  const dir = activePath(b);
  if (refuseInArchive(dir, "paste")) return;
  const clip = getState().clipboard;
  try {
    const ext = await api.clipInspect().catch(() => ({ kind: "empty" as const }));
    // Our own file clipboard wins while the system clipboard still holds its paths.
    const ours = clipShared ? ownsClipboard(clip.paths, ext) : clip.paths.length > 0 && (ext.kind !== "paths" || ownsClipboard(clip.paths, ext));
    // Something copied later in another program replaced them: forget ours, so the cut
    // marks go and a later paste cannot bring the old files back.
    if (!ours && clip.paths.length) {
      setState({ clipboard: { paths: [], cut: false } });
      api.clipClearFiles().catch(() => {});
    }
    if (ours) {
      const sameFolder = clip.paths.every((p) => parent(p) === dir);
      const choice = sameFolder && !clip.cut ? { policy: "keepBoth" as const, merge: false } : await askConflict(await api.conflictInfo(clip.paths, dir));
      if (!choice) return;
      const id = await api.clipPasteFiles(dir, choice.policy, undefined, choice.merge);
      if (clip.cut) cutPastes.set(id, clip.paths);
      if (replacing(choice.policy)) replaceRequests.set(id, { kind: clip.cut ? "move" : "copy", sources: clip.paths, dest: dir, merge: choice.merge });
      if (clip.cut) {
        setState({ clipboard: { paths: [], cut: false } });
        movedClip = lines(clip.paths.join("\n"));
      }
      return;
    }
    // The system clipboard still lists items moved by a paste of cut ones (as paths while
    // the move runs, as plain text once they are gone): pasting them again copied them a
    // second time, or made a text file of their old paths.
    const extText = ext.kind === "paths" ? ext.paths.join("\n") : ext.kind === "text" ? ext.text : null;
    if (movedClip && extText !== null && lines(extText) === movedClip) {
      notify(`${named(movedClip.split("\n"))} already moved; cut or copy again to paste`, "info");
      return;
    }
    switch (ext.kind) {
      case "paths":
        // A cut made in another program (x-special/gnome-copied-files "cut") moves.
        return transfer(ext.cut ? "move" : "copy", ext.paths, dir);
      case "image": {
        const p = await api.clipPasteAsFile(dir, "image");
        controller(b)?.select([p], p);
        return;
      }
      case "url": {
        const r = await dialog({
          title: tt("Paste link"),
          message: ext.url,
          buttons: [{ id: "cancel", label: "Cancel" }, { id: "text", label: tt("As text file"), key: "t" }, { id: "url", label: tt("As link shortcut"), primary: true, key: "l" }],
        });
        if (!r || r.button === "cancel") return;
        const p = await api.clipPasteAsFile(dir, r.button as "text" | "url");
        controller(b)?.select([p], p);
        return;
      }
      case "text": {
        const p = await api.clipPasteAsFile(dir, "text");
        controller(b)?.select([p], p);
        notify(tt("Created {value} from clipboard text", { value: basename(p) }), "success");
        return;
      }
      default:
        notify(tt("Clipboard is empty"), "info");
    }
  } catch (e) {
    fail(e);
  } finally {
    refreshUndo();
  }
}

export async function pasteAsLinks(b: number) {
  const clip = getState().clipboard;
  const dir = activePath(b);
  for (const p of clip.paths) await api.symlink(p, dir).catch(fail);
  refreshUndo();
}

export async function copyPaths(entries: FileEntry[], fallbackFolder: string) {
  const paths = entries.length ? entries.map((e) => e.path) : [fallbackFolder];
  try {
    const formatted = await Promise.all(paths.map((p) => api.pathFormat(p)));
    await api.clipSetText(formatted.join("\n"));
    notify(paths.length === 1 ? tt("Copied path: {value}", { value: formatted[0] }) : tt("Copied {length} paths", { length: paths.length }), "info");
  } catch (e) {
    fail(e);
  }
}

export async function tabFromClipboard(b: number) {
  const c = await api.clipInspect().catch(() => null);
  if (c?.kind !== "paths") return notify(tt("Clipboard does not contain a path"), "warn");
  try {
    const r = await api.pathResolve(c.paths[0]);
    if (!r.exists) return notify(`“${r.path}” does not exist`, "warn");
    sessionCmd({ op: "newTab", b, path: r.isDir ? r.path : parent(r.path) });
  } catch (e) {
    fail(e);
  }
}

export async function addTemplateFrom(entry: FileEntry) {
  try {
    await api.templateAdd(entry.path);
    notify(`"${entry.name}" added to templates`, "success");
  } catch (e) {
    fail(e);
  }
}

export function joinPath(dir: string, name: string) {
  return join(dir, name);
}

// ───────────── folder sizes (F-14) ─────────────

/** Accept progress only from the calculation that currently owns the row. */
export function applyFolderSizeProgress({ path, size, ticket }: { path: string; size: import("../api/types").DirSize; ticket: number }) {
  const cur = getState().folderSizes[path];
  if (cur?.done === false && cur.job === ticket) putFolderSize(path, { ...size, done: false, at: cur.at, job: ticket });
}

function putFolderSize(path: string, v: FolderSize | null) {
  setState((s) => {
    const folderSizes = { ...s.folderSizes };
    if (v) folderSizes[path] = v;
    else delete folderSizes[path];
    return { folderSizes };
  });
}

let sizeTicket = 0;
/** Running calculation of each browser (or other owner, like the Properties window). */
const sizeOwners = new Map<number | string, number>();
const sizePending = new Map<number | string, string[]>();
const sizePrevious = new Map<number, Map<string, FolderSize | undefined>>();
// Interested owners await each row independently, then take over if its owner cancels.
const sizeRows = new Map<string, { ticket: number; promise: Promise<boolean>; finish(success: boolean): void }>();

const sizeRevisions = new Map<string, number>();
let sizeRefreshTimer: ReturnType<typeof setTimeout> | undefined;

/** Watched changes invalidate cached totals for the changed folder and its ancestors. */
export function invalidateFolderSizes(path: string) {
  staleFolderSizes((p) => isInside(path, p));
}

/** A finished file task, undo, trash or sync changed folders the watcher does not see (only
 *  the visible folders are watched, not the subfolders listed in them), so every cached
 *  total goes stale. */
export function invalidateAllFolderSizes() {
  staleFolderSizes(() => true);
}

function staleFolderSizes(affects: (path: string) => boolean) {
  // One new map and one update for all of them: an update per folder copied the whole
  // map each time, which is felt after every file task once thousands are cached.
  let folderSizes: Record<string, FolderSize> | undefined;
  for (const [p, f] of Object.entries(getState().folderSizes)) {
    if (!affects(p)) continue;
    sizeRevisions.set(p, (sizeRevisions.get(p) ?? 0) + 1);
    folderSizes ??= { ...getState().folderSizes };
    folderSizes[p] = { ...f, at: 0 };
  }
  if (folderSizes) setState({ folderSizes });
  scheduleFolderSizeRefresh();
}

function scheduleFolderSizeRefresh() {
  if (sizeRefreshTimer !== undefined) return;
  sizeRefreshTimer = setTimeout(() => {
    sizeRefreshTimer = undefined;
    if (!getState().settings?.folderSizesAuto) return;
    for (const b of [0, 1]) {
      const c = controller(b);
      if (c && !archiveOf(c.path())) void calcFolderSizes(b, c.entries().filter((e) => e.isDir && !e.isSymlink).map((e) => e.path), 120_000);
    }
  }, 300);
}

/** Calculates the total size of the folders `paths` for `owner` (a browser index or a
 *  window), two at a time; a new call replaces the owner's running one. Sizes stream
 *  into `folderSizes` as they are counted. `skipFresh`: keep sizes younger than that (ms). */
export async function calcFolderSizes(owner: number | string, paths: string[], skipFresh = 0) {
  // Listing refreshes must not interrupt a manual or automatic count already in flight.
  if (skipFresh && sizeOwners.has(owner)) {
    sizePending.set(owner, paths);
    return;
  }
  cancelFolderSizes(owner);
  const now = Date.now();
  const queue = paths.filter((p) => {
    const f = getState().folderSizes[p];
    return !(skipFresh && f?.done && now - f.at < skipFresh);
  });
  if (!queue.length) return;
  const ticket = ++sizeTicket;
  sizeOwners.set(owner, ticket);
  const previous = new Map<string, FolderSize | undefined>();
  sizePrevious.set(ticket, previous);
  let changedWhileCounting = false;
  const worker = async () => {
    while (queue.length && sizeOwners.get(owner) === ticket) {
      const p = queue.shift()!;
      let sharedDone = false;
      while (sizeRows.has(p) && sizeOwners.get(owner) === ticket) {
        if (await sizeRows.get(p)!.promise) {
          sharedDone = true;
          break;
        }
      }
      if (sizeOwners.get(owner) !== ticket) break;
      if (sharedDone) continue;
      let finishRow!: (success: boolean) => void;
      const promise = new Promise<boolean>((resolve) => { finishRow = resolve; });
      const row = { ticket, promise, finish: finishRow };
      sizeRows.set(p, row);
      const prev = getState().folderSizes[p];
      const known = prev?.done ? prev : undefined;
      previous.set(p, known);
      putFolderSize(p, { bytes: 0, files: 0, dirs: 0, unreadable: 0, ...known, done: false, at: Date.now(), job: ticket });
      const revision = sizeRevisions.get(p);
      const r = await api.folderSize(p, ticket).catch(() => null);
      const ownsRow = getState().folderSizes[p]?.job === ticket && sizeOwners.get(owner) === ticket;
      if (sizeRows.get(p) === row) sizeRows.delete(p);
      if (r && ownsRow) {
        const changed = sizeRevisions.get(p) !== revision;
        changedWhileCounting ||= changed;
        putFolderSize(p, { ...r, done: true, at: changed ? 0 : Date.now() });
      }
      // Cancelled or failed: back to what was known, unless a newer calculation took over.
      else if (getState().folderSizes[p]?.job === ticket) putFolderSize(p, known ? { ...known, at: sizeRevisions.get(p) === revision ? known.at : 0 } : null);
      finishRow(!!r && ownsRow);
    }
  };
  await Promise.all([worker(), worker()]);
  sizePrevious.delete(ticket);
  if (sizeOwners.get(owner) === ticket) {
    sizeOwners.delete(owner);
    const pending = sizePending.get(owner);
    sizePending.delete(owner);
    if (pending && getState().settings?.folderSizesAuto) void calcFolderSizes(owner, pending, 120_000);
    if (changedWhileCounting) scheduleFolderSizeRefresh();
  }
}

/** Stops the folder size calculation of `owner`. */
export function cancelFolderSizes(owner: number | string) {
  sizePending.delete(owner);
  const t = sizeOwners.get(owner);
  if (t == null) return;
  sizeOwners.delete(owner);
  for (const [path, row] of sizeRows) {
    if (row.ticket === t) {
      sizeRows.delete(path);
      row.finish(false);
    }
  }
  // Retire the UI job immediately; late results and progress no longer own the row.
  for (const [path, f] of Object.entries(getState().folderSizes)) {
    if (f.job === t) {
      const known = sizePrevious.get(t)?.get(path);
      putFolderSize(path, known ? { ...known, at: f.at === 0 ? 0 : known.at } : null);
    }
  }
  api.folderSizesCancel(t).catch(() => {});
}

/** "1.2 GB" for a calculated folder size ("1.2 GB…" while counting), else "". */
export function folderSizeLabel(f: FolderSize | undefined): string {
  if (!f) return "";
  if (f.virtualFs) return "—";
  return formatSize(f.bytes) + (f.done ? "" : "…");
}

export function folderSizeTitle(f: FolderSize | undefined): string {
  if (!f) return "";
  if (f.virtualFs) return tt("Virtual filesystem; folder size is not applicable");
  const what = `${f.files.toLocaleString()} files, ${f.dirs.toLocaleString()} folders`;
  return `${f.bytes.toLocaleString()} bytes in ${what}${f.unreadable ? `; ${items(f.unreadable)} could not be read (the total is at least this)` : ""}${f.done ? "" : " (counting…)"}`;
}

/** Only completed, applicable totals participate in folder-size sorting. */
export function folderSizeBytes(f: FolderSize | undefined): number | undefined {
  return f?.done && !f.virtualFs ? f.bytes : undefined;
}

/** Properties text, without fabricated byte or item counts for virtual filesystems. */
export function folderSizeDetails(f: FolderSize | undefined): string {
  if (!f) return "counting…";
  if (f.virtualFs) return `${folderSizeLabel(f)} ${folderSizeTitle(f)}`;
  return `${folderSizeLabel(f)} (${f.bytes.toLocaleString()} bytes) in ${f.files.toLocaleString()} files, ${f.dirs.toLocaleString()} folders${f.unreadable ? `; ${items(f.unreadable)} unreadable` : ""}`;
}

/** Shows or hides the sidebar. In a narrow window (auto-hidden) the choice is temporary and the
 *  saved preference is left alone. */
export function toggleSidebar(show?: boolean) {
  const s = getState();
  const next = show ?? !sidebarShown(s);
  if (s.sidebarAuto) setState({ sidebarOverride: next });
  else {
    setState({ sidebarVisible: next });
    if (s.settings && s.settings.sidebarVisible !== next) void updateSettings({ sidebarVisible: next });
  }
  // The panes' share of the window changed: the adaptive layout may need another mode.
  syncViewport();
}

/** Shows or hides the sidebar system monitor (toolbar button, monitor menu). Showing it works
 *  even in a crowded sidebar and saves the wish; hiding saves the wish to keep it hidden. */
export function toggleSystemMonitor(show?: boolean) {
  const s = getState();
  const next = show ?? !monitorShown(s);
  // The override only matters while the sidebar is crowded; the sidebar clears it once the
  // monitor fits, so a later shrink hides the monitor again.
  setState({ monitorOverride: next });
  if (s.settings && s.settings.showSystemMonitor !== next) void updateSettings({ showSystemMonitor: next });
}

/** Opens the Git workspace of the repository containing `path` (asks to trust the folder first). */
/** The worktrees of the repository of a folder, to jump between them. */
export async function openWorktrees(path: string) {
  if (await api.gitRoot(path).catch(() => null)) openOverlay({ type: "worktrees", path });
  else notify(tt("This folder is not in a Git repository"), "warn");
}

export async function openGitWorkspace(path: string) {
  const root = await api.gitRoot(path).catch(() => null);
  if (root) return openOverlay({ type: "gitGraph", path });
  const refused = await trustGitFolder(path);
  if (!refused) notify(tt("This folder is not inside a Git repository"), "warn");
}

// ───────────── network mounts (GVFS) ─────────────

let networkSeq = 0;

/** Reloads the GVFS network mounts for the sidebar (errors, e.g. off Linux, mean none). A slow
 *  answer that was overtaken by a newer request is dropped. */
export async function refreshNetwork() {
  const seq = ++networkSeq;
  const mounts = await api.gvfsNetworkMounts().catch(() => []);
  if (seq !== networkSeq) return;
  if (JSON.stringify(getState().networkMounts) !== JSON.stringify(mounts)) setState({ networkMounts: mounts });
}

/** Asks for a server address and mounts it; the new mount shows up in the sidebar. */
export async function connectToServer(initial = "") {
  const { validateServerUri } = await import("../core/actionsMenu");
  let value = initial;
  for (;;) {
    const r = await dialog({
      title: tt("Connect to server"),
      message: tt("Address, for example smb://server/share, sftp://user@host/path, nfs://host/export, davs://host/dav or ftp://host."),
      input: { value, placeholder: "smb://server/share" },
      buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Connect", primary: true }],
    });
    if (!r || r.button === "cancel") return;
    value = r.value;
    const v = validateServerUri(value);
    if (!v.ok) { notify(v.error, "warn"); continue; }
    notify(tt("Connecting to {uri}…", { uri: v.uri }));
    try {
      await api.connectServer(v.uri);
      notify(tt("Connected to {uri}", { uri: v.uri }), "success");
    } catch (e) {
      fail(e);
    }
    void refreshNetwork();
    return;
  }
}

/** Disconnects a network mount; the browsers leave it once it is gone (the backend releases watchers first). */
export async function disconnectNetwork(m: { name: string; uri: string; mountPath: string }) {
  try {
    await api.gvfsUnmount(m.mountPath || m.uri);
    notify(tt("Disconnected “{name}”", { name: m.name }));
    if (m.mountPath) void leaveMount(m.mountPath);
  } catch (e) {
    fail(e);
  }
  void refreshNetwork();
}
