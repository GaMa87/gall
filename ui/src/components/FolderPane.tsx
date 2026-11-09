// Current Folder Pane (F-01…F-13, V-01, V-02): virtualised list with sorting,
// quick filter, selection (incl. rubber band), drag & drop and context menus.
import { archiveBaseName, archiveInfo, archiveOf, CREATE_FORMATS } from "../core/archive";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, fileSrc } from "../api/ipc";
import type { FileEntry, GitState, SortKey } from "../api/types";
import { age, ageColor, ageTier, ageWords, AGE_TIERS } from "../core/age";
import { hasNerdFont } from "../core/nerdFont";
import { quickFilterMatches } from "../core/filter";
import { focusAfterRemoval } from "../core/focus";
import { formatDate, formatSize, formatTime, unixMode, windowsMode } from "../core/format";
import { buildKeymap, primaryMod, typedChar, type Action } from "../core/keymap";
import { basename, parent, splitExt } from "../core/path";
import { sortEntries, type SortSpec } from "../core/sort";
import * as A from "../state/actions";
import { controller, registerController } from "../state/controllers";
import { draggedPaths, draggedSingle, dropInto, hintNoop, isFileDrag, onDragOverCompare, onDragOverTarget, startDrag } from "../state/dnd";
import { bumpNonce, useDirList, useGitStatus, useNonce, usePrefs, useSearchHidden, useSize, useTags } from "../state/hooks";
import { getState, setState, useStore, type MenuItem } from "../state/store";
import { useKeyLabels } from "../state/keyLabels";
import { FileIcon, Icon, TAG_COLORS } from "./Icon";

/** Nerd Font glyph when one is installed, otherwise the matching SVG line icon. */
function Glyph({ nf, icon }: { nf: string; icon: string }) {
  return hasNerdFont() ? <i className="nf">{nf}</i> : <Icon name={icon} size={12} style={{ opacity: 0.85 }} />;
}

const GIT_LETTER: Record<GitState, string> = { modified: "M", staged: "S", untracked: "U", conflict: "!", ignored: "" };
const GIT_TITLE: Record<GitState, (dir: boolean) => string> = {
  modified: (d) => (d ? "Git: changes inside" : "Git: modified"),
  staged: (d) => (d ? "Git: staged changes inside" : "Git: staged (added to the commit)"),
  untracked: (d) => (d ? "Git: new files inside, not added" : "Git: new, not added"),
  conflict: (d) => (d ? "Git: merge conflict inside" : "Git: merge conflict"),
  ignored: () => "Git: ignored",
};

const SORT_LABEL: Record<SortKey, string> = { name: "Name", ext: "Type", modified: "Modified", created: "Created", size: "Size" };

/** Actions that use the selection and so end vim visual mode. */
const VISUAL_ENDS = new Set<Action>([
  "copy", "cut", "delete", "deletePermanent", "copyToOther", "moveToOther", "tag", "duplicate",
  "rename", "open", "openWith", "edit", "editWith", "copyPath", "properties",
  "compressZip", "compressAs", "extractHere", "extractToFolder", "extractToOther",
]);

const IS_WINDOWS = typeof navigator !== "undefined" && /Win/i.test(navigator.platform || navigator.userAgent || "");

/** A Mode column coloured per letter like eza: the type in the accent colour, then
 *  `ls -l`'s r / w / x, or PowerShell's a / r / h / s (read-only as "w" is not there). */
const MODE_CLASS: Record<string, string> = { r: "pm-r", w: "pm-w", x: "pm-x", s: "pm-x", t: "pm-x", S: "pm-x", T: "pm-x" };
const WIN_CLASS: Record<string, string> = { a: "pm-a", r: "pm-r", h: "pm-h", s: "pm-w" };
function ModeText({ text, win }: { text: string; win?: boolean }) {
  return <>{[...text].map((c, i) => <span key={i} className={c === "-" ? "pm-none" : i === 0 ? "pm-type" : (win ? WIN_CLASS : MODE_CLASS)[c]}>{c}</span>)}</>;
}

export const VIEWS: { id: string; label: string }[] = [
  { id: "default", label: "Details" },
  { id: "names", label: "Names only" },
  // Windows: PowerShell's Get-ChildItem; Linux / macOS: `ls -l` (permissions, owner, link target).
  { id: "powershell", label: IS_WINDOWS ? "Mode · Date · Size · Name" : "Mode · Size · Owner · Date · Name" },
  { id: "thumbs", label: "Thumbnails + details" },
  { id: "grid", label: "Thumbnail grid" },
];

/** Column widths shared by the header and the rows (in `ch`, scale with font size). */
const CW = { ext: "6.2ch", age: "6.6ch", date: "10.6ch", time: "5.4ch", size: "7.8ch", mode: "6.4ch", unixMode: "11ch" } as const;

const IMG = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif"]);
/** Grid cell size: 92 leaves an 80 px thumbnail (Ctrl+wheel zooms in steps of 16). */
const GRID_DEFAULT = 92;
/** Zoom range: thumbnails from Explorer's "Medium icons" (48) to "Extra large" (256). */
const GRID_MIN = 60;
const GRID_MAX = 268;
/** Cell padding around the thumbnail (see .grid-cell). */
const GRID_PAD = 12;
/** Tiles view: thumbnail size and the slot icons are centred in. */
const TILE = 40;
const COVER_NAMES = ["cover.jpg", "folder.jpg", "front.jpg", "background.jpg", "poster.jpg"];

interface Props {
  b: number;
  path: string;
  active: boolean;
  /** Narrow mode (last column in Columns layout): fewer columns. */
  compact?: boolean;
}

export function FolderPane({ b, path, active, compact }: Props) {
  const settings = useStore((s) => s.settings)!;
  // Chords that are shortcuts: an Option character (macOS) typed with one is not text.
  const keymap = useMemo(() => buildKeymap(settings.shortcuts), [settings.shortcuts]);
  const isShortcut = (chord: string) => keymap.has(chord);
  // Git status badges and the branch (Settings › Files › Git status).
  const git = useGitStatus(path, settings.showGitStatus);
  // Items a .gallignore hides from search are dimmed (Hide from search / Show in search again).
  const searchHidden = useSearchHidden(path);
  // Git work tree of this folder (enables "Compare with Git HEAD").
  const [gitRoot, setGitRoot] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.gitRoot(path).then((r) => alive && setGitRoot(r)).catch(() => alive && setGitRoot(null));
    return () => { alive = false; };
  }, [path]);
  // Per-panel undo: the latest operation that touched this folder.
  const undoRev = useStore((s) => s.undoRev);
  const [undoHere, setUndoHere] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.undoPeekIn(path).then((d) => alive && setUndoHere(d)).catch(() => alive && setUndoHere(null));
    return () => { alive = false; };
  }, [path, undoRev]);
  const pinned = useStore((s) => s.favorites.groups.some((g) => g.items.some((f) => A.resolveSpecial(f.path) === path)));
  const pinKeys = useKeyLabels().action("togglePin");
  const longNames = useStore((s) => s.longNames);
  const clipboard = useStore((s) => s.clipboard);
  const nonce = useNonce();
  const dir = useDirList(path, settings);
  // An encrypted archive asks for its password as soon as it is opened (once per error:
  // a wrong password brings a new one; after Cancel the button in the message remains).
  const askedFor = useRef<string | null>(null);
  useEffect(() => {
    const err = dir.error;
    if (!active || !err?.includes("archive-password:") || askedFor.current === err) return;
    askedFor.current = err;
    A.askArchivePassword(err).then((ok) => {
      if (ok) {
        askedFor.current = null;
        dir.reload();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir.error, active]);
  // The folder vanished (deleted or renamed elsewhere, its drive gone): go up to the
  // nearest folder that exists instead of showing an error for a place that is gone.
  useEffect(() => {
    if (dir.error && A.isMissingError(dir.error) && !archiveOf(path)) void A.leaveMissing(b, path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir.error]);
  const [prefs, reloadPrefs] = usePrefs(path, nonce);
  const tags = useTags(path, nonce);
  const [filter, setFilter] = useState("");
  // Vim mode: letters are commands, so typing filters only after "/".
  const [filterMode, setFilterMode] = useState(false);
  const [visual, setVisual] = useState(false);
  useEffect(() => setVisual(false), [path]); // a new folder starts in normal mode
  // Vim "s" marks items one by one: while marking, j/k move the cursor without replacing
  // the selection (as in ranger / yazi). Esc, a click or another folder ends it.
  const [marking, setMarking] = useState(false);
  useEffect(() => setMarking(false), [path]);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [focus, setFocus] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [gridSize, setGridSize] = useState(GRID_DEFAULT);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [rubber, setRubber] = useState<{ x0: number; y0: number; x1: number; y1: number; base: Set<string> } | null>(null);
  const [excerpts, setExcerpts] = useState<Map<string, string | null>>(new Map());
  const [listRef, size] = useSize<HTMLDivElement>();
  const paneRef = useRef<HTMLDivElement>(null);
  const pendingSelect = useRef<{ paths: string[]; focus?: string } | null>(null);
  const lastClick = useRef<{ path: string; at: number } | null>(null);

  // Folders not remembering their own sort / view: this browser's own, in every folder.
  const remember = settings.rememberFolderViews;
  const panelView = useStore((s) => s.session?.browsers[b]?.view);
  const own = remember ? null : panelView;
  const view = own?.view ?? prefs?.view ?? settings.defaultView;
  const spec: SortSpec = {
    key: own?.sortKey ?? prefs?.sortKey ?? settings.defaultSort,
    descending: own?.descending ?? prefs?.descending ?? settings.defaultDescending,
    foldersOnTop: own?.foldersOnTop ?? prefs?.foldersOnTop ?? settings.foldersOnTop,
    foldersByName: settings.foldersByName,
  };
  // Turned off: each browser keeps what it shows now instead of jumping to the defaults.
  const shown = useRef({ view, spec, remember });
  useEffect(() => {
    const was = shown.current;
    if (was.remember && !remember) {
      A.sessionCmd({ op: "view", b, prefs: { view: was.view, sortKey: was.spec.key, descending: was.spec.descending, foldersOnTop: was.spec.foldersOnTop } });
    }
  }, [remember, b]);
  shown.current = { view, spec, remember };

  // Calculated folder sizes (F-14). Sorting by size uses finished ones only, so rows do
  // not jump around while folders are being counted.
  const folderSizes = useStore((s) => s.folderSizes);
  const doneSizes = spec.key === "size"
    ? Object.entries(folderSizes).filter(([, f]) => f.done && !f.virtualFs).map(([p, f]) => `${p}:${f.bytes}`).join("|")
    : "";
  const visible = useMemo(() => {
    const filtered = filter ? dir.entries.filter((e) => quickFilterMatches(e.name, filter)) : dir.entries;
    const sizeOf = (p: string) => A.folderSizeBytes(getState().folderSizes[p]);
    return sortEntries(filtered, spec, spec.key === "size" ? sizeOf : undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir.entries, filter, spec.key, spec.descending, spec.foldersOnTop, spec.foldersByName, doneSizes]);

  // Folder sizes: stop this browser's calculation when it leaves the folder; with
  // "calculate automatically" count every subfolder once the listing is in.
  useEffect(() => () => A.cancelFolderSizes(b), [path, b]);
  useEffect(() => {
    if (!settings.folderSizesAuto || dir.loading || archiveOf(path)) return;
    const dirs = dir.entries.filter((e) => e.isDir && !e.isSymlink).map((e) => e.path);
    if (dirs.length) A.calcFolderSizes(b, dirs, 120_000);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.folderSizesAuto, dir.loading, dir.entries, path]);
  const dirCount = useMemo(() => visible.reduce((n, e) => n + (e.isDir ? 1 : 0), 0), [visible]);

  const indexOf = useMemo(() => new Map(visible.map((e, i) => [e.path, i])), [visible]);

  // Reset per-folder UI state when the folder changes.
  useEffect(() => {
    setFilter("");
    setFilterMode(false);
    setSel(new Set());
    setFocus(null);
    setAnchor(null);
    setScrollTop(0);
    if (listRef.current) listRef.current.scrollTop = 0;
    setExcerpts(new Map());
  }, [path, listRef]);

  // Apply selections requested before the listing contained the new item.
  useEffect(() => {
    const p = pendingSelect.current;
    if (!p) return;
    const present = p.paths.filter((x) => indexOf.has(x));
    if (present.length) {
      setSel(new Set(present));
      setFocus(p.focus ?? present[0]);
      setAnchor(p.focus ?? present[0]);
      pendingSelect.current = null;
      scrollIntoView(p.focus ?? present[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexOf]);

  // Drop selection entries that vanished.
  useEffect(() => {
    if (!sel.size) return;
    const next = new Set([...sel].filter((p) => indexOf.has(p)));
    if (next.size !== sel.size) setSel(next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexOf]);

  // The focused item vanished (trashed, moved, renamed elsewhere): the cursor goes to the
  // item that took its place, selected when nothing selected is left (as in Explorer),
  // instead of being lost (the next arrow key started again at the top, Delete did nothing).
  const lastOrder = useRef<{ path: string; order: string[] }>({ path, order: [] });
  useEffect(() => {
    const prev = lastOrder.current;
    lastOrder.current = { path, order: visible.map((e) => e.path) };
    if (!focus || indexOf.has(focus) || prev.path !== path || !visible.length) return;
    // Only hidden by the quick filter, not gone: the filter places the cursor itself.
    if (dir.entries.some((e) => e.path === focus)) return;
    const next = focusAfterRemoval(prev.order, focus, (p) => indexOf.has(p));
    if (!next) return;
    setFocus(next);
    setAnchor(next);
    if (![...sel].some((p) => indexOf.has(p))) setSel(new Set([next]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexOf]);

  // ───────── geometry ─────────
  const rowH = Math.round(settings.fontSize * 1.45 + settings.rowPadding * 2);
  const isGrid = view === "grid";
  const isThumbs = view === "thumbs";
  // Tiles: as tall as the 40 px slot or the two text lines (name, details), whichever is more.
  const itemH = isGrid ? gridSize + 26
    : isThumbs ? Math.max(TILE + 4, Math.round(settings.fontSize * 2.35 + settings.rowPadding * 2))
    : rowH;
  const cols = isGrid ? Math.max(1, Math.floor((size.w - 8) / (gridSize + 8))) : 1;
  const cellW = isGrid ? Math.floor((size.w - 8) / cols) : size.w;
  const rows = Math.ceil(visible.length / cols);
  const totalH = rows * itemH + 8;
  const firstRow = Math.max(0, Math.floor(scrollTop / itemH) - 4);
  const lastRow = Math.min(rows, Math.ceil((scrollTop + size.h) / itemH) + 4);

  const posOf = (i: number) => ({ top: Math.floor(i / cols) * itemH, left: isGrid ? 4 + (i % cols) * cellW : 0 });

  const scrollIntoView = useCallback(
    (p: string) => {
      const i = indexOf.get(p);
      const el = listRef.current;
      if (i == null || !el) return;
      const top = Math.floor(i / cols) * itemH;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + itemH > el.scrollTop + el.clientHeight) el.scrollTop = top + itemH - el.clientHeight;
    },
    [indexOf, cols, itemH, listRef],
  );

  // Excerpts for thumbs view.
  useEffect(() => {
    if (!isThumbs) return;
    const want = visible
      .slice(firstRow * cols, lastRow * cols)
      .filter((e) => !e.isDir && !e.cloudOnly && settings.excerptExts.includes(e.ext) && !excerpts.has(e.path))
      .map((e) => e.path);
    if (!want.length) return;
    // A failed request counts as "no excerpt" for these files: otherwise it was an
    // unhandled rejection, asked again on every scroll step.
    api.excerpts(want, settings.excerptLines).catch(() => want.map(() => null)).then((r) => {
      setExcerpts((m) => {
        const n = new Map(m);
        want.forEach((p, i) => n.set(p, r[i] ?? null));
        return n;
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isThumbs, firstRow, lastRow, visible]);

  // ───────── selection helpers ─────────
  const selection = useCallback((): FileEntry[] => {
    if (sel.size) return visible.filter((e) => sel.has(e.path));
    const f = focus ? visible[indexOf.get(focus) ?? -1] : undefined;
    return f ? [f] : [];
  }, [sel, focus, visible, indexOf]);

  const focusedEntry = focus ? visible[indexOf.get(focus) ?? -1] ?? null : null;

  // Share focused entry with the details pane.
  useEffect(() => {
    setState((s) => {
      const focused = [...s.focused];
      focused[b] = focusedEntry;
      return { focused };
    });
  }, [focusedEntry, b]);

  const moveFocus = (to: number, extend: boolean, keepSel: boolean) => {
    if (!visible.length) return;
    const i = Math.max(0, Math.min(visible.length - 1, to));
    const p = visible[i].path;
    setFocus(p);
    if (extend) {
      const a = indexOf.get(anchor ?? p) ?? i;
      const [lo, hi] = a < i ? [a, i] : [i, a];
      setSel(new Set(visible.slice(lo, hi + 1).map((e) => e.path)));
    } else if (!keepSel) {
      setSel(new Set([p]));
      setAnchor(p);
    }
    scrollIntoView(p);
  };

  const clickRow = (e: React.MouseEvent, entry: FileEntry) => {
    paneRef.current?.focus();
    A.focusBrowser(b);
    const p = entry.path;
    if (e.ctrlKey || e.metaKey) {
      const n = new Set(sel);
      n.has(p) ? n.delete(p) : n.add(p);
      setSel(n);
      setFocus(p);
      setAnchor(p);
      return;
    }
    if (e.shiftKey && anchor) {
      const a = indexOf.get(anchor) ?? 0;
      const i = indexOf.get(p) ?? 0;
      const [lo, hi] = a < i ? [a, i] : [i, a];
      setSel(new Set(visible.slice(lo, hi + 1).map((x) => x.path)));
      setFocus(p);
      return;
    }
    // Slow second click renames (optional setting).
    const lc = lastClick.current;
    if (settings.slowClickRename && lc && lc.path === p && sel.has(p) && sel.size === 1 && Date.now() - lc.at > 600 && Date.now() - lc.at < 1600) {
      lastClick.current = null;
      A.renameEntry(b, entry);
      return;
    }
    lastClick.current = { path: p, at: Date.now() };
    setMarking(false);
    setSel(new Set([p]));
    setFocus(p);
    setAnchor(p);
    if (settings.singleClickOpen) A.openEntries(b, [entry]);
  };

  // ───────── prefs changes ─────────
  // "Remember view and sort per folder" off: a change here is this browser's, in every folder.
  const setSort = (key: SortKey, descending: boolean) => {
    if (!remember) { A.sessionCmd({ op: "view", b, prefs: { sortKey: key, descending } }); return; }
    api.setPrefs(path, { sortKey: key, descending }).then(reloadPrefs).catch(A.fail);
  };
  const toggleFoldersOnTop = () => {
    if (!remember) { A.sessionCmd({ op: "view", b, prefs: { foldersOnTop: !spec.foldersOnTop } }); return; }
    api.setPrefs(path, { foldersOnTop: !spec.foldersOnTop }).then(reloadPrefs).catch(A.fail);
  };
  const setView = (v: string) => {
    if (!remember) { A.sessionCmd({ op: "view", b, prefs: { view: v } }); return; }
    api.setPrefs(path, { view: v }).then(() => bumpNonce()).catch(A.fail);
  };

  // ───────── actions handled by this pane ─────────
  // Vim visual mode (v): motions extend the selection from where it started, until an
  // action uses the selection (then it ends, like in Vim) or Esc / v cancels it.
  const handle = (a: Action): boolean => {
    if (a === "visualMode") {
      if (visual) {
        setVisual(false);
      } else if (focus) {
        setVisual(true);
        setMarking(false);
        setAnchor(focus);
        setSel(new Set([focus]));
      }
      return true;
    }
    const done = handleAction(a);
    if (done && visual && VISUAL_ENDS.has(a)) {
      setVisual(false);
      setSel(new Set());
    }
    return done;
  };
  const handleAction = (a: Action): boolean => {
    const s = selection();
    switch (a) {
      case "open":
        A.openEntries(b, s);
        return true;
      case "rename":
        if (s.length === 1) A.renameEntry(b, s[0]);
        else if (s.length > 1) A.batchRename(b, s);
        else if (s.length > 1) A.notify("Batch rename is planned; rename items one at a time for now.", "warn");
        return true;
      case "delete":
        A.trashPaths(s.map((e) => e.path));
        return true;
      case "deletePermanent":
        A.deletePermanently(s.map((e) => e.path));
        return true;
      case "folderSizes": {
        // The selected folders, else every folder here.
        const picked = s.filter((e) => e.isDir);
        const dirs = (picked.length ? picked : visible.filter((e) => e.isDir && !e.isSymlink)).map((e) => e.path);
        if (dirs.length) A.calcFolderSizes(b, dirs);
        else A.notify("No folders here", "info");
        return true;
      }
      case "duplicate":
        if (A.refuseInArchive(s.map((e) => e.path), "duplicate")) return true;
        // A background task: the copies are selected as they appear.
        if (s.length)
          api.duplicate(s.map((e) => e.path)).then((p) => { pendingSelect.current = { paths: p, focus: p[0] }; }).catch(A.fail);
        return true;
      case "copy":
        A.copyToClipboard(s, false);
        return true;
      case "cut":
        A.copyToClipboard(s, true);
        return true;
      case "copyPath":
        A.copyPaths(s, path);
        return true;
      case "copyToOther":
      case "moveToOther": {
        // Never into a browser that is not on screen (Classic, a narrow window): the
        // items would land where nobody sees them.
        const o = A.otherTarget(b);
        if (o == null) {
          A.noOtherBrowser(a === "copyToOther" ? "Copy to other browser" : "Move to other browser");
          return true;
        }
        A.transfer(a === "copyToOther" ? "copy" : "move", s.map((e) => e.path), A.activePath(o));
        return true;
      }
      case "selectAll":
        setSel(new Set(visible.map((e) => e.path)));
        return true;
      case "toggleSelect":
        if (focus) {
          const n = new Set(sel);
          n.has(focus) ? n.delete(focus) : n.add(focus);
          setSel(n);
        }
        return true;
      case "properties":
        if (s[0]) A.openOverlay({ type: "properties", entry: s[0] });
        return true;
      case "tag":
        if (s.length) A.openOverlay({ type: "tag", paths: s.map((e) => e.path) });
        return true;
      case "preview":
        if (focusedEntry) A.openOverlay({ type: "previewWindow", b });
        return true;
      case "sortName": case "sortNameDesc": case "sortSize": case "sortSizeDesc":
      case "sortModified": case "sortModifiedDesc": case "sortExt": case "sortExtDesc":
      case "sortCreated": case "sortCreatedDesc": {
        const base = a.replace(/^sort|Desc$/g, "").toLowerCase();
        setSort((base === "name" ? "name" : base === "size" ? "size" : base === "modified" ? "modified" : base === "ext" ? "ext" : "created") as SortKey, a.endsWith("Desc"));
        return true;
      }
      case "cursorDown":
      case "cursorUp":
      case "cursorTop":
      case "cursorBottom":
      case "halfPageDown":
      case "halfPageUp": {
        const i = focus ? indexOf.get(focus) ?? -1 : -1;
        const half = Math.max(1, Math.floor(size.h / itemH / 2)) * cols;
        const to =
          a === "cursorDown" ? (i < 0 ? 0 : i + cols)
          : a === "cursorUp" ? (i < 0 ? 0 : i - cols)
          : a === "cursorTop" ? 0
          : a === "cursorBottom" ? visible.length - 1
          : a === "halfPageDown" ? i + half
          : i - half;
        moveFocus(to, visual, marking && !visual);
        return true;
      }
      case "enter":
        if (focusedEntry?.isDir) A.navigate(b, focusedEntry.path);
        else if (focusedEntry) A.openEntries(b, [focusedEntry]);
        return true;
      case "filter":
        setFilterMode(true);
        return true;
      case "selectDown":
        if (focus) {
          // The first "s" starts from nothing: the item the cursor selected stays marked
          // instead of being toggled off.
          const n = marking || visual || !(sel.size === 1 && sel.has(focus)) ? new Set(sel) : new Set<string>();
          setMarking(true);
          n.has(focus) ? n.delete(focus) : n.add(focus);
          setSel(n);
          const i = indexOf.get(focus) ?? 0;
          const next = visible[Math.min(visible.length - 1, i + 1)];
          if (next) {
            setFocus(next.path);
            setAnchor(next.path);
          }
        }
        return true;
      case "openWith":
      case "edit":
      case "editWith": {
        const targets = s.length ? s : focusedEntry ? [focusedEntry] : [];
        if (!targets.length) return true;
        (a === "openWith" ? A.openWith : a === "edit" ? A.editFiles : A.editWith)(targets);
        return true;
      }
      case "extractHere":
      case "extractToFolder":
      case "extractToOther": {
        const ap = s.filter((e) => !e.isDir && archiveInfo(e.name)).map((e) => e.path);
        if (!ap.length) {
          A.notify("Select an archive to extract (zip, 7z, tar, rar, …)", "warn");
          return true;
        }
        const other = a === "extractToOther" ? A.otherTarget(b) : null;
        if (a === "extractToOther" && other == null) {
          A.noOtherBrowser("Extract to other browser");
          return true;
        }
        A.extract(ap, other != null ? A.activePath(other) : path, a === "extractToFolder" ? "folder" : "smart");
        return true;
      }
      case "compressZip":
      case "compressAs": {
        if (!s.length) return true;
        const base = archiveBaseName(s, basename(path));
        const srcs = s.map((e) => e.path);
        if (a === "compressZip") A.compress(srcs, path, base + ".zip", "zip");
        else A.compressAs(srcs, path, base);
        return true;
      }
      case "reveal":
        api.revealPath(s[0]?.path ?? path).catch(A.fail);
        return true;
      case "viewNext": {
        const i = VIEWS.findIndex((v) => v.id === view);
        const next = VIEWS[(i + 1) % VIEWS.length];
        setView(next.id);
        A.notify(`View: ${next.label}`);
        return true;
      }
      case "toggleFoldersOnTop":
        toggleFoldersOnTop();
        return true;
      case "contextMenu": {
        const r = paneRef.current?.getBoundingClientRect();
        const i = focus ? indexOf.get(focus) ?? 0 : 0;
        const pos = posOf(i);
        openContextMenu((r?.left ?? 0) + 40, (r?.top ?? 0) + pos.top - scrollTop + itemH + 60, s.length ? s : null);
        return true;
      }
      default:
        return false;
    }
  };

  // Register controller for global shortcuts.
  const ctl = useRef({ handle, selection, visible, focusedEntry, path, visual });
  ctl.current = { handle, selection, visible, focusedEntry, path, visual };
  useEffect(() => {
    registerController(b, {
      path: () => ctl.current.path,
      entries: () => ctl.current.visible,
      selection: () => ctl.current.selection(),
      focused: () => ctl.current.focusedEntry,
      reload: () => dir.reload(),
      select: (paths, f) => {
        pendingSelect.current = { paths, focus: f };
        const present = paths.filter((x) => indexOf.has(x));
        if (present.length === paths.length && present.length) {
          setSel(new Set(present));
          setFocus(f ?? present[0]);
          setAnchor(f ?? present[0]);
          pendingSelect.current = null;
          requestAnimationFrame(() => scrollIntoView(f ?? present[0]));
        }
      },
      focus: () => paneRef.current?.focus(),
      handle: (a) => ctl.current.handle(a),
      visual: () => ctl.current.visual,
    });
    return () => registerController(b, null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [b, indexOf, dir.reload]);

  // Focus pane when its browser becomes active.
  useEffect(() => {
    if (active && !getState().overlay && document.activeElement?.tagName !== "INPUT") paneRef.current?.focus();
  }, [active, path]);

  // An overlay (Quick Open, Find in files, a dialog…) removes its input when it closes and
  // focus falls to <body>, so keys (e.g. Esc to clear the quick filter) would no longer reach
  // this pane. Take focus back, unless the overlay already moved it somewhere on purpose.
  const overlayOpen = useStore((s) => !!s.overlay);
  useEffect(() => {
    if (!active || overlayOpen) return;
    const ae = document.activeElement;
    if (!ae || ae === document.body) paneRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [overlayOpen]);

  // ───────── keyboard (navigation keys and quick filter) ─────────
  const onKeyDown = (e: React.KeyboardEvent) => {
    // A pending leader/vim sequence owns the keyboard (App resolves it); don't filter.
    if (getState().keySeq.length) return;
    const i = focus ? indexOf.get(focus) ?? -1 : -1;
    const page = Math.max(1, Math.floor(size.h / itemH) - 1) * cols;
    const extend = e.shiftKey || visual;
    const keep = (e.ctrlKey && !e.shiftKey) || (marking && !visual);
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        if (e.altKey) return;
        moveFocus(i < 0 ? 0 : i + cols, extend, keep);
        break;
      case "ArrowUp":
        if (e.altKey) return;
        moveFocus(i < 0 ? 0 : i - cols, extend, keep);
        break;
      case "ArrowRight":
        if (e.altKey) return;
        if (isGrid) moveFocus(i + 1, extend, keep);
        else if (focusedEntry?.isDir) A.navigate(b, focusedEntry.path);
        else handled = false;
        break;
      case "ArrowLeft":
        if (e.altKey) return;
        if (isGrid) moveFocus(i - 1, extend, keep);
        else goParent();
        break;
      case "Home":
        moveFocus(0, extend, keep);
        break;
      case "End":
        moveFocus(visible.length - 1, extend, keep);
        break;
      case "PageDown":
        moveFocus(i + page, extend, keep);
        break;
      case "PageUp":
        moveFocus(i - page, extend, keep);
        break;
      case "Enter":
        if (filterMode) setFilterMode(false);
        else handled = false;
        break;
      case "Escape":
        if (filterMode || filter) {
          setFilter("");
          setFilterMode(false);
        } else if (visual) {
          setVisual(false);
          setSel(new Set());
        }
        else if (sel.size) { setSel(new Set()); setMarking(false); }
        else if (marking) setMarking(false);
        else handled = false;
        break;
      case "Backspace":
        if (filter) setFilter((f) => f.slice(0, -1));
        else if (filterMode) setFilterMode(false);
        else handled = false;
        break;
      default:
        // Quick filter: letters, digits and '.', never Space (reserved for preview).
        // In vim mode letters are commands unless "/" started a filter.
        // A character typed with AltGr ("ś", "@"; Ctrl+Alt on Windows) or Option (macOS)
        // is typed text too, unless that chord is a shortcut.
        if (filterMode && typedChar(e, isShortcut)) {
          setFilter((f) => f + e.key);
          if (listRef.current) listRef.current.scrollTop = 0;
        } else if (!settings.vimKeys && typedChar(e, isShortcut) && /[\p{L}\p{N}._\-()]/u.test(e.key) && !(e.key === "\\" || e.key === "`")) {
          setFilter((f) => f + e.key);
          if (listRef.current) listRef.current.scrollTop = 0;
        } else handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const goParent = () => {
    const p = parent(path);
    if (p) {
      A.navigate(b, p);
      // Re-select the folder we came from.
      setTimeout(() => {
        controller(b)?.select([path], path);
      }, 120);
    }
  };

  // Focus first item after filter changes.
  useEffect(() => {
    if (filter && visible.length) {
      setFocus(visible[0].path);
      setSel(new Set([visible[0].path]));
      setAnchor(visible[0].path);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter]);

  // ───────── rubber band (left-drag on empty space, or middle-drag anywhere) ─────────
  const onListMouseDown = (e: React.MouseEvent) => {
    A.focusBrowser(b);
    paneRef.current?.focus();
    const onRow = (e.target as HTMLElement).closest("[data-path]");
    if (e.button === 1 || (e.button === 0 && !onRow)) {
      if (e.button === 1) e.preventDefault();
      const r = listRef.current!.getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top + listRef.current!.scrollTop;
      const base = primaryMod(e) || e.shiftKey ? new Set(sel) : new Set<string>();
      if (!primaryMod(e) && !e.shiftKey && e.button === 0) setSel(new Set());
      setRubber({ x0: x, y0: y, x1: x, y1: y, base });
    }
  };

  useEffect(() => {
    if (!rubber) return;
    const move = (e: MouseEvent) => {
      const el = listRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const x1 = e.clientX - r.left;
      const y1 = e.clientY - r.top + el.scrollTop;
      if (e.clientY > r.bottom - 10) el.scrollTop += 20;
      if (e.clientY < r.top + 10) el.scrollTop -= 20;
      setRubber((rb) => (rb ? { ...rb, x1, y1 } : rb));
      const [ya, yb] = [Math.min(rubber.y0, y1), Math.max(rubber.y0, y1)];
      const [xa, xb] = [Math.min(rubber.x0, x1), Math.max(rubber.x0, x1)];
      const n = new Set(rubber.base);
      visible.forEach((it, i) => {
        const p = posOf(i);
        const w = isGrid ? cellW - 8 : size.w;
        if (p.top + itemH > ya && p.top < yb && p.left + w > xa && p.left < xb) n.add(it.path);
      });
      setSel(n);
    };
    const up = () => setRubber(null);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up, { once: true });
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rubber?.x0, rubber?.y0, visible]);

  // ───────── context menus ─────────
  // `extra`: items appended after the folder menu (the header's "More actions" button).
  const openContextMenu = (x: number, y: number, entries: FileEntry[] | null, extra: MenuItem[] = []) => {
    const items: MenuItem[] = [];
    // The other browser, when there is one to look at (not in Classic or a narrow window).
    const others = A.hasOtherTarget(b) ? A.otherBrowser(b) : null;
    if (entries?.length) {
      const one = entries.length === 1 ? entries[0] : null;
      const dirs = entries.filter((e) => e.isDir);
      items.push({ label: "Open", shortcut: "Enter", run: () => A.openEntries(b, entries) });
      if (entries.some((e) => !e.isDir)) {
        items.push({ label: "Open with…", icon: "open", shortcut: "Ctrl+Enter", run: () => A.openWith(entries) });
        items.push({ label: "Edit", icon: "rename", shortcut: "F4", run: () => A.editFiles(entries), ctrl: { label: "Edit with…", run: () => A.editWith(entries) } });
      }
      if (one?.isDir) items.push({ label: "Open in editor", icon: "rename", shortcut: "F4", run: () => A.editFiles([one]), ctrl: { label: "Open with…", run: () => A.editWith([one]) } });
      if (one?.isDir) items.push({ label: "Terminal here", icon: "terminal", shortcut: "F10", run: () => api.openTerminal(one.path).catch(A.fail), ctrl: { label: "Terminal here with…", run: () => A.terminalWith(one.path) } });
      if (dirs.length) {
        items.push({ label: "Open in new tab", run: () => dirs.forEach((d) => A.sessionCmd({ op: "newTab", b, path: d.path, activate: false })) });
        items.push({ label: others != null ? "Open in other browser" : "Open in new tab", run: () => A.openInOther(b, dirs[0].path) });
        if (dirs.length === 1) items.push({ label: A.isPinned(dirs[0].path) ? "Unpin from sidebar" : "Pin to sidebar", icon: "star", run: () => A.togglePin(dirs[0].path) });
      }
      const files = entries.filter((e) => !e.isDir);
      if (files.length > 2)
        items.push({ label: `Compare ${files.length} files (similarity)`, icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.openOverlay({ type: "matrix", paths: files.slice(0, 30).map((f) => f.path) }) });
      if (files.length === 2 && entries.length === 2)
        items.push({ label: `Compare ${files[0].name} ↔ ${files[1].name}`, icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compare({ kind: "file", path: files[0].path }, { kind: "file", path: files[1].path }) });
      if (one && !one.isDir) items.push(A.compareMenu(b, one, !!gitRoot));
      if (dirs.length === 2 && entries.length === 2)
        items.push({ label: `Compare folders ${dirs[0].name} ↔ ${dirs[1].name}`, icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compareFolders(dirs[0].path, dirs[1].path) });
      if (one?.isDir) items.push({ label: "Compare folder with other panel", icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compareFolderWithOther(b, one) });
      if (one?.ext === "2do" || one?.ext === "2dx")
        items.push({ label: one.ext === "2do" ? "Mark done" : "Mark not done", run: () => api.toggleNote(one.path).catch(A.fail) });
      if (!archiveOf(path)) {
        if (searchHidden?.folder) {
          const f = searchHidden.folder;
          items.push({ label: f === path ? "Show this folder in search again" : `Show “${basename(f)}” in search again`, icon: "search", run: () => A.showInSearch(f) });
        } else if (one && searchHidden?.entries[one.name]) {
          items.push({ label: "Show in search again", icon: "search", run: () => A.showInSearch(one.path) });
        } else {
          const list = entries.filter((e) => !searchHidden?.entries[e.name]);
          if (list.length) items.push({ label: list.length > 1 ? `Hide ${list.length} items from search` : "Hide from search", icon: "search", run: () => A.hideFromSearch(list.map((e) => e.path)) });
        }
      }
      items.push({ separator: true });
      items.push({ label: "Cut", icon: "cut", shortcut: "Ctrl+X", run: () => A.copyToClipboard(entries, true) });
      items.push({ label: "Copy", icon: "copy", shortcut: "Ctrl+C", run: () => A.copyToClipboard(entries, false) });
      items.push({ label: "Paste", icon: "paste", shortcut: "Ctrl+V", run: () => A.paste(b) });
      if (one) items.push({ label: "Duplicate", shortcut: "Ctrl+D", run: () => handle("duplicate") });
      if (others != null) {
        items.push({ label: "Copy to other browser", shortcut: "Alt+C", run: () => handle("copyToOther") });
        items.push({ label: "Move to other browser", shortcut: "Alt+M", run: () => handle("moveToOther") });
      }
      items.push({
        label: "Copy path",
        shortcut: "Ctrl+Shift+C",
        submenu: (
          [["native", "As is"], ["nativeQuoted", "Quoted"], ["escaped", "Escaped \\\\"], ["forward", "Forward slashes /"], ["forwardQuoted", "Forward, quoted"], ["uri", "file:// URI"], ["linux", "Linux path (WSL)"]] as const
        ).filter(([f]) => f !== "linux" || getState().info?.platform === "windows").map(([f, l]) => ({
          label: l,
          run: async () => {
            const t = await Promise.all(entries.map((e) => api.pathFormat(e.path, f)));
            api.clipSetText(t.join("\n")).then(() => A.notify(t.length === 1 ? `Copied path of “${entries[0].name}”` : `Copied ${t.length} paths`)).catch(A.fail);
          },
        })),
      });
      items.push({ separator: true });
      // Archives: extract the selected archives, compress anything.
      const archives = entries.filter((e) => !e.isDir && archiveInfo(e.name));
      if (archives.length && archives.length === entries.length) {
        const ap = archives.map((e) => e.path);
        const stem = archives.length === 1 ? archiveInfo(archives[0].name)!.stem : null;
        // Holding Ctrl turns each into "… and trash archive" (one Ctrl+Z restores it).
        const andTrash = archives.length === 1 ? " and trash archive" : " and trash archives";
        const extractItem = (label: string, mode: "smart" | "folder", dest: () => string, icon: string | undefined, action: Action): MenuItem => ({
          label, icon, action, run: () => A.extract(ap, dest(), mode),
          ctrl: { label: label + andTrash, run: () => A.extract(ap, dest(), mode, true) },
        });
        items.push(extractItem("Extract here", "smart", () => path, "extract", "extractHere"));
        items.push(extractItem(stem ? `Extract to “${stem}/”` : "Extract each to its own folder", "folder", () => path, undefined, "extractToFolder"));
        if (others != null) items.push(extractItem("Extract to other browser", "smart", () => A.activePath(A.otherTarget(b) ?? others), undefined, "extractToOther"));
      }
      const base = archiveBaseName(entries, basename(path));
      const srcs = entries.map((e) => e.path);
      items.push({
        label: "Compress",
        icon: "archive",
        submenu: [
          ...CREATE_FORMATS.map(([f, ext]): MenuItem => ({ label: base + ext, action: f === "zip" ? "compressZip" : undefined, run: () => A.compress(srcs, path, base + ext, f) })),
          { separator: true },
          { label: "Name and format…", action: "compressAs", run: () => A.compressAs(srcs, path, base) },
          ...(others != null ? [{ label: "To other browser (.zip)", run: () => A.compress(srcs, A.activePath(A.otherTarget(b) ?? others), base + ".zip", "zip" as const) }] : []),
        ],
      });
      items.push({ separator: true });
      if (one) items.push({ label: "Rename", icon: "rename", shortcut: "F2", run: () => A.renameEntry(b, one) });
      else items.push({ label: `Rename ${A.items(entries.length)}…`, icon: "rename", shortcut: "F2", run: () => A.batchRename(b, entries) });
      items.push({ label: "Colour tag…", icon: "tag", shortcut: "Ctrl+Alt+T", run: () => handle("tag") });
      if (dirs.length) items.push({ label: dirs.length === 1 ? "Calculate size" : `Calculate sizes of ${A.items(dirs.length)}`, icon: "info", action: "folderSizes", run: () => A.calcFolderSizes(b, dirs.map((d) => d.path)) });
      if (dirs.length && dirs.length === entries.length) items.push(A.folderColorMenu(dirs.map((d) => d.path)));
      items.push({ label: "Move to trash", icon: "trash", action: "delete", run: () => handle("delete") });
      items.push({ label: "Delete permanently", danger: true, action: "deletePermanent", run: () => handle("deletePermanent") });
      items.push({ separator: true });
      if (one && !one.isDir) items.push({ label: "Create file template", run: () => A.addTemplateFrom(one) });
      items.push({ label: "Show in system file manager", action: "reveal", run: () => api.revealPath(entries[0].path).catch(A.fail) });
      if (one) items.push({ label: "Properties", icon: "info", shortcut: "Ctrl+I", run: () => A.openOverlay({ type: "properties", entry: one }) });
    } else {
      items.push({ label: "Paste", icon: "paste", shortcut: "Ctrl+V", run: () => A.paste(b) });
      items.push({ label: "Paste as symbolic links", shortcut: "Ctrl+Shift+V", disabled: !clipboard.paths.length, run: () => A.pasteAsLinks(b) });
      items.push({ separator: true });
      items.push({ label: "New folder", icon: "newFolder", shortcut: "Ctrl+Shift+N", run: () => A.newFolder(b) });
      items.push({ label: "New file from template", icon: "newFile", shortcut: "Ctrl+N", run: () => A.openOverlay({ type: "templates", dir: path, b }) });
      items.push({ label: "New note", icon: "note", shortcut: "Alt+N", run: () => A.openOverlay({ type: "note", dir: path }) });
      items.push({ separator: true });
      items.push({ label: "Sort by", icon: "sort", submenu: sortMenu() });
      items.push({ label: "View", icon: "view", action: "viewNext", submenu: VIEWS.map((v) => ({ label: v.label, checked: view === v.id, run: () => setView(v.id) })) });
      items.push({ label: "Folders on top", action: "toggleFoldersOnTop", checked: spec.foldersOnTop, run: toggleFoldersOnTop });
      items.push({ label: "Show hidden files", checked: settings.showHidden, shortcut: "Ctrl+H", run: () => A.updateSettings({ showHidden: !settings.showHidden }) });
      items.push({
        label: "Folder sizes", icon: "info", submenu: [
          { label: "Calculate now", action: "folderSizes", run: () => handle("folderSizes") },
          { label: "Calculate automatically", checked: settings.folderSizesAuto, run: () => A.updateSettings({ folderSizesAuto: !settings.folderSizesAuto }) },
        ],
      });
      items.push({
        label: "Default drop action",
        submenu: ([["consistent", "Move"], ["copy", "Copy"], ["system", "Move on same drive, copy across drives"], ["ask", "Ask (show menu)"]] as const).map(([v, l]) => ({
          label: l, checked: settings.dragBehavior === v, run: () => A.updateSettings({ dragBehavior: v }),
        })),
      });
      items.push({ separator: true });
      items.push({ label: "Select all", shortcut: "Ctrl+A", run: () => handle("selectAll") });
      items.push({ label: "Refresh", shortcut: "F5", run: () => dir.reload() });
      items.push({ label: "Terminal here", icon: "terminal", shortcut: "F10", run: () => api.openTerminal(path).catch(A.fail), ctrl: { label: "Terminal here with…", run: () => A.terminalWith(path) } });
      items.push({ label: "Open folder in editor", icon: "rename", run: () => A.editPaths([path]), ctrl: { label: "Open folder with…", run: () => A.openOverlay({ type: "openWith", paths: [path], mode: "editor" }) } });
      items.push({ label: "Copy folder path", run: () => A.copyPaths([], path) });
    }
    if (extra.length) items.push({ separator: true }, ...extra);
    A.showMenu(x, y, items);
  };

  const sortMenu = (): MenuItem[] =>
    (
      [["name", "Name"], ["ext", "Type"], ["modified", "Date modified"], ["created", "Date created"], ["size", "Size"]] as [SortKey, string][]
    ).flatMap(([k, l]) => [
      { label: `${l} ↑`, checked: spec.key === k && !spec.descending, run: () => setSort(k, false) },
      { label: `${l} ↓`, checked: spec.key === k && spec.descending, run: () => setSort(k, true) },
    ]);

  const onRowContext = (e: React.MouseEvent, entry: FileEntry) => {
    e.preventDefault();
    e.stopPropagation();
    let s = selection();
    if (!sel.has(entry.path)) {
      setSel(new Set([entry.path]));
      setFocus(entry.path);
      setAnchor(entry.path);
      s = [entry];
    }
    openContextMenu(e.clientX, e.clientY, s);
  };

  // ───────── column visibility by available width (removal order) ─────────
  // Widths measured in characters so the thresholds follow the font size.
  const chars = size.w / (settings.fontSize * 0.6);
  const show = {
    size: !longNames && chars > (compact ? 30 : 26),
    age: !longNames && chars > (compact ? 40 : 36),
    date: !longNames && chars > (compact ? 52 : 48),
    ext: !longNames && chars > 58,
    time: !longNames && chars > 66,
    meta: !longNames && chars > 76,
  };
  // Owner column (Linux / macOS "Mode" view): as wide as the longest owner here, 4–12 characters.
  const ownerW = useMemo(() => `${Math.min(12, Math.max(4, ...dir.entries.map((e) => e.owner?.length ?? 0))) + 1.2}ch`, [dir.entries]);
    const scrollbarW = listRef.current ? listRef.current.offsetWidth - listRef.current.clientWidth : 0;

  const background = useMemo(() => {
    if (!settings.folderBackgroundImages) return null;
    const c = dir.entries.find((e) => COVER_NAMES.includes(e.name.toLowerCase()));
    return c ? fileSrc(c.path) : null;
  }, [dir.entries, settings.folderBackgroundImages]);

  const now = Date.now();
  const cutSet = clipboard.cut ? new Set(clipboard.paths) : null;

  const renderRow = (entry: FileEntry, i: number) => {
    const pos = posOf(i);
    const selected = sel.has(entry.path);
    const focused = focus === entry.path;
    const tag = tags.get(entry.path);
    const gs = git ? git.all ?? git.entries[entry.name] : undefined;
    const cls = [
      isGrid ? "grid-cell" : "row",
      selected && "selected",
      focused && "focused",
      settings.marchingAnts && active && "ants",
      cutSet?.has(entry.path) && "cut",
      dropTarget === entry.path && "drop",
      isThumbs && "two-line",
      (entry.ext === "2do" || entry.ext === "2dx") && "note",
      gs === "ignored" && "git-ignored",
      (searchHidden?.folder || searchHidden?.entries[entry.name]) && "search-hidden",
    ]
      .filter(Boolean)
      .join(" ");
    const common = {
      key: entry.path,
      "data-path": entry.path,
      className: cls,
      draggable: true,
      onMouseDown: (e: React.MouseEvent) => {
        if (e.button === 1 && entry.isDir) {
          e.preventDefault();
          e.stopPropagation();
          if (e.altKey) A.openInOther(b, entry.path, false);
          else A.sessionCmd({ op: "newTab", b, path: entry.path, activate: false });
        }
      },
      onClick: (e: React.MouseEvent) => clickRow(e, entry),
      onDoubleClick: () => !settings.singleClickOpen && A.openEntries(b, [entry]),
      onContextMenu: (e: React.MouseEvent) => onRowContext(e, entry),
      onDragStart: (e: React.DragEvent) => {
        const paths = sel.has(entry.path) ? [...sel] : [entry.path];
        if (!sel.has(entry.path)) {
          setSel(new Set([entry.path]));
          setFocus(entry.path);
        }
        startDrag(e, paths);
      },
      onDragOver: (e: React.DragEvent) => {
        if (!entry.isDir) {
          // A file dropped on another file opens the comparison.
          const src = draggedSingle();
          if (onDragOverCompare(e, entry.path, !!src && !!visible.find((v) => v.path === src)?.isDir)) setDropTarget(entry.path);
          return;
        }
        if (sel.has(entry.path)) return;
        if (onDragOverTarget(e, entry.path)) setDropTarget(entry.path);
      },
      onDragLeave: () => setDropTarget((t) => (t === entry.path ? null : t)),
      onDrop: (e: React.DragEvent) => {
        if (!entry.isDir) {
          const src = draggedSingle();
          setDropTarget(null);
          if (src && src !== entry.path) {
            e.preventDefault();
            e.stopPropagation();
            A.compare({ kind: "file", path: src }, { kind: "file", path: entry.path });
          }
          return;
        }
        setDropTarget(null);
        dropInto(e, entry.path);
      },
    };

    if (isGrid) {
      const thumb = IMG.has(entry.ext) && !settings.noThumbnailExts.includes(entry.ext) && !entry.cloudOnly;
      return (
        <div {...common} style={{ top: pos.top + 4, left: pos.left, width: cellW - 8, height: itemH - 8 }} title={entry.name}>
          <div className="gthumb">
            {thumb ? <img src={fileSrc(entry.path)} loading="lazy" decoding="async" draggable={false} /> : <FileIcon entry={entry} size={Math.round((gridSize - GRID_PAD) * 0.7)} />}
          </div>
          <div className="gname">{tag ? <span className="tagdot" style={{ background: TAG_COLORS[tag], display: "inline-block", marginRight: 4 }} /> : null}{entry.name}</div>
        </div>
      );
    }

    const [stem, extPart] = entry.isDir ? [entry.name, ""] : splitExt(entry.name);
    const isNote = entry.ext === "2do" || entry.ext === "2dx";
    const nameCell = (
      <div className="name">
        {isNote ? (
          <input type="checkbox" className="note-check" checked={entry.ext === "2dx"} onClick={(e) => e.stopPropagation()}
            onChange={() => api.toggleNote(entry.path).catch(A.fail)} />
        ) : isThumbs && IMG.has(entry.ext) && !settings.noThumbnailExts.includes(entry.ext) && !entry.cloudOnly ? (
          <img className="thumb" src={fileSrc(entry.path)} loading="lazy" decoding="async" draggable={false} style={{ width: TILE, height: TILE }} />
        ) : isThumbs ? (
          // Same slot as a thumbnail, so names line up.
          <span style={{ width: TILE, display: "inline-flex", justifyContent: "center", flexShrink: 0 }}><FileIcon entry={entry} size={32} /></span>
        ) : (
          <FileIcon entry={entry} size={Math.round(settings.fontSize * 1.25)} />
        )}
        {isThumbs ? (
          <div className="stack">
            <span className="txt">{stem}<span className="extpart">{extPart}</span></span>
            <span className="sub">{excerpts.get(entry.path) ?? (entry.isDir ? "Folder" : `${formatSize(entry.size)} · ${formatDate(entry.modified)} ${formatTime(entry.modified)}`)}</span>
          </div>
        ) : (
          <span className="txt" style={isNote && entry.ext === "2dx" ? { textDecoration: "line-through", opacity: 0.7 } : undefined}>
            {stem}
            {extPart && <span className="extpart">{extPart}</span>}
          </span>
        )}
        {tag ? <span className="tagdot" style={{ background: TAG_COLORS[tag] }} /> : null}
        {gs && gs !== "ignored" && <span className={`git-badge g-${gs}`} title={GIT_TITLE[gs](entry.isDir)}>{GIT_LETTER[gs]}</span>}
        {entry.isSymlink && (view === "powershell" && entry.linkTarget != null
          ? <span className="meta link-target" title={entry.linkTarget}>→ {entry.linkTarget}</span>
          : show.meta && <span className="meta">link</span>)}
      </div>
    );
    const ageInfo = entry.modified != null ? age(entry.modified, now) : null;
    const ageCell = show.age && (
      <span className="col" style={{ width: CW.age, textAlign: "center" }}>
        {ageInfo && (() => {
          const tier = ageTier(entry.modified!, now);
          const c = ageColor(tier, settings.ageSaturation, settings.ageLightness);
          return <span className="age" title={`Modified ${ageWords(entry.modified!, now)} ago (${AGE_TIERS[tier].label}) · ${new Date(entry.modified!).toLocaleString()}\nAge: ' minutes, h hours, d days, M months, Y years`} style={{ background: c.bg, color: c.fg, boxShadow: `inset 0 0 0 1px ${c.border}` }}>{ageInfo.label}</span>;
        })()}
      </span>
    );
    const style = { top: pos.top, height: itemH };

    if (view === "names")
      return <div {...common} style={style}>{nameCell}</div>;

    if (view === "powershell") {
      const sizeCol = <span className="col" style={{ width: CW.size }} title={entry.isDir ? A.folderSizeTitle(folderSizes[entry.path]) : undefined}>{entry.isDir ? A.folderSizeLabel(folderSizes[entry.path]) : formatSize(entry.size)}</span>;
      return (
        <div {...common} style={style}>
          {entry.mode != null ? (
            // Linux / macOS: like `ls -l` — permissions, size, owner, date, name → target.
            // Windows (below): like PowerShell's Get-ChildItem — mode, date, size, name → target.
            <>
              <span className="col perms" style={{ width: CW.unixMode }}><ModeText text={unixMode(entry.mode)} /></span>
              {sizeCol}
              {show.date && <span className="col owner" style={{ width: ownerW }} title={entry.owner}>{entry.owner}</span>}
              {show.date && <span className="col" style={{ width: CW.date }}>{formatDate(entry.modified)}</span>}
              {show.time && <span className="col" style={{ width: CW.time }}>{formatTime(entry.modified)}</span>}
            </>
          ) : (
            <>
              {/* Windows: PowerShell's Mode; inside archives (no attributes) a short d r h l. */}
              <span className="col perms" style={{ width: CW.mode }}>
                {entry.attributes != null
                  ? <ModeText win text={windowsMode(entry.attributes, entry.isDir)} />
                  : <>{entry.isDir ? "d" : "-"}{entry.readonly ? "r" : "-"}{entry.isHidden ? "h" : "-"}{entry.isSymlink ? "l" : "-"}</>}
              </span>
              {show.date && <span className="col" style={{ width: CW.date }}>{formatDate(entry.modified)}</span>}
              {show.time && <span className="col" style={{ width: CW.time }}>{formatTime(entry.modified)}</span>}
              {sizeCol}
            </>
          )}
          {nameCell}
        </div>
      );
    }

    return (
      <div {...common} style={style}>
        {nameCell}
        {show.ext && <span className="col ext" style={{ width: CW.ext }} title={entry.ext.length > 5 ? entry.ext : undefined}>{entry.ext}</span>}
        {ageCell}
        {show.date && <span className="col" style={{ width: CW.date }}>{formatDate(entry.modified)}</span>}
        {show.time && <span className="col" style={{ width: CW.time }}>{formatTime(entry.modified)}</span>}
        {show.size && (
          <span className={`col ${entry.isDir && folderSizes[entry.path] && !folderSizes[entry.path].done ? "counting" : ""}`} style={{ width: CW.size }}
            title={entry.isDir ? A.folderSizeTitle(folderSizes[entry.path]) : undefined}>
            {entry.isDir ? A.folderSizeLabel(folderSizes[entry.path]) : formatSize(entry.size)}
          </span>
        )}
      </div>
    );
  };

  /** Column header: label; the sorted column shows its direction, clicking it again flips it. */
  const sortBtn = (k: SortKey, title: string, label: string, align: "left" | "center" | "right" = "right") => {
    const on = spec.key === k;
    return (
      <button className={`col-head ${on ? "on" : ""}`} style={{ justifyContent: align === "left" ? "flex-start" : align === "center" ? "center" : "flex-end" }}
        title={`Sort by ${title}${on ? ` (${spec.descending ? "descending" : "ascending"})` : ""}`}
        onClick={() => setSort(k, on ? !spec.descending : false)}>
        {/* Arrow sits on the side away from the text edge, so labels line up with cell text. */}
        {on && align === "right" && <span className="arrow">{spec.descending ? "↓" : "↑"}</span>}
        <span>{label}</span>
        {on && align !== "right" && <span className="arrow">{spec.descending ? "↓" : "↑"}</span>}
      </button>
    );
  };

  const selEntries = visible.filter((e) => sel.has(e.path));
  const selBytes = selEntries.reduce((a, e) => a + (e.isDir ? 0 : e.size), 0);

  useLayoutEffect(() => {
    if (listRef.current && Math.abs(listRef.current.scrollTop - scrollTop) > 1 && scrollTop === 0) listRef.current.scrollTop = 0;
  }, [scrollTop, listRef]);

  return (
    <div className="folder-pane" ref={paneRef} tabIndex={0} onKeyDown={onKeyDown}
      onFocus={() => A.focusBrowser(b)}
      onWheel={(e) => {
        if (isGrid && e.ctrlKey) {
          e.preventDefault();
          // Explorer's medium (48), large (96) and extra large (256) icons are all steps.
          setGridSize((g) => Math.max(GRID_MIN, Math.min(GRID_MAX, g - Math.sign(e.deltaY) * 16)));
        }
      }}>
      {/* Right-click on the folder's header: the same menu as on the empty list. */}
      <div className="fp-head" onContextMenu={(e) => {
        if ((e.target as HTMLElement).closest("input, textarea")) return;
        e.preventDefault();
        openContextMenu(e.clientX, e.clientY, null);
      }}>
        <div className="fp-heading">
          <div className="fp-title-row">
            <div className="fp-title" title={`${path}\nClick to edit (Ctrl+L)`} onClick={() => A.openOverlay({ type: "pathEdit", b })}>{basename(path) || path}</div>
            <button className={`pin-btn ${pinned ? "on" : ""}`} title={`${pinned ? "Unpin from sidebar" : "Pin to sidebar"}${pinKeys ? ` (${pinKeys})` : ""}`} onClick={() => A.togglePin(path)}><Icon name="star" size={14} /></button>
            {visual && (
              <span className="fp-visual" title="Visual mode: j/k, gg/G extend the selection; y y, d d, D, m, C, M … act on it; Esc or v cancels">
                -- VISUAL -- {sel.size}
              </span>
            )}
            {searchHidden?.folder && searchHidden.folderRule && (
              <span className="fp-archive fp-search-hidden" title={`Hidden from search by “${searchHidden.folderRule.line}” in ${searchHidden.folderRule.file}\nQuick Open, find in files and F3 skip this folder. Click to show it in search again.`}
                onClick={() => A.showInSearch(searchHidden.folder!)}>
                <Icon name="search" size={12} /> hidden from search
              </span>
            )}
            {archiveOf(path) && (
              <span className="fp-archive" title={`Inside ${archiveOf(path)}\nRead-only: copy items out (y y / Ctrl+C, then paste), drag them to the other browser, or extract the archive.`}>
                <Icon name="archive" size={12} /> archive · read-only
              </span>
            )}
          </div>
          {/* Quick filter: a notification-style card in the free space of the folder header. */}
          {(filter || filterMode) && (
            <div className={`quick-filter ${filterMode ? "typing" : ""} ${filter && !visible.length ? "none" : ""}`} role="search">
              <span className="qf-icon"><Icon name="search" size={14} /></span>
              <span className="qf-text">
                {filterMode && <span className="qf-slash">/</span>}
                {filter}
                {filterMode && <span className="qf-caret" />}
                {filterMode && !filter && <span className="qf-ph">type to filter this folder</span>}
              </span>
              {filter && <span className="qf-count">{visible.length} / {dir.entries.length}</span>}
              <kbd className="qf-esc">Esc</kbd>
              <button className="qf-x" title="Clear filter (Esc)" onClick={() => { setFilter(""); setFilterMode(false); }}><Icon name="close" size={12} /></button>
            </div>
          )}
        </div>
        <div className="fp-pill">
          <button title="Sort" onClick={(e) => A.showMenu(e.clientX, e.clientY, sortMenu())}>
            <Icon name="sort" size={13} /><span>{SORT_LABEL[spec.key]}</span><span className="dir">{spec.descending ? "↓" : "↑"}</span>
          </button>
          <i className="sep" />
          <button title="File view" onClick={(e) => A.showMenu(e.clientX, e.clientY, VIEWS.map((v) => ({ label: v.label, checked: view === v.id, run: () => setView(v.id) })))}>
            <Icon name="view" size={13} /><span>{VIEWS.find((v) => v.id === view)?.label.split(" ")[0] ?? "View"}</span>
          </button>
        </div>
        <button className="fp-undo" title={`${undoHere ? `Undo in this folder: ${undoHere}` : "Nothing to undo in this folder"}\nUndo history lasts until Gall is closed`} disabled={!undoHere} onClick={() => A.undoIn(path)}><Icon name="undo" size={15} /></button>
        {/* The same menu as a right-click on the empty list, plus a few folder-wide extras. */}
        <button className="fp-more" title="More actions" onClick={(e) => openContextMenu(e.clientX, e.clientY, null, [
          { label: pinned ? "Unpin from sidebar" : "Pin to sidebar", icon: "star", shortcut: "Ctrl+B", run: () => A.togglePin(path) },
          A.folderColorMenu([path]),
          { label: "Compare with other panel's folder", icon: "compare", shortcut: "Ctrl+Alt+D", run: () => A.comparePanelFolders(b) },
        ])}><Icon name="more" size={16} /></button>
      </div>
      {/* The header is laid out as a .row with the same cells as the list rows (same padding,
          border, gap and column widths), so labels always line up with the values below. */}
      <div className="fp-nav" style={{ paddingRight: scrollbarW }}>
        <div className="row fp-hrow">
          <div className="name fp-nav-name">
            <button className="icon-btn" title="Parent folder (Backspace)" disabled={!parent(path)} onClick={goParent}><Icon name="up" /></button>
            <button className="icon-btn" title="Back (Alt+←)" onClick={() => A.sessionCmd({ op: "back", b })}><Icon name="back" /></button>
            <button className="icon-btn" title="Forward (Alt+→)" onClick={() => A.sessionCmd({ op: "forward", b })}><Icon name="forward" /></button>
            <button className={`icon-btn ${spec.foldersOnTop ? "active" : ""}`} title="Folders on top" onClick={toggleFoldersOnTop}><Icon name="foldersTop" /></button>
            {!isGrid && view !== "powershell" && <span className="name-head"><i className="nav-sep" />{sortBtn("name", "name", "Name", "left")}</span>}
          </div>
          {!isGrid && view !== "powershell" && view !== "names" && (
            <>
              {show.ext && <span className="col" style={{ width: CW.ext }}>{sortBtn("ext", "type", "Type")}</span>}
              {show.age && <span className="col" style={{ width: CW.age }}>{sortBtn("modified", "age (date modified)", "Age", "center")}</span>}
              {show.date && <span className="col" style={{ width: CW.date }}>{sortBtn("created", "date created", "Date")}</span>}
              {show.time && <span className="col" style={{ width: CW.time }} />}
              {show.size && <span className="col" style={{ width: CW.size }}>{sortBtn("size", "size", "Size")}</span>}
            </>
          )}
        </div>
      </div>
      <div
        className={`fp-list ${background ? "bg-image" : ""}`}
        ref={listRef}
        style={background ? { backgroundImage: `url("${background}")` } : undefined}
        onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
        onMouseDown={onListMouseDown}
        onContextMenu={(e) => {
          if ((e.target as HTMLElement).closest("[data-path]")) return;
          e.preventDefault();
          setSel(new Set());
          openContextMenu(e.clientX, e.clientY, null);
        }}
        onDragOver={(e) => {
          if (!isFileDrag(e)) return;
          const own = draggedPaths(e.dataTransfer);
          if (own.length && own.every((p) => parent(p) === path)) return hintNoop("Already in this folder"); // same folder: no-op
          onDragOverTarget(e, path);
        }}
        onDrop={(e) => dropInto(e, path)}
      >
        <div style={{ height: totalH, position: "relative" }}>
          {visible.slice(firstRow * cols, lastRow * cols).map((e, k) => renderRow(e, firstRow * cols + k))}
        </div>
        {rubber && (
          <div className="rubber" style={{
            left: Math.min(rubber.x0, rubber.x1), top: Math.min(rubber.y0, rubber.y1),
            width: Math.abs(rubber.x1 - rubber.x0), height: Math.abs(rubber.y1 - rubber.y0),
          }} />
        )}
        {!dir.loading && !visible.length && (dir.error
          ? <div className="empty-msg error" role="alert">
              <Icon name="error" size={18} /><span>{A.readableError(dir.error)}</span>
              {dir.error.includes("archive-password:") && (
                <button className="btn primary" onClick={() => A.askArchivePassword(dir.error).then((ok) => ok && dir.reload())}>Enter password…</button>
              )}
            </div>
          : <div className="empty-msg">{filter ? `No items match “${filter}”` : "This folder is empty"}</div>)}
      </div>
      <div className="fp-status">
        {/* Waybar-style modules: Nerd Font glyph + value. BMP (Font Awesome) code points:
            WebKitGTK does not render the U+F0000+ Material range. */}
        <span className="mod" title={A.items(visible.length)}><Glyph nf={"\u{F07B}"} icon="folder" />{dirCount}</span>
        <span className="mod"><Glyph nf={"\u{F15B}"} icon="file" />{visible.length - dirCount}</span>
        {filter && <span className="mod accent" title={`Filtered from ${dir.entries.length}`}><Glyph nf={"\u{F0B0}"} icon="search" />{filter}</span>}
        <span style={{ flex: 1 }} />
        {git && (
          <button className="mod git" onClick={() => A.openOverlay({ type: "gitGraph", path: git.root })} title={`Git: ${git.branch ? `branch ${git.branch}` : "detached HEAD"}${git.ahead ? `, ${git.ahead} to push` : ""}${git.behind ? `, ${git.behind} to pull` : ""}${git.containingBranches?.length ? `, contained in ${git.containingBranches.join(", ")}` : ""}${git.subject ? ` · ${git.subject}` : ""} · ${git.root}`}>
            <Glyph nf={"\u{E0A0}"} icon="code" />{git.branch ?? (git.containingBranches?.length ? git.containingBranches.join(", ") : "detached")}{git.subject ? ` · ${git.subject}` : ""}{git.ahead ? ` ↑${git.ahead}` : ""}{git.behind ? ` ↓${git.behind}` : ""}
          </button>
        )}
        {sel.size > 0 && <span className="mod accent"><Glyph nf={"\u{F14A}"} icon="check" />{sel.size}{selBytes ? ` · ${formatSize(selBytes)}` : ""}</span>}
      </div>
    </div>
  );
}
