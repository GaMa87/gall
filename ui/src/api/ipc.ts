import type { ReportSections } from "../core/bugReport";
// Typed wrappers over Tauri commands. One function per Rust command.
import type { ArchiveFormat } from "../core/archive";
import { invoke as rawInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type * as T from "./types";

// Never record arguments or successful results; error messages are sanitized by the backend.
function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const start = performance.now();
  const record = !command.startsWith("bug_report_") && !command.startsWith("selftest_") && command !== "ui_log";
  return rawInvoke<T>(command, args).then((value) => {
    if (record) void rawInvoke("bug_report_record", { kind: "command", message: `${command} ok ${Math.round(performance.now() - start)}ms` }).catch(() => {});
    return value;
  }, (error) => {
    if (record) void rawInvoke("bug_report_record", { kind: "command", message: `${command} failed ${Math.round(performance.now() - start)}ms` }).catch(() => {});
    if (record) void rawInvoke("bug_report_record", { kind: "error", message: `${command} failed: ${String(error).slice(0, 1000)}` }).catch(() => {});
    throw error;
  });
}

export const api = {
  bugReportPrepare: (description: string, includeLog: boolean, snapshot: { viewMode?: string; activeDialog?: string; tabCount?: number; paneCount?: number; taskCount?: number }, previous: boolean) => invoke<ReportSections & { title: string; body: string; fullBody: string; log: string; previousUnclean: boolean }>("bug_report_prepare", { description, includeLog, snapshot, previous }),
  bugReportRecord: (kind: string, message: string) => invoke<void>("bug_report_record", { kind, message }),
  bugReportStatus: () => invoke<boolean>("bug_report_status"),
  bugReportDismiss: () => invoke<void>("bug_report_dismiss"),
  bugReportCopyText: (text: string) => invoke<void>("bug_report_copy_text", { text }),
  bugReportCopyScreenshot: (png: number[]) => invoke<{ width: number; height: number }>("bug_report_copy_screenshot", { png }),
  bugReportExport: (path: string, body: string, attachments: { name: string; bytes: number[]; mime: string }[]) => invoke<void>("bug_report_export", { path, body, attachments }),
  bugReportCapture: () => invoke<number[]>("bug_report_capture"),
  bugReportSaveMedia: (path: string, bytes: number[], mime: string) => invoke<void>("bug_report_save_media", { path, bytes, mime }),
  appInfo: () => invoke<T.AppInfo>("app_info"),
  windowReady: () => invoke<void>("window_ready"),
  getSettings: () => invoke<T.Settings>("get_settings"),
  setSettings: (settings: T.Settings) => invoke<void>("set_settings", { settings }),
  resetSettings: () => invoke<T.Settings>("reset_settings"),

  listDir: (path: string, opts: { showHidden: boolean; hideTemp: boolean; dirsOnly?: boolean }) =>
    invoke<T.FileEntry[]>("list_dir", { path, opts }),
  listDrives: () => invoke<T.Drive[]>("list_drives"),
  systemStats: () => invoke<T.SysStats>("system_stats"),
  diffCompare: (a: T.DiffSource, b: T.DiffSource, opts: T.DiffOptions) => invoke<T.DiffResult>("diff_compare", { a, b, opts }),
  diffHighlight: (name: string, lines: string[]) => invoke<T.SyntaxHighlight | null>("diff_highlight", { name, lines }),
  diffPatch: (a: T.DiffSource, b: T.DiffSource) => invoke<string>("diff_patch", { a, b }),
  diffSavePatch: (a: T.DiffSource, b: T.DiffSource, dest: string) => invoke<void>("diff_save_patch", { a, b, dest }),
  gitRoot: (path: string) => invoke<string | null>("git_root", { path }),
  dircmpCompare: (left: string, right: string, opts: T.DirCmpOptions) => invoke<T.DirCmpResult>("dircmp_compare", { left, right, opts }),
  dircmpCancel: () => invoke<void>("dircmp_cancel"),
  syncApply: (ops: T.SyncOp[]) => invoke<T.SyncReport>("sync_apply", { ops }),
  /** Resolves to whether the save can be undone (the old version went to the trash). */
  saveText: (path: string, text: string) => invoke<boolean>("save_text", { path, text }),
  imageDiff: (a: string, b: string) => invoke<T.ImageDiff>("image_diff", { a, b }),
  similarityMatrix: (paths: string[]) => invoke<T.SimilarityMatrix>("similarity_matrix", { paths }),
  tableCompare: (a: string, b: string, opts: T.TableDiffOptions) => invoke<T.TableDiffResult>("table_compare", { a, b, opts }),
  eject: (mount: string) => invoke<void>("eject", { mount }),
  otherVolumes: () => invoke<T.Volume[]>("other_volumes"),
  volumeMount: (id: string) => invoke<string>("volume_mount", { id }),
  volumeUnlock: (id: string, passphrase: string) => invoke<string | null>("volume_unlock", { id, passphrase }),
  driveLock: (mount: string) => invoke<void>("drive_lock", { mount }),
  resolvePrefs: (path: string) => invoke<T.ResolvedPrefs>("resolve_prefs", { path }),
  setPrefs: (path: string, prefs: T.FolderPrefs) => invoke<void>("set_prefs", { path, prefs }),
  clearPrefs: () => invoke<void>("clear_prefs"),

  sessionGet: () => invoke<T.Session>("session_get"),
  sessionCmd: (cmd: SessionCmd) => invoke<T.Session>("session_cmd", { cmd }),
  favoritesGet: () => invoke<T.Favorites>("favorites_get"),
  favoritesCmd: (cmd: FavCmd) => invoke<T.Favorites>("favorites_cmd", { cmd }),
  historyGet: () => invoke<string[]>("history_get"),
  historyRemove: (path: string | null) => invoke<void>("history_remove", { path }),

  previewInfo: (path: string) => invoke<T.PreviewInfo>("preview_info", { path }),
  videoProbe: (path: string) => invoke<T.VideoProbe>("video_probe", { path }),
  excerpts: (paths: string[], lines: number) => invoke<(string | null)[]>("excerpts", { paths, lines }),
  watch: (path: string) => invoke<void>("watch", { path }),
  unwatch: (path: string) => invoke<void>("unwatch", { path }),

  startTask: (req: { kind: T.TaskKind; sources: string[]; dest?: string; conflict?: T.ConflictPolicy; merge?: boolean; archiveName?: string; format?: ArchiveFormat; extractMode?: "smart" | "folder"; trashArchive?: boolean }) =>
    invoke<number>("start_task", { req }),
  cancelTask: (id: number) => invoke<boolean>("cancel_task", { id }),
  findConflicts: (sources: string[], dest: string) => invoke<string[]>("find_conflicts", { sources, dest }),
  conflictInfo: (sources: string[], dest: string) => invoke<T.ConflictInfo>("conflict_info", { sources, dest }),
  rename: (path: string, newName: string) => invoke<string>("rename", { path, newName }),
  /** Total size of a folder; null when cancelled. Running totals: events.folderSizeProgress. */
  folderSize: (path: string, ticket: number) => invoke<T.DirSize | null>("folder_size", { path, ticket }),
  folderSizesCancel: (ticket: number) => invoke<void>("folder_sizes_cancel", { ticket }),
  gitStatus: (dir: string) => invoke<T.GitStatus | null>("git_status", { dir }),
  gitCommits: (path: string) => invoke<{ id: string; subject: string }[]>("git_commits", { path }),
  /** Renames all items as one undo step (swaps allowed); resolves to the new paths. */
  batchRename: (items: [string, string][]) => invoke<string[]>("batch_rename", { items }),
  createDir: (parent: string, name: string) => invoke<string>("create_dir", { parent, name }),
  createFile: (parent: string, name: string, template?: string) => invoke<string>("create_file", { parent, name, template }),
  /** Starts a duplicate task; resolves to where the copies will appear. */
  duplicate: (paths: string[]) => invoke<string[]>("duplicate", { paths }),
  findExisting: (paths: string[]) => invoke<string[]>("find_existing", { paths }),
  trash: (paths: string[]) => invoke<void>("trash", { paths }),
  symlink: (target: string, destDir: string) => invoke<string>("symlink", { target, destDir }),
  fontFamilies: () => invoke<T.FontFamily[]>("font_families"),
  hexDiff: (left: string, right: string) => invoke<T.HexDiffResult | null>("hex_diff", { left, right }),
  hexDiffCancel: () => invoke<void>("hex_diff_cancel"),
  readBytes: (path: string, offset: number, len: number) => invoke<number[]>("read_bytes", { path, offset, len }),
  /** Windows / macOS: drags `paths` as real files (resolves "dropped" or "cancelled"). */
  startNativeDrag: (paths: string[]) => invoke<string>("start_native_drag", { paths }),
  undo: (permanent = false) => invoke<string>("undo", { permanent }),
  undoPeek: () => invoke<string | null>("undo_peek"),
  undoIn: (dir: string, permanent = false) => invoke<string>("undo_in", { dir, permanent }),
  undoPeekIn: (dir: string) => invoke<string | null>("undo_peek_in", { dir }),
  shortcutResolve: (path: string) => invoke<T.FileEntry | null>("shortcut_resolve", { path }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  trashList: () => invoke<T.TrashEntry[]>("trash_list"),
  trashRestore: (ids: string[]) => invoke<void>("trash_restore", { ids }),
  /** Deletes for good; no ids = empty the trash. */
  trashPurge: (ids: string[]) => invoke<void>("trash_purge", { ids }),
  openTerminal: (dir: string) => invoke<void>("open_terminal", { dir }),

  createNote: (dir: string, text: string) => invoke<string>("create_note", { dir, text }),
  toggleNote: (path: string) => invoke<string>("toggle_note", { path }),
  updateNote: (path: string, text: string) => invoke<void>("update_note", { path, text }),
  templatesList: () => invoke<T.Template[]>("templates_list"),
  templateAdd: (file: string) => invoke<string>("template_add", { file }),

  pathComplete: (typed: string) => invoke<string[]>("path_complete", { typed }),
  pathResolve: (input: string, base?: string) => invoke<{ path: string; isDir: boolean; exists: boolean }>("path_resolve", { input, base }),
  pathFormat: (path: string, format?: T.PathFormat) => invoke<string>("path_format", { path, format }),

  searchStart: (id: number, root: string, pattern: string, includeHidden: boolean, everything = false) =>
    invoke<void>("search_start", { id, root, pattern, includeHidden, everything }),
  searchCancel: () => invoke<void>("search_cancel"),

  indexStatus: () => invoke<T.IndexStatus>("index_status"),
  indexRefresh: (force: boolean) => invoke<boolean>("index_refresh", { force }),
  /** Adds the item to the `.gallignore` next to it; null when it was already there. */
  hideFromSearch: (path: string) => invoke<string | null>("hide_from_search", { path }),
  /** Undoes Hide from search, or adds a ! exception when a pattern hides the item. */
  showInSearch: (path: string) => invoke<T.ShowOutcome>("show_in_search", { path }),
  searchHidden: (dir: string) => invoke<T.HiddenState>("search_hidden", { dir }),
  /** Whether the system index holds `path`; Quick Open scans other folders itself. */
  indexCovers: (path: string) => invoke<{ covered: boolean; scanning: boolean; entries: number }>("index_covers", { path }),
  indexQuery: (query: string, scope: string | null, limit: number) =>
    invoke<T.IndexResult>("index_query", { query, scope, limit }),
  grepStart: (id: number, root: string, pattern: string, opts: Partial<T.GrepOptions>) =>
    invoke<void>("grep_start", { id, root, pattern, opts }),
  grepCancel: () => invoke<void>("grep_cancel"),
  grepContext: (path: string, line: number, before: number, after: number) =>
    invoke<[number, string][]>("grep_context", { path, line, before, after }),

  tagsIn: (dir: string) => invoke<T.Tagged[]>("tags_in", { dir }),
  setTag: (paths: string[], color: number) => invoke<void>("set_tag", { paths, color }),
  tagged: (color: number | null) => invoke<T.Tagged[]>("tagged", { color }),
  destinations: () => invoke<string[]>("destinations"),
  clearDestinations: () => invoke<void>("clear_destinations"),

  clipSetFiles: (paths: string[], cut: boolean) => invoke<void>("clip_set_files", { paths, cut }),
  clipGetFiles: () => invoke<{ paths: string[]; cut: boolean }>("clip_get_files"),
  clipClearFiles: () => invoke<void>("clip_clear_files"),
  clipSetText: (text: string) => invoke<void>("clip_set_text", { text }),
  clipInspect: () => invoke<T.ClipContent>("clip_inspect"),
  clipPasteAsFile: (dir: string, asKind: "text" | "image" | "url") => invoke<string>("clip_paste_as_file", { dir, asKind }),
  clipPasteFiles: (dest: string, conflict: T.ConflictPolicy, paths?: string[], merge = false) =>
    invoke<number>("clip_paste_files", { dest, conflict, paths, merge }),
  startupArgs: () => invoke<T.CliArgs>("startup_args"),
  modifiers: () => invoke<{ known: boolean; ctrl: boolean; shift: boolean; alt: boolean }>("modifiers"),
  selftestShot: (name: string) => invoke<string>("selftest_shot", { name }),
  selftestDone: (report: string) => invoke<void>("selftest_done", { report }),
  uiLog: (level: "info" | "warn" | "error", msg: string) => invoke<void>("ui_log", { level, msg }),
  logBacklog: () => invoke<BackendLog[]>("log_backlog"),
  listCloudFolders: () => invoke<T.CloudFolder[]>("list_cloud_folders"),
  openWithApps: (path: string) => invoke<T.AppChoice[]>("open_with_apps", { path }),
  openWithRun: (paths: string[], command: string) => invoke<void>("open_with_run", { paths, command }),
  terminalApps: () => invoke<T.AppChoice[]>("terminal_apps"),
  editorApps: () => invoke<T.AppChoice[]>("editor_apps"),
  openTerminalWith: (dir: string, command: string) => invoke<void>("open_terminal_with", { dir, command }),
  openWithApp: (path: string, id: string) => invoke<void>("open_with_app", { path, id }),
  openWithDialog: (path: string) => invoke<boolean>("open_with_dialog", { path }),
  archivePassword: (path: string, password: string) => invoke<void>("archive_password", { path, password }),
  archiveMaterialize: (paths: string[]) => invoke<string[]>("archive_materialize", { paths }),
  openWithCommand: (path: string, command: string) => invoke<void>("open_with_command", { path, command }),
};

export type SessionCmd =
  | { op: "navigate"; b: number; path: string }
  | { op: "back" | "forward" | "up" | "reopen" | "closeStale" | "focus"; b: number }
  | { op: "newTab"; b: number; path?: string | null; activate?: boolean }
  | { op: "activate" | "close" | "closeOthers"; b: number; idx: number }
  | { op: "reorder"; b: number; from: number; to: number }
  | { op: "transfer"; b: number; idx: number; copy: boolean }
  | { op: "alias"; b: number; idx: number; alias: string | null; keep: boolean }
  | { op: "layout"; layout: T.Layout }
  | { op: "collapse"; b: number; collapsed: boolean }
  | { op: "details"; b: number; on: boolean }
  | { op: "columns"; b: number; on: boolean }
  | { op: "view"; b: number; prefs: T.FolderPrefs }
  | { op: "swap" };

export type FavCmd =
  | { op: "addGroup"; name: string }
  | { op: "renameGroup"; id: number; name: string }
  | { op: "deleteGroup"; id: number }
  | { op: "moveGroup"; id: number; delta: number }
  | { op: "collapse"; id: number; collapsed: boolean }
  | { op: "addItem"; id: number; path: string; index?: number | null }
  | { op: "removeItem"; id: number; index: number }
  | { op: "alias"; id: number; index: number; alias: string | null }
  | { op: "moveItem"; fromGroup: number; fromIndex: number; toGroup: number; toIndex: number }
  | { op: "restoreDefault" };

export const events = {
  fsChanged: (cb: (p: { path: string; storm: boolean }) => void): Promise<UnlistenFn> =>
    listen<{ path: string; storm: boolean }>("fs-changed", (e) => cb(e.payload)),
  task: (cb: (e: T.TaskEvent) => void) => listen<T.TaskEvent>("task", (e) => cb(e.payload)),
  searchBatch: (cb: (p: { id: number; entries: T.FileEntry[] }) => void) =>
    listen<{ id: number; entries: T.FileEntry[] }>("search-batch", (e) => cb(e.payload)),
  searchDone: (cb: (p: { id: number; hits: number; cancelled: boolean }) => void) =>
    listen<{ id: number; hits: number; cancelled: boolean }>("search-done", (e) => cb(e.payload)),
  indexUpdated: (cb: (s: T.IndexStatus) => void) => listen<T.IndexStatus>("index-updated", (e) => cb(e.payload)),
  grepBatch: (cb: (p: { id: number; files: T.FileMatches[] }) => void) =>
    listen<{ id: number; files: T.FileMatches[] }>("grep-batch", (e) => cb(e.payload)),
  grepDone: (cb: (p: { id: number; stats: T.GrepStats; cancelled: boolean; error: string | null }) => void) =>
    listen<{ id: number; stats: T.GrepStats; cancelled: boolean; error: string | null }>("grep-done", (e) => cb(e.payload)),
  secondInstance: (cb: (a: T.CliArgs) => void) => listen<T.CliArgs>("second-instance", (e) => cb(e.payload)),
  folderSizeProgress: (cb: (p: { path: string; size: T.DirSize; ticket: number }) => void) =>
    listen<{ path: string; size: T.DirSize; ticket: number }>("folder-size-progress", (e) => cb(e.payload)),
  backendLog: (cb: (l: BackendLog) => void) => listen<BackendLog>("backend-log", (e) => cb(e.payload)),
};

/** A warning or error logged by the backend. */
export interface BackendLog { level: "warn" | "error"; msg: string }

export const fileSrc = (path: string) => convertFileSrc(path);
