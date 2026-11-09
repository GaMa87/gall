// DTOs mirrored from the Rust core (serde camelCase).

export type EntryKind = "dir" | "file" | "symlink" | "other";

export interface FileEntry {
  name: string;
  path: string;
  kind: EntryKind;
  isDir: boolean;
  isHidden: boolean;
  isSymlink: boolean;
  size: number;
  modified: number | null;
  created: number | null;
  ext: string;
  readonly: boolean;
  /** Kept only in the cloud (OneDrive Files On-Demand): reading it downloads it, so no
   *  thumbnail or excerpt. Absent when false. */
  cloudOnly?: boolean;
  /** Unix only: `st_mode` of the item itself (a symlink's own, as `ls -l` shows it). */
  mode?: number;
  /** Unix only: the owner's user name. */
  owner?: string;
  /** Where a symlink (Windows: also a junction) points, as written in the link. */
  linkTarget?: string;
  /** Windows only: the item's own file attributes, for PowerShell's `Mode` column. */
  attributes?: number;
}

export type DiffSource =
  | { kind: "file"; path: string }
  | { kind: "gitHead"; path: string }
  | { kind: "gitCommit"; path: string; commit: string }
  | { kind: "text"; label: string; text: string };

export interface DiffOptions {
  ignoreWhitespace: boolean;
  ignoreCase: boolean;
  ignoreEol: boolean;
  /** Fewest changed lines (Myers) instead of the more readable histogram diff. */
  minimal: boolean;
}

export interface DiffSide { label: string; path: string | null; size: number; crlf: boolean; finalNewline: boolean }
export interface DiffOp { tag: "equal" | "delete" | "insert" | "replace"; a: number; aLen: number; b: number; bLen: number }
/** `len` lines removed at left `a` reappear unchanged at right `b`. */
export interface DiffMove { a: number; b: number; len: number }
export interface DiffLinePair { a: number; b: number; aSpans: [number, number][]; bSpans: [number, number][] }
/** Syntax colours of one compare side: per line [start, end, category] (UTF-16). */
export interface SyntaxHighlight {
  language: string;
  lines: [number, number, number][][];
}

export interface DiffResult {
  left: DiffSide;
  right: DiffSide;
  text: { leftLines: string[]; rightLines: string[]; ops: DiffOp[]; pairs: DiffLinePair[]; added: number; removed: number; ratio: number; moves: DiffMove[] } | null;
  binary: { identical: boolean; firstDifference: number | null; reason: string } | null;
}

export interface ImageDiff { leftSize: [number, number]; rightSize: [number, number]; compared: number; changed: number; maxDelta: number; diffPng: string }

export interface SimilarityMatrix { paths: string[]; names: string[]; sizes: number[]; kinds: string[]; ratios: number[][]; order: number[] }

export interface TableDiffOptions { key: string | null; header: boolean; numeric: boolean; tolerance: number; ignoreWhitespace: boolean; ignoreCase: boolean; formulas: boolean }
export interface TableColumn { name: string; left: number | null; right: number | null }
export interface TableRowDiff { status: "same" | "added" | "removed" | "changed"; left: number | null; right: number | null; cells: number[] }
export interface SheetDiff {
  name: string;
  presence: "both" | "leftOnly" | "rightOnly";
  columns: TableColumn[];
  key: string | null;
  keyCandidates: string[];
  leftRows: string[][];
  rightRows: string[][];
  rows: TableRowDiff[];
  counts: { same: number; added: number; removed: number; changed: number; cells: number };
  truncated: boolean;
}
export interface TableDiffResult { sheets: SheetDiff[] }

export interface DirCmpOptions { content: boolean; hidden: boolean; respectGitignore: boolean }
export interface DirMeta { isDir: boolean; size: number; mtime: number }
export type DirStatus = "same" | "different" | "leftOnly" | "rightOnly";
export interface DirItem {
  rel: string;
  name: string;
  depth: number;
  isDir: boolean;
  status: DirStatus;
  reason: "content" | "size" | "date" | "type" | null;
  newer: "left" | "right" | null;
  left: DirMeta | null;
  right: DirMeta | null;
}
export interface DirCmpResult {
  left: string;
  right: string;
  items: DirItem[];
  counts: { same: number; different: number; leftOnly: number; rightOnly: number };
  truncated: boolean;
  elapsedMs: number;
}
export type SyncOp = { op: "copy"; src: string; dest: string } | { op: "trash"; path: string };
export interface SyncReport { copied: number; replaced: number; trashed: number; bytes: number }

/** Fallback info for a video the built-in player cannot play (video_probe; needs FFmpeg). */
export interface VideoProbe {
  platform: string;
  ffmpeg: boolean;
  codec: string | null;
  codecLong: string | null;
  profile: string | null;
  width: number | null;
  height: number | null;
  duration: number | null;
  poster: string | null;
}

export interface SysStats {
  host: string;
  os: string;
  cpuModel: string;
  cpuCores: number;
  cpu: number;
  cpuTemp: number | null;
  gpu: number | null;
  gpuTemp: number | null;
  /** Dedicated GPU memory in use / in total, bytes (the dedicated GPU; null if unknown). */
  gpuMemUsed: number | null;
  gpuMemTotal: number | null;
  memUsed: number;
  memTotal: number;
  netRx: number;
  netTx: number;
  diskRead: number | null;
  diskWrite: number | null;
}

export interface Drive {
  short: string;
  label: string;
  mount: string;
  fs: string;
  total: number;
  available: number;
  removable: boolean;
  /** Unlocked encrypted volume that can be locked again. */
  lockable: boolean;
  /** Mount of the row that already shows this one's space ("~" on the root partition → "/"). */
  spaceShownBy?: string;
}

/** A volume that is not mounted yet: a partition to mount or an encrypted one to unlock. */
export interface Volume { id: string; label: string; size: number; fs: string; locked: boolean; removable: boolean }

export type CloudProvider = "onedrive" | "dropbox" | "gdrive" | "icloud" | "box" | "mega" | "pcloud" | "nextcloud" | "proton" | "other";

/** A cloud storage folder found on this computer. */
export interface CloudFolder {
  provider: CloudProvider;
  name: string;
  path: string;
}

export type SortKey = "name" | "ext" | "modified" | "created" | "size";
export type Layout = "standard" | "columns" | "classic";
export type ThemeMode = "system" | "dark" | "light";
export type PathFormat = "native" | "nativeQuoted" | "escaped" | "forward" | "forwardQuoted" | "uri" | "linux";

export interface Tab {
  id: number;
  path: string;
  alias: string | null;
  keepAlias: boolean;
  history: { back: string[]; forward: string[] };
  lastUsed: number;
}

export interface BrowserState {
  tabs: Tab[];
  active: number;
  closed: Tab[];
  collapsed: boolean;
  /** Preview & details pane; null = the showDetailsPane setting. */
  details?: boolean | null;
  /** Miller columns (true) or breadcrumbs (false), chosen per layout; a missing layout uses its default. */
  columns?: Partial<Record<Layout, boolean>>;
  /** Sort / view in every folder of this browser while folders do not remember their own. */
  view?: FolderPrefs;
}

export interface Session {
  name: string | null;
  layout: Layout;
  browsers: BrowserState[];
  activeBrowser: number;
}

export interface Favorite {
  path: string;
  alias: string | null;
}
export interface FavoriteGroup {
  id: number;
  name: string;
  items: Favorite[];
  collapsed: boolean;
}
export interface Favorites {
  groups: FavoriteGroup[];
}

export interface Settings {
  theme: ThemeMode;
  customTheme: string;
  accent: string;
  fontFamily: string;
  /** "regular", "bold", "italic" or "boldItalic". */
  fontStyle: string;
  fontSize: number;
  rowPadding: number;
  thinScrollbars: boolean;
  animations: boolean;
  ageSaturation: number;
  ageLightness: number;
  folderBackgroundImages: boolean;
  showHidden: boolean;
  hideTemp: boolean;
  foldersOnTop: boolean;
  foldersByName: boolean;
  defaultSort: SortKey;
  defaultDescending: boolean;
  defaultView: string;
  rememberFolderViews: boolean;
  marchingAnts: boolean;
  showGitStatus: boolean;
  folderSizesAuto: boolean;
  showDetailsPane: boolean;
  showNavColumns: boolean;
  columnHoverExpand: boolean;
  columnsFromHome: boolean;
  columnExpandDelayMs: number;
  columnExpandDurationMs: number;
  columnMinWidth: number;
  columnMaxWidth: number;
  singleClickOpen: boolean;
  dragBehavior: "consistent" | "copy" | "system" | "ask";
  slowClickRename: boolean;
  startupPaths: string[];
  newTabPath: string;
  newTabPlacement: "afterCurrent" | "end";
  tabsShowFullPath: boolean;
  startMaximized: boolean;
  showDrives: boolean;
  showCloud: boolean;
  showHistory: boolean;
  showSystemMonitor: boolean;
  historyCount: number;
  terminal: string;
  editor: string;
  openRules: OpenRule[];
  notificationMs: number;
  pathCopyFormat: PathFormat;
  recentDestinations: number;
  previewTextLimit: number;
  autoplayMedia: boolean;
  videoSeek: "seconds" | "fraction";
  videoSeekSeconds: number;
  videoSeekFraction: number;
  mapsUrl: string;
  noThumbnailExts: string[];
  excerptExts: string[];
  excerptLines: number;
  indexRoots: string[];
  indexHidden: boolean;
  indexRespectGitignore: boolean;
  indexExclude: string[];
  indexRefreshMinutes: number;
  searchRespectGitignore: boolean;
  keyboardProfile?: "explorer" | "totalCommander" | "vim" | null;
  vimKeys: boolean;
  leaderKey: string;
  leaderKeyBasic: string;
  watcherDebounceMs: number;
  shortcuts: Record<string, string>;
  folderColors: Record<string, string>;
}

// ───────── Quick Open index / find in files ─────────
export interface IndexHit {
  path: string;
  name: string;
  isDir: boolean;
  score: number;
  /** UTF-16 offsets into `path` to highlight. */
  positions: number[];
}

export interface IndexResult {
  hits: IndexHit[];
  total: number;
  searched: number;
  elapsedMs: number;
  /** The folder is not in the system index (WSL, a network share) and is being scanned. */
  scanning: boolean;
}

export interface IndexStatus {
  entries: number;
  roots: string[];
  building: boolean;
  builtAt: number | null;
  buildMs: number;
}

export interface GrepOptions {
  case: "smart" | "sensitive" | "insensitive";
  wholeWord: boolean;
  regex: boolean;
  include: string[];
  exclude: string[];
  includeHidden: boolean;
  respectGitignore: boolean;
  /** Search everything: no excluded folders, .gitignore, .gallignore or cache folders. */
  everything: boolean;
  maxMatches: number;
}

/** The .gallignore line that hides an item from search. */
export interface HiddenBy {
  file: string;
  line: string;
}

/** Which items of a folder a .gallignore hides from search. */
export interface HiddenState {
  /** The folder itself or one above it is hidden: everything in it is. */
  folder: string | null;
  folderRule: HiddenBy | null;
  entries: Record<string, HiddenBy>;
}

export type ShowOutcome =
  | { kind: "removed"; file: string }
  | { kind: "excepted"; file: string }
  | { kind: "insideHidden"; folder: string }
  | { kind: "notHidden" };

export interface LineMatch {
  line: number;
  text: string;
  /** UTF-16 [start, end) ranges into `text`. */
  ranges: [number, number][];
}

export interface FileMatches {
  path: string;
  matches: LineMatch[];
}

export interface GrepStats {
  files: number;
  matches: number;
  searched: number;
  /** Cloud-only files (OneDrive Files On-Demand) not searched: reading them downloads them. */
  cloudOnly: number;
  truncated: boolean;
  elapsedMs: number;
}

export interface FolderPrefs {
  sortKey?: SortKey | null;
  descending?: boolean | null;
  foldersOnTop?: boolean | null;
  view?: string | null;
}

export interface ResolvedPrefs {
  sortKey: SortKey;
  descending: boolean;
  foldersOnTop: boolean;
  view: string;
  viewSource: string | null;
}

export type PreviewKind = "folder" | "image" | "video" | "audio" | "pdf" | "text" | "archive" | "note" | "none";

export interface PreviewInfo {
  kind: PreviewKind | null;
  entry: FileEntry | null;
  width: number | null;
  height: number | null;
  text: string | null;
  textTruncated: boolean;
  binary: boolean;
  archive: { name: string; size: number; isDir: boolean }[];
  archiveTotal: number;
  folderItems: number | null;
  folderBytes: number | null;
  /** Item inside an archive: its extracted copy, which the webview loads. */
  localPath?: string | null;
}

/** "Always open … with …" (Open with › Ctrl+Enter). target: "ext:md" or "path:<file>"; dir limits it to a folder. */
export interface OpenRule { target: string; dir?: string; name: string; command: string }

/** An application that can open a file, or a terminal. `command` is what runs and is stored. */
export interface AppChoice { id: string; name: string; default: boolean; command: string }

export type TaskKind = "copy" | "move" | "delete" | "compress" | "extract" | "duplicate";
/** `overwrite` trashes the replaced item; `overwritePermanently` deletes it (offered when the trash refuses). */
export type ConflictPolicy = "keepBoth" | "overwrite" | "skip" | "overwritePermanently" | "overwriteOlder";

/** Total size of a folder (folder_size). */
export interface DirSize {
  bytes: number;
  files: number;
  dirs: number;
  unreadable: number;
  /** Kernel virtual filesystem: totals are not applicable. Absent when false. */
  virtualFs?: boolean;
}

export type GitState = "ignored" | "untracked" | "staged" | "modified" | "conflict";
export interface GitStatus {
  subject?: string | null;
  containingBranches?: string[];
  root: string;
  branch: string | null;
  ahead: number;
  behind: number;
  /** State of each item of the folder that has one, by name. */
  entries: Record<string, GitState>;
  /** The folder itself is untracked or ignored: every item in it is. */
  all: GitState | null;
}

/** What a copy or move would run into (see `conflict_info` in the core). */
export interface ConflictInfo {
  paths: string[];
  /** How many conflicts are folders on both sides (they can be merged). */
  folders: number;
  /** Files existing on both sides after merging the folders. */
  filesWhenMerged: number;
  sample: string[];
}

export interface TaskProgress {
  id: number;
  kind: TaskKind;
  doneBytes: number;
  totalBytes: number;
  doneItems: number;
  totalItems: number;
  current: string;
}

export type TaskEvent =
  | ({ state: "progress" } & TaskProgress)
  | { state: "done"; id: number; kind: TaskKind; items: number; skipped: number; name: string | null; to: string | null }
  | { state: "failed"; id: number; kind: TaskKind; error: string }
  | { state: "cancelled"; id: number; kind: TaskKind };

export interface Template {
  name: string;
  path: string;
  suggested: string;
}

/** An item in the trash (sidebar › Trash window). */
export interface TrashEntry {
  /** Opaque id for restore / delete. */
  id: string;
  name: string;
  /** The folder it was deleted from. */
  originalParent: string;
  /** Unix seconds. */
  deleted: number;
  isDir: boolean;
  /** Bytes; a folder's whole contents. */
  size: number;
}

export interface Tagged {
  path: string;
  color: number;
}

export interface AppInfo {
  version: string;
  platform: string;
  home: string;
  separator: string;
  configDir: string;
  dataDir: string;
  templatesDir: string;
  trashDir: string | null;
  startupNotice: string | null;
  /** First start (no settings file yet): the welcome setup is shown. */
  firstRun: boolean;
}

export type ClipContent =
  | { kind: "empty" }
  | { kind: "text"; text: string }
  | { kind: "url"; url: string }
  | { kind: "paths"; paths: string[] }
  | { kind: "image"; width: number; height: number };

export interface CliArgs {
  path1: string | null;
  path2: string | null;
  newTab: boolean;
  noWindow: boolean;
  selftest: boolean;
  selftestMode: string;
}

/** An installed font family (Settings › Appearance › Font). */
export interface FontFamily {
  name: string;
  monospace: boolean;
  bold: boolean;
  italic: boolean;
}

/** Where two files differ byte by byte (hex comparison). */
export interface HexDiffResult {
  leftSize: number;
  rightSize: number;
  /** `[start, end)` byte ranges, ascending. */
  ranges: [number, number][];
  differing: number;
  truncated: boolean;
}
