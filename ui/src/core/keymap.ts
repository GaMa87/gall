// Keyboard shortcuts (see docs/01-feature-inventory.md, M:shortcutkeys).
// Users can override any action via settings.shortcuts (C-04).

export type Action =
  | "open" | "openWith" | "parent" | "back" | "forward" | "editPath" | "copyPath" | "copyFolderPath"
  | "rename" | "copyToOther" | "moveToOther" | "duplicate" | "delete" | "deletePermanent" | "properties"
  | "tag" | "longNames" | "newFile" | "newFolder" | "newNote" | "newTab" | "reopenTab" | "closeTab"
  | "nextTab" | "prevTab" | "focusOther" | "copy" | "cut" | "paste" | "pasteAsLinks" | "preview"
  | "selectAll" | "toggleSelect" | "refresh" | "toggleHidden" | "toggleColumns" | "toggleSidebar"
  | "togglePreviewPane" | "swapBrowsers" | "notifications" | "search" | "terminal" | "terminalWith" | "edit" | "editWith" | "openWithSettings" | "undo" | "reloadTheme"
  | "settings" | "layoutNext" | "tabFromClipboard" | "contextMenu"
  | "quickOpen" | "commandPalette" | "grepFolder" | "grepRoot" | "keyHelp" | "togglePin"
  // List motions (used by vim keys; not bound to chords by default)
  | "cursorDown" | "cursorUp" | "cursorTop" | "cursorBottom" | "halfPageDown" | "halfPageUp"
  | "enter" | "filter" | "selectDown" | "goHome" | "goRoot"
  | "tab1" | "tab2" | "tab3" | "tab4" | "tab5" | "tab6" | "tab7" | "tab8" | "tab9"
  | "focusLeft" | "focusRight"
  | "sortName" | "sortNameDesc" | "sortSize" | "sortSizeDesc" | "sortModified" | "sortModifiedDesc"
  | "sortExt" | "sortExtDesc" | "sortCreated" | "sortCreatedDesc"
  | "compare" | "compareClipboard" | "compareGitHead" | "compareFolders"
  // Archives and the rest of the context menu (vim / leader keys; no chords by default)
  | "extractHere" | "extractToFolder" | "extractToOther" | "compressZip" | "compressAs"
  | "reveal" | "viewNext" | "toggleFoldersOnTop" | "visualMode" | "folderSizes";

export const DEFAULT_KEYMAP: Record<Action, string[]> = {
  open: ["Enter"],
  openWith: ["Ctrl+Enter"],
  parent: ["Backspace", "Alt+ArrowUp"],
  back: ["Alt+ArrowLeft"],
  forward: ["Alt+ArrowRight"],
  editPath: ["\\", "Alt+D", "Ctrl+L"],
  copyPath: ["Ctrl+Shift+C"],
  copyFolderPath: ["Ctrl+Shift+Alt+C"],
  rename: ["F2"],
  copyToOther: ["Alt+C"],
  moveToOther: ["Alt+M"],
  duplicate: ["Ctrl+D"],
  delete: ["Delete"],
  deletePermanent: ["Shift+Delete"],
  properties: ["Ctrl+I", "Alt+Enter"],
  tag: ["Ctrl+Alt+T"],
  longNames: ["F7"],
  newFile: ["Ctrl+N"],
  newFolder: ["Ctrl+Shift+N"],
  newNote: ["Alt+N"],
  newTab: ["Ctrl+T"],
  reopenTab: ["Ctrl+Shift+T"],
  tabFromClipboard: ["Ctrl+Shift+Alt+T"],
  closeTab: ["Ctrl+W"],
  nextTab: ["Ctrl+Tab"],
  prevTab: ["Ctrl+Shift+Tab"],
  focusOther: ["Alt+F"],
  copy: ["Ctrl+C"],
  cut: ["Ctrl+X"],
  paste: ["Ctrl+V"],
  pasteAsLinks: ["Ctrl+Shift+V"],
  preview: ["Space"],
  selectAll: ["Ctrl+A"],
  toggleSelect: ["Ctrl+Space"],
  refresh: ["F5", "Ctrl+R"],
  toggleHidden: ["Ctrl+H"],
  toggleColumns: ["`"],
  toggleSidebar: ["Alt+S"],
  togglePreviewPane: ["Alt+P"],
  // Total Commander's "swap panels".
  swapBrowsers: ["Ctrl+U"],
  notifications: ["Shift+~"],
  search: ["F3"],
  quickOpen: ["Ctrl+P", "Ctrl+E"],
  commandPalette: ["Ctrl+Shift+P", "F1"],
  grepFolder: ["Ctrl+F"],
  grepRoot: ["Ctrl+Shift+F"],
  keyHelp: [],
  togglePin: ["Ctrl+B"],
  cursorDown: [],
  cursorUp: [],
  cursorTop: [],
  cursorBottom: [],
  halfPageDown: [],
  halfPageUp: [],
  enter: [],
  filter: [],
  selectDown: [],
  goHome: [],
  goRoot: [],
  tab1: [], tab2: [], tab3: [], tab4: [], tab5: [], tab6: [], tab7: [], tab8: [], tab9: [],
  focusLeft: [], focusRight: [],
  sortName: [], sortNameDesc: [], sortSize: [], sortSizeDesc: [], sortModified: [], sortModifiedDesc: [],
  sortExt: [], sortExtDesc: [], sortCreated: [], sortCreatedDesc: [],
  compare: ["Ctrl+Shift+D"], compareClipboard: [], compareGitHead: [], compareFolders: ["Ctrl+Alt+D"],
  extractHere: [], extractToFolder: [], extractToOther: [], compressZip: [], compressAs: [],
  reveal: [], viewNext: [], toggleFoldersOnTop: [], visualMode: [],
  folderSizes: ["Ctrl+Alt+S"],
  terminal: ["F10"],
  terminalWith: ["Ctrl+F10"],
  edit: ["F4"],
  editWith: ["Shift+F4"],
  openWithSettings: [],
  undo: ["Ctrl+Z"],
  reloadTheme: ["Ctrl+Shift+F5"],
  settings: ["Ctrl+,"],
  layoutNext: ["Ctrl+Shift+L"],
  contextMenu: ["Shift+F10", "ContextMenu"],
};

export const ACTION_LABELS: Record<Action, string> = {
  open: "Open", openWith: "Open with…", parent: "Parent folder", back: "Back", forward: "Forward",
  editPath: "Edit path", copyPath: "Copy path of selection", copyFolderPath: "Copy current folder path",
  rename: "Rename", copyToOther: "Copy to other browser", moveToOther: "Move to other browser",
  duplicate: "Duplicate", delete: "Move to trash", deletePermanent: "Delete permanently",
  properties: "Properties", tag: "Colour tag", longNames: "Show long names", newFile: "New file from template",
  newFolder: "New folder", newNote: "New note", newTab: "New tab", reopenTab: "Reopen closed tab",
  tabFromClipboard: "New tab from clipboard path", closeTab: "Close tab", nextTab: "Next tab",
  prevTab: "Previous tab", focusOther: "Focus other browser", copy: "Copy", cut: "Cut", paste: "Paste",
  pasteAsLinks: "Paste as links", preview: "Preview window", selectAll: "Select all",
  toggleSelect: "Toggle selection", refresh: "Refresh", toggleHidden: "Show hidden files",
  toggleColumns: "Toggle columns / breadcrumbs", toggleSidebar: "Toggle sidebar",
  togglePreviewPane: "Toggle details pane (this browser)", swapBrowsers: "Swap folders of the two browsers", notifications: "Notifications",
  terminal: "Terminal here", terminalWith: "Terminal here with…", edit: "Edit in editor", editWith: "Edit with…", openWithSettings: "Default programs (Settings › Open with)", undo: "Undo", reloadTheme: "Reload theme", settings: "Settings",
  layoutNext: "Next layout", contextMenu: "Context menu",
  search: "Search names (wildcards, tags)",
  quickOpen: "Go to file… (Quick Open)", commandPalette: "Command palette",
  grepFolder: "Find in files: this folder", grepRoot: "Find in files: whole system",
  keyHelp: "Keyboard shortcuts", togglePin: "Pin / unpin current folder in the sidebar", cursorDown: "Cursor down", cursorUp: "Cursor up",
  cursorTop: "First item", cursorBottom: "Last item", halfPageDown: "Half page down",
  halfPageUp: "Half page up", enter: "Enter folder / open", filter: "Filter this folder",
  selectDown: "Toggle selection and move down", goHome: "Go to home folder", goRoot: "Go to file system root",
  tab1: "Go to tab 1", tab2: "Go to tab 2", tab3: "Go to tab 3", tab4: "Go to tab 4", tab5: "Go to tab 5",
  tab6: "Go to tab 6", tab7: "Go to tab 7", tab8: "Go to tab 8", tab9: "Go to tab 9",
  focusLeft: "Focus left browser", focusRight: "Focus right browser",
  sortName: "Sort by name", sortNameDesc: "Sort by name, descending", sortSize: "Sort by size",
  sortSizeDesc: "Sort by size, largest first", sortModified: "Sort by date modified, oldest first",
  sortModifiedDesc: "Sort by date modified, newest first", sortExt: "Sort by type", sortExtDesc: "Sort by type, descending",
  sortCreated: "Sort by date created, oldest first", sortCreatedDesc: "Sort by date created, newest first",
  compare: "Compare files (two selected, or with the other panel)", compareClipboard: "Compare file with clipboard",
  compareGitHead: "Compare file with Git HEAD (my changes)",
  compareFolders: "Compare the folders of both panels (sync)",
  extractHere: "Extract here", extractToFolder: "Extract each archive to its own folder",
  extractToOther: "Extract to other browser", compressZip: "Compress to .zip", compressAs: "Compress: name and format…",
  reveal: "Show in system file manager", viewNext: "Next view (details, names, icons…)",
  toggleFoldersOnTop: "Folders on top", visualMode: "Visual mode (motions extend the selection)",
  folderSizes: "Calculate folder sizes (selected, else all here)",
};

type KeyLike = { key: string; code?: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey?: boolean; getModifierState?: (k: "AltGraph") => boolean };

/** True when AltGr typed a character: on Windows AltGr arrives as Ctrl+Alt, so "\" (AltGr+Q,
 *  Slovenian), "@" (AltGr+Q, German) or "ś" (AltGr+S, Polish) would otherwise look like the
 *  shortcuts Ctrl+Alt+Q / Ctrl+Alt+S. A real Ctrl+Alt+T gives the key's own letter "t". */
export function altGrChar(e: KeyLike): boolean {
  if (e.key.length !== 1 || e.metaKey) return false;
  if (!((e.ctrlKey && e.altKey) || e.getModifierState?.("AltGraph"))) return false;
  const m = /^(?:Key|Digit)(.)$/.exec(e.code ?? "");
  return !m || e.key.toLowerCase() !== m[1].toLowerCase();
}

export const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform || navigator.userAgent || "");

/** macOS: a character typed with Option, the Mac's AltGr ("@" is Option+L, "[" Option+5
 *  on German and Slovenian layouts). It is typed text unless the chord is a shortcut
 *  (`isShortcut`: Option+C stays "Copy to other browser"). */
export function optionChar(e: KeyLike & { shiftKey?: boolean }, isShortcut: (chord: string) => boolean, mac = IS_MAC): boolean {
  if (!mac || !e.altKey || e.ctrlKey || e.metaKey || e.key.length !== 1) return false;
  const m = /^(?:Key|Digit)(.)$/.exec(e.code ?? "");
  if (m && e.key.toLowerCase() === m[1].toLowerCase()) return false;
  return !isShortcut(chordOf({ ...e, shiftKey: !!e.shiftKey }));
}

/** A key that types `e.key` into a text the app keeps itself (the quick filter): no
 *  modifier, AltGr (Windows, Linux) or Option (macOS) when no shortcut has the chord. */
export function typedChar(e: KeyLike & { shiftKey?: boolean }, isShortcut: (chord: string) => boolean, mac = IS_MAC): boolean {
  if (e.key.length !== 1) return false;
  return (!e.ctrlKey && !e.altKey && !e.metaKey) || altGrChar(e) || optionChar(e, isShortcut, mac);
}

/** Ctrl, or Cmd on macOS: the key that [`chordOf`] reads as "Ctrl". For windows that read
 *  their own keys (Ctrl+Enter, Ctrl+S, Ctrl+click), so Cmd works there as in the shortcuts. */
export function primaryMod(e: { ctrlKey: boolean; metaKey: boolean }): boolean {
  return e.ctrlKey || e.metaKey;
}

/** Normalises a keyboard event to a chord string like "Ctrl+Shift+N". */
export function chordOf(ev: KeyLike & { shiftKey: boolean }): string {
  // A character typed with AltGr is that character, without the Ctrl and Alt it came with.
  const e = altGrChar(ev) ? { ...ev, key: ev.key, code: undefined, ctrlKey: false, altKey: false } : ev;
  let key = e.key;
  if (key === " ") key = "Space";
  // With Alt held, e.key may be a composed character; fall back to the physical key.
  if (e.altKey && e.code && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  if (key.length === 1) key = key.toUpperCase();
  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("Ctrl");
  // Shift is implicit in shifted symbols like "~", except when other modifiers are present.
  if (e.shiftKey && (key.length > 1 || /[A-Z0-9]/.test(key) || mods.length || e.altKey || key === "~")) mods.push("Shift");
  if (e.altKey) mods.push("Alt");
  if (["Control", "Shift", "Alt", "Meta"].includes(key)) return mods.join("+");
  return [...mods, key].join("+");
}

export function normalizeChord(c: string): string {
  const parts = c.split("+").map((p) => p.trim()).filter(Boolean);
  const order = ["Ctrl", "Shift", "Alt"];
  const mods = order.filter((m) => parts.some((p) => p.toLowerCase() === m.toLowerCase()));
  const key = parts.filter((p) => !order.some((m) => m.toLowerCase() === p.toLowerCase())).pop() ?? "";
  return [...mods, key.length === 1 ? key.toUpperCase() : key].join("+");
}

export type Keymap = Map<string, Action>;

/** Builds chord → action lookup. Overrides replace all default chords of that action. */
export function buildKeymap(overrides: Record<string, string>): Keymap {
  const m: Keymap = new Map();
  const user = new Set<string>();
  for (const [action, chords] of Object.entries(DEFAULT_KEYMAP) as [Action, string[]][]) {
    const o = overrides[action];
    const list = o !== undefined ? o.split(",").map((s) => s.trim()).filter(Boolean) : chords;
    for (const c of list) {
      const k = normalizeChord(c);
      // A user-chosen chord always beats a default that happens to use the same keys.
      if (o === undefined && user.has(k)) continue;
      if (o !== undefined) user.add(k);
      m.set(k, action);
    }
  }
  return m;
}
