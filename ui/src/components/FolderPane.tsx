// Current Folder Pane (F-01…F-13, V-01, V-02): virtualised list with sorting,
// quick filter, selection (incl. rubber band), drag & drop and context menus.
import { appendActionsToMenu } from "../core/actionsMenu";
import { eventAnchor, textAnchor, type Anchor } from "../core/anchor";
import { maxOf } from "../core/maxOf";
import { archiveBaseName, archiveInfo, archiveOf, CREATE_FORMATS, isBrowsableArchive } from "../core/archive";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, fileSrc } from "../api/ipc";
import type { FileEntry, GitState, GroupMode, GroupOrder, SortKey } from "../api/types";
import { age, ageColor, ageTier, ageWords, AGE_TIERS } from "../core/age";
import { hasNerdFont } from "../core/nerdFont";
import { quickFilterMatches } from "../core/filter";
import { focusAfterRemoval } from "../core/focus";
import { formatDate, formatSize, formatTime, unixMode, windowsMode } from "../core/format";
import { buildKeymap, IS_MAC, primaryMod, typedChar, type Action } from "../core/keymap";
import { basename, parent, splitExt } from "../core/path";
import { findPairForTab, pairColor, tandemRelativeDisplay } from "../core/pairs";
import type { SortSpec } from "../core/sort";
import { byType, foldersFirst, groupAt, groupEntries, GROUP_MODES, GROUP_ORDERS, headTop, layoutOf, moveCursor, openGroups, placeOf, toggleFolders, windowOf, type Cursor, type Group } from "../core/sortGroup";
import * as A from "../state/actions";
import { controller, registerController } from "../state/controllers";
import { draggedPaths, draggedSingle, dropInto, hintNoop, isFileDrag, onDragOverCompare, onDragOverTarget, startDrag } from "../state/dnd";
import { bumpNonce, useDirList, useGitStatus, useGitUntrusted, useNonce, usePrefs, useSearchHidden, useSize, useTags } from "../state/hooks";
import { arrangement, getState, setState, useStore, type MenuItem } from "../state/store";
import { useKeyLabels } from "../state/keyLabels";
import { isPatchFile } from "../core/patchImport";
import { matchesFilter } from "../core/pick";
import { t, tt } from "../i18n";
import { alignedRowHeight, snapCss } from "../core/pixelGrid";
import { folderPaneSizeChanged, quantizeListHeight } from "../core/resizeThrottle";
import { useDpr } from "../state/dpr";
import { worktreeColor } from "../core/gitWorktree";
import { WorktreeDot } from "./GitWorktrees";
import { FileIcon, Icon, TAG_COLORS } from "./Icon";
import { platformLabel } from "../core/keymap";

/** Nerd Font glyph when one is installed, otherwise the matching SVG line icon. */
function Glyph({ nf, icon }: { nf: string; icon: string }) {
  return hasNerdFont() ? <i className="nf">{nf}</i> : <Icon name={icon} size={12} style={{ opacity: 0.85 }} />;
}

const GIT_LETTER: Record<GitState, string> = { modified: "M", staged: "S", untracked: "U", conflict: "!", ignored: "" };
const GIT_TITLE: Record<GitState, (dir: boolean) => string> = {
  modified: (d) => (d ? tt("Git: changes inside") : tt("Git: modified")),
  staged: (d) => (d ? tt("Git: staged changes inside") : tt("Git: staged (added to the commit)")),
  untracked: (d) => (d ? tt("Git: new files inside, not added") : tt("Git: new, not added")),
  conflict: (d) => (d ? tt("Git: merge conflict inside") : tt("Git: merge conflict")),
  ignored: () => tt("Git: ignored"),
};

const SORT_LABEL: Record<SortKey, string> = { name: "Name", ext: "Type", modified: "Modified", created: "Created", size: "Size" };

export const GROUP_LABEL: Record<GroupMode, string> = { none: "No grouping", folders: "Folders first", type: "Type", foldersType: "Folders first + type" };
export const GROUP_ORDER_LABEL: Record<GroupOrder, string> = { name: "Alphabetical", count: "Most files first", recent: "Most recent change first", size: "Largest total size first" };
const KIND_LABEL: Record<string, string> = { image: "Images", document: "Documents", audio: "Audio", video: "Video", archive: "Archives", code: "Code", other: "Other" };

function groupLabel(g: Group): string {
  if (g.id === "dir") return tt("Folders");
  if (g.id === "dot") return tt("Dotfiles");
  if (g.id === "none") return tt("No extension");
  if (g.id === "rest") return tt("Other ({count} rarer types)", { count: g.types });
  return g.id.startsWith("kind:") ? tt(KIND_LABEL[g.id.slice(5)]) : g.id.slice(4).toUpperCase();
}

/** Collapsed groups of folders that do not remember their view: until Gall is closed. */
const collapsedHere = new Map<string, string[]>();

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

// Shared across panes in this window so switching between left and right panels still updates
// `selection.json`, while redundant folder refreshes with unchanged selection skip IPC.
let lastPublishedSelection: string | null = null;

export function FolderPane({ b, path, active, compact }: Props) {
  const settings = useStore((s) => s.settings)!;
  // Chords that are shortcuts: an Option character (macOS) typed with one is not text.
  const keymap = useMemo(() => buildKeymap(settings.shortcuts), [settings.shortcuts]);
  const isShortcut = (chord: string) => keymap.has(chord);
  // Git status badges and the branch (Settings › Files › Git status).
  const git = useGitStatus(path, settings.showGitStatus);
  // The path bar marks the same worktree without asking Git again.
  const linkedRoot = git?.linked ? git.root : null;
  useEffect(() => {
    setState((s) => {
      const worktreeOf = [...s.worktreeOf];
      worktreeOf[b] = linkedRoot;
      return { worktreeOf };
    });
  }, [b, linkedRoot]);
  const gitRefused = useGitUntrusted(path, settings.showGitStatus, !!git);
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
  const keys = useKeyLabels();
  const pinKeys = keys.action("togglePin");
  const hiddenKeys = keys.action("toggleHidden");
  const hiddenAllKeys = keys.action("toggleHiddenAll");
  const showHidden = useStore((s) => A.hiddenShown(s, b));
  const hiddenOverride = useStore((s) => A.hiddenDiverged(s, b));
  const vertical = useStore((s) => arrangement(s).vertical);
  const longNames = useStore((s) => s.longNames);
  const clipboard = useStore((s) => s.clipboard);
  const nonce = useNonce();
  const dir = useDirList(path, settings, false, showHidden);
  const hiddenCount = useMemo(
    () => (hiddenOverride === true ? dir.entries.filter((e) => e.isHidden).length : 0),
    [dir.entries, hiddenOverride],
  );
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
  const home = useStore((s) => s.info?.home ?? "");
  const tandemPair = useStore((s) => {
    const br = s.session?.browsers[b];
    const tab = br?.tabs[br.active];
    if (!tab) return null;
    const p = findPairForTab(s.session, b, tab.id);
    return p?.mode === "tandem" ? p : null;
  });
  const tandemMissing = !!(tandemPair && dir.error && A.isMissingError(dir.error) && !archiveOf(path));
  // The folder vanished (deleted or renamed elsewhere, its drive gone): go up to the
  // nearest folder that exists instead of showing an error for a place that is gone.
  // In Tandem mode, stay on the missing path non-destructively so the pair stays intact
  // and the user can Create folder, Open parent, or Unlink.
  useEffect(() => {
    if (dir.error && A.isMissingError(dir.error) && !archiveOf(path) && !tandemPair) void A.leaveMissing(b, path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir.error, tandemPair?.id]);
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
  // The cursor on the header of a group; an entry with the focus takes it away.
  const [headFocus, setHeadFocus] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [gridSize, setGridSize] = useState(GRID_DEFAULT);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [rubber, setRubber] = useState<{ x0: number; y0: number; x1: number; y1: number; base: Set<string> } | null>(null);
  const [excerpts, setExcerpts] = useState<Map<string, string | null>>(new Map());
  const paneRef = useRef<HTMLDivElement>(null);
  const pendingSelect = useRef<{ paths: string[]; focus?: string } | null>(null);
  const lastClick = useRef<{ path: string; at: number } | null>(null);

  // Folders not remembering their own sort / view: this browser's own, in every folder.
  const remember = settings.rememberFolderViews;
  const panelView = useStore((s) => s.session?.browsers[b]?.view);
  const own = remember ? null : panelView;
  const view = own?.view ?? prefs?.view ?? settings.defaultView;
  const isGrid = view === "grid";
  const sizeFilterRef = useRef({ isGrid: false, fontSize: 13, compact: false, itemH: 24 });
  const [listRef, size] = useSize<HTMLDivElement>(
    (prev, next) => folderPaneSizeChanged(prev, next, sizeFilterRef.current),
    isGrid,
  );
  const spec: SortSpec = {
    key: own?.sortKey ?? prefs?.sortKey ?? settings.defaultSort,
    descending: own?.descending ?? prefs?.descending ?? settings.defaultDescending,
    foldersOnTop: foldersFirst(own?.group ?? prefs?.group ?? settings.group),
    foldersByName: settings.foldersByName,
  };
  const group: GroupMode = own?.group ?? prefs?.group ?? settings.group;
  const groupOrder: GroupOrder = own?.groupOrder ?? prefs?.groupOrder ?? settings.groupOrder;
  // Turned off: each browser keeps what it shows now instead of jumping to the defaults.
  const shown = useRef({ view, spec, group, groupOrder, remember });
  useEffect(() => {
    const was = shown.current;
    if (was.remember && !remember) {
      A.sessionCmd({ op: "view", b, prefs: { view: was.view, sortKey: was.spec.key, descending: was.spec.descending, group: was.group, groupOrder: was.groupOrder } });
    }
  }, [remember, b]);
  shown.current = { view, spec, group, groupOrder, remember };

  // Calculated folder sizes (F-14). Sorting by size uses finished ones only, so rows do
  // not jump around while folders are being counted.
  const folderSizes = useStore((s) => s.folderSizes);
  const doneSizes = spec.key === "size"
    ? Object.entries(folderSizes).filter(([, f]) => f.done && !f.virtualFs).map(([p, f]) => `${p}:${f.bytes}`).join("|")
    : "";
  // A picker window's file-type filter hides the files it does not list (folders stay: they lead to files).
  const pickFilter = useStore((s) => (s.pick && s.pick.mode !== "dir" ? s.pick.filters[s.pickFilter] : undefined));
  const { entries: sorted, groups } = useMemo(() => {
    const listed = pickFilter?.patterns.length ? dir.entries.filter((e) => e.isDir || matchesFilter(e.name, pickFilter)) : dir.entries;
    const filtered = filter ? listed.filter((e) => quickFilterMatches(e.name, filter)) : listed;
    const sizeOf = (p: string) => A.folderSizeBytes(getState().folderSizes[p]);
    return groupEntries(filtered, spec, { mode: group, order: groupOrder, byKind: settings.groupByKind }, spec.key === "size" ? sizeOf : undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dir.entries, filter, pickFilter, spec.key, spec.descending, spec.foldersByName, group, groupOrder, settings.groupByKind, doneSizes]);
  // Headers and collapsed groups. Without headers nothing is collapsed: it could not be opened again.
  const heads = settings.groupHeaders ? groups : null;
  const [collapsedNow, setCollapsedNow] = useState<{ path: string; ids: string[] } | null>(null);
  const collapsedIds = collapsedNow?.path === path ? collapsedNow.ids
    : remember ? (prefs?.path === path ? prefs.collapsed : undefined) : collapsedHere.get(path);
  const collapsed = useMemo(() => new Set(collapsedIds ?? []), [collapsedIds]);
  const { visible, closed } = useMemo(() => openGroups(sorted, heads, collapsed, !!filter), [sorted, heads, collapsed, filter]);
  const setCollapsed = (ids: string[]) => {
    setCollapsedNow({ path, ids });
    if (remember) api.setPrefs(path, { collapsed: ids }).catch(A.fail);
    else collapsedHere.set(path, ids);
  };
  /** The group an entry of this folder is in, on screen or collapsed. */
  const groupOf = (p: string): string | null => {
    let at = heads ? sorted.findIndex((e) => e.path === p) : -1;
    if (at < 0) return null;
    for (const g of heads!) {
      if (at < g.count) return g.id;
      at -= g.count;
    }
    return null;
  };

  // Folder sizes: stop this browser's calculation when it leaves the folder; with
  // "calculate automatically" count every subfolder once the listing is in.
  useEffect(() => () => A.cancelFolderSizes(b), [path, b]);
  useEffect(() => {
    if (!settings.folderSizesAuto || dir.loading || archiveOf(path)) return;
    const dirs = dir.entries.filter((e) => e.isDir && !e.isSymlink).map((e) => e.path);
    if (dirs.length) A.calcFolderSizes(b, dirs, 120_000);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.folderSizesAuto, dir.loading, dir.entries, path]);
  const dirCount = useMemo(() => sorted.reduce((n, e) => n + (e.isDir ? 1 : 0), 0), [sorted]);

  const indexOf = useMemo(() => new Map(visible.map((e, i) => [e.path, i])), [visible]);

  // Reset per-folder UI state when the folder changes.
  useEffect(() => {
    setFilter("");
    setFilterMode(false);
    setSel(new Set());
    setFocus(null);
    setHeadFocus(null);
    setCollapsedNow(null);
    setAnchor(null);
    setScrollTop(0);
    if (listRef.current) listRef.current.scrollTop = 0;
    setExcerpts(new Map());
  }, [path, listRef]);

  /** Opens the collapsed group an entry is in (a new file, the folder we came from). */
  const reveal = (p: string) => {
    const inside = closed.size ? groupOf(p) : null;
    if (inside && closed.has(inside)) setCollapsed([...collapsed].filter((id) => id !== inside));
  };

  // The folder whose first item was selected on entry (see the effect below).
  const autoSelected = useRef<string | null>(null);

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
      // The effect below runs in this same commit and would see no pending selection: it would
      // select the first item over the requested one (a jump to a mark's item landed on the first row).
      autoSelected.current = path;
      scrollIntoView(p.focus ?? present[0]);
      return;
    }
    reveal(p.focus ?? p.paths[0]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexOf]);

  // The cursor was in a group that is collapsed now: it goes to the header.
  useEffect(() => {
    if (!focus || indexOf.has(focus) || !closed.size || pendingSelect.current) return;
    const inside = groupOf(focus);
    if (!inside) return;
    setFocus(null);
    setAnchor(null);
    setHeadFocus(inside);
    setVisual(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexOf]);

  // A freshly entered folder starts with its first item selected, so the cursor is
  // visible and Enter / arrows work at once (once per folder: later clicks on empty
  // space may still clear the selection).
  useEffect(() => {
    if (autoSelected.current !== path) autoSelected.current = null;
    if (autoSelected.current || dir.loading || pendingSelect.current) return;
    if (!visible.length) {
      // Everything here is collapsed: the cursor starts on the first header.
      if (heads?.length) { autoSelected.current = path; setHeadFocus(heads[0].id); }
      return;
    }
    autoSelected.current = path;
    setFocus(visible[0].path);
    // A folder picker chooses the folder it is in unless the user selects one: nothing is pre-selected.
    if (getState().pick?.mode !== "dir") setSel(new Set([visible[0].path]));
    setAnchor(visible[0].path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, dir.loading, visible]);

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
  const dpr = useDpr();
  // The icon and the row are sized in whole device pixels so the icon is centred on the pixel grid.
  const listIcon = snapCss(Math.round(settings.fontSize * 1.25), dpr);
  const rowH = alignedRowHeight(Math.round(settings.fontSize * 1.45 + settings.rowPadding * 2), listIcon, dpr);
  const isThumbs = view === "thumbs";
  // Tiles: as tall as the 40 px slot or the two text lines (name, details), whichever is more.
  const itemH = isGrid ? gridSize + 26
    : isThumbs ? Math.max(TILE + 4, Math.round(settings.fontSize * 2.35 + settings.rowPadding * 2))
    : rowH;
  sizeFilterRef.current = { isGrid, fontSize: settings.fontSize, compact: !!compact, itemH };
  const cols = isGrid ? Math.max(1, Math.floor((size.w - 8) / (gridSize + 8))) : 1;
  const cellW = isGrid ? Math.floor((size.w - 8) / cols) : size.w;
  const headH = snapCss(rowH + 4, dpr);
  const layout = useMemo(() => layoutOf(visible.length, heads, closed, cols, itemH, headH), [visible, heads, closed, cols, itemH, headH]);
  const totalH = layout.height + 8;
  const virtualH = isGrid ? size.h : quantizeListHeight(size.h, itemH);
  const { from: firstShown, to: lastShown, heads: shownHeads } = windowOf(layout, scrollTop, virtualH, 4 * itemH);
  const onHead = focus == null && headFocus != null && layout.headers ? layout.sections.find((s) => s.group?.id === headFocus) ?? null : null;

  const posOf = (i: number) => {
    const p = placeOf(layout, i);
    return { top: p.top, left: isGrid ? 4 + p.col * cellW : 0 };
  };
  const scrollTo = (top: number, h: number) => {
    const el = listRef.current;
    if (!el) return;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + h > el.scrollTop + el.clientHeight) el.scrollTop = top + h - el.clientHeight;
  };

  const scrollIntoView = useCallback(
    (p: string) => {
      const i = indexOf.get(p);
      if (i != null) scrollTo(placeOf(layout, i).top, itemH);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [indexOf, layout, itemH, listRef],
  );

  // Excerpts for thumbs view.
  useEffect(() => {
    if (!isThumbs) return;
    const want = visible
      .slice(firstShown, lastShown)
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
  }, [isThumbs, firstShown, lastShown, visible]);

  // ───────── selection helpers ─────────
  const selection = useCallback((): FileEntry[] => {
    if (sel.size) return visible.filter((e) => sel.has(e.path));
    const f = focus ? visible[indexOf.get(focus) ?? -1] : undefined;
    return f ? [f] : [];
  }, [sel, focus, visible, indexOf]);

  const focusedEntry = focus ? visible[indexOf.get(focus) ?? -1] ?? null : null;

  // `gall selection` reads what the user has selected in the pane they used last.
  useEffect(() => {
    if (!active) return;
    const t = setTimeout(() => {
      const items = sel.size ? visible.filter((e) => sel.has(e.path)).map((e) => e.path) : [];
      const focusedPath = focusedEntry?.path ?? null;
      const key = `${path}\0${focusedPath ?? ""}\0${items.join("\0")}`;
      if (lastPublishedSelection === key) return;
      lastPublishedSelection = key;
      api.selectionPublish(path, items, focusedPath).catch(() => {
        if (lastPublishedSelection === key) lastPublishedSelection = null;
      });
    }, 200);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, path, sel, focus, visible]);

  // Share focused entry with the details pane.
  useEffect(() => {
    setState((s) => {
      const focused = [...s.focused];
      focused[b] = focusedEntry;
      return { focused };
    });
  }, [focusedEntry, b]);

  const focusHead = (id: string) => {
    setFocus(null);
    setAnchor(null);
    setHeadFocus(id);
    if (!marking) setSel(new Set());
    const top = headTop(layout, id);
    if (top != null) scrollTo(top, headH);
  };
  /** Moves the cursor by lines, through the headers of the groups too. */
  const moveRows = (by: number, extend: boolean, keepSel: boolean) => {
    const i = focus ? indexOf.get(focus) : undefined;
    const at: Cursor | null = i != null ? { index: i } : onHead ? { head: onHead.group!.id } : null;
    // Without groups the last line is reached as before: the index is simply limited.
    if (!layout.headers) { moveFocus(i == null ? 0 : i + by * cols, extend, keepSel); return; }
    const to = moveCursor(layout, at, by, extend);
    if (!to) return;
    if ("index" in to) moveFocus(to.index, extend, keepSel);
    else focusHead(to.head);
  };
  const toggleGroup = (id: string, collapse = !collapsed.has(id)) => {
    if (collapse === collapsed.has(id)) return;
    // Only groups this folder still has are remembered (a filter hides some of them for now).
    const known = new Set(groups?.map((g) => g.id));
    const kept = [...collapsed].filter((x) => x !== id && (known.has(x) || !!filter || dir.loading));
    setCollapsed(collapse ? [...kept, id] : kept);
  };
  /** The group the cursor is in. */
  const currentGroup = () => onHead?.group?.id ?? (focus && indexOf.has(focus) ? groupAt(layout, indexOf.get(focus)!) : null);

  const moveFocus = (to: number, extend: boolean, keepSel: boolean) => {
    if (!visible.length) {
      const last = layout.sections[to > 0 ? layout.sections.length - 1 : 0]?.group;
      if (last) focusHead(last.id);
      return;
    }
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
  // As the sort: kept for this folder (and, like the view, those below), else this browser's.
  const setGrouping = (p: { group?: GroupMode; groupOrder?: GroupOrder }) => {
    if (!remember) { A.sessionCmd({ op: "view", b, prefs: p }); return; }
    api.setPrefs(path, p).then(reloadPrefs).catch(A.fail);
  };
  const toggleFoldersOnTop = () => setGrouping({ group: toggleFolders(group) });
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
        if (onHead) toggleGroup(onHead.group!.id);
        else A.openEntries(b, s);
        return true;
      case "groupNext": {
        const next = GROUP_MODES[(GROUP_MODES.indexOf(group) + 1) % GROUP_MODES.length];
        setGrouping({ group: next });
        A.notify(tt("Grouping: {label}", { label: tt(GROUP_LABEL[next]) }));
        return true;
      }
      case "groupOrderNext": {
        const next = GROUP_ORDERS[(GROUP_ORDERS.indexOf(groupOrder) + 1) % GROUP_ORDERS.length];
        setGrouping({ groupOrder: next });
        A.notify(tt("Order of the groups: {label}", { label: tt(GROUP_ORDER_LABEL[next]) }));
        return true;
      }
      case "groupToggle":
      case "groupCollapseAll":
      case "groupExpandAll": {
        if (!heads?.length) {
          A.notify(groups ? tt("The group headers are switched off (Settings › View)") : tt("The list is not grouped by type"));
          return true;
        }
        const id = currentGroup();
        if (a !== "groupToggle") setCollapsed(a === "groupCollapseAll" ? [...new Set([...collapsed, ...heads.map((g) => g.id)])] : []);
        else if (id) toggleGroup(id);
        return true;
      }
      case "rename":
        if (s.length === 1) A.renameEntry(b, s[0]);
        else if (s.length > 1) A.batchRename(b, s);
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
        else A.notify(tt("No folders here"), "info");
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
          A.noOtherBrowser(a === "copyToOther" ? tt("Copy to other browser") : tt("Move to other browser"));
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
        const half = Math.max(1, Math.floor((listRef.current?.clientHeight || size.h) / itemH / 2));
        const keep = marking && !visual;
        if (a === "cursorTop") moveFocus(0, visual, keep);
        else if (a === "cursorBottom") moveFocus(Infinity, visual, keep);
        else moveRows(a === "cursorDown" ? 1 : a === "cursorUp" ? -1 : a === "halfPageDown" ? half : -half, visual, keep);
        return true;
      }
      case "enter":
        // l on a header opens the group, then enters it.
        if (onHead) { if (onHead.collapsed) toggleGroup(onHead.group!.id, false); else moveRows(1, false, false); }
        else if (focusedEntry?.isDir) A.navigate(b, focusedEntry.path);
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
          A.notify(tt("Select an archive to extract (zip, 7z, tar, rar, …)"), "warn");
          return true;
        }
        const other = a === "extractToOther" ? A.otherTarget(b) : null;
        if (a === "extractToOther" && other == null) {
          A.noOtherBrowser(tt("Extract to other browser"));
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
        A.notify(tt("View: {label}", { label: next.label }));
        return true;
      }
      case "toggleFoldersOnTop":
        toggleFoldersOnTop();
        return true;
      case "contextMenu": {
        // Beside the focused item, whose element says where it is.
        const at = focus ?? s[0]?.path;
        const item = () => (at ? listRef.current?.querySelector(`[data-path="${CSS.escape(at)}"]`) ?? null : null);
        const open = () => {
          const list = listRef.current?.getBoundingClientRect(), row = item();
          openContextMenu(textAnchor(row, row?.querySelector(".name .txt, .gname"), { x: (list?.left ?? 0) + 40, y: (list?.top ?? 0) + 8 }), s.length ? s : null);
        };
        if (at) scrollIntoView(at);
        // Scrolled far away, the item is not in the (virtualised) list until it is rendered again.
        if (!at || item()) open();
        else requestAnimationFrame(() => requestAnimationFrame(open));
        return true;
      }
      default:
        return false;
    }
  };

  // Register controller for global shortcuts.
  const explicitSelection = () => visible.filter((e) => sel.has(e.path));
  const ctl = useRef({ handle, selection, explicitSelection, visible, focusedEntry, path, visual, reveal });
  ctl.current = { handle, selection, explicitSelection, visible, focusedEntry, path, visual, reveal };
  useEffect(() => {
    registerController(b, {
      path: () => ctl.current.path,
      entries: () => ctl.current.visible,
      selection: () => ctl.current.selection(),
      explicitSelection: () => ctl.current.explicitSelection(),
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
        } else if (paths.length) ctl.current.reveal(f ?? paths[0]);
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
    const page = Math.max(1, Math.floor((listRef.current?.clientHeight || size.h) / itemH) - 1);
    const extend = e.shiftKey || visual;
    const keep = (e.ctrlKey && !e.shiftKey) || (marking && !visual);
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        if (e.altKey) return;
        moveRows(1, extend, keep);
        break;
      case "ArrowUp":
        if (e.altKey) return;
        moveRows(-1, extend, keep);
        break;
      case "ArrowRight":
        if (e.altKey) return;
        if (onHead) handleAction("enter");
        else if (isGrid) moveFocus(i + 1, extend, keep);
        else if (focusedEntry?.isDir) A.navigate(b, focusedEntry.path);
        else handled = false;
        break;
      case "ArrowLeft":
        if (e.altKey) return;
        // On the header of an open group: collapses it. On a collapsed one: the parent folder, as elsewhere.
        if (onHead && !onHead.collapsed) toggleGroup(onHead.group!.id, true);
        else if (isGrid && i >= 0) moveFocus(i - 1, extend, keep);
        else goParent();
        break;
      case "Home":
        moveFocus(0, extend, keep);
        break;
      case "End":
        moveFocus(Infinity, extend, keep);
        break;
      case "PageDown":
        moveRows(page, extend, keep);
        break;
      case "PageUp":
        moveRows(-page, extend, keep);
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
        // Vim's h on the header of an open group collapses it (elsewhere: the parent folder).
        if (e.key === "h" && settings.vimKeys && !filterMode && onHead && !onHead.collapsed && !e.ctrlKey && !e.altKey && !e.metaKey) {
          toggleGroup(onHead.group!.id, true);
          break;
        }
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
      setHeadFocus(null);
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
    const onRow = (e.target as HTMLElement).closest("[data-path], [data-group]");
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
        const w = isGrid ? cellW - 8 : el.clientWidth;
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
  const openContextMenu = (anchor: Anchor, entries: FileEntry[] | null, extra: MenuItem[] = []) => {
    const items: MenuItem[] = [];
    // The other browser, when there is one to look at (not in Classic or a narrow window).
    const others = A.hasOtherTarget(b) ? A.otherBrowser(b) : null;
    const openWithEntries = entries?.filter((e) => !e.isDir) ?? [];
    let openWithItem: MenuItem | null = null;
    if (entries?.length) {
      const one = entries.length === 1 ? entries[0] : null;
      const dirs = entries.filter((e) => e.isDir);
      const files = openWithEntries;
      const platform = getState().info?.platform;
      const isMacApp = platform === "macos" && !!one?.isDir && one.name.toLowerCase().endsWith(".app");
      const isWinElevatable = platform === "windows" && !!one && !one.isDir && ["exe", "msi", "bat", "cmd", "ps1", "msc", "lnk", "vbs", "wsf"].includes(one.ext.toLowerCase());
      const isArchive = !!one && !one.isDir && isBrowsableArchive(one.name);
      items.push({ label: t("menu.open"), shortcut: "Enter", run: () => A.openEntries(b, entries) });
      if (isMacApp && one) {
        items.push({ label: tt("Show package contents"), icon: "open", run: () => A.navigate(b, one.path) });
      }
      if (isWinElevatable && one) {
        items.push({ label: tt("Run as administrator"), icon: "open", run: () => api.openElevated(one.path).catch(A.fail) });
      }
      if (isArchive && one) {
        items.push({ label: tt("Open in default app"), icon: "open", run: () => api.openPath(one.path).catch(A.fail) });
      }
      if (files.length) {
        const filePaths = files.map((f) => f.path);
        const baseSubmenu: MenuItem[] = [
          { label: tt("Loading applications…"), disabled: true },
          { separator: true },
          { label: tt("Choose another app…"), icon: "open", shortcut: "Ctrl+Enter", run: () => A.openWith(entries) },
          ...(platform === "windows" && filePaths.length === 1 ? [{ label: tt("Windows “Open with” dialog…"), run: () => api.openWithDialog(filePaths[0]).catch(A.fail) }] : []),
        ];
        openWithItem = { label: t("menu.openWith"), icon: "open", shortcut: "Ctrl+Enter", run: () => A.openWith(entries), submenu: baseSubmenu };
        items.push(openWithItem);
        items.push({ label: t("menu.edit"), icon: "rename", shortcut: "F4", run: () => A.editFiles(entries), ctrl: { label: t("menu.editWith"), run: () => A.editWith(entries) } });
      }
      if (one && (one.isSymlink || one.ext === "lnk")) {
        items.push({ label: tt("Open target location"), icon: "forward", run: () => void A.openTargetLocation(b, one) });
      }
      if (one?.isDir) items.push({ label: t("menu.openInEditor"), icon: "rename", shortcut: "F4", run: () => A.editFiles([one]), ctrl: { label: t("menu.openWith"), run: () => A.editWith([one]) } });
      if (one?.isDir) items.push({ label: t("menu.terminalHere"), icon: "terminal", shortcut: "F10", run: () => api.openTerminal(one.path).catch(A.fail), ctrl: { label: tt("Terminal here with…"), run: () => A.terminalWith(one.path) } });
      if (dirs.length) {
        items.push({ label: t("menu.openNewTab"), run: () => dirs.forEach((d) => A.sessionCmd({ op: "newTab", b, path: d.path, activate: false })) });
        items.push({ label: tt("Open in new window"), run: () => api.newWindow({ paths: dirs.map((d) => d.path) }).catch(A.fail) });
        items.push({ label: others != null ? t("menu.openOtherBrowser") : t("menu.openNewTab"), run: () => A.openInOther(b, dirs[0].path) });
        if (dirs.length === 1) items.push({ label: A.isPinned(dirs[0].path) ? t("menu.unpin") : t("menu.pin"), icon: "star", run: () => A.togglePin(dirs[0].path) });
      }
      if (files.length > 2)
        items.push({ label: tt("Compare {length} files (similarity)", { length: files.length }), icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.openOverlay({ type: "matrix", paths: files.slice(0, 30).map((f) => f.path) }) });
      if (files.length === 2 && entries.length === 2)
        items.push({ label: tt("Compare {name} ↔ {name2}", { name: files[0].name, name2: files[1].name }), icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compare({ kind: "file", path: files[0].path }, { kind: "file", path: files[1].path }) });
      if (one && !one.isDir) items.push(A.compareMenu(b, one, !!gitRoot));
      if (one && isPatchFile(one) && !archiveOf(path))
        items.push({ label: tt("Apply this patch…"), icon: "paste", run: () => A.openOverlay({ type: "applyPatch", root: path, file: one.path }) });
      if (dirs.length === 2 && entries.length === 2)
        items.push({ label: tt("Compare folders {name} ↔ {name2}", { name: dirs[0].name, name2: dirs[1].name }), icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compareFolders(dirs[0].path, dirs[1].path) });
      if (one?.isDir) items.push({ label: t("menu.compareFolderOther"), icon: "compare", shortcut: "Ctrl+Shift+D", run: () => A.compareFolderWithOther(b, one) });
      if (one?.ext === "2do" || one?.ext === "2dx")
        items.push({ label: one.ext === "2do" ? tt("Mark done") : tt("Mark not done"), run: () => api.toggleNote(one.path).catch(A.fail) });
      if (!archiveOf(path)) {
        if (searchHidden?.folder) {
          const f = searchHidden.folder;
          items.push({ label: f === path ? tt("Show this folder in search again") : tt("Show “{value}” in search again", { value: basename(f) }), icon: "search", run: () => A.showInSearch(f) });
        } else if (one && searchHidden?.entries[one.name]) {
          items.push({ label: t("menu.showInSearch"), icon: "search", run: () => A.showInSearch(one.path) });
        } else {
          const list = entries.filter((e) => !searchHidden?.entries[e.name]);
          if (list.length) items.push({ label: t("menu.hideFromSearch", { count: list.length }), icon: "search", run: () => A.hideFromSearch(list.map((e) => e.path)) });
        }
      }
      items.push({ separator: true });
      items.push({ label: t("menu.cut"), icon: "cut", shortcut: "Ctrl+X", run: () => A.copyToClipboard(entries, true) });
      items.push({ label: t("menu.copy"), icon: "copy", shortcut: "Ctrl+C", run: () => A.copyToClipboard(entries, false) });
      items.push({ label: t("menu.paste"), icon: "paste", shortcut: "Ctrl+V", run: () => A.paste(b) });
      if (one) items.push({ label: t("menu.duplicate"), shortcut: "Ctrl+D", run: () => handle("duplicate") });
      if (others != null) {
        items.push({ label: t("menu.copyToOther"), shortcut: "Alt+C", run: () => handle("copyToOther") });
        items.push({ label: t("menu.moveToOther"), shortcut: "Alt+M", run: () => handle("moveToOther") });
      }
      items.push({
        label: t("menu.copyPath"),
        shortcut: "Ctrl+Shift+C",
        submenu: (
          [["native", "As is"], ["nativeQuoted", "Quoted"], ["escaped", "Escaped \\\\"], ["forward", "Forward slashes /"], ["forwardQuoted", "Forward, quoted"], ["uri", "file:// URI"], ["linux", "Linux path (WSL)"]] as const
        ).filter(([f]) => f !== "linux" || getState().info?.platform === "windows").map(([f, l]) => ({
          label: l,
          run: async () => {
            const t = await Promise.all(entries.map((e) => api.pathFormat(e.path, f)));
            api.clipSetText(t.join("\n")).then(() => A.notify(t.length === 1 ? tt("Copied path of “{name}”", { name: entries[0].name }) : tt("Copied {length} paths", { length: t.length }))).catch(A.fail);
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
        items.push(extractItem(tt("Extract here"), "smart", () => path, "extract", "extractHere"));
        items.push(extractItem(stem ? tt("Extract to “{stem}/”", { stem }) : tt("Extract each to its own folder"), "folder", () => path, undefined, "extractToFolder"));
        if (others != null) items.push(extractItem(tt("Extract to other browser"), "smart", () => A.activePath(A.otherTarget(b) ?? others), undefined, "extractToOther"));
      }
      const base = archiveBaseName(entries, basename(path));
      const srcs = entries.map((e) => e.path);
      items.push({
        label: t("menu.compress"),
        icon: "archive",
        submenu: [
          ...CREATE_FORMATS.map(([f, ext]): MenuItem => ({ label: base + ext, action: f === "zip" ? "compressZip" : undefined, run: () => A.compress(srcs, path, base + ext, f) })),
          { separator: true },
          { label: t("menu.compressAs"), action: "compressAs", run: () => A.compressAs(srcs, path, base) },
          ...(others != null ? [{ label: t("menu.compressOther"), run: () => A.compress(srcs, A.activePath(A.otherTarget(b) ?? others), base + ".zip", "zip" as const) }] : []),
        ],
      });
      items.push({ separator: true });
      if (one) items.push({ label: t("menu.rename"), icon: "rename", shortcut: "F2", run: () => A.renameEntry(b, one) });
      else items.push({ label: tt("Rename {A}…", { A: A.items(entries.length) }), icon: "rename", shortcut: "F2", run: () => A.batchRename(b, entries) });
      items.push({ label: t("menu.colourTag"), icon: "tag", shortcut: "Ctrl+Alt+T", run: () => handle("tag") });
      if (dirs.length) items.push({ label: t("menu.calcSizes", { count: dirs.length }), icon: "info", action: "folderSizes", run: () => A.calcFolderSizes(b, dirs.map((d) => d.path)) });
      if (dirs.length && dirs.length === entries.length) items.push(A.folderColorMenu(dirs.map((d) => d.path)));
      items.push({ label: t("menu.moveToTrash"), icon: "trash", action: "delete", run: () => handle("delete") });
      items.push({ label: t("menu.deletePermanently"), danger: true, action: "deletePermanent", run: () => handle("deletePermanent") });
      items.push({ separator: true });
      if (one && !one.isDir) items.push({ label: t("menu.createTemplate"), run: () => A.addTemplateFrom(one) });
      items.push({ label: t("menu.showInFileManager"), action: "reveal", run: () => api.revealPath(entries[0].path).catch(A.fail) });
      if (one) items.push({ label: t("menu.properties"), icon: "info", shortcut: "Ctrl+I", run: () => A.openOverlay({ type: "properties", entry: one }) });
    } else {
      items.push({ label: t("menu.paste"), icon: "paste", shortcut: "Ctrl+V", run: () => A.paste(b) });
      items.push({ label: t("menu.pasteAsLinks"), shortcut: "Ctrl+Shift+V", disabled: !clipboard.paths.length, run: () => A.pasteAsLinks(b) });
      items.push({ separator: true });
      items.push({ label: t("menu.newFolder"), icon: "newFolder", shortcut: "Ctrl+Shift+N", run: () => A.newFolder(b) });
      items.push({ label: t("menu.newFileTemplate"), icon: "newFile", shortcut: "Ctrl+N", run: () => A.openOverlay({ type: "templates", dir: path, b }) });
      items.push({ label: t("menu.newNote"), icon: "note", shortcut: "Alt+N", run: () => A.openOverlay({ type: "note", dir: path }) });
      items.push({ separator: true });
      items.push({ label: t("menu.sortBy"), icon: "sort", submenu: sortMenu() });
      items.push({ label: tt("Group by"), icon: "foldersTop", action: "groupNext", submenu: groupMenu() });
      items.push({ label: t("menu.view"), icon: "view", action: "viewNext", submenu: VIEWS.map((v) => ({ label: v.label, checked: view === v.id, run: () => setView(v.id) })) });
      items.push({ label: t("menu.foldersOnTop"), action: "toggleFoldersOnTop", checked: spec.foldersOnTop, run: toggleFoldersOnTop });
      items.push({ label: t("menu.showHidden"), checked: showHidden, action: "toggleHidden", run: () => A.toggleHidden(b) });
      items.push({
        label: t("menu.folderSizes"), icon: "info", submenu: [
          { label: t("menu.calcNow"), action: "folderSizes", run: () => handle("folderSizes") },
          { label: t("menu.calcAuto"), checked: settings.folderSizesAuto, run: () => A.updateSettings({ folderSizesAuto: !settings.folderSizesAuto }) },
        ],
      });
      items.push({
        label: t("menu.defaultDrop"),
        submenu: ([["consistent", "Move"], ["copy", "Copy"], ["system", "Move on same drive, copy across drives"], ["ask", "Ask (show menu)"]] as const).map(([v, l]) => ({
          label: l, checked: settings.dragBehavior === v, run: () => A.updateSettings({ dragBehavior: v }),
        })),
      });
      items.push({ separator: true });
      items.push({ label: t("menu.selectAll"), shortcut: "Ctrl+A", run: () => handle("selectAll") });
      items.push({ label: t("sidebar.refresh"), shortcut: "F5", run: () => dir.reload() });
      items.push({ label: t("menu.terminalHere"), icon: "terminal", shortcut: "F10", run: () => api.openTerminal(path).catch(A.fail), ctrl: { label: tt("Terminal here with…"), run: () => A.terminalWith(path) } });
      items.push({ label: t("menu.openFolderEditor"), icon: "rename", run: () => A.editPaths([path]), ctrl: { label: t("menu.openFolderWith"), run: () => A.openOverlay({ type: "openWith", paths: [path], mode: "editor" }) } });
      items.push({ label: t("menu.copyFolderPath"), run: () => A.copyPaths([], path) });
    }
    if (extra.length) items.push({ separator: true }, ...extra);
    const menuToken = A.showMenuAt(anchor, items);
    const updateMenu = (updater: (cur: MenuItem[]) => MenuItem[]) => A.updateOpenMenu(menuToken, updater);
    if (openWithItem && openWithEntries.length) {
      const targetItem = openWithItem;
      const filePaths = openWithEntries.map((f) => f.path);
      const platform = getState().info?.platform;
      void api.openWithApps(filePaths[0]).then((apps) => {
        const appItems: MenuItem[] = apps.slice(0, 12).map((app) => ({
          label: app.name,
          checked: app.default,
          run: () => api.openWithRun(filePaths, app.command).catch(A.fail),
        }));
        const nextSubmenu: MenuItem[] = [
          ...appItems,
          ...(appItems.length ? [{ separator: true }] : []),
          { label: tt("Choose another app…"), icon: "open", shortcut: "Ctrl+Enter", run: () => A.openWith(entries ?? []) },
          ...(platform === "windows" && filePaths.length === 1 ? [{ label: tt("Windows “Open with” dialog…"), run: () => api.openWithDialog(filePaths[0]).catch(A.fail) }] : []),
        ];
        updateMenu((cur) => cur.map((it) => (it === targetItem ? { ...it, submenu: nextSubmenu } : it)));
      }).catch(() => {
        const fallbackSubmenu: MenuItem[] = [
          { label: tt("Choose another app…"), icon: "open", shortcut: "Ctrl+Enter", run: () => A.openWith(entries ?? []) },
          ...(platform === "windows" && filePaths.length === 1 ? [{ label: tt("Windows “Open with” dialog…"), run: () => api.openWithDialog(filePaths[0]).catch(A.fail) }] : []),
        ];
        updateMenu((cur) => cur.map((it) => (it === targetItem ? { ...it, submenu: fallbackSubmenu } : it)));
      });
    }
    // actions.d entries (LocalSend, Transcode…) match the selection asynchronously and are
    // appended to this very menu when they arrive.
    void appendActionsToMenu(items, entries ?? [], path, A.fail, updateMenu);
  };

  /** The grouping, with where it applies. */
  const groupMenu = (): MenuItem[] => [
    { label: !remember ? tt("In this browser, every folder") : prefs?.groupSource && prefs.groupSource !== path ? tt("Set in {folder}, here from now on", { folder: basename(prefs.groupSource) || prefs.groupSource }) : prefs?.groupSource ? tt("Set for this folder and those below") : tt("The default; a choice applies to this folder and those below"), disabled: true },
    ...GROUP_MODES.map((m): MenuItem => ({ label: GROUP_LABEL[m], checked: group === m, run: () => setGrouping({ group: m }) })),
    { separator: true },
    { label: tt("Order of the groups"), action: "groupOrderNext", disabled: !byType(group), submenu: GROUP_ORDERS.map((o): MenuItem => ({ label: GROUP_ORDER_LABEL[o], checked: groupOrder === o, run: () => setGrouping({ groupOrder: o }) })) },
    { label: tt("Kinds (images, documents…) instead of extensions"), checked: settings.groupByKind, disabled: !byType(group), run: () => A.updateSettings({ groupByKind: !settings.groupByKind }) },
    { label: tt("Group headers"), checked: settings.groupHeaders, disabled: !byType(group), run: () => A.updateSettings({ groupHeaders: !settings.groupHeaders }) },
    { label: tt("Collapse all groups"), action: "groupCollapseAll", disabled: !heads?.length, run: () => handle("groupCollapseAll") },
    { label: tt("Expand all groups"), action: "groupExpandAll", disabled: !collapsed.size, run: () => handle("groupExpandAll") },
    { separator: true },
    { label: tt("Use this grouping by default everywhere"), disabled: settings.group === group && settings.groupOrder === groupOrder, run: () => A.updateSettings({ group, groupOrder }) },
  ];

  const onHeadContext = (e: React.MouseEvent, g: Group) => {
    e.preventDefault();
    e.stopPropagation();
    paneRef.current?.focus();
    focusHead(g.id);
    const isClosed = closed.has(g.id);
    A.showMenuAt(eventAnchor(e), [
      { label: isClosed ? tt("Expand group") : tt("Collapse group"), action: "groupToggle", run: () => toggleGroup(g.id) },
      { label: tt("Select this group"), disabled: isClosed, run: () => selectGroup(g.id) },
      { separator: true },
      ...groupMenu(),
    ]);
  };
  const selectGroup = (id: string) => {
    const s = layout.sections.find((x) => x.group?.id === id);
    const paths = s ? visible.slice(s.start, s.start + s.count).map((x) => x.path) : [];
    if (!paths.length) return;
    setSel(new Set(paths));
    setFocus(paths[0]);
    setAnchor(paths[0]);
  };

  const sortMenu = (withGroups = false): MenuItem[] => [...sortItems(), ...(withGroups ? [{ separator: true }, { label: tt("Group by"), icon: "foldersTop", action: "groupNext" as Action, submenu: groupMenu() }] : [])];
  const sortItems = (): MenuItem[] =>
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
    openContextMenu(eventAnchor(e), s);
  };

  // ───────── column visibility by available width (removal order) ─────────
  // Widths measured in characters so the thresholds follow the font size.
  const chars = size.w / (settings.fontSize * 0.6);
  const dateSort = spec.key === "created";
  const fullDates = chars > (compact ? 66 : 62);
  const oneDateColumn = view !== "powershell" && !fullDates;
  const show = {
    size: !longNames && chars > (compact ? 30 : 26),
    age: !longNames && chars > (compact ? 40 : 36) && (!oneDateColumn || !dateSort),
    date: !longNames && (oneDateColumn ? dateSort && chars > (compact ? 45 : 41) : chars > (compact ? 52 : 48)),
    ext: !longNames && chars > 58,
    time: !longNames && chars > 66,
    meta: !longNames && chars > 76,
  };
  // Owner column (Linux / macOS "Mode" view): as wide as the longest owner here, 4–12 characters.
  const ownerW = useMemo(() => `${Math.min(12, maxOf(dir.entries, (e) => e.owner?.length ?? 0, 4)) + 1.2}ch`, [dir.entries]);
  const scrollbarW = size.scrollbarW;

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
    const gs = git ? git.all ?? git.entries[IS_MAC ? entry.name.normalize("NFC") : entry.name] : undefined;
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
    const hiddenDates = view !== "powershell" && view !== "names" && !longNames && oneDateColumn
      ? [
        !show.age && entry.modified != null && `${tt("Modified")}: ${new Date(entry.modified).toLocaleString()}`,
        !show.date && entry.created != null && `${tt("Date created")}: ${new Date(entry.created).toLocaleString()}`,
      ].filter(Boolean)
      : [];
    const nameCell = (
      <div className="name" title={hiddenDates.length ? [entry.name, ...hiddenDates].join("\n") : undefined}>
        {isNote ? (
          <input type="checkbox" className="note-check" checked={entry.ext === "2dx"} onClick={(e) => e.stopPropagation()}
            onChange={() => api.toggleNote(entry.path).catch(A.fail)} />
        ) : isThumbs && IMG.has(entry.ext) && !settings.noThumbnailExts.includes(entry.ext) && !entry.cloudOnly ? (
          <img className="thumb" src={fileSrc(entry.path)} loading="lazy" decoding="async" draggable={false} style={{ width: TILE, height: TILE }} />
        ) : isThumbs ? (
          // Same slot as a thumbnail, so names line up.
          <span style={{ width: TILE, display: "inline-flex", justifyContent: "center", flexShrink: 0 }}><FileIcon entry={entry} size={32} /></span>
        ) : (
          <FileIcon entry={entry} size={listIcon} />
        )}
        {isThumbs ? (
          <div className="stack">
            <span className="txt">{stem}<span className="extpart">{extPart}</span></span>
            <span className="sub">{excerpts.get(entry.path) ?? (entry.isDir ? tt("Folder") : `${formatSize(entry.size)} · ${formatDate(entry.modified)} ${formatTime(entry.modified)}`)}</span>
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
          : show.meta && <span className="meta">{tt("link")}</span>)}
      </div>
    );
    const ageInfo = entry.modified != null ? age(entry.modified, now) : null;
    const ageCell = show.age && (
      <span className="col fp-age-col" style={{ width: CW.age, textAlign: "center" }}>
        {ageInfo && (() => {
          const tier = ageTier(entry.modified!, now);
          const c = ageColor(tier, settings.ageSaturation, settings.ageLightness);
          return <span className="age" title={tt("Modified {value} ago ({label}) · {value2}\nAge: ' minutes, h hours, d days, M months, Y years", { value: ageWords(entry.modified!, now), label: AGE_TIERS[tier].label, value2: new Date(entry.modified!).toLocaleString() })} style={{ background: c.bg, color: c.fg, boxShadow: `inset 0 0 0 1px ${c.border}` }}>{ageInfo.label}</span>;
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
        {show.date && <span className="col fp-date-col" style={{ width: CW.date }} title={entry.created == null ? undefined : new Date(entry.created).toLocaleString()}>{formatDate(entry.created)}</span>}
        {show.time && <span className="col" style={{ width: CW.time }}>{formatTime(entry.created)}</span>}
        {show.size && (
          <span className={`col fp-size-col ${entry.isDir && folderSizes[entry.path] && !folderSizes[entry.path].done ? "counting" : ""}`} style={{ width: CW.size }}
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
        title={tt("Sort by {title}{value}", { title: tt(title), value: on ? ` (${tt(spec.descending ? "descending" : "ascending")})` : "" })}
        onClick={() => setSort(k, on ? !spec.descending : false)}>
        {/* Arrow sits on the side away from the text edge, so labels line up with cell text. */}
        {on && align === "right" && <span className="arrow">{spec.descending ? "↓" : "↑"}</span>}
        <span>{tt(label)}</span>
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
        openContextMenu(eventAnchor(e), null);
      }}>
        <div className="fp-heading">
          <div className="fp-title-row">
            <div className="fp-title" title={platformLabel(`${path}\nClick to edit (Ctrl+L)`)} onClick={() => A.openOverlay({ type: "pathEdit", b })}>{basename(path) || path}</div>
            <button className={`pin-btn ${pinned ? "on" : ""}`} title={`${pinned ? t("menu.unpin") : t("menu.pin")}${pinKeys ? ` (${pinKeys})` : ""}`} onClick={() => A.togglePin(path)}><Icon name="star" size={14} /></button>
            {visual && (
              <span className="fp-visual" title={tt("Visual mode: j/k, gg/G extend the selection; y y, d d, D, c, C, M … act on it; Esc or v cancels")}>{tt("-- VISUAL --")} {sel.size}
              </span>
            )}
            {searchHidden?.folder && searchHidden.folderRule && (
              <span className="fp-archive fp-search-hidden" title={tt("Hidden from search by “{line}” in {file}\nQuick Open, find in files and F3 skip this folder. Click to show it in search again.", { line: searchHidden.folderRule.line, file: searchHidden.folderRule.file })}
                onClick={() => A.showInSearch(searchHidden.folder!)}>
                <Icon name="search" size={12} /> {tt("hidden from search")}
              </span>
            )}
            {git?.linked && (
              <button className="fp-worktree" style={{ borderColor: worktreeColor(git.root) }} title={tt("Worktree {path}\nClick to switch to another one", { path: git.root })} onClick={() => void A.openWorktrees(path)}>
                <WorktreeDot path={git.root} /><span>{tt("worktree")} {basename(git.root)}</span>
              </button>
            )}
            {archiveOf(path) && (
              <span className="fp-archive" title={platformLabel(tt("Inside {value}\nRead-only: copy items out (y y / Ctrl+C, then paste), drag them to the other browser, or extract the archive.", { value: archiveOf(path) ?? "" }))}>
                <Icon name="archive" size={12} /> {tt("archive · read-only")}
              </span>
            )}
          </div>
          {/* Quick filter: a notification-style card in the free space of the folder header. */}
          {(filter || filterMode) && (
            <div className={`quick-filter ${filterMode ? "typing" : ""} ${filter && !sorted.length ? "none" : ""}`} role="search">
              <span className="qf-icon"><Icon name="search" size={14} /></span>
              <span className="qf-text">
                {filterMode && <span className="qf-slash">/</span>}
                {filter}
                {filterMode && <span className="qf-caret" />}
                {filterMode && !filter && <span className="qf-ph">{tt("type to filter this folder")}</span>}
              </span>
              {filter && <span className="qf-count">{sorted.length} / {dir.entries.length}</span>}
              <kbd className="qf-esc">{tt("Esc")}</kbd>
              <button className="qf-x" title={tt("Clear filter (Esc)")} onClick={() => { setFilter(""); setFilterMode(false); }}><Icon name="close" size={12} /></button>
            </div>
          )}
        </div>
        {hiddenOverride !== null && (
          <div className={`fp-hidden-badge ${hiddenOverride ? "on" : "off"}`}>
            <button
              type="button"
              className="fp-hidden-toggle"
              title={hiddenOverride
                ? `${tt("Hidden files shown in this browser only. Click to hide here")}${hiddenKeys ? ` (${hiddenKeys})` : ""}`
                : `${tt("Hidden files hidden in this browser only. Click to show here")}${hiddenKeys ? ` (${hiddenKeys})` : ""}`}
              aria-label={hiddenOverride ? tt("Hidden files shown in this browser only") : tt("Hidden files hidden in this browser only")}
              onClick={() => A.toggleHidden(b)}
            >
              {hiddenOverride ? <span className="fp-hidden-dot" aria-hidden="true" /> : <Icon name="eyeOff" size={13} />}
              {hiddenOverride && hiddenCount > 0 && <span className="fp-hidden-count">{hiddenCount}</span>}
              <Icon name={vertical ? (b === 0 ? "paneTop" : "paneBottom") : (b === 0 ? "paneLeft" : "paneRight")} size={13} />
            </button>
            <i className="fp-hidden-sep" aria-hidden="true" />
            <button
              type="button"
              className="fp-hidden-sync"
              title={hiddenOverride
                ? `${tt("Show hidden files in both browsers")}${hiddenAllKeys ? ` (${hiddenAllKeys})` : ""}`
                : `${tt("Hide hidden files in both browsers")}${hiddenAllKeys ? ` (${hiddenAllKeys})` : ""}`}
              aria-label={hiddenOverride ? tt("Show hidden files in both browsers") : tt("Hide hidden files in both browsers")}
              onClick={() => A.applyHiddenAll(hiddenOverride)}
            >
              <Icon name={vertical ? "paneBothV" : "paneBoth"} size={13} />
            </button>
          </div>
        )}
        <div className="fp-pill">
          <button title={tt("Sort · grouping: {label}", { label: tt(GROUP_LABEL[group]) })} onClick={(e) => A.showMenuFor(e, sortMenu(true))}>
            <Icon name="sort" size={13} /><span>{tt(SORT_LABEL[spec.key])}</span><span className="dir">{spec.descending ? "↓" : "↑"}</span>
          </button>
          <i className="sep" />
          <button title={tt("File view")} onClick={(e) => A.showMenuFor(e, VIEWS.map((v) => ({ label: v.label, checked: view === v.id, run: () => setView(v.id) })))}>
            <Icon name="view" size={13} /><span>{tt(VIEWS.find((v) => v.id === view)?.label.split(" ")[0] ?? "View")}</span>
          </button>
        </div>
        <button className="fp-undo" title={`${undoHere ? tt("Undo in this folder: {undoHere}", { undoHere }) : tt("Nothing to undo in this folder")}\nUndo history lasts until Gall is closed`} disabled={!undoHere} onClick={() => A.undoIn(path)}><Icon name="undo" size={15} /></button>
        {/* The same menu as a right-click on the empty list, plus a few folder-wide extras. */}
        <button className="fp-more" title={tt("More actions")} onClick={(e) => openContextMenu(eventAnchor(e), null, [
          { label: pinned ? t("menu.unpin") : t("menu.pin"), icon: "star", shortcut: "Ctrl+B", run: () => A.togglePin(path) },
          A.folderColorMenu([path]),
          { label: t("menu.compareOtherFolder"), icon: "compare", shortcut: "Ctrl+Alt+D", run: () => A.comparePanelFolders(b) },
        ])}><Icon name="more" size={16} /></button>
      </div>
      {/* The header is laid out as a .row with the same cells as the list rows (same padding,
          border, gap and column widths), so labels always line up with the values below. */}
      <div className="fp-nav" style={{ paddingRight: scrollbarW }} onContextMenu={(e) => {
        e.preventDefault();
        A.showMenuAt(eventAnchor(e), sortMenu(true));
      }}>
        <div className="row fp-hrow">
          <div className="name fp-nav-name">
            <button className="icon-btn" title={tt("Parent folder (Backspace)")} disabled={!parent(path)} onClick={goParent}><Icon name="up" /></button>
            <button className="icon-btn" title={tt("Back (Alt+←)")} onClick={() => A.sessionCmd({ op: "back", b })}><Icon name="back" /></button>
            <button className="icon-btn" title={tt("Forward (Alt+→)")} onClick={() => A.sessionCmd({ op: "forward", b })}><Icon name="forward" /></button>
            <button className={`icon-btn ${spec.foldersOnTop ? "active" : ""}`} title={tt("Folders on top")} onClick={toggleFoldersOnTop}><Icon name="foldersTop" /></button>
            {!isGrid && view !== "powershell" && <span className="name-head"><i className="nav-sep" />{sortBtn("name", "name", "Name", "left")}</span>}
          </div>
          {!isGrid && view !== "powershell" && view !== "names" && (
            <>
              {show.ext && <span className="col" style={{ width: CW.ext }}>{sortBtn("ext", "type", "Type")}</span>}
              {show.age && <span className="col fp-age-col" style={{ width: CW.age }}>{sortBtn("modified", "age (date modified)", "Age", "center")}</span>}
              {show.date && <span className="col fp-date-col" style={{ width: CW.date }}>{sortBtn("created", "date created", "Date")}</span>}
              {show.time && <span className="col" style={{ width: CW.time }} />}
              {show.size && <span className="col fp-size-col" style={{ width: CW.size }}>{sortBtn("size", "size", "Size")}</span>}
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
          if ((e.target as HTMLElement).closest("[data-path], [data-group]")) return;
          e.preventDefault();
          setSel(new Set());
          openContextMenu(eventAnchor(e), null);
        }}
        onDragOver={(e) => {
          if (!isFileDrag(e)) return;
          const own = draggedPaths(e.dataTransfer);
          if (own.length && own.every((p) => parent(p) === path)) return hintNoop(tt("Already in this folder")); // same folder: no-op
          onDragOverTarget(e, path);
        }}
        onDrop={(e) => dropInto(e, path)}
      >
        <div style={{ height: totalH, position: "relative" }}>
          {shownHeads.map((s) => {
            const g = s.group!, label = groupLabel(g);
            return (
              <div key={`group ${g.id}`} data-group={g.id} role="button" aria-expanded={!s.collapsed} className={`fp-group ${onHead === s ? "focused" : ""}`}
                style={{ top: s.top, height: s.head }} title={s.collapsed ? tt("Expand group") : tt("Collapse group")}
                onClick={() => { paneRef.current?.focus(); A.focusBrowser(b); focusHead(g.id); toggleGroup(g.id); }}
                onContextMenu={(e) => onHeadContext(e, g)}>
                <Icon name="chevronRight" size={12} style={s.collapsed ? undefined : { transform: "rotate(90deg)" }} />
                <b>{label}</b><span className="n">{g.count}</span><i />
                {g.id !== "dir" && <span className="size">{formatSize(g.size)}</span>}
              </div>
            );
          })}
          {visible.slice(firstShown, lastShown).map((e, k) => renderRow(e, firstShown + k))}
        </div>
        {rubber && (
          <div className="rubber" style={{
            left: Math.min(rubber.x0, rubber.x1), top: Math.min(rubber.y0, rubber.y1),
            width: Math.abs(rubber.x1 - rubber.x0), height: Math.abs(rubber.y1 - rubber.y0),
          }} />
        )}
        {!dir.loading && !sorted.length && (tandemMissing && tandemPair
          ? <div className="tandem-ghost" role="status" style={{ "--pair-color": pairColor(tandemPair) } as React.CSSProperties}>
              <div className="tg-badge">🔗 {t("pair.tandem")}</div>
              <div className="tg-title">{t("pair.missingTitle")}</div>
              <div className="tg-path" title={path}>
                {tandemRelativeDisplay(path, b === 0 ? tandemPair.leftRoot : tandemPair.rightRoot, home)}
              </div>
              <div className="tg-actions">
                <button
                  type="button"
                  className="btn primary"
                  title={t("pair.createFolderHint", { path })}
                  onClick={() => { void A.createTandemFolder(path, () => dir.reload()); }}
                >
                  {t("pair.createFolder")}
                </button>
                <button
                  type="button"
                  className="btn"
                  title={t("pair.openParentHint")}
                  onClick={() => { void A.openTandemParent(b, path, b === 0 ? tandemPair.leftRoot : tandemPair.rightRoot); }}
                >
                  {t("pair.openParent")}
                </button>
                <button
                  type="button"
                  className="btn"
                  title={t("pair.unpairHint")}
                  onClick={() => {
                    A.unpairTabs(tandemPair.id);
                    void A.leaveMissing(b, path);
                  }}
                >
                  {t("pair.unlink")}
                </button>
              </div>
            </div>
          : dir.error
          ? <div className="empty-msg error" role="alert">
              <Icon name="error" size={18} /><span>{A.readableError(dir.error)}</span>
              {dir.error.includes("archive-password:") && (
                <button className="btn primary" onClick={() => A.askArchivePassword(dir.error).then((ok) => ok && dir.reload())}>{tt("Enter password…")}</button>
              )}
            </div>
          : <div className="empty-msg">{filter ? tt("No items match “{filter}”", { filter }) : tt("This folder is empty")}</div>)}
      </div>
      <div className="fp-status">
        {/* Waybar-style modules: Nerd Font glyph + value. BMP (Font Awesome) code points:
            WebKitGTK does not render the U+F0000+ Material range. */}
        <span className="mod" title={A.items(sorted.length)}><Glyph nf={"\u{F07B}"} icon="folder" />{dirCount}</span>
        <span className="mod"><Glyph nf={"\u{F15B}"} icon="file" />{sorted.length - dirCount}</span>
        {filter && <span className="mod accent" title={tt("Filtered from {length}", { length: dir.entries.length })}><Glyph nf={"\u{F0B0}"} icon="search" />{filter}</span>}
        <span style={{ flex: 1 }} />
        {git && (
          <button className="mod git" onClick={() => A.openOverlay({ type: "gitGraph", path: git.root })} title={tt("Git: {value}{value2}{value3}{value4}{value5} · {root}", { value: git.branch ? `branch ${git.branch}` : "detached HEAD", value2: git.ahead ? `, ${git.ahead} to push` : "", value3: git.behind ? `, ${git.behind} to pull` : "", value4: git.containingBranches?.length ? `, contained in ${git.containingBranches.join(", ")}` : "", value5: git.subject ? ` · ${git.subject}` : "", root: git.root })}>
            <Glyph nf={"\u{E0A0}"} icon="code" />{git.branch ?? (git.containingBranches?.length ? git.containingBranches.join(", ") : "detached")}{git.subject ? ` · ${git.subject}` : ""}{git.ahead ? ` ↑${git.ahead}` : ""}{git.behind ? ` ↓${git.behind}` : ""}
          </button>
        )}
        {!git && gitRefused && (
          <button className="mod git refused" onClick={() => void A.trustGitFolder(path)} title={tt("Git refuses this repository (owned by another user): {gitRefused}. Click to trust it{value}.", { gitRefused, value: settings.vimKeys ? ", or press gr" : "" })}>
            <Glyph nf={"\u{E0A0}"} icon="code" />{tt("Git blocked")}
          </button>
        )}
        {sel.size > 0 && <span className="mod accent"><Glyph nf={"\u{F14A}"} icon="check" />{sel.size}{selBytes ? ` · ${formatSize(selBytes)}` : ""}</span>}
      </div>
    </div>
  );
}
