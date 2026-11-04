// Application actions: everything that changes state or touches the backend.
import { archiveOf, createFormatFor, isBrowsableArchive, type ArchiveFormat } from "../core/archive";
import { api, events, type FavCmd, type SessionCmd } from "../api/ipc";
import type { ConflictInfo, ConflictPolicy, DiffSource, Drive, FileEntry, Layout, Settings, TaskKind, Volume } from "../api/types";
import { etaSeconds, updateRate, type Rate } from "../core/eta";
import { ancestors, backOutside, basename, isInside, join, parent, splitExt, tildify } from "../core/path";
import { controller, emitFolderChanged, refreshAll } from "./controllers";
import { getState, setState, type DialogSpec, type FolderSize, type MenuItem, type NoteKind, type Overlay, type UiState } from "./store";
import { formatSize } from "../core/format";

let noteSeq = 1;
/** Smoothed speed per running task, for the time-left estimate. */
const taskRates = new Map<number, Rate>();

// ───────────── notifications (W-09) ─────────────

/** Shows a notification. Errors stay until dismissed; the same message is not stacked twice. */
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

export function notify(text: string, kind: NoteKind = "info", action?: { label: string; run: () => void }) {
  const n = { id: noteSeq++, kind, text, at: Date.now(), action };
  setState((s) => ({
    notifications: [...s.notifications.filter((o) => o.text !== text || o.kind !== kind), n].slice(-5),
    notificationLog: [n, ...s.notificationLog].slice(0, 100),
  }));
  // Errors stay longer (and in the notification log, bell); none stays for good.
  const ms = getState().settings?.notificationMs ?? 6000;
  const life = kind === "error" ? Math.max(20_000, ms * 4) : kind === "warn" || action ? ms * 1.5 : ms;
  noteDue.set(n.id, Date.now() + life);
  // Those pushed out of the five shown are forgotten.
  const shown = new Set(getState().notifications.map((o) => o.id));
  for (const id of noteDue.keys()) if (!shown.has(id)) noteDue.delete(id);
  scheduleNotes();
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
  notesHeld = hold;
  if (!hold) {
    const min = Date.now() + 2500;
    for (const [id, due] of noteDue) if (due < min) noteDue.set(id, min);
  }
  scheduleNotes();
}

export function fail(err: unknown) {
  notify(readableError(String(err)), "error");
  api.uiLog("error", String(err)).catch(() => {});
}

/** "No such file or directory" (Unix) or "cannot find the path / file" (Windows). */
export function isMissingError(s: string): boolean {
  return /os error [23]\)|No such file or directory|cannot find the (path|file)/i.test(s);
}

/** The same `os error` number means different things on Windows and Unix (32 is a
 *  sharing violation on Windows, a broken pipe on Linux; 39 a full disk vs. a folder that
 *  is not empty), so the numbers are read by the platform; the texts match on any. */
const OS_ERRORS: Record<string, { windows: number[]; unix: number[]; text: RegExp }> = {
  denied: { windows: [5], unix: [1, 13], text: /Permission denied|Access is denied|Operation not permitted/i },
  busy: { windows: [32, 33], unix: [16, 26], text: /being used by another process|locked a portion|resource busy|Text file busy/i },
  full: { windows: [39, 112], unix: [28, 122], text: /not enough space|No space left|quota exceeded/i },
  long: { windows: [206], unix: [36], text: /name too long|path is too long/i },
  readOnly: { windows: [19], unix: [30], text: /Read-only file system|write.?protected/i },
};

function osError(s: string, kind: keyof typeof OS_ERRORS): boolean {
  const k = OS_ERRORS[kind];
  const code = /\(os error (\d+)\)/.exec(s);
  const codes = getState().info?.platform === "windows" ? k.windows : k.unix;
  return (code !== null && codes.includes(Number(code[1]))) || k.text.test(s);
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


/** Backend errors in words: "archive-password:needed:C:\x\a.7z" → "“a.7z” is encrypted …". */
export function readableError(s: string): string {
  s = s.replace(/^no-trash: /, "");
  // "'C:\…\Dokumenti\New folder' already exists" → "“New folder” already exists in “Dokumenti”".
  const exists = /^'(.+)' (already exists|does not exist)$/.exec(s);
  if (exists) {
    const dir = parent(exists[1]);
    return `“${basename(exists[1])}” ${exists[2]}${dir ? ` in “${basename(dir) || dir}”` : ""}`;
  }
  // A folder that cannot be listed: say it in words (the details stay in the log).
  const list = /^list failed for '(.+)': (.+)$/.exec(s);
  if (list) {
    const name = `“${basename(list[1]) || list[1]}”`;
    if (osError(list[2], "denied")) return `You have no permission to open ${name}.`;
    if (isMissingError(list[2])) return `${name} does not exist (any more).`;
    if (/os error 20\)|Not a directory/i.test(list[2])) return `${name} is not a folder.`;
    return `${name} cannot be opened: ${list[2].replace(/ \(os error \d+\)$/, "")}`;
  }
  // Any other file operation: "rename failed for 'C:\…\a.txt': … (os error 32)" →
  // "Cannot rename “a.txt”: it is open in another program."
  const io = /^([a-z][a-z ]*) failed for '(.+)': (.+)$/i.exec(s);
  if (io) {
    const [, op, path, why] = io;
    const name = `“${basename(path) || path}”`;
    const reason =
      osError(why, "denied") ? "you have no permission"
      : osError(why, "busy") ? "it is open in another program"
      : isMissingError(why) ? "it does not exist (any more)"
      : osError(why, "full") ? "the drive is full"
      : osError(why, "long") ? "the name or path is too long"
      : osError(why, "readOnly") ? "the drive is read-only"
      : why.replace(/ \(os error \d+\)$/, "").replace(/\.$/, ""); // Windows texts end with "."
    return `Cannot ${op} ${name}: ${reason}.`;
  }
  const bad = /^invalid name '(.*)'$/.exec(s);
  if (bad) return `“${bad[1]}” is not a valid name${getState().info?.platform === "windows" ? ` (Windows names cannot contain < > : " / \\ | ? * or end with a dot or space)` : " (names cannot contain /)"}.`;
  const inside = /^cannot (\w+): '(.+)' is inside '(.+)'$/.exec(s);
  if (inside) return `Cannot ${inside[1]} “${basename(inside[3])}” into itself (“${basename(inside[2])}” is inside it).`;
  const m = /archive-password:(needed|wrong):(.+)$/.exec(s);
  if (!m) return s;
  const name = basename(m[2].trim());
  return m[1] === "wrong"
    ? `Wrong password for “${name}”. Open it (Enter) to type it again.`
    : `“${name}” is encrypted. Open it (Enter) to type the password, then try again.`;
}

// ───────────── overlays / dialogs / menus ─────────────

export function openOverlay(o: Overlay | null) {
  setState({ overlay: o, menu: null });
}

export function dialog(spec: Omit<DialogSpec, "resolve">): Promise<{ button: string; value: string } | null> {
  return new Promise((resolve) => {
    openOverlay({
      type: "dialog",
      dialog: {
        ...spec,
        resolve: (r) => {
          openOverlay(null);
          resolve(r);
        },
      },
    });
  });
}

export async function confirm(title: string, message: string, okLabel = "OK", danger = false): Promise<boolean> {
  const r = await dialog({ title, message, buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: okLabel, primary: true, danger }] });
  return r?.button === "ok";
}

export async function promptText(title: string, value: string, okLabel = "OK", select?: [number, number]): Promise<string | null> {
  const r = await dialog({ title, input: { value, select }, buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: okLabel, primary: true }] });
  return r && r.button !== "cancel" ? r.value : null;
}

export function showMenu(x: number, y: number, items: MenuItem[]) {
  setState({ menu: { x, y, items } });
}

// ───────────── bootstrap ─────────────

export async function bootstrap() {
  // Startup timing goes to the log: slow calls show up there.
  const timed = <T,>(name: string, p: Promise<T>): Promise<T> => {
    const t = performance.now();
    return p.finally(() => {
      const ms = performance.now() - t;
      if (ms > 50) api.uiLog("info", `startup: ${name} took ${Math.round(ms)} ms`).catch(() => {});
    });
  };
  const [info, settings, session, favorites, history, drives, args] = await Promise.all([
    timed("appInfo", api.appInfo()), timed("settings", api.getSettings()), timed("session", api.sessionGet()),
    timed("favorites", api.favoritesGet()), timed("history", api.historyGet()), timed("drives", api.listDrives()),
    timed("args", api.startupArgs()),
  ]);
  setState({ info, settings, session, favorites, history, drives, sidebarVisible: true });
  if (info.startupNotice) notify(info.startupNotice, "warn");
  applyCliArgs(args);
  // First start: the welcome setup (not during self-tests, which start with fresh settings).
  if (info.firstRun && !args.selftest) openOverlay({ type: "welcome" });
  if (args.selftest) setTimeout(() => import("../selftest").then((m) => (args.selftestMode === "demo" ? m.runDemo() : args.selftestMode === "design" ? m.runDesignShots() : args.selftestMode === "audit" ? m.runUiAudit() : m.runSelfTest())), 1500);

  await events.fsChanged(({ path, storm }) => {
    if (storm) setStormNotice(path);
    emitFolderChanged(path, storm);
  });
  await events.task((ev) => {
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
    if (ev.state === "done" && (ev.kind === "compress" || (ev.kind === "extract" && ev.items === 1))) {
      notify(ev.kind === "compress" ? `Created “${ev.name}”` : `Extracted to “${ev.name}”`, "success", { label: "Undo", run: undo });
    } else if (ev.state === "done") {
      const what = ev.name && ev.items === 1 ? `“${ev.name}”` : items(ev.items);
      const to = ev.to && ev.kind !== "delete" ? ` to ${ev.to}` : "";
      const text = ev.name && ev.items === 0 && ev.skipped
        ? `Skipped “${ev.name}”`
        : `${verb[ev.kind]} ${what}${to}${ev.skipped ? `, skipped ${ev.skipped}` : ""}`;
      notify(text, "success",
        ev.kind !== "delete" ? { label: "Undo", run: undo } : undefined);
    } else if (ev.state === "failed" && replaceReq && String(ev.error).startsWith("no-trash:")) {
      void offerPermanentReplace(replaceReq, String(ev.error));
    } else if (ev.state === "failed") {
      const msg = readableError(String(ev.error));
      notify(msg.startsWith("Cannot ") ? msg : `${ev.kind} failed: ${msg}`, "error");
    }
    else notify(`${ev.kind} cancelled`, "warn");
    refreshUndo();
    // A folder deleted or moved away leaves the history (the backend drops gone folders).
    if (ev.kind === "delete" || ev.kind === "move") {
      api.historyGet().then((history) => setState({ history })).catch(() => {});
    }
  });
  await events.folderSizeProgress(({ path, size }) => {
    // Running totals only for calculations still on (a late event must not undo "done").
    const cur = getState().folderSizes[path];
    if (cur?.done === false) putFolderSize(path, { ...size, done: false, at: Date.now(), job: cur.job });
  });
  await events.secondInstance((a) => applyCliArgs(a));
  // Backend warnings and errors: never only in the log file.
  await events.backendLog((l) => notify(l.msg, l.level));
  for (const l of await api.logBacklog().catch(() => [])) notify(l.msg, l.level);
  setInterval(refreshDrives, 5000);
  refreshCloud();
  // Clients can be installed or signed in while Gall runs.
  setInterval(refreshCloud, 60_000);
}

let stormShown = 0;
function setStormNotice(path: string) {
  if (Date.now() - stormShown > 30_000) {
    stormShown = Date.now();
    notify(`Something is changing files very rapidly in ${path}; refreshes are throttled.`, "warn");
  }
}

function applyCliArgs(a: { path1: string | null; path2: string | null; newTab: boolean }) {
  if (a.path1) {
    if (a.newTab) sessionCmd({ op: "newTab", b: 0, path: a.path1 });
    else navigate(0, a.path1);
  }
  if (a.path2) navigate(1, a.path2);
}

export async function refreshDrives() {
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
  }
}

/** Mounts a volume from "Not mounted" (asking for the passphrase if it is encrypted)
 *  and opens it in the active browser. */
export async function mountVolume(v: Volume) {
  const name = `“${v.label}”`;
  try {
    let mount: string | null = null;
    if (!v.locked) {
      notify(`Mounting ${name}…`);
      mount = await api.volumeMount(v.id);
    } else if (getState().info?.platform === "windows") {
      await api.volumeUnlock(v.id, ""); // Windows asks in its own BitLocker dialog
      setTimeout(refreshDrives, 3000);
      return;
    } else {
      let message = `${v.label} is encrypted. Enter its passphrase to unlock and mount it.`;
      for (;;) {
        const r = await dialog({
          title: `Unlock ${name}`,
          message,
          input: { value: "", password: true, placeholder: "Passphrase" },
          buttons: [{ id: "cancel", label: "Cancel" }, { id: "ok", label: "Unlock", primary: true }],
        });
        if (!r || r.button !== "ok" || !r.value) return;
        try {
          mount = await api.volumeUnlock(v.id, r.value);
          break;
        } catch (e) {
          message = `${String(e)}. Try again.`;
        }
      }
    }
    await refreshDrives();
    notify(`${v.locked ? "Unlocked" : "Mounted"} ${name}`, "success");
    if (mount) navigate(activeBrowser(), mount);
  } catch (e) {
    fail(e);
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
  await leaveMount(d.mount);
  try {
    await api.eject(d.mount);
    notify(`Ejected “${d.label || d.short}”`, "success");
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
    notify(`Locked “${d.label || d.short}”`, "success");
    await refreshDrives();
  } catch (e) {
    fail(e);
  }
}

/** Looks up the cloud folders again (start-up, drive changes, every minute). */
export async function refreshCloud() {
  try {
    const cloud = await api.listCloudFolders();
    if (JSON.stringify(getState().cloud) !== JSON.stringify(cloud)) setState({ cloud });
  } catch {
    /* ignore transient */
  }
}

// ───────────── settings ─────────────

export async function updateSettings(patch: Partial<Settings>) {
  const s = getState().settings;
  if (!s) return;
  const next = { ...s, ...patch };
  setState({ settings: next });
  try {
    await api.setSettings(next);
    setState({ settingsRev: getState().settingsRev + 1 });
  } catch (e) {
    fail(e);
  }
}

/** Forgets the sort and view kept for every folder (Settings › View). */
export async function forgetFolderViews() {
  if (!(await confirm("Forget folder views?", "Every folder goes back to the default sort and view.", "Forget", true))) return;
  try {
    await api.clearPrefs();
    setState({ settingsRev: getState().settingsRev + 1 });
    notify("Folder views forgotten");
  } catch (e) {
    fail(e);
  }
}

/** Factory reset (Settings › Advanced): every setting back to its default. */
export async function resetAllSettings() {
  if (!(await confirm("Reset all settings?",
    "Every setting, shortcut, folder colour, remembered program and folder view goes back to the default. Favourites, colour tags, history and open tabs stay.",
    "Reset", true))) return;
  try {
    const settings = await api.resetSettings();
    setState({ settings, settingsRev: getState().settingsRev + 1 });
    notify("Settings reset to defaults");
    // Back to first-start settings: the welcome setup again (it replaces the settings window).
    openOverlay({ type: "welcome" });
  } catch (e) {
    fail(e);
  }
}

// ───────────── session / navigation ─────────────

export async function sessionCmd(cmd: SessionCmd) {
  try {
    const session = await api.sessionCmd(cmd);
    setState({ session });
    if (cmd.op === "navigate" || cmd.op === "back" || cmd.op === "forward" || cmd.op === "up") {
      api.historyGet().then((history) => setState({ history })).catch(() => {});
    }
  } catch (e) {
    fail(e);
  }
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
  return layout === "standard" ? s.settings?.showNavColumns ?? true : layout === "columns";
}

/** Switches columns / breadcrumbs of one browser in the current layout; remembered with the session. */
export function toggleColumns(b: number, on = !columnsShown(getState(), b)) {
  sessionCmd({ op: "columns", b, on });
}

/** Toggles the preview & details pane of one browser only. */
export function toggleDetails(b: number) {
  sessionCmd({ op: "details", b, on: !detailsShown(getState(), b) });
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
  const r = await api.pathResolve(input, activePath(b));
  if (!r.exists) return fail(`Path not found: ${r.path}`);
  if (r.isDir) navigate(b, r.path);
  else {
    const p = parent(r.path);
    if (p) navigate(b, p);
    setTimeout(() => controller(b)?.select([r.path], r.path), 150);
  }
}

// ───────────── favourites ─────────────

export async function favCmd(cmd: FavCmd) {
  try {
    setState({ favorites: await api.favoritesCmd(cmd) });
  } catch (e) {
    fail(e);
  }
}

// ───────────── compare ─────────────

const fileSrc = (path: string): DiffSource => ({ kind: "file", path });

export function compare(a: DiffSource, b: DiffSource) {
  openOverlay({ type: "diff", a, b });
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

export function compareFolders(left: string, right: string) {
  if (left === right) return notify("Both sides are the same folder", "warn");
  // A folder and one inside it: a sync would copy the folder into itself or trash its own files.
  if (isInside(left, right) || isInside(right, left))
    return notify(`“${basename(isInside(left, right) ? left : right)}” is inside the other folder: compare two separate folders`, "warn");
  openOverlay({ type: "dircmp", left, right });
}

/** The folders shown in both browsers (browser 1 on the left). */
export function comparePanelFolders(b: number) {
  const o = otherBrowser(b);
  if (o === b || getState().session?.layout === "classic") return notify("Compare folders needs two panels", "warn");
  compareFolders(activePath(0), activePath(1));
}

/** One folder: the folder with the same name in the other panel, else the other panel's folder. */
export async function compareFolderWithOther(b: number, dir: FileEntry) {
  const o = otherBrowser(b);
  if (o === b) return notify("Compare folders needs two panels", "warn");
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
  if (!one) return notify("Select two files to compare (or one, to compare with the other panel)", "warn");
  const partner = (await otherPanelPartner(b, one)) ?? (await pickCompareFile(b, one));
  if (!partner) return;
  b === 0 ? compare(fileSrc(one.path), fileSrc(partner)) : compare(fileSrc(partner), fileSrc(one.path));
}

/** No partner in the other panel: let the user choose one, starting in the other panel's folder. */
async function pickCompareFile(b: number, one: FileEntry): Promise<string | null> {
  const o = otherBrowser(b);
  const start = o !== b && getState().session?.layout !== "classic" ? activePath(o) : parent(one.path);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const p = await open({ title: `Compare ${one.name} with…`, defaultPath: start ?? undefined, multiple: false, directory: false });
  return typeof p === "string" && p !== one.path ? p : null;
}

export async function compareWithClipboard(path: string) {
  const c = await api.clipInspect().catch(() => null);
  if (c?.kind === "text") return compare(fileSrc(path), { kind: "text", label: "Clipboard", text: c.text });
  if (c?.kind === "url") return compare(fileSrc(path), { kind: "text", label: "Clipboard", text: c.url });
  if (c?.kind === "paths" && c.paths.length === 1) return compare(fileSrc(path), fileSrc(c.paths[0]));
  notify("The clipboard holds no text or single file to compare with", "warn");
}

export function compareWithGitHead(path: string) {
  compare({ kind: "gitHead", path }, fileSrc(path));
}

/** "Compare with…" submenu for one file. `inRepo` comes from the pane (Git root of its folder). */
export function compareMenu(b: number, entry: FileEntry, inRepo: boolean): MenuItem {
  const o = otherBrowser(b);
  const focusedOther = o !== b ? controller(o)?.focused() : null;
  const other = focusedOther && !focusedOther.isDir && focusedOther.path !== entry.path ? focusedOther.name : null;
  return {
    label: "Compare with",
    icon: "compare",
    submenu: [
      { label: other ? `Other panel: ${other}` : "Other panel (same name)", shortcut: "Ctrl+Shift+D", run: async () => {
          const partner = (await otherPanelPartner(b, entry)) ?? (await pickCompareFile(b, entry));
          if (!partner) return;
          b === 0 ? compare(fileSrc(entry.path), fileSrc(partner)) : compare(fileSrc(partner), fileSrc(entry.path));
        } },
      { label: "Clipboard", run: () => compareWithClipboard(entry.path) },
      { label: inRepo ? "Git HEAD (my changes)" : "Git HEAD (not in a repository)", disabled: !inRepo, run: () => compareWithGitHead(entry.path) },
      { separator: true },
      { label: "Choose file…", run: async () => {
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
    label: "Folder colour",
    icon: "folder",
    submenu: [
      { label: "Default", checked: !cur, run: () => setFolderColor(paths, null) },
      { separator: true },
      ...FOLDER_COLORS.map(([label, c]) => ({ label, swatch: c, checked: cur === c, run: () => setFolderColor(paths, c) })),
      { separator: true },
      {
        label: "Custom…",
        swatch: cur && !FOLDER_COLORS.some(([, c]) => c === cur) ? cur : undefined,
        run: async () => {
          const v = await promptText("Folder colour (e.g. #ff8800 or rebeccapurple; empty = default)", cur ?? "", "Apply");
          if (v === null) return;
          const t = v.trim();
          if (!t) return setFolderColor(paths, null);
          if (!CSS.supports("color", t)) return notify(`Not a valid colour: ${t}`, "warn");
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

/** Pin a folder to the sidebar, or unpin it from every group it is in (with undo). */
export async function togglePin(path: string) {
  try {
    const spots = pinSpots(path);
    if (spots.length) {
      // Remove from the back so earlier indices stay valid.
      for (const s of [...spots].sort((a, b) => b.index - a.index)) {
        setState({ favorites: await api.favoritesCmd({ op: "removeItem", id: s.id, index: s.index }) });
      }
      notify(`Unpinned ${basename(path) || path}`, "info", {
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
    notify(`Pinned ${basename(path) || path} to the sidebar`, "success");
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
  await runUndo((permanent) => api.undoIn(dir, permanent));
}

export async function undo() {
  await runUndo((permanent) => api.undo(permanent));
}

/** Runs an undo. When the trash refuses what the operation created (a copy on a network
 *  drive), asks to delete it for good instead: otherwise that undo would fail every time
 *  and block Ctrl+Z for everything older. */
async function runUndo(call: (permanent: boolean) => Promise<string>) {
  try {
    // The backend says what the undo did: "Removed “a.pdf” (in the trash)".
    notify(await call(false), "info");
    refreshAll();
  } catch (e) {
    if (!String(e).startsWith("no-trash:")) fail(e);
    else if (await confirm("Undo without the trash?",
      `${readableError(String(e))} Undo deletes them for good instead; they cannot be restored.`, "Delete for good", true)) {
      try {
        notify(await call(true), "info");
        refreshAll();
      } catch (e2) {
        fail(e2);
      }
    }
  }
  refreshUndo();
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
      + (info.folders ? `\n${allFolders ? (one ? "It is a folder on both sides" : "They are folders on both sides") : `${items(info.folders)} are folders on both sides`}: Merge puts the contents together.` : ""),
    buttons: [
      { id: "cancel", label: "Cancel", key: "c" },
      { id: "skip", label: "Skip", key: "s" },
      { id: "overwrite", label: "Replace", danger: true, key: "r", hint: "old one to the trash" },
      ...(info.folders ? [{ id: "merge", label: "Merge", key: "m", hint: "folders", primary: true }] : []),
      { id: "keepBoth", label: "Keep both", primary: !info.folders, key: "k" },
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
      { id: "overwriteOlder", label: "Replace older", key: "o", hint: "if newer" },
      { id: "overwrite", label: "Replace", danger: true, key: "r", hint: "old to the trash" },
      { id: "keepBoth", label: "Keep both", primary: true, key: "k" },
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
  const v = await promptText("Compress to", `${base}.zip`, "Compress", [0, base.length]);
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
  notify(`Can't ${what} inside “${basename(inside)}”: archives open read-only. Copy items out (y y / Ctrl+C) or extract it.`, "warn");
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
    title: wrong ? "Wrong password" : "Password required",
    message: wrong ? `That password does not open “${basename(archive)}”. Try again.` : `“${basename(archive)}” is encrypted.`,
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
  const n = setTimeout(() => notify(`Extracting ${named(paths)} from the archive…`), 400);
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

async function restoreCut(paths: string[]) {
  if (getState().clipboard.paths.length) return; // something else was cut or copied since
  const left = await api.findExisting(paths).catch(() => []);
  if (!left.length || getState().clipboard.paths.length) return;
  await api.clipSetFiles(left, true).catch(() => {});
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
    "Replace permanently?",
    `${readableError(error)} Replace without the trash? The replaced items will be deleted and cannot be restored.`,
    "Replace permanently", true);
  if (!ok) return notify(`${req.kind === "move" ? "Move" : "Copy"} stopped; nothing more was replaced`, "warn");
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
  if (sources.some((s) => isInside(dest, s) && s !== dest)) return fail(`Cannot ${kind} a folder into itself`);
  if (kind === "move" && sources.every((s) => parent(s) === dest)) return;
  try {
    const choice = await askConflict(await api.conflictInfo(sources, dest));
    if (!choice) return;
    const id = await api.startTask({ kind, sources, dest, conflict: choice.policy, merge: choice.merge });
    if (replacing(choice.policy)) replaceRequests.set(id, { kind, sources, dest, merge: choice.merge });
  } catch (e) {
    fail(e);
  }
}

export async function trashPaths(paths: string[]) {
  if (!paths.length) return;
  if (refuseInArchive(paths, "delete")) return;
  try {
    await api.trash(paths);
    notify(`Moved ${named(paths)} to trash`, "info", { label: "Undo", run: undo });
    refreshUndo();
  } catch (e) {
    refreshUndo(); // part of the items may be in the trash
    const msg = String(e);
    if (!msg.startsWith("trash:")) return fail(e);
    // No trash here (network share, read-only trash, …): offer to delete for good.
    const left = await api.findExisting(paths).catch(() => paths);
    if (!left.length) return fail(e);
    const ok = await confirm("Can't move to the trash",
      `${msg.replace(/^trash:\s*/, "")}. Delete ${named(left)} permanently instead? This cannot be undone.`,
      "Delete permanently", true);
    if (ok) await api.startTask({ kind: "delete", sources: left }).catch(fail);
  }
}

export async function deletePermanently(paths: string[]) {
  if (!paths.length) return;
  if (refuseInArchive(paths, "delete")) return;
  if (!(await confirm("Delete permanently?", `${named(paths)} will be deleted and cannot be restored.`, "Delete", true))) return;
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
    if (!(await confirm("Remove extension?", `"${newName}" has no extension. The file may no longer open with the right program.`, "Rename"))) return;
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
  const name = await freeName(dir, "New folder");
  const r = await dialog({
    title: "New folder",
    input: { value: name, select: [0, name.length] },
    buttons: [
      { id: "cancel", label: "Cancel" },
      ...(sel.length ? [{ id: "moveIn", label: "Create & move selection inside", hint: "Shift+Enter" }] : []),
      { id: "open", label: "Create & open", hint: "Ctrl+Enter" },
      { id: "ok", label: "Create", primary: true },
    ],
  });
  if (!r || r.button === "cancel" || !r.value.trim()) return;
  try {
    const p = await api.createDir(dir, r.value.trim());
    if (r.button === "open") navigate(b, p);
    else if (r.button === "moveIn") await api.startTask({ kind: "move", sources: sel.map((e) => e.path), dest: p, conflict: "keepBoth" });
    else controller(b)?.select([p], p);
    if (r.button !== "moveIn") notify(`Created folder “${basename(p)}”`, "success", { label: "Undo", run: undo });
    refreshUndo();
  } catch (e) {
    fail(e);
  }
}

export async function createFromTemplate(b: number, dir: string, template: string | undefined, name: string, open: boolean) {
  try {
    const p = await api.createFile(dir, name, template);
    if (open) await api.openPath(p);
    controller(b)?.select([p], p);
    notify(`Created “${basename(p)}”`, "success", { label: "Undo", run: undo });
    refreshUndo();
  } catch (e) {
    fail(e);
  }
}

export async function openEntries(b: number, entries: FileEntry[]) {
  if (!entries.length) return;
  const dirs = entries.filter((e) => e.isDir);
  const files = entries.filter((e) => !e.isDir);
  if (files.length >= 10 && !(await confirm("Open many files?", `Open ${files.length} files at once?`, "Open"))) return;
  if (dirs.length === 1 && !files.length) return navigate(b, dirs[0].path);
  // An archive opens like a folder (Open with… still hands it to another program).
  if (entries.length === 1 && isBrowsableArchive(files[0].name)) return navigate(b, files[0].path);
  for (const d of dirs) sessionCmd({ op: "newTab", b, path: d.path, activate: false });
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
    withArchivePassword(() => api.openPath(f.path)).catch(fail);
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
    await api.clipSetFiles(paths, cut);
  } catch (e) {
    return fail(e);
  }
  setState({ clipboard: { paths, cut } });
  // Nothing is copied yet: say what is on the clipboard and what pasting will do,
  // so this does not read like the "Copied …" that a finished paste shows.
  notify(`${named(paths)} on the clipboard; paste to ${cut ? "move" : "copy"}`, "info");
}

/** "1 item", "3 items". */
export function items(n: number): string {
  return `${n} item${n === 1 ? "" : "s"}`;
}

/** What a message is about: the quoted name of a single item (“a.txt”), else the count (3 items). */
export function named(paths: string[]): string {
  return paths.length === 1 ? `“${basename(paths[0])}”` : items(paths.length);
}

/** Ctrl+V: files from our clipboard, external paths, or text/image/URL → new file (O-10). */
export async function paste(b: number) {
  const dir = activePath(b);
  if (refuseInArchive(dir, "paste")) return;
  const clip = getState().clipboard;
  try {
    const ext = await api.clipInspect().catch(() => ({ kind: "empty" as const }));
    // Our own file clipboard wins if the system clipboard still holds its paths.
    const ours = clip.paths.length && (ext.kind !== "paths" || ext.paths.join("\n") === clip.paths.join("\n"));
    if (ours) {
      const sameFolder = clip.paths.every((p) => parent(p) === dir);
      const choice = sameFolder && !clip.cut ? { policy: "keepBoth" as const, merge: false } : await askConflict(await api.conflictInfo(clip.paths, dir));
      if (!choice) return;
      const id = await api.clipPasteFiles(dir, choice.policy, undefined, choice.merge);
      if (clip.cut) cutPastes.set(id, clip.paths);
      if (replacing(choice.policy)) replaceRequests.set(id, { kind: clip.cut ? "move" : "copy", sources: clip.paths, dest: dir, merge: choice.merge });
      if (clip.cut) setState({ clipboard: { paths: [], cut: false } });
      return;
    }
    switch (ext.kind) {
      case "paths":
        return transfer("copy", ext.paths, dir);
      case "image": {
        const p = await api.clipPasteAsFile(dir, "image");
        controller(b)?.select([p], p);
        return;
      }
      case "url": {
        const r = await dialog({
          title: "Paste link",
          message: ext.url,
          buttons: [{ id: "cancel", label: "Cancel" }, { id: "text", label: "As text file" }, { id: "url", label: "As link shortcut", primary: true }],
        });
        if (!r || r.button === "cancel") return;
        const p = await api.clipPasteAsFile(dir, r.button as "text" | "url");
        controller(b)?.select([p], p);
        return;
      }
      case "text": {
        const p = await api.clipPasteAsFile(dir, "text");
        controller(b)?.select([p], p);
        notify(`Created ${basename(p)} from clipboard text`, "success");
        return;
      }
      default:
        notify("Clipboard is empty", "info");
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
  const formatted = await Promise.all(paths.map((p) => api.pathFormat(p)));
  await api.clipSetText(formatted.join("\n")).catch(fail);
  notify(paths.length === 1 ? `Copied path: ${formatted[0]}` : `Copied ${paths.length} paths`, "info");
}

export async function tabFromClipboard(b: number) {
  const c = await api.clipInspect().catch(() => null);
  if (c?.kind === "paths") {
    const r = await api.pathResolve(c.paths[0]);
    sessionCmd({ op: "newTab", b, path: r.isDir ? r.path : parent(r.path) });
  } else notify("Clipboard does not contain a path", "warn");
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

/** Calculates the total size of the folders `paths` for `owner` (a browser index or a
 *  window), two at a time; a new call replaces the owner's running one. Sizes stream
 *  into `folderSizes` as they are counted. `skipFresh`: keep sizes younger than that (ms). */
export async function calcFolderSizes(owner: number | string, paths: string[], skipFresh = 0) {
  cancelFolderSizes(owner);
  const now = Date.now();
  const queue = paths.filter((p) => {
    const f = getState().folderSizes[p];
    return !(skipFresh && f?.done && now - f.at < skipFresh);
  });
  if (!queue.length) return;
  const ticket = ++sizeTicket;
  sizeOwners.set(owner, ticket);
  const worker = async () => {
    while (queue.length && sizeOwners.get(owner) === ticket) {
      const p = queue.shift()!;
      const prev = getState().folderSizes[p];
      const known = prev?.done ? prev : undefined;
      putFolderSize(p, { bytes: 0, files: 0, dirs: 0, unreadable: 0, ...known, done: false, at: Date.now(), job: ticket });
      const r = await api.folderSize(p, ticket).catch(() => null);
      if (r) putFolderSize(p, { ...r, done: true, at: Date.now() });
      // Cancelled or failed: back to what was known, unless a newer calculation took over.
      else if (getState().folderSizes[p]?.job === ticket) putFolderSize(p, known ?? null);
    }
  };
  await Promise.all([worker(), worker()]);
  if (sizeOwners.get(owner) === ticket) sizeOwners.delete(owner);
}

/** Stops the folder size calculation of `owner`. */
export function cancelFolderSizes(owner: number | string) {
  const t = sizeOwners.get(owner);
  if (t == null) return;
  sizeOwners.delete(owner);
  api.folderSizesCancel(t).catch(() => {});
}

/** "1.2 GB" for a calculated folder size ("1.2 GB…" while counting), else "". */
export function folderSizeLabel(f: FolderSize | undefined): string {
  if (!f) return "";
  return formatSize(f.bytes) + (f.done ? "" : "…");
}

export function folderSizeTitle(f: FolderSize | undefined): string {
  if (!f) return "";
  const what = `${f.files.toLocaleString()} files, ${f.dirs.toLocaleString()} folders`;
  return `${f.bytes.toLocaleString()} bytes in ${what}${f.unreadable ? `; ${items(f.unreadable)} could not be read (the total is at least this)` : ""}${f.done ? "" : " (counting…)"}`;
}
