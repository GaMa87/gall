// Tiny external store (no dependency) consumed with useSyncExternalStore.
import { useSyncExternalStore } from "react";
import type { Action } from "../core/keymap";
import type { AppInfo, CloudFolder, DiffSource, DirSize, Drive, Volume,Favorites, FileEntry, Session, Settings, TaskProgress } from "../api/types";

export type NoteKind = "info" | "success" | "warn" | "error";
export interface Notification {
  id: number;
  kind: NoteKind;
  text: string;
  at: number;
  action?: { label: string; run: () => void };
}

export interface MenuItem {
  label?: string;
  shortcut?: string;
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
  | { type: "pathEdit"; b: number }
  | { type: "search"; root: string; pattern?: string; everything?: boolean }
  | { type: "trash" }
  | { type: "quickOpen"; initial?: string }
  | { type: "grep"; root: string; scope: "folder" | "home" | "root" }
  | { type: "keys" }
  | { type: "settings"; section?: string }
  | { type: "about" }
  | { type: "welcome" }
  | { type: "previewWindow"; b: number }
  | { type: "templates"; dir: string; b: number }
  | { type: "note"; dir: string; path?: string; text?: string }
  | { type: "tag"; paths: string[] }
  | { type: "properties"; entry: FileEntry }
  | { type: "batchRename"; b: number; entries: FileEntry[] }
  | { type: "diff"; a: DiffSource; b: DiffSource; back?: Overlay }
  | { type: "dircmp"; left: string; right: string }
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
  input?: { value: string; select?: [number, number]; placeholder?: string; toggleSelections?: [number, number][]; password?: boolean };
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
  favorites: Favorites;
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
  menu: { x: number; y: number; items: MenuItem[] } | null;
  sidebarVisible: boolean;
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
  /** Focused entry of each browser's folder pane (drives the details pane). */
  focused: (FileEntry | null)[];
  /** Pending vim/leader key sequence, e.g. ["Space", "f"] (drives which-key). */
  keySeq: string[];
  /** Files recently opened through Quick Open, newest first. */
  recentFiles: string[];
}

let state: UiState = {
  info: null,
  settings: null,
  settingsRev: 0,
  session: null,
  favorites: { groups: [] },
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
  shownBrowsers: [0, 1],
  longNames: false,
  clipboard: { paths: [], cut: false },
  undoLabel: null,
  undoRev: 0,
  folderSizes: {},
  themeNonce: 0,
  focused: [null, null],
  keySeq: [],
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

export function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useStore<T>(sel: (s: UiState) => T): T {
  return useSyncExternalStore(subscribe, () => sel(state));
}
