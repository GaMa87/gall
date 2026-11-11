// Vim-style keys for the file list, plus a LazyVim-like leader menu.
//
// Normal-mode keys work on the focused folder pane (j/k, gg/G, yy/dd/p …).
// The leader key (Space by default) opens a which-key menu of grouped
// commands. Sequences are resolved here as pure data so they are testable;
// App.tsx feeds key tokens in and runs the resulting action.
import { altGrChar, buildKeymap, normalizeChord, optionChar, type Action, type Keymap } from "./keymap";

export interface KeyNode {
  label: string;
  action?: Action;
  children?: Map<string, KeyNode>;
  /** The next key names a folder mark (m{letter}, '{letter} …) instead of a child. */
  mark?: MarkOp;
}

type Spec = [keys: string, label: string, action?: Action];

/** What a folder-mark key does with the letter typed after it. */
export type MarkOp = "set" | "jump" | "jumpItem" | "delete";

/** Key of the automatic mark that holds the folder left by the last jump ('' / ``). */
export const PREVIOUS_MARK = "'";

/** Vim folder marks: m{a-zA-Z} sets, '{x} goes there, `{x} also selects the marked item,
 *  '' / `` go back to the folder before the last jump, dm{x} deletes a mark. */
export const MARK_KEYS: [keys: string, label: string, op: MarkOp][] = [
  ["m", "Mark this folder", "set"],
  ["'", "Go to mark", "jump"],
  ["`", "Go to mark, select the marked item", "jumpItem"],
  ["dm", "Delete mark", "delete"],
];

/** The mark key a typed letter stands for, or null when `op` does not take it. */
export function markKey(op: MarkOp, tok: string): string | null {
  if (/^[a-z]$/i.test(tok)) return tok;
  if ((op === "jump" || op === "jumpItem") && (tok === "'" || tok === "`")) return PREVIOUS_MARK;
  return null;
}

/** Keys typed directly on the file list (no leader). Conventions follow Vim, yazi/ranger
 *  (file managers), lazygit ([ ] tabs, / filter) and LazyVim (leader groups). */
export const NORMAL_KEYS: Spec[] = [
  ["j", "Down", "cursorDown"],
  ["k", "Up", "cursorUp"],
  ["h", "Parent folder", "parent"],
  ["l", "Enter folder / open", "enter"],
  ["-", "Parent folder", "parent"],
  ["g", "+go"],
  ["gg", "First item", "cursorTop"],
  ["gr", "Git graph / switch revision", "gitGraph"],
  ["gh", "Home folder", "goHome"],
  ["g/", "File system root", "goRoot"],
  ["gp", "Edit path", "editPath"],
  ["gt", "Next tab", "nextTab"],
  ["gT", "Previous tab", "prevTab"],
  ["G", "Last item", "cursorBottom"],
  ["Ctrl+d", "Half page down", "halfPageDown"],
  ["Ctrl+u", "Half page up", "halfPageUp"],
  ["H", "Back", "back"],
  ["L", "Forward", "forward"],
  ["/", "Filter this folder", "filter"],
  ["v", "Visual mode: j/k select, then y/d/D/c…; Esc ends", "visualMode"],
  ["s", "Toggle selection, move down", "selectDown"],
  ["V", "Select all", "selectAll"],
  ["y", "+yank"],
  ["yy", "Copy", "copy"],
  ["yp", "Copy path", "copyPath"],
  ["yP", "Copy folder path", "copyFolderPath"],
  ["yd", "Duplicate", "duplicate"],
  ["d", "+cut"],
  ["dd", "Cut", "cut"],
  ["dD", "Delete permanently", "deletePermanent"],
  ["z", "+zip (archives)"],
  ["zx", "Extract here", "extractHere"],
  ["zf", "Extract to own folder", "extractToFolder"],
  ["zo", "Extract to other browser", "extractToOther"],
  ["zz", "Compress to .zip", "compressZip"],
  ["zc", "Compress: name and format…", "compressAs"],
  ["N", "New note", "newNote"],
  ["gs", "Show in system file manager", "reveal"],
  ["gv", "Next view", "viewNext"],
  ["x", "Cut", "cut"],
  ["p", "Paste", "paste"],
  ["P", "Paste as links", "pasteAsLinks"],
  ["D", "Move to trash", "delete"],
  ["r", "Rename", "rename"],
  ["a", "New file", "newFile"],
  ["A", "New folder", "newFolder"],
  ["o", "Open", "open"],
  ["e", "Edit in editor", "edit"],
  ["O", "Open with…", "openWith"],
  ["i", "Preview", "preview"],
  ["I", "Properties", "properties"],
  ["C", "Copy to other browser", "copyToOther"],
  ["M", "Move to other browser", "moveToOther"],
  // m is Vim's mark key (MARK_KEYS); c for colour.
  ["c", "Colour tag", "tag"],
  ["b", "Pin / unpin folder in the sidebar", "togglePin"],
  ["f", "Search names / tags", "search"],
  ["F", "Find in files (this folder)", "grepFolder"],
  [".", "Show hidden files", "toggleHidden"],
  ["u", "Undo", "undo"],
  ["t", "New tab", "newTab"],
  ["q", "Close tab", "closeTab"],
  ["X", "Reopen closed tab", "reopenTab"],
  ["[", "Previous tab", "prevTab"],
  ["]", "Next tab", "nextTab"],
  ...([1, 2, 3, 4, 5, 6, 7, 8, 9] as const).map((n): Spec => [String(n), `Tab ${n}`, `tab${n}`]),
  ["Tab", "Other browser", "focusOther"],
  ["Ctrl+h", "Left browser", "focusLeft"],
  ["Ctrl+l", "Right browser", "focusRight"],
  ["Ctrl+w", "+window"],
  ["Ctrl+w w", "Other browser", "focusOther"],
  ["Ctrl+w h", "Left browser", "focusLeft"],
  ["Ctrl+w l", "Right browser", "focusRight"],
  // Ctrl+U (swap) is vim's half page up; Ctrl+w x is vim's own "exchange windows".
  ["Ctrl+w x", "Swap folders of the two browsers", "swapBrowsers"],
  [",", "+sort"],
  [",n", "Name ↑", "sortName"],
  [",N", "Name ↓", "sortNameDesc"],
  [",s", "Size ↑", "sortSize"],
  [",S", "Size ↓", "sortSizeDesc"],
  [",m", "Modified ↑", "sortModified"],
  [",M", "Modified ↓", "sortModifiedDesc"],
  [",e", "Type ↑", "sortExt"],
  [",E", "Type ↓", "sortExtDesc"],
  [",c", "Created ↑", "sortCreated"],
  [",C", "Created ↓", "sortCreatedDesc"],
  ["=", "+compare"],
  ["==", "Compare selection / other panel", "compare"],
  ["=h", "Compare with Git HEAD", "compareGitHead"],
  ["=g", "Compare with a Git commit", "compareGitCommit"],
  ["=c", "Compare with clipboard", "compareClipboard"],
  ["=f", "Compare panel folders", "compareFolders"],
  ["+", "+windows"],
  ["++", "Other browser", "focusOther"],
  ["+h", "Left browser", "focusLeft"],
  ["+l", "Right browser", "focusRight"],
  ["+s", "Swap folders", "swapBrowsers"],
  ["R", "Refresh", "refresh"],
  ["!", "Terminal here", "terminal"],
  [":", "Command palette", "commandPalette"],
  ["?", "Keyboard shortcuts", "keyHelp"],
];

/** Leader menu, written relative to the leader key (LazyVim layout where it applies). */
export const LEADER_KEYS: Spec[] = [
  ["Space", "Go to file", "quickOpen"],
  ["/", "Grep this folder", "grepFolder"],
  [":", "Commands", "commandPalette"],
  ["e", "Toggle sidebar", "toggleSidebar"],
  ["*", "Pin / unpin folder", "togglePin"],
  ["?", "Keyboard shortcuts", "keyHelp"],
  ["v", "Toggle selection", "toggleSelect"],
  ["o", "+open"],
  ["oo", "Open with…", "openWith"],
  ["ot", "Terminal here with…", "terminalWith"],
  ["oe", "Edit in editor", "edit"],
  ["oE", "Edit with…", "editWith"],
  ["od", "Default programs", "openWithSettings"],
  ["om", "Context menu", "contextMenu"],
  ["f", "+file/find"],
  ["ff", "Go to file", "quickOpen"],
  ["fn", "New file", "newFile"],
  ["fN", "New folder", "newFolder"],
  ["fm", "New note (memo)", "newNote"],
  ["fd", "Duplicate", "duplicate"],
  ["fs", "Show in system file manager", "reveal"],
  ["fz", "Folder sizes", "folderSizes"],
  ["a", "+archive"],
  ["ax", "Extract here", "extractHere"],
  ["af", "Extract to own folder", "extractToFolder"],
  ["ao", "Extract to other browser", "extractToOther"],
  ["az", "Compress to .zip", "compressZip"],
  ["ac", "Compress: name and format…", "compressAs"],
  ["fc", "Settings", "settings"],
  ["ft", "Terminal here", "terminal"],
  ["s", "+search"],
  ["sg", "Grep this folder", "grepFolder"],
  ["sG", "Grep whole system", "grepRoot"],
  ["ss", "Search names / tags", "search"],
  ["sk", "Keymaps", "keyHelp"],
  ["sc", "Commands", "commandPalette"],
  ["sn", "Notifications", "notifications"],
  ["d", "+diff"],
  ["dd", "Compare selection / other panel", "compare"],
  ["dc", "Compare with clipboard", "compareClipboard"],
  ["wr", "Git graph / switch revision", "gitGraph"],
  ["dg", "Compare with Git HEAD", "compareGitHead"],
  ["dh", "Compare with a Git commit", "compareGitCommit"],
  ["df", "Compare panel folders", "compareFolders"],
  ["b", "+buffers (tabs)"],
  ["bd", "Close tab", "closeTab"],
  ["bb", "Previous tab", "prevTab"],
  ["bp", "Pin / unpin folder", "togglePin"],
  ["t", "+tabs"],
  ["tn", "New tab", "newTab"],
  ["tc", "Close tab", "closeTab"],
  ["tr", "Reopen closed", "reopenTab"],
  ["tl", "Next tab", "nextTab"],
  ["th", "Previous tab", "prevTab"],
  ["tp", "Tab from clipboard path", "tabFromClipboard"],
  ["w", "+windows"],
  ["ww", "Other browser", "focusOther"],
  ["wh", "Left browser", "focusLeft"],
  ["wl", "Right browser", "focusRight"],
  ["wn", "Next layout", "layoutNext"],
  ["wd", "Details pane", "togglePreviewPane"],
  ["ws", "Swap folders", "swapBrowsers"],
  ["wc", "Columns / breadcrumbs", "toggleColumns"],
  ["c", "+copy"],
  ["cp", "Copy path", "copyPath"],
  ["cf", "Copy folder path", "copyFolderPath"],
  ["co", "Copy to other browser", "copyToOther"],
  ["cm", "Move to other browser", "moveToOther"],
  ["g", "+go"],
  ["gh", "Home", "goHome"],
  ["gr", "Root", "goRoot"],
  ["gp", "Edit path", "editPath"],
  ["gb", "Back", "back"],
  ["gf", "Forward", "forward"],
  ["u", "+ui"],
  ["uh", "Hidden files", "toggleHidden"],
  ["un", "Long names", "longNames"],
  ["uv", "Next view", "viewNext"],
  ["uf", "Folders on top", "toggleFoldersOnTop"],
  ["ur", "Reload theme", "reloadTheme"],
  ["us", "Settings", "settings"],
  ...NORMAL_KEYS.filter(([keys]) => keys.startsWith(",")),
];

/** Internal token for a tap of the Ctrl key (used as a leader). */
export const CTRL_TAP = "CtrlTap";

/** Settings value → leader token: "Ctrl" / "Ctrl Ctrl" (tap / double tap) → CTRL_TAP. */
export function leaderToken(setting: string): string {
  return setting === "Ctrl" || setting === "Ctrl Ctrl" ? CTRL_TAP : setting;
}

/** Splits "gT" / "Ctrl+d" / "Space" style specs into key tokens. */
export function splitKeys(spec: string): string[] {
  if (spec.includes(" ")) return spec.split(" ");
  if (/^(Ctrl\+.|Space|Tab|CtrlTap)$/.test(spec)) return [spec];
  return [...spec];
}

const VIM_FIRST = new Set([...NORMAL_KEYS, ...MARK_KEYS].map(([k]) => splitKeys(k)[0]));

/** A chord shortcut that vim keys take over, so it never reaches the shortcut map while
 *  vim keys are on: `Ctrl+H` (`Ctrl+h` = left browser) or a plain character a vim key or
 *  prefix starts with (`` ` `` = go to mark, `.` = hidden files). */
export function shadowedByVim(chord: string): boolean {
  const ctrl = /^Ctrl\+(.)$/i.exec(chord);
  if (ctrl) return VIM_FIRST.has(`Ctrl+${ctrl[1].toLowerCase()}`);
  const shifted = /^Shift\+([a-z])$/i.exec(chord);
  if (shifted) return VIM_FIRST.has(shifted[1].toUpperCase());
  if (chord.length !== 1) return false;
  return VIM_FIRST.has(/[a-z]/i.test(chord) ? chord.toLowerCase() : chord);
}

/** The physical letter of a key (`KeyA` → `a`, with Shift `A`), for a mark name typed on a
 *  layout that composes it with a dead key before it (`'` then `a` → `á`). */
export function physicalLetter(e: { code?: string; shiftKey?: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }): string | null {
  const m = /^Key([A-Z])$/.exec(e.code ?? "");
  if (!m || e.ctrlKey || e.metaKey || e.altKey) return null;
  return e.shiftKey ? m[1] : m[1].toLowerCase();
}

export interface KeyLabelCtx {
  vim: boolean;
  keymap: Keymap;
  /** Leader setting as shown ("Space", "\\", "Ctrl Ctrl", "" = none). */
  leader: string;
}

const showSpec = (k: string) => showKeys(splitKeys(k));
let defaultKeymap: Keymap | undefined;

/** A chord that never reaches its action now: vim keys took it over, or it is the leader
 *  key (which opens the leader menu instead). */
export function shadowed(chord: string, c: KeyLabelCtx): boolean {
  if (c.vim && shadowedByVim(chord)) return true;
  const leader = leaderToken(c.leader);
  return !!leader && leader !== CTRL_TAP && normalizeChord(chord) === normalizeChord(leader);
}

/** The keys that run `action` in the current mode: vim's own key first when vim keys are
 *  on, then a chord vim leaves alone, then the leader menu. */
export function keysForAction(action: Action, c: KeyLabelCtx): string | null {
  if (c.vim) {
    const n = NORMAL_KEYS.find(([, , a]) => a === action);
    if (n) return showSpec(n[0]);
  }
  for (const [chord, a] of c.keymap) if (a === action && !shadowed(chord, c)) return chord;
  const l = c.leader && LEADER_KEYS.find(([, , a]) => a === action);
  if (l) return `${c.leader === "Space" ? "␣" : c.leader} ${showSpec(l[0])}`;
  return null;
}

/** What a menu should print next to an item written with a default chord (`Ctrl+H`):
 *  the keys that really do it now (`.` with vim keys, a user's own chord), or nothing. */
export function shortcutLabel(chord: string, c: KeyLabelCtx): string | null {
  // Menus are written with the default chords; the user may have rebound them.
  const k = normalizeChord(chord);
  const a = (defaultKeymap ??= buildKeymap({}, "vim")).get(k);
  if (a) {
    // The written chord still does it and vim has no key of its own: keep it.
    const vimOwn = c.vim && NORMAL_KEYS.some(([, , x]) => x === a);
    if (!vimOwn && c.keymap.get(k) === a && !shadowed(chord, c)) return chord;
    return keysForAction(a, c);
  }
  return shadowed(chord, c) ? null : chord;
}

function build(specs: Spec[], leader?: string): KeyNode {
  const root: KeyNode = { label: "", children: new Map() };
  for (const [keys, label, action] of specs) {
    const toks = leader ? [leader, ...splitKeys(keys)] : splitKeys(keys);
    let n = root;
    toks.forEach((t, i) => {
      if (!n.children) n.children = new Map();
      let c = n.children.get(t);
      if (!c) n.children.set(t, (c = { label: "" }));
      if (i === toks.length - 1) {
        c.label = label;
        c.action = action;
      }
      n = c;
    });
  }
  return root;
}

/** Builds the full key tree for a leader token (e.g. "Space", ";" or CTRL_TAP). */
export function buildVimTree(leader: string): KeyNode {
  const tree = build(NORMAL_KEYS);
  for (const [keys, label, op] of MARK_KEYS) {
    let n = tree;
    for (const t of splitKeys(keys)) {
      let c = n.children?.get(t);
      if (!c) (n.children ??= new Map()).set(t, (c = { label: "" }));
      n = c;
    }
    n.label = label;
    n.mark = op;
  }
  if (leader) tree.children!.set(leader, leaderNode(leader));
  return tree;
}

/** Leader menu only (vim keys off): letters keep filtering until the leader is pressed. */
export function buildLeaderTree(leader: string): KeyNode {
  const tree: KeyNode = { label: "", children: new Map() };
  if (leader) tree.children!.set(leader, leaderNode(leader));
  return tree;
}

function leaderNode(leader: string): KeyNode {
  const n = build(LEADER_KEYS, leader).children!.get(leader)!;
  n.label = "+leader";
  return n;
}

/** The active tree for the current settings. */
export function treeForSettings(s: { vimKeys: boolean; leaderKey: string; leaderKeyBasic: string }): KeyNode {
  return s.vimKeys ? buildVimTree(leaderToken(s.leaderKey || "Space")) : buildLeaderTree(leaderToken(s.leaderKeyBasic ?? "Ctrl Ctrl"));
}

export type Resolved =
  | { kind: "action"; action: Action }
  | { kind: "pending"; node: KeyNode }
  | { kind: "mark"; op: MarkOp; key: string }
  | { kind: "none" };

export function resolve(tree: KeyNode, seq: string[]): Resolved {
  let n: KeyNode | undefined = tree;
  for (let i = 0; i < seq.length; i++) {
    if (n.mark) {
      // The key after a mark key is the mark's name and ends the sequence.
      const key = i === seq.length - 1 ? markKey(n.mark, seq[i]) : null;
      return key ? { kind: "mark", op: n.mark, key } : { kind: "none" };
    }
    n = n.children?.get(seq[i]);
    if (!n) return { kind: "none" };
  }
  if (n.children?.size || n.mark) return { kind: "pending", node: n };
  if (n.action) return { kind: "action", action: n.action };
  return { kind: "none" };
}

/** Converts a keyboard event into a vim key token; letter case follows Shift, not Caps Lock. */
export function vimToken(e: { key: string; code?: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey?: boolean; getModifierState?: (k: "AltGraph") => boolean }): string | null {
  if (["Control", "Shift", "Alt", "Meta", "CapsLock", "AltGraph"].includes(e.key)) return null;
  // "[", "~" or "\" typed with AltGr (Ctrl+Alt on Windows) or Option (macOS) are those
  // keys. An Option chord that is a shortcut (Option+C) still reaches the shortcut map:
  // its character is bound to no vim key.
  if (altGrChar(e) || optionChar(e, () => false)) return e.key;
  if (e.altKey) return null;
  if (e.ctrlKey || e.metaKey) {
    // Ctrl+Shift+letter belongs to the normal shortcuts (Ctrl+Shift+D compare,
    // Ctrl+Shift+L layout); folding it into Ctrl+d / Ctrl+l would hijack them.
    if (e.shiftKey) return null;
    return e.key.length === 1 ? `Ctrl+${e.key.toLowerCase()}` : null;
  }
  if (e.key === " ") return "Space";
  if (e.key === "Tab") return "Tab";
  // A dead key (US International ' and `, the key left of 1 on many European layouts)
  // types nothing by itself: the mark keys ' and ` follow the physical key there.
  if (e.key === "Dead" && !e.shiftKey) return e.code === "Quote" ? "'" : e.code === "Backquote" ? "`" : null;
  if (/^[a-z]$/i.test(e.key)) return e.shiftKey ? e.key.toUpperCase() : e.key.toLowerCase();
  return e.key.length === 1 ? e.key : null;
}

/** Display form of a token sequence: "Space f g", "g g", "Ctrl+d". */
export function showKeys(seq: string[]): string {
  return seq.map(showKey).join(" ");
}

export function showKey(t: string): string {
  return t === "Space" ? "␣" : t === CTRL_TAP ? "Ctrl" : t;
}

/** Flattens the tree into [keys, label] rows (for the help sheet / palette). */
export function listBindings(tree: KeyNode, prefix: string[] = []): { keys: string[]; label: string; action: Action }[] {
  const out: { keys: string[]; label: string; action: Action }[] = [];
  for (const [k, n] of tree.children ?? []) {
    const keys = [...prefix, k];
    if (n.action) out.push({ keys, label: n.label, action: n.action });
    if (n.children) out.push(...listBindings(n, keys));
  }
  return out;
}
