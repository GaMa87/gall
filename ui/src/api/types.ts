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
  | { kind: "gitBlob"; root: string; file: string; commit: string; isHead?: boolean; lastTouch?: WorktreeComparison["lastTouch"] }
  | { kind: "text"; label: string; text: string };

export interface GitChangedFile { path: string; original: string | null; status: string; added: number | null; deleted: number | null }
export interface GitCommitDetails {
  root: string; id: string; parents: string[]; author: string; email: string; date: string;
  committer: string; commitDate: string; message: string; base: string | null; files: GitChangedFile[];
}

export interface WorktreeComparison {
  left: Extract<DiffSource, { kind: "gitBlob" }> | null;
  right: Extract<DiffSource, { kind: "file" | "text" }> | null;
  lastTouch: { id: string; subject: string; author: string; date: string; file: string } | null;
}

/** Which whitespace differences do not count (the old `ignoreWhitespace` boolean means `all`). */
export type WhitespaceMode = "none" | "leading" | "trailing" | "all";
/** How finely a changed line is marked inside; `char` works on Unicode grapheme clusters. */
export type Granularity = "line" | "word" | "char";

export interface DiffOptions {
  /** Saved before `whitespace` existed; true means `whitespace: "all"`. */
  ignoreWhitespace?: boolean;
  whitespace?: WhitespaceMode;
  granularity?: Granularity;
  /** Ignore comments of the file's language (never inside string literals). */
  ignoreComments?: boolean;
  /** Regular expressions (at most 16 of 512 characters): a line that one matches is ignored. */
  ignoreLines?: string[];
  /** Regular expressions: the text one matches is ignored inside a line. */
  ignoreText?: string[];
  ignoreCase: boolean;
  ignoreEol: boolean;
  /** Fewest changed lines (Myers) instead of the more readable histogram diff. */
  minimal: boolean;
  /** Compare changed lines by tokens: layout changes are told apart from content changes. */
  structural: boolean;
  /** Also find moved blocks that were edited on the way (anchored by lines unique in both files). */
  movesEdited?: boolean;
}

export interface DiffSide { label: string; path: string | null; size: number; crlf: boolean; finalNewline: boolean; /** Of a file when it was read; a save that carries it is refused if the file changed since. */ revision?: string }
/** `cosmetic`: both sides hold the same tokens, only the layout differs (structural diff). */
export interface DiffOp { tag: "equal" | "delete" | "insert" | "replace"; a: number; aLen: number; b: number; bLen: number; cosmetic?: boolean }
/** Structural diff, line → changed ranges: an empty list means only the layout of the line differs; a line not listed changed as a whole. */
export type DiffMarks = Record<number, [number, number][]>;
/**
 * `len` lines removed at left `a` reappear at right `b` (`bLen` lines). Unchanged (indentation aside), or `edited`:
 * changed on the way and anchored by lines unique in both files. `id` (1, 2, … in left order) is shared by both ends.
 */
export interface DiffMove { id: number; a: number; b: number; len: number; bLen: number; edited: boolean }
export interface DiffLinePair { a: number; b: number; aSpans: [number, number][]; bSpans: [number, number][] }
/** Syntax colours of one compare side: per line [start, end, category] (UTF-16). */
export interface SyntaxHighlight {
  language: string;
  lines: [number, number, number][][];
}

export interface DiffResult {
  left: DiffSide;
  right: DiffSide;
  text: { leftLines: string[]; rightLines: string[]; ops: DiffOp[]; pairs: DiffLinePair[]; added: number; removed: number; ratio: number; moves: DiffMove[]; leftMarks: DiffMarks; rightMarks: DiffMarks } | null;
  binary: { identical: boolean; firstDifference: number | null; reason: string } | null;
}

export interface CompareTextShape { lines: number; maxLineColumns: number; crlf: boolean; finalNewline: boolean }
export interface CompareSummary {
  left: DiffSide;
  right: DiffSide;
  binary: DiffResult["binary"];
  limit: "tooManyBytes" | "tooManyLines" | "lineTooLong" | "minimalWorkLimit" | null;
  text: { left: CompareTextShape; right: CompareTextShape; onlyCrlfDiffers: boolean; added: number; removed: number; ratio: number } | null;
}
export interface ComparePage<T> { start: number; next: number | null; total: number; items: T[] }
export interface CompareHunk extends DiffOp { id: string }

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

/**
 * `include` and `exclude` are globs (a pattern without `/` matches a name at any depth); the bounds are inclusive,
 * sizes in bytes and times in seconds since the epoch. What they leave out is left out on both sides.
 */
export interface DirCmpOptions {
  content: boolean; hidden: boolean; respectGitignore: boolean;
  /** Read every pair again, ignoring the remembered results of earlier scans. */
  fresh?: boolean;
  include?: string[]; exclude?: string[];
  minSize?: number | null; maxSize?: number | null;
  modifiedAfter?: number | null; modifiedBefore?: number | null;
}
export interface DirMeta { isDir: boolean; size: number; mtime: number }
export type DirStatus = "same" | "different" | "leftOnly" | "rightOnly";
export interface DirItem {
  rel: string;
  name: string;
  depth: number;
  isDir: boolean;
  status: DirStatus;
  reason: "content" | "size" | "date" | "type" | "unreadable" | null;
  newer: "left" | "right" | null;
  left: DirMeta | null;
  right: DirMeta | null;
  /** The path as spelled on disk on that side when it differs from `rel` (composed/decomposed Unicode). */
  leftRel?: string;
  rightRel?: string;
  /** A folder that holds something the filters left out: no whole-tree copy or trash of it, only of the paths listed below it. */
  hasExcluded?: boolean;
}
export interface DirCmpResult {
  left: string;
  right: string;
  items: DirItem[];
  counts: { same: number; different: number; leftOnly: number; rightOnly: number };
  truncated: boolean;
  incompleteReasons: ("traversal" | "metadata" | "pathIdentity" | "read")[];
  elapsedMs: number;
}
/** What a file looked like at the comparison (size, seconds); `stamp: null` means the path did not exist. Checked before the sync writes. */
export interface SyncCheck { path: string; stamp: { size: number; mtime: number } | null; /** The path must still be a folder (a whole-tree copy or trash of it is planned). */ dir?: boolean; /** What was below that folder when the plan was shown (the fingerprint of everything in it). */ tree?: string; /** The size and time to the nanosecond of that file when the plan was shown. */ exact?: string }
export type SyncOp = { op: "copy"; src: string; dest: string } | { op: "trash"; path: string };
export interface SyncReport { copied: number; replaced: number; trashed: number; bytes: number; undoId?: number | null }

/** Fallback info for media the built-in player cannot play (video_probe; needs FFmpeg). */
export interface VideoProbe {
  platform: string;
  ffmpeg: boolean;
  codec: string | null;
  codecLong: string | null;
  profile: string | null;
  width: number | null;
  height: number | null;
  duration: number | null;
  fps?: number | null;
  hasVideo?: boolean;
  hasAudio?: boolean;
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
export type GroupMode = "none" | "folders" | "type" | "foldersType";
export type GroupOrder = "name" | "count" | "recent" | "size";
export type Layout = "standard" | "columns" | "classic";
export type ThemeMode = "system" | "dark" | "light" | "omarchy";
export type WindowButtonsMode = "auto" | "always" | "never";
export type WorkspaceBarMode = "always" | "multiple" | "never";
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
  /** Hidden files override for this browser; null = synced with the showHidden setting. */
  showHidden?: boolean | null;
  /** Miller columns (true) or breadcrumbs (false), chosen per layout; a missing layout uses its default. */
  columns?: Partial<Record<Layout, boolean>>;
  /** Sort / view in every folder of this browser while folders do not remember their own. */
  view?: FolderPrefs;
}

export type PairMode = "linked" | "tandem" | "flow";
export type FlowDirection = "leftToRight" | "rightToLeft";
export type FlowOp = "copy" | "move";

export interface TabPair {
  id: number;
  leftTabId: number;
  rightTabId: number;
  mode: PairMode;
  leftRoot: string;
  rightRoot: string;
  flowDir: FlowDirection;
  flowOp: FlowOp;
  colorSlot: number;
}

export interface Session {
  name: string | null;
  layout: Layout;
  browsers: BrowserState[];
  activeBrowser: number;
  pairs?: TabPair[];
}

export interface WorkspaceSummary {
  id: number;
  index: number;
  name: string | null;
  accent: string | null;
  layout: Layout;
  activeBrowser: number;
  leftPath: string;
  rightPath: string;
  leftTabs: number;
  rightTabs: number;
  pairCount: number;
}

export interface WorkspaceSnapshot {
  workspaces: WorkspaceSummary[];
  active: number;
  session: Session;
}

export type GravityReason = "learned" | "worktree";

export interface GravitySuggestion {
  sourcePath: string;
  path: string;
  reason: GravityReason;
  score: number;
  suggestedMode: PairMode;
  hint: string | null;
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

/** A vim folder mark: the folder, and the item focused in it when it was set. */
export interface Mark {
  path: string;
  item: string | null;
}
/** Mark key (a–z, A–Z, or ' for the folder before the last jump) → mark. */
export interface Marks {
  marks: Record<string, Mark>;
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
  /** Before `group` existed; kept as it was saved. */
  foldersOnTop: boolean;
  foldersByName: boolean;
  group: GroupMode;
  groupOrder: GroupOrder;
  groupByKind: boolean;
  groupHeaders: boolean;
  defaultSort: SortKey;
  defaultDescending: boolean;
  defaultView: string;
  rememberFolderViews: boolean;
  marchingAnts: boolean;
  showGitStatus: boolean;
  worktreeLocation: string;
  gitDoubleClick: string;
  gitForecastBranches: number;
  worktreeLocations: Record<string, string>;
  gitBranchWip: boolean;
  gitCommands: import("../core/gitCustom").GitCommandDef[];
  worktreeCommands: Record<string, string[]>;
  worktreeLinks: Record<string, boolean>;
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
  showWorkspaces?: WorkspaceBarMode;
  liveTabPairs?: boolean;
  pairGravity?: boolean;
  startMaximized: boolean;
  showDrives: boolean;
  showCloud: boolean;
  showHistory: boolean;
  showSystemMonitor: boolean;
  historyCount: number;
  /** Which changes of defaults the settings file has been through; passed back unchanged. */
  defaultsVersion: number;
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
  /** Follow Omarchy's Glassy toggle (translucent window, sharp text). */
  omarchyGlassy: boolean;
  /** Use Omarchy's monospace font unless a font is chosen explicitly. */
  omarchyFollowFont: boolean;
  /** Interface language: "auto" (Omarchy, else the system) or a code such as "sl". */
  language: string;
  /** One pane / two panes side by side / two stacked, depending on the window size. */
  adaptiveLayout: boolean;
  /** The sidebar is shown (the user's choice; a narrow window hides it temporarily). */
  sidebarVisible: boolean;
  /** Minimize / maximize / close buttons: hidden in tiling window managers when "auto". */
  windowButtons: WindowButtonsMode;
  vimKeys: boolean;
  leaderKey: string;
  leaderKeyBasic: string;
  watcherDebounceMs: number;
  generatedPatterns: string[];
  autoDownloadUpdates: boolean;
  shortcuts: Record<string, string>;
  folderColors: Record<string, string>;
}

// ───────── Quick Open index / find in files ─────────
export interface HiddenCount {
  count: number;
  /** True when counting stopped at the match cap or budget (render as `N+`). */
  capped: boolean;
}

export interface SearchHit extends FileEntry {
  /** Normally hidden by search filters (shown when Alt+I / "ignored" is enabled). */
  hidden?: boolean;
}

export interface IndexHit {
  path: string;
  name: string;
  isDir: boolean;
  score: number;
  /** UTF-16 offsets into `path` to highlight. */
  positions: number[];
  /** Normally hidden by search filters (shown when Alt+I is enabled). */
  hidden?: boolean;
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
  /** macOS: TCC-protected folders (such as ~/Library subfolders) skipped without Full Disk Access. */
  restrictedDirs?: number;
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
  /** Normally hidden by search filters (shown when Alt+I is enabled). */
  hidden?: boolean;
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
  group?: GroupMode | null;
  groupOrder?: GroupOrder | null;
  view?: string | null;
  collapsed?: string[] | null;
}

export interface ResolvedPrefs {
  path: string;
  sortKey: SortKey;
  descending: boolean;
  group: GroupMode;
  groupOrder: GroupOrder;
  view: string;
  viewSource: string | null;
  groupSource: string | null;
  collapsed: string[];
}

export type PreviewKind = "folder" | "image" | "video" | "audio" | "pdf" | "text" | "archive" | "note" | "model" | "none";

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
  /** Linux: GStreamer lacks plugins for this media file and FFmpeg is available. */
  useFfmpeg?: boolean;
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
  /** In a linked worktree, not the main one. */
  linked?: boolean;
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
  | { state: "done"; id: number; kind: TaskKind; items: number; skipped: number; name: string | null; to: string | null; undoId?: number | null }
  | { state: "failed"; id: number; kind: TaskKind; error: string; undoId?: number | null }
  | { state: "cancelled"; id: number; kind: TaskKind; undoId?: number | null };

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
  /** First start (no settings file yet): the welcome setup is shown. */
  firstRun: boolean;
  /** Linux: URL prefix of the local media server that video/audio play from. */
  mediaBase: string | null;
  installDir: string;
  updatesDir: string;
  updateStrategy: "inPlace" | "overlay" | "storeManaged" | "unconfigured";
  installKind: string;
  baseExePath: string;
  baseVersion: string;
  runningOverlay: boolean;
  /** macOS: whether ~/.Trash can be listed directly (Full Disk Access granted). */
  macosFullDiskAccess?: boolean | null;
}

export type DefaultFileManagerUnavailableReason =
  | "macosAppBundleRequired"
  | "executableNotFound"
  | "unsupportedPlatform";

export interface DefaultFileManagerStatus {
  platform: string;
  isDefault: boolean;
  currentHandler: string | null;
  currentId: string | null;
  canSet: boolean;
  unavailableReason: DefaultFileManagerUnavailableReason | null;
  canRestore: boolean;
  restoreTarget: string | null;
}

export type UpdateStage = "downloading" | "verifying" | "extracting" | "probing" | "applying" | "restarting";
export type UpdateStrategy = "inPlace" | "overlay" | "storeManaged" | "unconfigured";

export interface UpdateProgressEvent {
  stage: UpdateStage;
  downloaded: number;
  total: number | null;
}

export interface UpdateCheckResult {
  available: boolean;
  version: string | null;
  notes: string | null;
  staged: boolean;
  strategy: UpdateStrategy;
  installKind: string;
  target: string;
  reason: string;
  installDir: string;
  updatesDir: string;
  baseExePath: string;
  baseVersion: string;
  runningOverlay: boolean;
  activeOverlayVersion: string | null;
}

export type StartupUpdateNotice =
  | { kind: "updated"; version: string; fromVersion: string | null }
  | { kind: "rolledBack"; failedVersion: string; restoredVersion: string; reason: string }
  | { kind: "reverted"; fromVersion: string; toVersion: string };

export type ClipContent =
  | { kind: "empty" }
  | { kind: "text"; text: string }
  | { kind: "url"; url: string }
  | { kind: "paths"; paths: string[]; cut?: boolean }
  | { kind: "image"; width: number; height: number };

export interface PickFilter {
  name: string;
  /** Globs such as "*.png"; empty means every file. */
  patterns: string[];
}

/** An extra option of a picker: a checkbox (no `options`; value "true"/"false") or a combo box. */
export interface PickChoice {
  id: string;
  label: string;
  /** `[id, label]` pairs. */
  options: [string, string][];
  initial: string;
}

/** `gall --pick`: this window is a file picker answering another program. */
export interface PickArgs {
  mode: "open" | "save" | "dir";
  result: string;
  multiple: boolean;
  title: string | null;
  name: string | null;
  accept: string | null;
  filters: PickFilter[];
  choices: PickChoice[];
}

export interface CliArgs {
  path1: string | null;
  path2: string | null;
  paths: string[];
  /** Items to select after opening their parent folder (`--select`, FileManager1 ShowItems). */
  selectPaths: string[];
  newTab: boolean;
  newWindow: boolean;
  noWindow: boolean;
  daemon: boolean;
  /** `--compare <left> <right>`. */
  compare: [string, string] | null;
  wait: boolean;
  merge: { base: string | null; local: string; remote: string; output: string } | null;
  comparePicker: boolean;
  compareSeed: string | null;
  /** `--git [path]`. */
  gitRepo: string | null;
  selftest: boolean;
  selftestMode: string;
  pick: PickArgs | null;
}

/** `org.freedesktop.FileManager1` request routed to a window (camelCase fields, `kind` tag). */
export type FileManagerAction =
  | { kind: "showFolders"; folders: string[]; startupId: string }
  | { kind: "showItems"; folders: string[]; selectPaths: string[]; startupId: string }
  | { kind: "showItemProperties"; folder: string; target: string; startupId: string };

/** Live Omarchy desktop state (theme palette, Hyprland rounding, font, Glassy, tiling WM). */
export interface OmarchyState {
  /** Omarchy was detected (colors.toml present). */
  active: boolean;
  themeMode: "dark" | "light" | string;
  /** CSS custom property name (with its leading `--`, e.g. `--bg-2`) → value. */
  paletteTokens: Record<string, string>;
  customCss: string | null;
  /** Hyprland `decoration:rounding` in px, null when unknown. */
  rounding: number | null;
  glassy: boolean;
  monospaceFont: string | null;
  tilingWm: boolean;
  /** True while Gall has a window on an Omarchy floating workspace (`omarchy-floating-workspace`). */
  floatingWorkspace?: boolean;
  /** The Omarchy interface language (null off Omarchy). */
  language: OmarchyLanguage | null;
}

export interface OmarchyLanguage {
  code: string;
  /** BCP 47 tag for numbers, dates and sizes, e.g. "sl-SI". */
  formatLocale: string;
  direction: "ltr" | "rtl";
}

/** GVFS network mount (SMB, NFS, SFTP, WebDAV, FTP…). */
export interface NetworkMount {
  name: string;
  scheme: string;
  uri: string;
  mountPath: string;
}

/** A context action from `actions.d/*.json`. */
export interface ActionDef {
  id: string;
  label: string;
  pluralLabel?: string;
  icon?: string;
  single?: boolean;
  selection?: string;
}

export interface ActionTarget {
  path: string;
  isDir: boolean;
  ext: string;
  mime?: string | null;
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

// ───────────── three-way merge (gall-core `merge`) ─────────────

export type MergeStage = "base" | "left" | "right";
export type MergeChoice = "left" | "right" | "both" | "combine" | "base";
export type MergeChunkKind = "stable" | "left" | "right" | "same" | "conflict";
/** A range of lines of one source. */
export interface MergeSpan { start: number; len: number }
/** A part of the base, cut by the changes of the two sides. A stable chunk has no lines of its own: read them through the spans. */
export interface MergeChunkInfo {
  /** Stable for the same inputs: the key of a choice. */
  id: string;
  kind: MergeChunkKind;
  base: MergeSpan;
  left: MergeSpan;
  right: MergeSpan;
  baseLines: string[];
  leftLines: string[];
  rightLines: string[];
  combinedLines: string[] | null;
}
export interface MergeModel { chunks: MergeChunkInfo[]; conflicts: string[] }
/** The merge, or why there is none: a binary stage, a stage that is missing (a deletion) or too large. */
export type MergeOutcome =
  | { kind: "text"; merge: MergeModel }
  | { kind: "binary"; stage: MergeStage }
  | { kind: "unavailable"; stage: MergeStage; reason: "missing" | "tooLarge" };
export type MergeOrigin = "base" | "left" | "right" | "same" | "chosenLeft" | "chosenRight" | "chosenBoth" | "chosenCombine" | "chosenBase";
export interface MergeSegment { chunk: string; origin: MergeOrigin; start: number; len: number }
/** Only `resolved` can be saved. */
export type MergeResolution =
  | { kind: "resolved"; text: string; provenance: MergeSegment[] }
  | { kind: "unresolved"; conflicts: string[] }
  | { kind: "binary"; stage: MergeStage }
  | { kind: "unavailable"; stage: MergeStage; reason: "missing" | "tooLarge" };
/** An unfinished merge with Git-style markers for the conflicts that have no choice; for display and hand editing, not a result. */
export interface MergePreview { text: string; unresolved: string[] }
/** Where an input of a merge comes from; `gitStage`: 1 the common ancestor, 2 ours, 3 theirs of an unresolved file. */
export type MergeSource =
  | { kind: "text"; text: string }
  | { kind: "file"; path: string }
  | { kind: "gitStage"; path: string; file: string; stage: 1 | 2 | 3 };
/** An input that is null does not exist (no common ancestor, a deleted side). */
export interface MergeRequest { base: MergeSource | null; left: MergeSource | null; right: MergeSource | null }

export interface DocBlockGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  pageWidth: number;
  pageHeight: number;
}

export type DocBlockKind = "heading" | "paragraph" | "listItem" | "tableRow" | "sectionBreak";

export interface DocBlock {
  line: number;
  page: number;
  block: number;
  kind: DocBlockKind;
  headingLevel?: number | null;
  table?: number | null;
  tableRow?: number | null;
  cells?: string[] | null;
  geometry?: DocBlockGeometry | null;
  text: string;
}

/** A document as lines of text and positioned blocks for comparison (`.docx` and `.pdf`). */
export interface DocExtract {
  format: string;
  text: string;
  blocks: number;
  sections: number;
  items: DocBlock[];
  warnings: string[];
  truncated: boolean;
}

/** Syntax-tree comparison (Rust, JavaScript, TypeScript): conservative; `unknown` when it cannot be sure. */
export type SemanticChangeKind = "rename" | "signature" | "moved" | "behavior" | "added" | "removed";
export interface SemanticSpan { start: number; end: number; line: number }
export interface SemanticChange { kind: SemanticChangeKind; name?: string | null; left?: SemanticSpan | null; right?: SemanticSpan | null }
export type SemanticVerdict =
  | { verdict: "equivalent"; evidence: "identical" | "sameTree" }
  | { verdict: "different"; changes: SemanticChange[] }
  | { verdict: "unknown"; reason: { why: "unsupported" | "tooLarge" | "grammar" } | { why: "parseError"; side: "left" | "right"; line: number } }
  | { verdict: "cancelled" };

/** A node of the structural outline; lines are 1-based. */
export interface OutlineNode {
  kind: "module" | "class" | "struct" | "enum" | "trait" | "impl" | "interface" | "function" | "method" | "field" | "closure" | "macro";
  /** Null for an anonymous function or closure that no name is given to. */
  name: string | null;
  span: SemanticSpan;
  endLine: number;
  children: OutlineNode[];
}
/** The outline, or why there is none (`Result` of the core). */
/** The nodes, and whether the list was cut short (a very large file lists only its first nodes). */
export interface Outline { nodes: OutlineNode[]; truncated: boolean }
export type OutlineResult = { Ok: Outline } | { Err: SemanticVerdict };
/** Why a file counts as generated; presentation only. */
export type GeneratedReason = "lockfile" | "artifactPath" | "generatedHeader" | "minifiedBundle" | "gitAttributes" | "customPattern";
export interface Generated { reason: GeneratedReason }

/** The local-model explanation of a comparison (opt-in; see docs/compare-local-ai.md). */
export type AiCategory = "bugfix" | "feature" | "refactor" | "performance" | "formatting" | "unknown";
export type AiExplainScope =
  | { kind: "all" }
  | { kind: "hunk"; index: number }
  | { kind: "selection"; side: "left" | "right"; startLine: number; endLine: number };
export interface AiEvidence { hunk: number; side: "left" | "right"; startLine: number; endLine: number }
export interface AiItem { hunk: number; category: AiCategory; confidence: number; note: string }
export interface AiExplanation { protocol: number; summary: string; why: string; risk: string; uncertainty: string; category: AiCategory; confidence: number; items: AiItem[]; evidence: AiEvidence[] }
/** The settings of the local explanation model (`compare-ai.json`). */
export interface CompareAiConfig { enabled: boolean; program: string; args: string[]; model: string; deadlineSecs: number; env: [string, string][] }
export type AiOutcome =
  | { outcome: "explained"; explanation: AiExplanation; model: string; cached: boolean }
  | { outcome: "disabled" }
  | { outcome: "refused"; reason: string }
  | { outcome: "tooLarge" }
  | { outcome: "timeout" }
  | { outcome: "cancelled" }
  | { outcome: "failed"; message: string }
  | { outcome: "malformed"; reason: string };

/** A proposal of the local model for one merge conflict (opt-in): lines to preview and accept, never applied by itself. */
export interface MergeConflictInput { id: string; baseLines: string[]; leftLines: string[]; rightLines: string[]; before: string[]; after: string[] }
export interface MergeSyntaxCheck { fileName: string; before: string; after: string }
export interface MergeEvidence { source: "base" | "left" | "right"; startLine: number; endLine: number }
export interface MergeProposal { protocol: number; revision: string; replacement: string[]; explanation: string; risk: string; uncertainty: string; confidence: number; evidence: MergeEvidence[] }
export type MergeAiOutcome =
  | { outcome: "proposed"; proposal: MergeProposal; model: string; cached: boolean }
  | { outcome: "stale" } | { outcome: "disabled" } | { outcome: "refused"; reason: string } | { outcome: "tooLarge" }
  | { outcome: "timeout" } | { outcome: "cancelled" } | { outcome: "failed"; message: string } | { outcome: "malformed"; reason: string };

/** A patch format: `unified` (`diff -u`), `context` (`diff -c`) or `normal` (plain `diff`). */
export type PatchFormat = "unified" | "context" | "normal";

/** Saved tabs of the comparison window. A side with unsaved text has `source: null` and is never written to disk. */
export interface CompareSourceRecord { source: DiffSource | null; label: string; revision?: string | null }
export interface CompareTabRecord {
  left: CompareSourceRecord;
  right: CompareSourceRecord;
  mode: "split" | "unified";
  options: Partial<DiffOptions>;
  /** A line of one side, independent of the window size. */
  scroll?: { left: boolean; line: number } | null;
  activeHunk?: number | null;
}
export interface CompareSessions { version: number; tabs: CompareTabRecord[]; active: number }
export type CompareSourceState = "ready" | "changed" | "missing" | "notSaved";
export interface CompareTabCheck { index: number; left: CompareSourceState; right: CompareSourceState; openable: boolean }
export interface CompareSessionsLoaded { sessions: CompareSessions; checks: CompareTabCheck[]; foreignVersion: boolean }
export interface PatchPreviewFile { path: string; name: string; hunks: number; offsets: number[] }
/** What importing a patch would do; a non-empty `problems` means that nothing can be applied. */
export interface PatchPreview { files: PatchPreviewFile[]; problems: string[]; /** What the patch would write: apply refuses if the plan it makes now differs. */ fingerprint: string }
