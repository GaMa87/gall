import type { ReportSections } from "../core/bugReport";
// Typed wrappers over Tauri commands. One function per Rust command.
import type { ArchiveFormat } from "../core/archive";
import type { ImportChoice, ImportPlace, ImportReceipt, ImportSourceInfo } from "../core/favoritesImport";
import { invoke as rawInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type * as T from "./types";
import { operationError } from "../core/operationError";
import { browserDiffPageClose, browserDiffPageHunks, browserDiffPageOpen, browserDiffPageSmall } from "../core/compareBrowserPages";
import { currentBrowserEnv } from "../core/compareBrowser";

/** The page runs outside the desktop app: the comparison goes to the browser worker instead of IPC. */
const inBrowser = () => currentBrowserEnv().desktop === false;

export interface TextFileVersion { canonicalPath: string; revision: string | null }

// Notes for a bug report are collected and sent together: one message per note doubled
// the traffic with the backend (a navigation runs about ten commands). They go out
// shortly after the last one, at once after a failure, and when the window is hidden.
const NOTE_DELAY_MS = 400;
let platform: string | undefined;
let notes: [string, string][] = [];
let noteTimer: ReturnType<typeof setTimeout> | undefined;
function flushNotes() {
  if (noteTimer !== undefined) clearTimeout(noteTimer);
  noteTimer = undefined;
  if (!notes.length) return;
  const entries = notes;
  notes = [];
  void rawInvoke("bug_report_record_many", { entries }).catch(() => {});
}
function note(kind: string, message: string, now = false) {
  notes.push([kind, message]);
  if (now || notes.length >= 100) flushNotes();
  else if (noteTimer === undefined) noteTimer = setTimeout(flushNotes, NOTE_DELAY_MS);
}
if (typeof window !== "undefined") window.addEventListener("pagehide", flushNotes);

// Never record arguments or successful results; error messages are sanitized by the backend.
let compareMeasure: ((command: string, milliseconds: number, bytes: number) => void) | undefined;

/** Used by the bundled scale scenario to measure actual comparison responses. */
export function measureCompareResponses(observer?: typeof compareMeasure) {
  compareMeasure = observer;
}

// Background polls and high-frequency cursor previews fire without user action (or on every
// arrow key). Recording their routine successes would flood the 200-event ring buffer and keep
// rewriting `diagnostics.json` while Gall is idle; failures are still recorded below.
const QUIET_SUCCESS_COMMANDS = new Set([
  "system_stats",
  "list_drives",
  "other_volumes",
  "gvfs_network_mounts",
  "list_cloud_folders",
  "selection_publish",
  "grep_context",
]);

export function shouldRecordSuccessCommand(command: string): boolean {
  return (
    !command.startsWith("bug_report_") &&
    !command.startsWith("selftest_") &&
    command !== "ui_log" &&
    !QUIET_SUCCESS_COMMANDS.has(command)
  );
}

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const start = performance.now();
  const record = !command.startsWith("bug_report_") && !command.startsWith("selftest_") && command !== "ui_log";
  // A report is built from what the backend has: everything noted so far goes first.
  if (command.startsWith("bug_report_")) flushNotes();
  return rawInvoke<T>(command, args).then((value) => {
    if (compareMeasure && command.startsWith("diff_page_")) {
      compareMeasure(command, performance.now() - start, new TextEncoder().encode(JSON.stringify(value)).length);
    }
    if (shouldRecordSuccessCommand(command)) note("command", `${command} ok ${Math.round(performance.now() - start)}ms`);
    return value;
  }, (error) => {
    if (record) {
      const raw = String(error);
      const { kind, reportable } = operationError(raw, platform);
      const outcome = reportable ? "failed" : kind === "info" ? "cancelled" : "blocked";
      note("command", `${command} ${outcome} ${Math.round(performance.now() - start)}ms`);
      if (reportable) note("error", `${command} failed: ${raw}`, true);
    }
    throw error;
  });
}

export const api = {
  bugReportPrepare: (description: string, includeLog: boolean, snapshot: { viewMode?: string; activeDialog?: string; tabCount?: number; paneCount?: number; taskCount?: number; triggerError?: string }, previous: boolean) => invoke<ReportSections & { title: string; body: string; fullBody: string; log: string; previousUnclean: boolean }>("bug_report_prepare", { description, includeLog, snapshot, previous }),
  bugReportRecord: (kind: string, message: string) => invoke<void>("bug_report_record", { kind, message }),
  bugReportStatus: () => invoke<boolean>("bug_report_status"),
  bugReportDismiss: () => invoke<void>("bug_report_dismiss"),
  bugReportCopyText: (text: string) => invoke<void>("bug_report_copy_text", { text }),
  bugReportCopyScreenshot: (png: number[]) => invoke<{ width: number; height: number }>("bug_report_copy_screenshot", { png }),
  bugReportExport: (path: string, body: string, attachments: { name: string; bytes: number[]; mime: string }[]) => invoke<void>("bug_report_export", { path, body, attachments }),
  bugReportCapture: () => invoke<number[]>("bug_report_capture"),
  bugReportSaveMedia: (path: string, bytes: number[], mime: string) => invoke<void>("bug_report_save_media", { path, bytes, mime }),
  appInfo: async () => {
    const info = await invoke<T.AppInfo>("app_info");
    platform = info.platform;
    return info;
  },
  windowReady: () => invoke<boolean>("window_ready"),
  windowFrameReady: () => invoke<void>("window_frame_ready"),
  windowMinimize: () => (inBrowser() ? Promise.resolve() : invoke<void>("window_minimize")),
  windowIsMaximized: () => (inBrowser() ? Promise.resolve(false) : invoke<boolean>("window_is_maximized")),
  windowToggleMaximize: () => (inBrowser() ? Promise.resolve(false) : invoke<boolean>("window_toggle_maximize")),
  setWindowBackgroundColor: (r: number, g: number, b: number, a: number) =>
    inBrowser() ? Promise.resolve() : invoke<void>("set_window_background_color", { r, g, b, a }),
  getSettings: () => invoke<T.Settings>("get_settings"),
  setSettings: (settings: T.Settings) => invoke<void>("set_settings", { settings }),
  resetSettings: () => invoke<T.Settings>("reset_settings"),

  listDir: (path: string, opts: { showHidden: boolean; hideTemp: boolean; dirsOnly?: boolean }) =>
    invoke<T.FileEntry[]>("list_dir", { path, opts }),
  listDrives: () => invoke<T.Drive[]>("list_drives"),
  systemStats: () => invoke<T.SysStats>("system_stats"),
  gitLastFileChange: (path: string) => invoke<{ history: { entries: NonNullable<T.WorktreeComparison["lastTouch"]>[] } }>("git_scoped_history", { path, file: path, limit: 1 }),
  gitCommitDetails: (path: string, commit: string, base: string | null = null) => invoke<T.GitCommitDetails>("git_commit_details", { path, commit, base }),
  gitWorktreeComparison: (path: string) => invoke<T.WorktreeComparison>("git_worktree_comparison", { path }),
  diffCompare: (a: T.DiffSource, b: T.DiffSource, opts: T.DiffOptions) => invoke<T.DiffResult>("diff_compare", { a, b, opts }),
  diffPageOpen: (id: number, a: T.DiffSource, b: T.DiffSource, opts: T.DiffOptions) => inBrowser() ? browserDiffPageOpen(id, a, b, opts) : invoke<T.CompareSummary>("diff_page_open", { id, a, b, opts }),
  diffPageLines: (id: number, kind: "left" | "right", start: number, count: number) => invoke<T.ComparePage<string>>("diff_page_range", { id, kind, start, count }),
  diffPageSmall: (id: number) => inBrowser() ? Promise.resolve(browserDiffPageSmall(id)) : invoke<T.DiffResult["text"]>("diff_page_range", { id, kind: "small", start: 0, count: 0 }),
  diffPageHunks: (id: number, start: number, count: number) => inBrowser() ? Promise.resolve(browserDiffPageHunks(id, start, count)) : invoke<T.ComparePage<T.CompareHunk>>("diff_page_range", { id, kind: "hunks", start, count }),
  diffPageClose: (through: number) => { if (inBrowser()) { browserDiffPageClose(through); return Promise.resolve(); } return invoke<void>("diff_page_close", { through }); },
  diffHighlight: (name: string, lines: string[]) => invoke<T.SyntaxHighlight | null>("diff_highlight", { name, lines }),
  /** Per line of a unified patch: null (no change or changed as a whole), [] (layout only) or the changed ranges. */
  diffPatchMarks: (name: string, patch: string) => invoke<([number, number][] | null)[]>("diff_patch_marks", { name, patch }),
  diffPatch: (a: T.DiffSource, b: T.DiffSource, format?: T.PatchFormat) => invoke<string>("diff_patch", { a, b, format }),
  diffSavePatch: (a: T.DiffSource, b: T.DiffSource, dest: string, format?: T.PatchFormat) => invoke<void>("diff_save_patch", { a, b, dest, format }),
  compareSessionsLoad: () => invoke<T.CompareSessionsLoaded>("compare_sessions_load"),
  compareSessionsSave: (sessions: T.CompareSessions) => invoke<void>("compare_sessions_save", { sessions }),
  compareSessionsRelink: (from: string, to: string, sessions?: T.CompareSessions) => invoke<T.CompareSessionsLoaded>("compare_sessions_relink", { from, to, sessions }),
  /** Standalone HTML report of the pairs (no script, no remote asset), written off the UI thread. */
  compareReportHtml: (dest: string, title: string, pairs: [T.DiffSource, T.DiffSource][], options?: T.DiffOptions) => invoke<void>("compare_report_html", { dest, title, pairs, options }),
  compareReportPdf: (dest: string, title: string, pairs: [T.DiffSource, T.DiffSource][], options?: T.DiffOptions) => invoke<void>("compare_report_pdf", { dest, title, pairs, options }),
  /** Syntax-tree comparison of one file's two sides; `name` gives the language. Disk paths are read verbatim. */
  semanticCompare: (name: string, left?: string | null, right?: string | null, leftPath?: string | null, rightPath?: string | null) =>
    invoke<T.SemanticVerdict>("semantic_compare", { name, left: left ?? null, right: right ?? null, leftPath: leftPath ?? null, rightPath: rightPath ?? null }),
  /** The structural outline of one source (`name` gives the language), or the reason there is none. */
  semanticOutline: (name: string, text: string, right?: boolean) => invoke<T.OutlineResult>("semantic_outline", { name, text, right }),
  /** Whether a file is generated (reason code), for collapsing it by default; presentation only. */
  generatedClassify: (path: string, text: string) => invoke<T.Generated | null>("generated_classify", { path, text }),
  /** Explains the changes with the local model the user has set up; only on request. `disabled` without opt-in. */
  compareAiExplain: (
    requestId: number,
    leftLabel: string,
    rightLabel: string,
    left: string,
    right: string,
    options?: T.DiffOptions,
    scope?: T.AiExplainScope,
  ) => invoke<T.AiOutcome>("compare_ai_explain", { requestId, leftLabel, rightLabel, left, right, options: options ?? null, scope: scope ?? null }),
  /** The revision of a conflict's text, to ask for and accept a proposal against exactly that text. */
  mergeAiRevision: (conflict: T.MergeConflictInput) => invoke<string>("merge_ai_revision", { conflict }),
  /** Asks the local model for lines that replace one conflict; nothing is applied (opt-in, only on request). */
  mergeAiPropose: (requestId: number, conflict: T.MergeConflictInput, seenRevision: string, syntax?: T.MergeSyntaxCheck | null) => invoke<T.MergeAiOutcome>("merge_ai_propose", { requestId, conflict, seenRevision, syntax: syntax ?? null }),
  compareAiCancel: (requestId: number) => invoke<void>("compare_ai_cancel", { requestId }),
  compareAiConfigGet: () => invoke<T.CompareAiConfig>("compare_ai_config_get"),
  compareAiConfigSave: (config: T.CompareAiConfig) => invoke<void>("compare_ai_config_save", { config }),
  semanticCompareCancel: () => invoke<void>("semantic_compare_cancel"),
  compareReportCancel: () => invoke<void>("compare_report_cancel"),
  // Importing a patch: preview writes nothing; apply checks again, writes atomically and puts files back if a write fails.
  /** The format a patch text is written in, from its own marks, or null. */
  patchSniff: (text: string) => invoke<T.PatchFormat | null>("patch_sniff", { text }),
  patchPreview: (root: string, text: string, format: T.PatchFormat, strip?: number, target?: string) => invoke<T.PatchPreview>("patch_preview", { root, text, format, strip, target }),
  patchApply: (root: string, text: string, format: T.PatchFormat, strip: number | undefined, target: string | undefined, fingerprint: string) => invoke<string[]>("patch_apply", { root, text, format, strip, target, fingerprint }),
  gitRoot: (path: string) => invoke<string | null>("git_root", { path }),
  dircmpCompare: (left: string, right: string, opts: T.DirCmpOptions) => invoke<T.DirCmpResult>("dircmp_compare", { left, right, opts }),
  dircmpCancel: () => invoke<void>("dircmp_cancel"),
  syncApply: (ops: T.SyncOp[], checks: T.SyncCheck[]) => invoke<T.SyncReport>("sync_apply", { ops, checks }),
  syncTreeFingerprints: (paths: string[]) => invoke<string[]>("sync_tree_fingerprints", { paths }),
  syncFileFingerprints: (paths: string[]) => invoke<(string | null)[]>("sync_file_fingerprints", { paths }),
  /** A UTF-8 text file with the revision a later save is checked against; a pipe, a binary or a too large file is refused. */
  readTextFile: (path: string) => invoke<TextFileVersion & { text: string }>("read_text_file", { path }),
  textFileVersion: (path: string) => invoke<TextFileVersion | null>("text_file_version", { path }),
  /** Resolves to whether the save can be undone (the old version went to the trash). */
  saveText: (path: string, text: string, expected?: string, expectMissing?: boolean) =>
    invoke<boolean>("save_text", { path, text, expected, expectMissing }),
  saveTextWithUndo: (path: string, text: string, expected?: string, expectMissing?: boolean) =>
    invoke<{ undoable: boolean; undoId: number | null }>("save_text_with_undo", { path, text, expected, expectMissing }),
  imageDiff: (a: string, b: string) => invoke<T.ImageDiff>("image_diff", { a, b }),
  similarityMatrix: (paths: string[]) => invoke<T.SimilarityMatrix>("similarity_matrix", { paths }),
  tableCompare: (a: string, b: string, opts: T.TableDiffOptions) => invoke<T.TableDiffResult>("table_compare", { a, b, opts }),
  eject: (mount: string, powerOff?: boolean) => invoke<void>("eject", { mount, powerOff }),
  otherVolumes: () => invoke<T.Volume[]>("other_volumes"),
  volumeMount: (id: string) => invoke<string>("volume_mount", { id }),
  volumePowerOff: (id: string) => invoke<void>("volume_power_off", { id }),
  volumeRepairMount: (id: string) => invoke<string>("volume_repair_mount", { id }),
  volumeUnlock: (id: string, passphrase: string, remember = false) =>
    invoke<string | null>("volume_unlock", { id, passphrase, remember }),
  driveLock: (mount: string) => invoke<void>("drive_lock", { mount }),
  resolvePrefs: (path: string) => invoke<T.ResolvedPrefs>("resolve_prefs", { path }),
  setPrefs: (path: string, prefs: T.FolderPrefs) => invoke<void>("set_prefs", { path, prefs }),
  clearPrefs: () => invoke<void>("clear_prefs"),

  sessionGet: () => invoke<T.Session>("session_get"),
  sessionCmd: (cmd: SessionCmd, workspaceId?: number) => invoke<T.Session>("session_cmd", { cmd, workspaceId }),
  workspacesGet: () => invoke<T.WorkspaceSnapshot>("workspaces_get"),
  workspaceCmd: (cmd: WorkspaceCmd, sourceWorkspaceId?: number, expectedWorkspaceIds?: number[]) =>
    invoke<T.WorkspaceSnapshot>("workspace_cmd", { cmd, sourceWorkspaceId, expectedWorkspaceIds }),
  gravitySuggest: (path: string, oppositePaths: string[]) =>
    invoke<T.GravitySuggestion | null>("gravity_suggest", { path, oppositePaths }),
  gravityRecord: (pathA: string, pathB: string, weight: number, unsuppress = false) =>
    invoke<void>("gravity_record", { pathA, pathB, weight, unsuppress }),
  gravityDismiss: (pathA: string, pathB: string) =>
    invoke<void>("gravity_dismiss", { pathA, pathB }),
  // Three-way merge. Reads only; a `ticket` lets `mergeCancel` stop it. Nothing is written by any of them.
  mergeThreeWay: (request: T.MergeRequest, ticket: number) => invoke<T.MergeOutcome>("merge_three_way", { request, ticket }),
  mergeResolve: (request: T.MergeRequest, choices: Record<string, T.MergeChoice>, ticket: number) => invoke<T.MergeResolution>("merge_resolve", { request, choices, ticket }),
  mergePreview: (request: T.MergeRequest, choices: Record<string, T.MergeChoice>, ticket: number) => invoke<T.MergePreview | null>("merge_preview", { request, choices, ticket }),
  mergeCancel: (ticket: number) => invoke<void>("merge_cancel", { ticket }),
  // Text of a document (.docx or .pdf) for comparison; a `ticket` lets `docExtractCancel` stop it.
  docExtract: (path: string, ticket: number) => invoke<T.DocExtract>("doc_extract", { path, ticket }),
  docExtractCancel: (ticket: number) => invoke<void>("doc_extract_cancel", { ticket }),
  favoritesGet: () => invoke<T.Favorites>("favorites_get"),
  favoritesCmd: (cmd: FavCmd) => invoke<T.Favorites>("favorites_cmd", { cmd }),
  // Import of the places other file managers keep. Nothing is called at startup: the Welcome step and the Settings
  // dialog detect when they open, and reading is per source.
  favoritesImportDetect: () => invoke<ImportSourceInfo[]>("favorites_import_detect"),
  favoritesImportRead: (id: string) => invoke<ImportPlace[]>("favorites_import_read", { id }),
  favoritesImportApply: (choices: ImportChoice[]) => invoke<{ favorites: T.Favorites; receipt: ImportReceipt }>("favorites_import_apply", { choices }),
  favoritesImportUndo: (receipt: ImportReceipt) => invoke<T.Favorites>("favorites_import_undo", { receipt }),
  marksGet: () => invoke<T.Marks>("marks_get"),
  marksCmd: (cmd: MarkCmd) => invoke<T.Marks>("marks_cmd", { cmd }),
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
  /** The repository path when Git refuses it as owned by another user ("dubious ownership"), else null. */
  gitUntrusted: (dir: string) => invoke<string | null>("git_untrusted", { dir }),
  /** Adds the refused repository to Git's global safe.directory; returns the trusted path. */
  gitTrust: (dir: string) => invoke<string>("git_trust", { dir }),
  gitCommits: (path: string) => invoke<{ id: string; subject: string }[]>("git_commits", { path }),
  /** Renames all items as one undo step (swaps allowed); resolves to the new paths. */
  batchRename: (items: [string, string][]) => invoke<string[]>("batch_rename", { items }),
  batchRenameWithUndo: (items: [string, string][]) => invoke<{ paths: string[]; undoId: number | null }>("batch_rename_with_undo", { items }),
  createDir: (parent: string, name: string) => invoke<string>("create_dir", { parent, name }),
  ensureDir: (path: string) => invoke<string>("ensure_dir", { path }),
  createDirWithUndo: (parent: string, name: string) => invoke<{ path: string; undoId: number | null }>("create_dir_with_undo", { parent, name }),
  createFile: (parent: string, name: string, template?: string) => invoke<string>("create_file", { parent, name, template }),
  createFileWithUndo: (parent: string, name: string, template?: string) => invoke<{ path: string; undoId: number | null }>("create_file_with_undo", { parent, name, template }),
  /** Starts a duplicate task; resolves to where the copies will appear. */
  duplicate: (paths: string[]) => invoke<string[]>("duplicate", { paths }),
  findExisting: (paths: string[]) => invoke<string[]>("find_existing", { paths }),
  trash: (paths: string[]) => invoke<number | null>("trash", { paths }),
  symlink: (target: string, destDir: string) => invoke<string>("symlink", { target, destDir }),
  fontFamilies: () => invoke<T.FontFamily[]>("font_families"),
  hexDiff: (left: string, right: string) => invoke<T.HexDiffResult | null>("hex_diff", { left, right }),
  hexDiffCancel: () => invoke<void>("hex_diff_cancel"),
  readBytes: async (path: string, offset: number, len: number) =>
    new Uint8Array(await invoke<ArrayBuffer>("read_bytes", { path, offset, len })),
  /** Windows / macOS: drags `paths` as real files (resolves "dropped" or "cancelled"). */
  startNativeDrag: (paths: string[]) => invoke<string>("start_native_drag", { paths }),
  undo: (permanent = false) => invoke<string | null>("undo", { permanent }),
  undoById: (id: number, permanent = false) => invoke<string | null>("undo_by_id", { id, permanent }),
  undoPeek: () => invoke<string | null>("undo_peek"),
  undoIn: (dir: string, permanent = false) => invoke<string | null>("undo_in", { dir, permanent }),
  undoPeekIn: (dir: string) => invoke<string | null>("undo_peek_in", { dir }),
  shortcutResolve: (path: string) => invoke<T.FileEntry | null>("shortcut_resolve", { path }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openPaths: (paths: string[]) => invoke<void>("open_paths", { paths }),
  openElevated: (path: string) => invoke<void>("open_elevated", { path }),
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
  indexQuery: (query: string, scope: string | null, limit: number, includeIgnored?: boolean, priority?: string | null) =>
    invoke<T.IndexResult>(
      "index_query",
      includeIgnored
        ? (priority ? { query, scope, limit, includeIgnored, priority } : { query, scope, limit, includeIgnored })
        : { query, scope, limit },
    ),
  indexHiddenCount: (query: string, scope: string | null, priority?: string | null) =>
    invoke<T.HiddenCount>("index_hidden_count", priority ? { query, scope, priority } : { query, scope }),
  indexHiddenCancel: () => invoke<void>("index_hidden_cancel"),
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
  macosDragPaths: () => invoke<string[]>("macos_drag_paths"),
  openFullDiskAccessSettings: () => invoke<void>("open_full_disk_access_settings"),
  /** Reports the active window's folder and selection for `gall selection`. */
  selectionPublish: (folder: string, selection: string[], focused: string | null) =>
    invoke<void>("selection_publish", { folder, selection, focused }),
  /** A picker window answers with the chosen paths (and the index of the active filter). */
  pickFinish: (paths: string[], filter: number | null, choices: [string, string][] = []) => invoke<void>("pick_finish", { paths, filter, choices }),
  pickCancel: () => invoke<void>("pick_cancel"),
  clipSetText: (text: string) => invoke<void>("clip_set_text", { text }),
  selftestClipDump: () => invoke<[string, string][]>("selftest_clip_dump"),
  clipInspect: () => invoke<T.ClipContent>("clip_inspect"),
  clipPasteAsFile: (dir: string, asKind: "text" | "image" | "url") => invoke<string>("clip_paste_as_file", { dir, asKind }),
  clipPasteFiles: (dest: string, conflict: T.ConflictPolicy, paths?: string[], merge = false) =>
    invoke<number>("clip_paste_files", { dest, conflict, paths, merge }),
  startupArgs: () => invoke<T.CliArgs>("cli_initial_args"),
  finishCompareWait: (outcome: "accepted" | "cancelled" | "error", content?: string, message?: string) =>
    invoke<void>("compare_wait_finish", { outcome, content, message }),
  /** Launch requests that reached this window before it was ready (call after `windowReady`). */
  cliTakePending: () => invoke<T.CliArgs[]>("cli_take_pending"),
  filemanagerTakePending: () => invoke<T.FileManagerAction[]>("filemanager_take_pending"),
  newWindow: (opts: { paths?: string[]; selectPaths?: string[]; compare?: [string, string]; gitRepo?: string } = {}) =>
    invoke<string>("new_window", { paths: opts.paths, selectPaths: opts.selectPaths, compare: opts.compare, gitRepo: opts.gitRepo }),
  fileManagerStatus: () => invoke<{ folderHandler: string | null; revealHandler: string | null; revealAvailable: boolean }>("file_manager_status"),
  defaultFileManagerStatus: () => invoke<T.DefaultFileManagerStatus>("default_file_manager_status"),
  defaultFileManagerSet: () => invoke<T.DefaultFileManagerStatus>("default_file_manager_set"),
  defaultFileManagerRestore: () => invoke<T.DefaultFileManagerStatus>("default_file_manager_restore"),
  omarchyStateGet: () => invoke<T.OmarchyState>("omarchy_state_get"),
  gvfsNetworkMounts: () => invoke<T.NetworkMount[]>("gvfs_network_mounts"),
  connectServer: (uri: string) => invoke<void>("connect_server", { uri }),
  gvfsUnmount: (target: string) => invoke<void>("gvfs_unmount", { target }),
  actionsList: () => invoke<T.ActionDef[]>("actions_list"),
  actionsForSelection: (targets: T.ActionTarget[]) => invoke<T.ActionDef[]>("actions_for_selection", { targets }),
  actionRun: (actionId: string, paths: string[], dir: string) => invoke<void>("action_run", { actionId, paths, dir }),
  zoxideDirs: () => invoke<string[]>("zoxide_dirs"),
  omarchyTerminalCwd: () => invoke<string | null>("omarchy_terminal_cwd"),
  modifiers: () => invoke<{ known: boolean; ctrl: boolean; shift: boolean; alt: boolean }>("modifiers"),
  selftestShot: (name: string) => invoke<string>("selftest_shot", { name }),
  selftestSimulateUwsm: () => invoke<boolean>("selftest_simulate_uwsm"),
  selftestResize: (width: number, height: number) => invoke<void>("selftest_resize", { width, height }),
  selftestFrameTrace: (start: boolean) => invoke<[timestampNs: number, refreshNs: number][] | null>("selftest_frame_trace", { start }),
  selftestDone: (report: string) => invoke<void>("selftest_done", { report }),
  uiLog: (level: "info" | "warn" | "error", msg: string) => invoke<void>("ui_log", { level, msg }),
  logBacklog: () => invoke<BackendLog[]>("log_backlog"),
  listCloudFolders: () => invoke<T.CloudFolder[]>("list_cloud_folders"),
  openWithApps: (path: string) => invoke<T.AppChoice[]>("open_with_apps", { path }),
  openWithAllApps: () => invoke<T.AppChoice[]>("open_with_all_apps"),
  openWithSetDefault: (path: string, id: string) => invoke<string>("open_with_set_default", { path, id }),
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
  | { op: "hidden"; b: number | null; on: boolean | null }
  | { op: "columns"; b: number; on: boolean }
  | { op: "view"; b: number; prefs: T.FolderPrefs }
  | { op: "swap" }
  | { op: "navigateSolo"; b: number; path: string }
  | { op: "pairTabs"; leftIdx: number; rightIdx: number; mode: T.PairMode; flowDir?: T.FlowDirection; flowOp?: T.FlowOp }
  | { op: "unpair"; pairId: number }
  | { op: "pairMode"; pairId: number; mode: T.PairMode; flowDir?: T.FlowDirection; flowOp?: T.FlowOp }
  | { op: "pairReanchor"; pairId: number };

export type WorkspaceCmd =
  | { op: "switch"; idx: number }
  | { op: "create"; name?: string | null; accent?: string | null; cloneCurrent?: boolean }
  | { op: "duplicate"; idx: number }
  | { op: "rename"; idx: number; name: string | null }
  | { op: "accent"; idx: number; accent: string | null }
  | { op: "reorder"; from: number; to: number }
  | { op: "close"; idx: number };

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

export type MarkCmd =
  | { op: "set"; key: string; path: string; item: string | null }
  | { op: "remove"; key: string };

export const events = {
  fsChanged: (cb: (p: { path: string; storm: boolean }) => void): Promise<UnlistenFn> =>
    listen<{ path: string; storm: boolean }>("fs-changed", (e) => cb(e.payload)),
  task: (cb: (e: T.TaskEvent) => void) => listen<T.TaskEvent>("task", (e) => cb(e.payload)),
  searchBatch: (cb: (p: { id: number; entries: T.SearchHit[] }) => void) =>
    listen<{ id: number; entries: T.SearchHit[] }>("search-batch", (e) => cb(e.payload)),
  searchDone: (cb: (p: { id: number; hits: number; cancelled: boolean }) => void) =>
    listen<{ id: number; hits: number; cancelled: boolean }>("search-done", (e) => cb(e.payload)),
  searchHidden: (cb: (p: { id: number; count: number; capped: boolean }) => void) =>
    listen<{ id: number; count: number; capped: boolean }>("search-hidden", (e) => cb(e.payload)),
  indexUpdated: (cb: (s: T.IndexStatus) => void) => listen<T.IndexStatus>("index-updated", (e) => cb(e.payload)),
  grepBatch: (cb: (p: { id: number; files: T.FileMatches[] }) => void) =>
    listen<{ id: number; files: T.FileMatches[] }>("grep-batch", (e) => cb(e.payload)),
  grepDone: (cb: (p: { id: number; stats: T.GrepStats; cancelled: boolean; error: string | null }) => void) =>
    listen<{ id: number; stats: T.GrepStats; cancelled: boolean; error: string | null }>("grep-done", (e) => cb(e.payload)),
  grepHidden: (cb: (p: { id: number; count: number; capped: boolean }) => void) =>
    listen<{ id: number; count: number; capped: boolean }>("grep-hidden", (e) => cb(e.payload)),
  /** A launch request for this window (CLI, second instance, FileManager1). */
  cliArgs: (cb: (a: T.CliArgs) => void) => listen<T.CliArgs>("cli-args", (e) => cb(e.payload)),
  compareWaitClose: (cb: () => void) => listen("compare-wait-close", cb),
  fileManagerAction: (cb: (a: T.FileManagerAction) => void) => listen<T.FileManagerAction>("filemanager-action", (e) => cb(e.payload)),
  omarchyStateChanged: (cb: (s: T.OmarchyState) => void) => listen<T.OmarchyState>("omarchy-state-changed", (e) => cb(e.payload)),
  folderSizeProgress: (cb: (p: { path: string; size: T.DirSize; ticket: number }) => void) =>
    listen<{ path: string; size: T.DirSize; ticket: number }>("folder-size-progress", (e) => cb(e.payload)),
  backendLog: (cb: (l: BackendLog) => void) => listen<BackendLog>("backend-log", (e) => cb(e.payload)),
};

/** A warning or error logged by the backend. */
export interface BackendLog { level: "warn" | "error"; msg: string }

export const fileSrc = (path: string) => convertFileSrc(path);

/** URL prefix of the backend's local media server (Linux), set from `AppInfo` at startup. */
let mediaBase: string | null = null;
export const setMediaBase = (base: string | null | undefined) => { mediaBase = base ?? null; };

/** Source for `<video>`/`<audio>`. WebKitGTK cannot play media from the asset protocol, so on
 *  Linux it comes from the loopback media server; elsewhere it is the asset URL. */
export const mediaSrc = (path: string) =>
  mediaBase ? mediaBase + path.split("/").map(encodeURIComponent).join("/").replace(/^\//, "") : fileSrc(path);
