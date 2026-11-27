// Tiny external store (no dependency) consumed with useSyncExternalStore.
import { useSyncExternalStore } from "react";
import type { AdaptiveMode } from "../core/adaptiveLayout";
import type { Anchor } from "../core/anchor";
import type { Action } from "../core/keymap";
import type { ThemeContrastReport } from "../core/themeContrast";
import type { ActionDef, AppInfo, PickArgs, CloudFolder, DiffSource, DirSize, Drive, Volume, Favorites, FileEntry, GravitySuggestion, Marks, NetworkMount, OmarchyState, Session, Settings, TaskProgress, WorkspaceSummary } from "../api/types";

export type NoteKind = "info" | "success" | "warn" | "error";
export type NoteAction = { label: string; run: () => void };
export interface Notification {
  id: number;
  kind: NoteKind;
  text: string;
  at: number;
  action?: NoteAction;
  /** Further buttons after `action`. */
  more?: NoteAction[];
  reportError?: string;
  details?: string;
}

export interface MenuItem {
  label?: string;
  shortcut?: string;
  /** A key that runs this item while the menu is open (shown at its right edge). Give none of h j k l q: Vim keys navigate with them. */
  accel?: string;
  /** A key shown as it is (Git commands have keys of their own, unrelated to the chords of the file list). */
  keyCap?: string;
  /** Shows this action's keys in the current mode (vim / leader / chord) as the shortcut. */
  action?: Action;
  icon?: string;
  disabled?: boolean;
  danger?: boolean;
  separator?: boolean;
  checked?: boolean;
  /** Colour chip shown in the icon slot (e.g. folder colour picker). */
  swatch?: string;
  submenu?: MenuItem[];
  run?: () => void;
  /** Variant shown and run while Ctrl is held (e.g. "Extract here" → "… and trash archive"). */
  ctrl?: { label: string; run: () => void };
}

export type Overlay =
  | { type: "bugReport"; previous?: boolean; resume?: boolean; triggerError?: string }
  | { type: "pathEdit"; b: number }
  | { type: "search"; root: string; pattern?: string; everything?: boolean }
  | { type: "trash" }
  | { type: "quickOpen"; initial?: string }
  | { type: "grep"; root: string; scope: "folder" | "home" | "root" }
  | { type: "keys" }
  | { type: "gitCommits"; path: string; onlyFile?: boolean }
  | { type: "gitGraph"; path: string; focusFile?: string; compare?: [string, string]; show?: string }
  | { type: "worktrees"; path: string; cleanup?: boolean }
  | { type: "settings"; section?: string }
  | { type: "about" }
  | { type: "welcome" }
  | { type: "favoritesImport" }
  /** The dialog that applies a patch (unified, context or normal) below a folder: its text given, or read from a file;
   *  closing it opens `back` (the comparison it was asked from). */
  | { type: "applyPatch"; root?: string; text?: string; file?: string; back?: Overlay }
  | { type: "previewWindow"; b: number }
  | { type: "templates"; dir: string; b: number }
  | { type: "note"; dir: string; path?: string; text?: string }
  | { type: "tag"; paths: string[] }
  | { type: "properties"; entry: FileEntry }
  | { type: "batchRename"; b: number; entries: FileEntry[] }
  | { type: "diff"; a: DiffSource; b: DiffSource; back?: Overlay; wait?: boolean }
  | { type: "dircmp"; left: string; right: string; wait?: boolean }
  /** The standalone three-way merge of three files (any of them may be given); the result is saved to a chosen file. */
  | { type: "standaloneMerge"; base?: string; left?: string; right?: string; waitOutput?: string }
  | { type: "matrix"; paths: string[] }
  | { type: "openWith"; paths: string[]; mode: "file" | "terminal" | "editor" }
  /** `under`: the window the dialog was asked from; it stays open beneath and comes back. */
  | { type: "dialog"; dialog: DialogSpec; under?: Overlay | null };

export interface FolderSize extends DirSize {
  done: boolean;
  /** When it was calculated (ms). */
  at: number;
  /** The calculation counting it (while not done). */
  job?: number;
}

export interface DialogSpec {
  title: string;
  message?: string;
  input?: { value: string; select?: [number, number]; placeholder?: string; toggleSelections?: [number, number][]; password?: boolean; multiline?: boolean };
  /** `key`: single-letter accelerator (lower case), underlined in the label; only without `input`. */
  buttons: { id: string; label: string; primary?: boolean; danger?: boolean; hint?: string; key?: string }[];
  resolve: (r: { button: string; value: string } | null) => void;
}

export interface UiState {
  info: AppInfo | null;
  settings: Settings | null;
  /** Bumped once changed settings are saved in the backend (folder prefs resolve with them). */
  settingsRev: number;
  session: Session | null;
  /** Lightweight summaries of all numbered workspaces in this window (inactive ones stay lazy). */
  workspaces: WorkspaceSummary[];
  /** Index of the active workspace in `workspaces`. */
  activeWorkspace: number;
  favorites: Favorites;
  /** Vim folder marks (m{letter} / '{letter}). */
  marks: Marks;
  history: string[];
  drives: Drive[];
  /** Volumes not mounted yet (sidebar "Not mounted"). */
  volumes: Volume[];
  /** Cloud storage folders found on this computer (sidebar Cloud section). */
  cloud: CloudFolder[];
  notifications: Notification[];
  notificationLog: Notification[];
  showNotificationLog: boolean;
  /** Running file operations, with the estimated seconds left (null while unknown). */
  tasks: Record<number, TaskProgress & { eta: number | null }>;
  overlay: Overlay | null;
  menu: (Anchor & { items: MenuItem[] }) | null;
  sidebarVisible: boolean;
  /** Manual sidebar toggle made while the window is narrow (auto-hidden); reset when it widens. */
  sidebarOverride: boolean | null;
  /** The sidebar is hidden automatically because the window is narrow (not a user choice). */
  sidebarAuto: boolean;
  /** The sidebar is too short for five History rows and the system monitor; the monitor yields. */
  monitorCrowded: boolean;
  /** The user showed the system monitor while the sidebar was crowded; reset once it fits again. */
  monitorOverride: boolean;
  /** Inner size of this window in CSS px (drives the adaptive layout and sidebar auto-hide). */
  viewport: { w: number; h: number };
  /** Pane arrangement the window size asks for (null when adaptive layout is off). Applies to the
   *  "standard" layout only: Classic (single) and Stacked are explicit choices that win. */
  adaptiveMode: AdaptiveMode | null;
  /** This window is a file picker (`gall --pick`) answering another program. */
  pick: PickArgs | null;
  /** Index of the file-type filter chosen in the picker's drop-down. */
  pickFilter: number;
  /** The values of the picker's extra options (the portal's `choices`), by choice id. */
  pickChoices: Record<string, string>;
  /** Live Omarchy desktop state (null until loaded; `active` false off Omarchy). */
  omarchy: OmarchyState | null;
  /** GVFS network mounts (sidebar Network section). */
  networkMounts: NetworkMount[];
  /** Context actions from actions.d (all, before per-selection filtering). */
  actionDefs: ActionDef[];
  /** Folder of the active Omarchy terminal, when known (command palette entry). */
  terminalCwd: string | null;
  /** Browsers on screen: both, or one (Classic layout, a collapsed browser, a narrow window). */
  shownBrowsers: number[];
  longNames: boolean;
  clipboard: { paths: string[]; cut: boolean };
  undoLabel: string | null;
  /** Bumped whenever the undo journal may have changed (per-panel undo buttons re-check). */
  undoRev: number;
  /** Folder sizes calculated so far, by path; `done` false while counting. */
  folderSizes: Record<string, FolderSize>;
  themeNonce: number;
  /** Contrast diagnostics for the active theme (limits that CSS variables alone cannot solve). */
  themeContrastReport: ThemeContrastReport | null;
  /** Focused entry of each browser's folder pane (drives the details pane). */
  focused: (FileEntry | null)[];
  /** The linked worktree each browser is in (shown in the path bar). */
  worktreeOf: (string | null)[];
  /** Pending vim/leader key sequence, e.g. ["Space", "f"] (drives which-key). */
  keySeq: string[];
  /** Currently hovered live tab pair ID (highlights the matching partner tab across the divider). */
  hoveredPairId: number | null;
  /** Async Pair Gravity ghost tab suggestion for each browser (`[leftGhost, rightGhost]`). */
  gravityGhost: [GravitySuggestion | null, GravitySuggestion | null];
  /** Files recently opened through Quick Open, newest first. */
  recentFiles: string[];
}

let state: UiState = {
  info: null,
  settings: null,
  settingsRev: 0,
  session: null,
  workspaces: [],
  activeWorkspace: 0,
  favorites: { groups: [] },
  marks: { marks: {} },
  history: [],
  drives: [],
  volumes: [],
  cloud: [],
  notifications: [],
  notificationLog: [],
  showNotificationLog: false,
  tasks: {},
  overlay: null,
  menu: null,
  sidebarVisible: true,
  sidebarOverride: null,
  sidebarAuto: false,
  monitorCrowded: false,
  monitorOverride: false,
  viewport: { w: typeof window !== "undefined" ? window.innerWidth : 1280, h: typeof window !== "undefined" ? window.innerHeight : 800 },
  adaptiveMode: null,
  pick: null,
  pickFilter: 0,
  pickChoices: {},
  omarchy: null,
  networkMounts: [],
  actionDefs: [],
  terminalCwd: null,
  shownBrowsers: [0, 1],
  longNames: false,
  clipboard: { paths: [], cut: false },
  undoLabel: null,
  undoRev: 0,
  folderSizes: {},
  themeNonce: 0,
  themeContrastReport: null,
  focused: [null, null],
  worktreeOf: [],
  keySeq: [],
  hoveredPairId: null,
  gravityGhost: [null, null],
  recentFiles: loadRecent(),
};

function loadRecent(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem("gall.recentFiles") ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string").slice(0, 50) : [];
  } catch {
    return [];
  }
}

/** Remembers a path opened from Quick Open (most recent first, max 50). */
export function pushRecent(path: string) {
  const list = [path, ...state.recentFiles.filter((p) => p !== path)].slice(0, 50);
  setState({ recentFiles: list });
  try {
    localStorage.setItem("gall.recentFiles", JSON.stringify(list));
  } catch {
    /* storage full or unavailable: recents are a convenience only */
  }
}

const listeners = new Set<() => void>();

export function getState(): UiState {
  return state;
}

export function setState(patch: Partial<UiState> | ((s: UiState) => Partial<UiState>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  listeners.forEach((l) => l());
}

/** Updates `viewport` in place without waking unrelated `useStore` subscribers on every resize pixel. */
export function setViewportSilent(w: number, h: number) {
  if (state.viewport.w === w && state.viewport.h === h) return;
  state = { ...state, viewport: { w, h } };
}

export function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useStore<T>(sel: (s: UiState) => T): T {
  return useSyncExternalStore(subscribe, () => sel(state));
}

/** Is the sidebar on screen: the user's choice, unless a narrow window hid it by itself
 *  (then only a manual toggle made in that narrow window shows it). */
export function sidebarShown(s: Pick<UiState, "sidebarVisible" | "sidebarAuto" | "sidebarOverride">): boolean {
  return s.sidebarAuto ? s.sidebarOverride === true : s.sidebarVisible;
}

/** Whether the system monitor is on screen. The saved setting is the user's wish; a crowded
 *  sidebar hides the monitor until the window grows, unless the user showed it anyway. */
export function monitorShown(s: Pick<UiState, "settings" | "monitorCrowded" | "monitorOverride">): boolean {
  return (s.settings?.showSystemMonitor ?? true) && (!s.monitorCrowded || s.monitorOverride);
}

/** How the two browsers are arranged: stacked (`vertical`) and/or reduced to one by the window
 *  size (`autoSingle`). An explicit Classic / Stacked layout always wins over the adaptive mode. */
export function arrangement(s: Pick<UiState, "session" | "adaptiveMode">): { vertical: boolean; autoSingle: boolean } {
  const layout = s.session?.layout ?? "standard";
  if (layout === "standard" && s.adaptiveMode) return { vertical: s.adaptiveMode === "vertical", autoSingle: s.adaptiveMode === "single" };
  return { vertical: layout === "columns", autoSingle: false };
}
