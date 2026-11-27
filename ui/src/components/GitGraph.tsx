import { Icon } from "./Icon";
import { ThemedSelect } from "./ThemedSelect";
import { maxOf } from "../core/maxOf";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GitChanges } from "./GitChanges";
import { GitCommitDetails } from "./GitCommitDetails";
import { GitAdvanced } from "./GitAdvanced";
import { GitRebase } from "./GitRebase";
import { GitStashList, GitStashPreview } from "./GitStashes";
import { GitWorktreeList } from "./GitWorktreeList";
import { GitMergeEditor } from "./GitMerge";
import { chooseMainline, targetCommit, useGitRepositoryActions, type GitTarget } from "./useGitRepositoryActions";
import { createProgressId } from "../core/gitProgress";
import { GitProgressBar, useGitProgress, useLocalProgress, type GitProgressSeed } from "./GitProgressBar";
import * as A from "../state/actions";
import { ROW_HEIGHT, ROW_MID, edgePath, inFocus, keepListed, layoutHistory, oldestFirst, rangeIds, reachableFrom, toggleId } from "../core/gitGraph";
import { badgeRefs, remoteTagName } from "../core/gitBadges";
import { hitHistory, historyQuery, nextHit, parseSearch, searchMarks, splitMatches } from "../core/gitSearch";
import { fileHistoryRows, restoreFileHistoryCursor, type HistoryEntry } from "../core/gitFileHistory";
import { useCommitSearch } from "./useCommitSearch";
import { GitJournal } from "./GitJournal";
import { GitReflog } from "./GitReflog";
import { journalKinds, jumpPreview, type JournalView } from "../core/gitJournal";
import { save } from "@tauri-apps/plugin-dialog";
import { activeConflicts, gitActionEnabled, gitMenuGroup, gitMenuGroups, gitNavigationHint, gitPanelOf, rewritesHistory, stashAtCommit, stashEntryActions, type GitReference, type Repository } from "../core/gitRepository";
import { GIT_SCOPE_LABELS, gitCommandForKey, gitCommands, gitKeyOf, gitSharedCommands, gitShownKey, isGitChord, type GitCommand, type GitContext } from "../core/gitCommands";
import { useGitKeys } from "../state/gitKeys";
import { GitKeyCap } from "./GitKeyCap";
import { GitPaletteHost, type PaletteHandle } from "./GitPalette";
import { keepsSelection, type Candidate, type PaletteCommand } from "../core/gitPalette";
import { platformLabel } from "../core/keymap";
import { commitUrl, copyChoices, type CopyChoice } from "../core/gitCopy";
import { basename } from "../core/path";
import { worktreeSource, type WorktreeSource } from "../core/gitWorktree";
import { forecastText, useForecasts } from "../state/gitForecast";
import { commitMessages } from "../state/gitMessages";
import { createWorktree, tryInWorktree, draggedChange, endChangeDrag } from "../state/gitWorktree";
import { refreshAll } from "../state/controllers";
import { ROOT_BASE, combinedMessage, squashRange, type PlanCommit } from "../core/gitRebase";
import { HintLabels } from "./HintLabels";
import { CUSTOM_PREFIX, customTarget, expandCommand, joinCommandLine, type GitCommandDef } from "../core/gitCustom";
import { getState, setState, useStore, type MenuItem } from "../state/store";
import { RepoPicker } from "./RepoPicker";
import "./GitGraph.css";
import { tt } from "../i18n";
import { belowAnchor, elementAnchor, eventAnchor, textAnchor, type Anchor } from "../core/anchor";

type Graph = { history: string; branches: string[]; head: string; changes: string; readOnlyReason?: string | null };
type Conflict = { file: string; base: string; ours: string; theirs: string; result: string; binary: boolean; token: string };
type Sidebar = "branches" | "tags" | "stashes" | "remotes" | "worktrees" | "conflicts";
type RemoteTag = { name: string; id: string; peeled: string | null };
const isRemoteTag = (ref: GitReference, refs: GitReference[]) => remoteTagName(ref, refs) !== null;
const refGroups: Record<"branches" | "tags", [string, string, (ref: GitReference, refs: GitReference[]) => boolean][]> = {
  branches: [["branch", "Local branches", (ref) => ref.kind === "branch"], ["remote", "Remote branches", (ref, refs) => ref.kind === "remote" && !isRemoteTag(ref, refs)]],
  tags: [["tag", "Tags", (ref) => ref.kind === "tag"], ["remoteTag", "Remote tags", isRemoteTag]],
};
/** A remote branch or tag represented by a local counterpart, or a remote HEAD. */
function hasLocalCounterpart(ref: GitReference, refs: GitReference[]) {
  if (ref.kind !== "remote") return false;
  if (isRemoteTag(ref, refs)) return true;
  return /\/HEAD$/.test(ref.name) || refs.some((r) => r.kind === "branch" && r.upstream === ref.name);
}
function tagRemoteState(tag: GitReference, remote: RemoteTag[] | null) {
  if (!remote) return "";
  const match = remote.find((item) => item.name === tag.name);
  return !match ? "local" : (match.peeled ?? match.id) === tag.id ? "synced" : "differs";
}
/** Drag type of a branch or tag in the sidebar; dropped on the Worktrees tab it gets a worktree. */
const REF_DRAG = "text/x-gall-ref";
const COMMIT_DRAG = "text/x-gall-commits";
const sideNoun: Record<Sidebar, string> = { branches: "branch", tags: "tag", remotes: "remote", stashes: "stash", worktrees: "worktree", conflicts: "conflict" };
const sideLabels: Record<Sidebar, string> = { branches: "Branches", tags: "Tags", stashes: "Stashes", remotes: "Remotes", worktrees: "Worktrees", conflicts: "Conflicts" };
/** A submodule at any depth: `label` is its path from the opened repository, `path` its absolute folder. */
type SubmoduleEntry = { label: string; path: string; state: string; depth: number; name: string };
/** The submodules of a repository and, recursively, those of the submodules (depth first), listed by a light backend command. */
const listSubmodules = (dir: string) => invoke<{ repository: string | null; entries: SubmoduleEntry[] }>("git_submodule_tree", { path: dir });

/** Presses a key (as the registry names it) on an element, for hints that are clicked instead. */
function pressKey(name: string, element: HTMLElement) {
  const parts = name.split("+"), key = parts.pop() || "+";
  element.dispatchEvent(new KeyboardEvent("keydown", { key, ctrlKey: parts.includes("Ctrl"), altKey: parts.includes("Alt"), shiftKey: parts.includes("Shift") || /^[A-Z]$/.test(key), bubbles: true, cancelable: true }));
}
export function GitGraph({ path: initialPath, compareFile, focusFile = compareFile, startScoped = !!focusFile && !compareFile, compare, show }: { path: string; compareFile?: string; focusFile?: string; startScoped?: boolean; /** Two commits to show the difference of. */ compare?: [string, string]; /** A commit to go to. */ show?: string }) {
  // The workspace follows the selected repository: the opened one, or one of its submodules.
  const [path, setPath] = useState(initialPath), [mainRoot, setMainRoot] = useState(initialPath);
  const [reflog, setReflog] = useState(false), [limit, setLimit] = useState(200), [reference, setReference] = useState(""), [search, setSearch] = useState("");
  // Visual focus on a branch (full reference name): only dims what its tip does not reach. It is not the keyboard focus, the
  // action target, the checked-out branch or the explicit reference query above, and it never starts a Git request.
  const [focusRef, setFocusRef] = useState<string | null>(null);
  const [onlyFile, setOnlyFile] = useState(startScoped);
  const [fileLimit, setFileLimit] = useState(200);
  const [fileHistory, setFileHistory] = useState<{ entries: HistoryEntry[]; truncated: boolean; incomplete: boolean; readOnlyReason?: string | null } | null>(null);
  const scoped = onlyFile && !!focusFile && path === initialPath;
  const scopeKey = JSON.stringify([path, scoped ? focusFile : null]);
  const cursors = useRef(new Map<string, { id: string | null; inspected: string | null; scroll: number }>());
  const displayedScope = useRef<string | null>(null);
  const liveCursor = useRef<{ id: string | null; inspected: string | null } | null>(null);
  const [loading, setLoading] = useState(false), [graph, setGraph] = useState<Graph | null>(null), [repository, setRepository] = useState<Repository | null>(null), [error, setError] = useState("");
  const [remoteTagResult, setRemoteTagResult] = useState<{ path: string; url: string; token: string; tags: RemoteTag[] } | null>(null);
  const [remoteTagRefresh, setRemoteTagRefresh] = useState(0);
  const originUrl = repository?.remotes.find((remote) => remote.name === "origin")?.fetchUrl;
  const referenceVersion = `${remoteTagRefresh}:${repository?.refs.map((ref) => `${ref.fullName}:${ref.id}`).join("\n") ?? ""}`;
  const remoteTags = !repository ? null : !originUrl ? (repository.remotes.length ? null : []) :
    remoteTagResult?.path === path && remoteTagResult.url === originUrl && remoteTagResult.token === referenceVersion ? remoteTagResult.tags : null;
  useEffect(() => {
    if (!repository) return;
    if (!originUrl) return;
    const token = referenceVersion;
    let active = true;
    let registered = false;
    const id = crypto.randomUUID();
    void invoke("git_remote_tags_register", { id }).then(async () => {
      registered = true;
      if (!active) {
        await invoke("git_remote_tags_release", { id });
        return;
      }
      const tags = await invoke<RemoteTag[]>("git_remote_tags", { path, remote: "origin", id });
      if (active) setRemoteTagResult({ path, url: originUrl, token, tags });
    }).catch(() => { if (active) setRemoteTagResult(null); });
    return () => {
      active = false;
      if (registered) void invoke("git_remote_tags_release", { id });
    };
  }, [path, originUrl, referenceVersion]);

  const [childBusy, setChildBusy] = useState(false), [switchBusy, setSwitchBusy] = useState(false);
  const [switchProgressTask, setSwitchProgressTask] = useState<{ id: string; seed: GitProgressSeed } | null>(null);
  const switchProgress = useGitProgress(switchProgressTask?.id ?? null, switchProgressTask?.seed);
  const loadProgress = useLocalProgress(loading && !graph, "Loading history");
  const [showChanges, setShowChanges] = useState(false), [showAdvanced, setShowAdvanced] = useState(false), [advancedTab, setAdvancedTab] = useState<string | undefined>(undefined), [showRebase, setShowRebase] = useState(false), [rebaseOnto, setRebaseOnto] = useState(false);
  const [inspected, setInspected] = useState<string | null>(null), [compareBase, setCompareBase] = useState<string | null>(null), [rangeBase, setRangeBase] = useState<string | null>(null);
  const [selected, setSelected] = useState(0), [target, setTarget] = useState<GitTarget | null>({ kind: "working" }), [context, setContext] = useState<GitContext>("history");
  const [sidebar, setSidebar] = useState<Sidebar>("branches"), [showSidebar, setShowSidebar] = useState(() => window.innerWidth >= 1000 && !compareFile), [help, setHelp] = useState(false), [dropTab, setDropTab] = useState(false), [showJournal, setShowJournal] = useState(false), [showReflog, setShowReflog] = useState(false), [openError, setOpenError] = useState("");
  const [showAllGroups, setShowAllGroups] = useState<ReadonlySet<string>>(new Set());
  // Several commits chosen for one action; `rangeFrom` is set while `v` extends the choice with the cursor.
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set()), [rangeFrom, setRangeFrom] = useState<string | null>(null);
  const clickAnchor = useRef<string | null>(null);
  const doubleClick = useStore((s) => s.settings?.gitDoubleClick ?? "details");
  const [submodules, setSubmodules] = useState<SubmoduleEntry[]>([]), [conflict, setConflict] = useState<Conflict | null>(null), [conflictError, setConflictError] = useState("");
  const request = useRef(0), root = useRef<HTMLDivElement>(null), searchRef = useRef<HTMLInputElement>(null);
  const gitKeys = useGitKeys(), { vim, shortcuts } = gitKeys;
  const capOf = (command: GitCommand | undefined) => command ? platformLabel(gitShownKey(command, vim)) || undefined : undefined;
  useLayoutEffect(() => { liveCursor.current = { id: targetCommit(target), inspected }; }, [target, inspected]);
  const load = useCallback(async () => {
    const ticket = ++request.current; setLoading(true);
    try {
      const result = scoped
        ? await invoke<{ history: NonNullable<typeof fileHistory>; repository: Repository }>("git_scoped_history", { path, file: focusFile, limit: fileLimit })
        : null;
      const { graph: value, repository: repo } = result
        ? { graph: { history: fileHistoryRows(result.history.entries), branches: [], head: result.repository.head, changes: "", readOnlyReason: result.history.readOnlyReason }, repository: result.repository }
        : await invoke<{ graph: Graph; repository: Repository }>("git_graph_view", { path, comparison: !!compareFile, reflog, limit, reference: reference || null });
      if (ticket !== request.current) return;
      setGraph(value);
      setRepository(repo);
      setFileHistory(result?.history ?? null);
      setError("");
      setTarget((current) => current?.kind === "conflict" && !repo.conflicts.includes(current.file) ? { kind: "working" } : current);
      if (focusFile) {
        const sameScope = displayedScope.current === scopeKey;
        displayedScope.current = scopeKey;
        const ids = (result?.history.entries ?? layoutHistory(value.history)).map((row) => row.id);
        const saved = sameScope
          ? { ...liveCursor.current, scroll: root.current?.querySelector(".git-graph-lines")?.scrollTop ?? 0 }
          : cursors.current.get(scopeKey);
        const id = restoreFileHistoryCursor(ids, saved?.id);
        const working = !compareFile && !scoped && !saved?.id;
        setTarget(id && !working ? { kind: "commit", id } : scoped || compareFile ? null : { kind: "working" });
        const index = id ? ids.indexOf(id) : 0;
        const headIndex = ids.indexOf(value.head);
        setSelected(working ? Math.max(0, headIndex) : index + Number(!compareFile && !scoped && headIndex >= 0 && headIndex <= index));
        setInspected(id === saved?.inspected ? id : null);
        if (saved) requestAnimationFrame(() => {
          if (ticket !== request.current) return;
          const list = root.current?.querySelector(".git-graph-lines");
          if (list) list.scrollTop = saved.scroll;
        });
      }
    } catch (e) { if (ticket === request.current) setError(String(e)); }
    finally { if (ticket === request.current) setLoading(false); }
  }, [path, compareFile, reflog, limit, reference, focusFile, scoped, scopeKey, fileLimit]);
  useLayoutEffect(() => {
    displayedScope.current = null;
    setGraph(null); setError(""); setSelected(0); setTarget(compareFile || scoped ? null : { kind: "working" });
    setInspected(null); setPicked(new Set()); setRangeFrom(null);
    void load(); root.current?.focus();
    return () => { request.current++; };
  }, [load, compareFile, scoped]);
  const rememberScope = () => {
    if (displayedScope.current === scopeKey) {
      cursors.current.set(scopeKey, {
        id: targetCommit(target), inspected,
        scroll: root.current?.querySelector(".git-graph-lines")?.scrollTop ?? 0,
      });
    }
  };
  const toggleFileScope = (checked: boolean) => {
    rememberScope();
    beforeSearch.current = null;
    setSearch("");
    setShowChanges(false);
    setRangeBase(null);
    setCompareBase(null);
    setOnlyFile(checked);
  };
  useEffect(() => {
    const onFocus = () => { if (!document.hidden) void load(); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);
  useEffect(() => { setPath(initialPath); setMainRoot(initialPath); setSubmodules([]); }, [initialPath]);
  useEffect(() => {
    if (compareFile || path !== initialPath) return;
    let active = true;
    void listSubmodules(path).then((value) => { if (active) { setSubmodules(value.entries); if (value.repository) setMainRoot(value.repository); } }).catch(() => { if (active) setSubmodules([]); });
    return () => { active = false; };
  }, [path, initialPath, compareFile, repository]);
  const switchRepository = async (next: string) => {
    if (next === path || busy) return;
    if (!await A.confirm(tt("Switch repository?"), tt("Unsaved plans and conflict choices in this window are discarded."), "Switch")) return;
    setReference(""); setFocusRef(null); setSearch(""); beforeSearch.current = null; setLimit(200); setShowChanges(false); setShowAdvanced(false); setShowRebase(false); setCompareBase(null); setRangeBase(null); setPath(next);
  };  const reload = useCallback(async () => { refreshAll(); await load(); }, [load]);
  // Opt-in: changes stay with the branch they were made on. The branches that remember some are listed lazily, after the first
  // paint and only with the setting on, from the same repository token the rest of the window uses.
  const wipOn = useStore((s) => s.settings?.gitBranchWip ?? false);
  type Wip = { branch: string; id: string; created: number };
  const [wip, setWip] = useState<Wip[]>([]);
  const wipTicket = useRef(0);
  const loadWip = useCallback(async (): Promise<Wip[]> => {
    const ticket = ++wipTicket.current;
    if (!wipOn) { setWip([]); return []; }
    const found = await invoke<Wip[]>("git_wip_list", { path }).catch(() => [] as Wip[]);
    if (ticket === wipTicket.current) setWip(found);
    return found;
  }, [path, wipOn]);
  useEffect(() => { const timer = window.setTimeout(() => void loadWip(), 0); return () => { window.clearTimeout(timer); wipTicket.current++; }; }, [loadWip, repository?.token]);
  const wipBranches = useMemo(() => new Set(wip.map((item) => item.branch)), [wip]);
  const checkout = async (destination: string, branch: boolean, fresh = false) => {
    if (loading || childBusy || compareFile) return;
    setSwitchBusy(true); setError("");
    try {
      const latest = await invoke<Graph>("git_graph", { path });
      let action = "clean", message = "";
      // With the setting on, the changes stay with this branch (a detached HEAD has none to keep them for) and return with it.
      const remember = wipOn && (!latest.changes || !!repository?.branch);
      const hadWip = wipOn && branch && wipBranches.has(destination);
      if (latest.changes && !remember) {
        const result = await A.dialog({ title: tt("Local changes"), message: tt("Choose how to preserve or discard these changes before switching:\n\n{changes}", { changes: latest.changes }), buttons: [{ id: "cancel", label: "Cancel" }, { id: "clean", label: tt("Keep local changes"), key: "k" }, { id: "stash", label: "Stash", key: "s" }, { id: "commit", label: tt("Commit…"), key: "c" }, { id: "discard", label: tt("Discard…"), danger: true, key: "d" }] });
        if (!result || result.button === "cancel") return;
        action = result.button;
        if (action === "commit") { const value = await A.promptText(tt("Commit all tracked and untracked changes"), "", tt("Commit & switch")); if (!value?.trim()) return; message = value; }
        if (action === "discard" && !await A.confirm(tt("Discard local changes?"), tt("Tracked changes and untracked files will be removed from the working folder. Ignored files are kept. Gall retains a hidden recovery copy (a reference under refs/gall/recovery, not a stash).\n\n{changes}", { changes: latest.changes }), tt("Discard & switch"), true)) return;
      } else if (!latest.changes && !await A.confirm(tt("Checkout revision?"), tt("Switch {value} to {value2}? This replaces working files and can update their filesystem timestamps.", { value: repository?.branch ?? "detached HEAD", value2: branch ? destination : destination.slice(0, 12) }), "Checkout")) return;
      if (remember) action = "wip";
      const progressId = createProgressId("switch");
      setSwitchProgressTask({ id: progressId, seed: { operation: "checkout", phase: branch ? "Switching branch" : "Checking out commit", detail: branch ? destination : destination.slice(0, 8) } });
      await invoke("git_switch", { path, target: destination, branch, action, message, expected: fresh ? null : repository?.token ?? null, progressId });
      setShowChanges(false); setInspected(null); setTarget({ kind: "working" }); setSelected(0);
      await reload(); A.notify(branch ? tt("Switched to {destination}", { destination }) : tt("Detached HEAD at {destination}", { destination: destination.slice(0, 12) }));
      if (remember) {
        const left = repository?.branch;
        if (latest.changes && left) A.notify(tt("Your changes stay with {branch} and come back when you return to it.", { branch: left }));
        const now = await loadWip();
        if (hadWip && now.some((item) => item.branch === destination)) A.notify(tt("The changes {branch} remembers do not fit on top of it as it is now. They are kept; save them as a stash to apply them yourself.", { branch: destination }), "warn", [{ label: tt("Save as stash"), run: () => void wipAction("wipStash", destination) }]);
        else if (hadWip) A.notify(tt("The changes you left on {branch} are back.", { branch: destination }), "success");
      }
    } catch (e) { actions.setError(String(e)); await reload(); }
    finally { setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  const actions = useGitRepositoryActions(path, repository, reload, checkout, (head) => { setTarget({ kind: "commit", id: head }); setInspected(head); setRangeBase(null); setShowChanges(false); });
  const busy = childBusy || switchBusy || actions.busy;
  /** Parts of the window that hold unsaved work (see `markDirty`); closing asks only while this is not empty. */
  const dirty = useRef(new Set<string>());
  const markDirty = useCallback((key: string, on: boolean) => { if (on) dirty.current.add(key); else dirty.current.delete(key); }, []);
  const markMerge = useCallback((on: boolean) => markDirty("merge", on), [markDirty]);
  const markRebase = useCallback((on: boolean) => markDirty("rebase", on), [markDirty]);
  const closing = useRef(false), busyRef = useRef(false), askCloseRef = useRef<() => Promise<void>>(async () => {}), stepOutRef = useRef<() => boolean>(() => false);
  const askClose = async () => {
    if (closing.current) return;
    closing.current = true;
    // Only unsaved work in this window (conflict choices, manual edits, a rebase plan) needs a confirmation.
    try { if (dirty.current.size === 0 || await A.confirm(tt("Close Git workspace?"), tt("Unsaved conflict choices, manual edits and rebase plans in this window will be discarded."), "Close")) A.openOverlay(null); else root.current?.focus(); }
    finally { closing.current = false; }
  };
  busyRef.current = busy; askCloseRef.current = askClose;
  // Rows are disabled while an action runs, which makes the browser drop focus; give it back once the action
  // (or the confirmation it opened, even when cancelled) is over, so keys keep working without pressing Esc twice.
  const lastFocus = useRef<HTMLElement | null>(null);
  /** Position of the last focused sidebar row, so deleting a row (worktree, branch, tag…) keeps focus in the sidebar on its neighbour. */
  const lastSideRow = useRef(-1);
  const overlayType = useStore((s) => s.overlay?.type);
  useEffect(() => {
    if (busy || loading || (overlayType !== "gitGraph" && overlayType !== "gitCommits")) return;
    const active = document.activeElement;
    if (active && active !== document.body && root.current?.contains(active)) return;
    const remembered = lastFocus.current;
    if (remembered?.isConnected && root.current?.contains(remembered) && !(remembered as HTMLButtonElement).disabled) remembered.focus();
    else if (lastSideRow.current >= 0) {
      const rows = root.current?.querySelectorAll<HTMLElement>(".gg-sidebar .gg-side-row:not(:disabled)");
      const row = rows?.length ? rows[Math.min(lastSideRow.current, rows.length - 1)] : undefined;
      if (row) row.focus(); else root.current?.focus();
    }
    // The window itself: focusing a row here would run its selection side effects (e.g. closing the Tools panel).
    else root.current?.focus();
  }, [busy, loading, overlayType]);
  // When nothing inside the window has focus (after a click on the backdrop or a closed dialog), Esc must still reach it:
  // like inside the window, it first leaves an open lower panel and only then closes the window. (`q` needs no case of its
  // own: `Overlays` turns it into an Escape on the same element in every profile.)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const unfocused = target === document.body || target === document.documentElement || !!target?.classList?.contains("backdrop");
      if (e.key !== "Escape" || e.defaultPrevented || !unfocused) return;
      const o = getState().overlay;
      if (!o || (o.type !== "gitGraph" && o.type !== "gitCommits") || busyRef.current || getState().menu) return;
      e.preventDefault();
      if (!stepOutRef.current()) void askCloseRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const resolveWith = async (content: string) => {
    if (target?.kind !== "conflict") return;
    setSwitchBusy(true);
    try {
      await invoke<string>("git_repository_action", { path, request: { action: "resolveContent", file: target.file, content }, expected: repository?.token ?? null });
      A.notify(tt("Resolved {file}", { file: target.file })); await reload();
      // The next conflicted file opens by itself; after the last one the Continue button of the operation bar has focus, so
      // Enter is the one key that goes on (the operation bar enables it once no conflict is left).
      const fresh = await invoke<Repository>("git_repository", { path });
      const next = fresh.conflicts.find((file) => file !== target.file);
      if (next) { setTarget({ kind: "conflict", file: next }); setContext("conflicts"); }
      else if (fresh.operation) {
        A.notify(tt("All conflicts are resolved. Press Enter to continue the {operation}.", { operation: fresh.operation }));
        later(() => root.current?.querySelector<HTMLButtonElement>(".gg-operation button:not([disabled])")?.focus(), 120);
      }
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchBusy(false); }
  };
  const allRows = useMemo(() => layoutHistory(graph?.history ?? ""), [graph?.history]);
  // The focused branch and what its tip reaches in the loaded rows: recomputed when the tip moves or more history loads.
  const focusedRef = focusRef && !reference && !scoped && !compareFile ? repository?.refs.find((ref) => ref.fullName === focusRef && (ref.kind === "branch" || ref.kind === "remote")) : undefined;
  const reached = useMemo(() => focusedRef ? reachableFrom(allRows, focusedRef.id) : null, [allRows, focusedRef?.id]);
  useEffect(() => { if (focusRef && (reference || scoped || compareFile || (repository && !repository.refs.some((ref) => ref.fullName === focusRef)))) setFocusRef(null); }, [focusRef, reference, scoped, compareFile, repository]);
  useEffect(() => { setFocusRef(null); }, [path]);
  // Plain text filters the loaded commits; a prefix, or text that matches none of them, searches the whole history.
  const parsed = useMemo(() => parseSearch(search), [search]);
  const loadedRows = useMemo(() => { const text = scoped ? search.trim().toLocaleLowerCase() : parsed.kind === "text" ? parsed.text.toLocaleLowerCase() : ""; return text ? allRows.filter((row) => [row.id, row.subject, row.author, ...row.refs].join(" ").toLocaleLowerCase().includes(text)) : allRows; }, [allRows, parsed, scoped, search]);
  const wanted = graph && !scoped ? historyQuery(parsed, loadedRows.length) : null;
  const found = useCommitSearch(path, wanted, reflog, reference, graph?.history);
  const searchProgress = useGitProgress(found.running ? found.progressId : null, { operation: "search", phase: "Searching history", cancellable: true });
  const foundRows = useMemo(() => layoutHistory(hitHistory(found.lines)), [found.lines]);
  const rows = wanted ? foundRows : loadedRows;
  const searching = parsed.kind !== "none", marks = useMemo(() => searchMarks(parsed), [parsed]);
  const marked = (text: string, needle: string) => splitMatches(text, needle).map((piece, i) => i % 2 ? <mark key={i}>{piece}</mark> : piece);
  /** What was shown before the search, for Esc. */
  const beforeSearch = useRef<{ target: GitTarget | null; selected: number; inspected: string | null; scroll: number } | null>(null);
  const changeSearch = (text: string) => {
    if (!search.trim() && text.trim()) beforeSearch.current = { target, selected, inspected, scroll: root.current?.querySelector(".git-graph-lines")?.scrollTop ?? 0 };
    setSearch(text);
    if (text.trim()) { setSelected(0); setTarget(null); setInspected(null); setShowChanges(false); }
    else leaveSearch();
  };
  const leaveSearch = () => {
    const before = beforeSearch.current; beforeSearch.current = null;
    setSearch("");
    if (!before) return;
    setTarget(before.target); setSelected(before.selected); setInspected(before.inspected);
    // After the whole list is back.
    later(() => { const list = root.current?.querySelector(".git-graph-lines"); if (list) list.scrollTop = before.scroll; }, 60);
  };
  useEffect(() => { setPicked((current) => keepListed(current, allRows)); setRangeFrom((from) => from && !allRows.some((row) => row.id === from) ? null : from); }, [allRows]);
  const conflictCount = repository?.conflicts.length ?? 0;
  const sideTabs = (Object.keys(sideLabels) as Sidebar[]).filter((name) => name !== "conflicts" || conflictCount > 0).sort((x, y) => Number(y === "conflicts") - Number(x === "conflicts"));
  // The Conflicts tab exists only while there are conflicts: show it when they appear, leave it when they are gone.
  useEffect(() => {
    if (conflictCount) { setSidebar("conflicts"); setShowSidebar(true); } else setSidebar((current) => current === "conflicts" ? "branches" : current);
    // The keys follow the tab that is shown: the branch keys must not act on the branch selected before.
    setContext((current) => conflictCount ? gitPanelOf(current) === "sidebar" ? "conflicts" : current : current === "conflicts" ? "branches" : current);
  }, [conflictCount > 0]);
  // That switch replaces the rows of the sidebar after the operation gave the focus back to one of them: without this the
  // keyboard is left with nothing focused.
  const conflictsShown = sidebar === "conflicts";
  useEffect(() => {
    if (busy || loading || (overlayType !== "gitGraph" && overlayType !== "gitCommits")) return;
    const active = document.activeElement;
    if (active && active !== document.body && root.current?.contains(active)) return;
    ((conflictsShown ? root.current?.querySelector<HTMLElement>(".gg-side-row:not(:disabled)") : null) ?? root.current)?.focus();
  }, [conflictsShown]);
  const items = rows.flatMap((row) => !compareFile && !scoped && row.id === graph?.head && !searching ? [null, row] : [row]);
  if (!compareFile && !scoped && allRows.length === 0 && graph && !searching) items.push(null);
  const headRow = allRows.find((r) => r.id === graph?.head), workingLane = headRow?.width ?? 1;
  const graphWidth = maxOf(wanted ? foundRows : allRows, (r) => r.width * 18 + 24, Math.max(56, workingLane * 18 + 36));
  const grid = `${graphWidth}px minmax(180px, 1fr) 110px 92px 76px 28px`;
  // While a lower panel has focus the list is short: keep the commit whose details are shown in the middle of it.
  const lowerPanel = context === "details" || context === "files" || context === "tools";
  const lastFocusedRef = useRef<string | null>(null);
  const lastMissingRef = useRef<string | null>(null);
  const selectedRefForScroll = useRef<{ path: string; name: string; id: string } | null>(null);
  selectedRefForScroll.current = target?.kind === "reference" ? { path, name: target.ref.fullName, id: target.ref.id } : null;
  const scrollRef = target?.kind === "reference" && (target.ref.kind === "tag" || isRemoteTag(target.ref, repository?.refs ?? [])) ? target.ref : focusedRef;
  useEffect(() => {
    if (!scrollRef) { lastFocusedRef.current = null; lastMissingRef.current = null; return; }
    const scrollKey = JSON.stringify([path, scrollRef.fullName, scrollRef.id, reference, search]);
    const row = root.current?.querySelector<HTMLElement>(`.git-graph-lines .gg-row[data-commit="${scrollRef.id}"]`);
    if (!row) {
      if ((scrollRef.kind === "tag" || isRemoteTag(scrollRef, repository?.refs ?? [])) && !loading && lastMissingRef.current !== scrollKey) {
        lastMissingRef.current = scrollKey;
        const selected = { path, name: scrollRef.fullName, id: scrollRef.id };
        A.notify(tt("This tag's commit is not in the visible history."), "warn", [{
          label: tt("Show history of this tag"),
          run: () => {
            const current = selectedRefForScroll.current;
            if (!current || current.path !== selected.path || current.name !== selected.name || current.id !== selected.id) return;
            if (scoped) toggleFileScope(false);
            setReference(selected.name);
            setSearch("");
            beforeSearch.current = null;
            setSelected(0);
          },
        }]);
      }
      return;
    }
    if (scrollKey === lastFocusedRef.current) return;
    const list = root.current?.querySelector<HTMLElement>(".git-graph-lines");
    if (!list) return;
    lastMissingRef.current = null;
    lastFocusedRef.current = scrollKey;
    list.scrollTo({ top: list.scrollTop + row.getBoundingClientRect().top - list.getBoundingClientRect().top - row.getBoundingClientRect().height, behavior: "smooth" });
    row.classList.add("gg-ref-arrival");
    const timer = window.setTimeout(() => row.classList.remove("gg-ref-arrival"), 650);
    return () => window.clearTimeout(timer);
  }, [scrollRef?.fullName, scrollRef?.id, rows, loading, path, reference, search, scoped]);
  useEffect(() => {
    if (scrollRef && target?.kind === "reference") return;
    root.current?.querySelector(".git-graph-lines .selected")?.scrollIntoView({ block: lowerPanel ? "center" : "nearest" });
  }, [selected, lowerPanel, inspected, scrollRef?.fullName, target]);
  useEffect(() => {
    let active = true; setConflictError(""); setConflict((current) => current && target?.kind === "conflict" && current.file === target.file ? current : null);
    if (target?.kind === "conflict") void invoke<Conflict>("git_conflict", { path, file: target.file }).then((data) => { if (active) setConflict(data); }).catch((e) => { if (active) setConflictError(String(e)); });
    return () => { active = false; };
  }, [path, target, repository]);
  const inspect = (id: string, selection: GitTarget = { kind: "commit", id }) => {
    setTarget(selection); setInspected(id); setShowChanges(false); setShowAdvanced(false); setShowRebase(false);
    const index = items.findIndex((item) => item?.id === id); if (index >= 0) setSelected(index);
  };
  const choose = (id: string) => {
    if (compareFile) {
      if (busy || loading) return;
      const entry = scoped ? fileHistory?.entries.find((entry) => entry.id === id) : undefined;
      const source = entry && repository
        ? { kind: "gitBlob" as const, root: repository.root, file: entry.file, commit: entry.id, lastTouch: entry }
        : { kind: "gitCommit" as const, path: compareFile, commit: id };
      A.compare(source, { kind: "file", path: compareFile });
    } else { setContext("history"); setRangeBase(null); inspect(id); }
  };
  // Panel navigation (Vim keys): sidebar <-> commit list <-> details/changes.
  const focusHistory = () => (root.current?.querySelector<HTMLElement>(".git-graph-lines .gg-row.selected") ?? root.current?.querySelector<HTMLElement>(".git-graph-lines .gg-row"))?.focus();
  // An empty tab (no remotes, no stashes) has no rows, so focus falls back to the active tab button instead of leaving the dialog.
  const focusSidebar = () => {
    if (!showSidebar || compareFile) return;
    // Separate queries: one combined selector would return whichever element comes first in the document, i.e. the tab button.
    const row = root.current?.querySelector<HTMLElement>(".gg-side-row.selected") ?? root.current?.querySelector<HTMLElement>(".gg-side-row");
    (row ?? root.current?.querySelector<HTMLElement>(".gg-side-tabs [aria-pressed='true']"))?.focus();
  };
  /** Timers for deferred focus; all of them are cleared when the workspace closes. */
  const timers = useRef(new Set<number>());
  useEffect(() => { const pending = timers.current; return () => { pending.forEach((id) => window.clearTimeout(id)); pending.clear(); }; }, []);
  const later = (run: () => void, ms: number) => {
    const id = window.setTimeout(() => { timers.current.delete(id); run(); }, ms);
    timers.current.add(id);
  };
  /** Focuses a panel that is about to render; retries briefly instead of trusting a fixed delay. */
  const focusSoon = (selector: string, tries = 20) => {
    const element = root.current?.querySelector<HTMLElement>(selector);
    if (element) element.focus(); else if (tries > 0) later(() => focusSoon(selector, tries - 1), 30);
  };
  /** Opens the Tools panel (lazygit: `w` worktrees, `b` bisect; `x` is Gall's generic opener) and moves keyboard focus into it. */
  const openTools = (tab?: string) => {
    if (compareFile) return;
    setAdvancedTab(tab); setShowAdvanced(true); setShowRebase(false); setInspected(null); setShowChanges(false);
    // After the re-render: changing the tab remounts the panel, which would drop focus set on the old one.
    later(() => focusSoon(".git-advanced"), 60);
  };
  /** The graph row of the commit a key acts on. A mouse click selects without moving keyboard focus in WebKit, so the focused
   *  element can be a row far away (the sidebar); the selection, not the focus, says where the menu belongs. */
  const selectionRow = (selection: GitTarget | null): Element | null => {
    const focused = (document.activeElement as HTMLElement | null)?.closest(".gg-row.selected, .gg-side-row.selected") ?? null;
    const id = selection?.kind === "commit" ? targetCommit(selection) : null;
    return focused ?? (id ? root.current?.querySelector(`.gg-row[data-commit="${CSS.escape(id)}"]`) : null) ?? root.current?.querySelector(".gg-side-row.selected, .gg-row.selected") ?? null;
  };
  /** Where a menu opened with a key goes: right of the selected row's text, level with the row, so it does not cover the commit (the
   *  columns right of the subject are too narrow to hold it). With nothing selected: under the toolbar Actions button. */
  const keyMenuAnchor = (selection: GitTarget | null): Anchor => {
    const row = selectionRow(selection);
    row?.scrollIntoView({ block: "nearest" });
    return row ? textAnchor(row, row.querySelector(".gg-message") ?? row) : belowAnchor(root.current?.querySelector(".gg-actions"));
  };
  /** `y` on a commit: a menu of attributes to copy (hash, subject, message, author, date, URL, diff, patch, tags). */
  const copyMenu = async (selection: GitTarget | null) => {
    const id = targetCommit(selection); if (!id) return;
    const anchor = keyMenuAnchor(selection);
    if (selection?.kind === "commit" && picked.size > 1 && picked.has(id)) {
      const ids = actionCommits(selection), count = ids.length, rowsOf = ids.map((one) => [...rows, ...allRows].find((row) => row.id === one)!);
      A.showMenuAt(anchor, [
        { label: tt("Copy {count} hashes", { count }), accel: vim ? "y" : undefined, run: () => void copyText(() => ids.join("\n"), tt("Copied {count} hashes.", { count })) },
        { label: tt("Copy {count} subjects", { count }), accel: vim ? "s" : undefined, run: () => void copyText(() => rowsOf.map((row) => row.subject).join("\n"), tt("Copied {count} subjects.", { count })) },
        { label: tt("Copy {count} commits as a patch", { count }), accel: vim ? "p" : undefined, run: () => void copyPatches(ids) },
      ]);
      return;
    }
    try {
      const details = await invoke<{ id: string; author: string; email: string; date: string; message: string }>("git_commit_details", { path, commit: id, base: null });
      const remote = repository?.remotes.find((r) => r.name === "origin") ?? repository?.remotes[0];
      const tags = repository?.refs.filter((ref) => ref.kind === "tag" && ref.id === details.id).map((ref) => ref.name) ?? [];
      const copy = async (choice: CopyChoice) => {
        try { await invoke("clip_set_text", { text: typeof choice.text === "string" ? choice.text : await choice.text() }); A.notify(tt("Copied {value}.", { value: choice.label.toLowerCase() })); }
        catch (error) { setError(String(error)); }
      };
      const choices = copyChoices({ ...details, tags, url: remote ? commitUrl(remote.fetchUrl, details.id) : null }, () => invoke<string>("git_commit_diff", { path, commit: details.id }), () => invoke<string>("git_commit_patch_text", { path, commit: details.id }));
      A.showMenuAt(anchor, choices.map((choice) => ({ label: choice.label, accel: vim ? choice.key : undefined, run: () => void copy(choice) })));
    } catch (error) { setError(String(error)); }
  };
  /** `a`: applies a patch text from the clipboard (for example one copied with `y` `p`) to the index and working folder. No commit is created. */
  const applyClipboardPatch = async () => {
    if (busy || loading || compareFile) return;
    setSwitchBusy(true); setError("");
    try {
      const clip = await invoke<{ kind: string; text?: string }>("clip_inspect");
      const text = clip.kind === "text" ? clip.text ?? "" : "";
      if (!text.trim()) { setError(tt("The clipboard has no patch text. Copy one first, for example with y then p on a commit.")); return; }
      const files = text.split("\n").filter((line) => line.startsWith("diff --git ")).length;
      const preview = text.split("\n").slice(0, 8).join("\n");
      const state = await invoke<{ token: string }>("git_advanced", { path, commit: null });
      if (!await A.confirm(tt("Apply patch from clipboard?"), tt("Check this patch ({files} {value}) and apply it to the index and working folder. No commit is created, and existing work must be committed or stashed first.\n\n{preview}{value2}", { files, value: files === 1 ? "file" : "files", preview, value2: text.split("\n").length > 8 ? "\n…" : "" }), "Apply")) return;
      await invoke<string>("git_advanced_action", { path, request: { action: "applyPatchText", text }, expected: state.token, id: `patch-${Date.now()}-${Math.random().toString(36).slice(2)}` });
      await reload(); A.notify(tt("Patch applied to the index and working folder."));
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchBusy(false); }
  };
  /** `W`: a worktree for the selected branch, tag or commit in the preset folder, without a form. `N` asks for the name of a new branch. */
  const quickWorktree = async (selection: GitTarget | null, newBranch = false) => {
    if (!repository || compareFile || busy || loading || stashRow(selection) || (selection?.kind !== "commit" && selection?.kind !== "reference")) return;
    setSwitchBusy(true); setError("");
    try {
      const id = selection.kind === "commit" ? selection.id : selection.ref.id;
      let source: WorktreeSource | { error: string; name: string } = worktreeSource(selection, repository);
      if (newBranch) {
        const name = await A.promptText(tt("New branch in a new worktree at {target}", { target: selection.kind === "reference" ? selection.ref.name : id.slice(0, 8) }), "", "Create worktree");
        if (!name?.trim()) return;
        source = { kind: "newBranch", name: name.trim(), id };
      }
      if ("error" in source) { actions.setError(tt(source.error, { name: source.name })); return; }
      await createWorktree(path, source, () => { if (root.current) void load(); });
      await reload();
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchBusy(false); }
  };
  const compared = useRef(false);
  useEffect(() => {
    if (compared.current || !graph || (!compare && !show)) return;
    compared.current = true;
    if (compare) { setRangeBase(compare[0]); inspect(compare[1]); }
    else if (show) choose(show);
    // Once, when the workspace was opened for it.
  }, [graph]); // eslint-disable-line react-hooks/exhaustive-deps
  /** `X`: the commit in a temporary worktree, opened in a new tab. */
  const scratchWorktree = async (selection: GitTarget | null) => {
    if (!repository || compareFile || busy || loading || stashRow(selection) || (selection?.kind !== "commit" && selection?.kind !== "reference")) return;
    setSwitchBusy(true); setError("");
    const id = selection.kind === "commit" ? selection.id : selection.ref.id;
    try { await tryInWorktree(path, id, id.slice(0, 8)); }
    catch (e) { actions.setError(String(e)); }
    finally { setSwitchBusy(false); }
  };
  const focusFirstStash = () => (root.current?.querySelector<HTMLElement>(".gg-sidebar .gg-side-list .gg-side-row.selected") ?? root.current?.querySelector<HTMLElement>(".gg-sidebar .gg-side-list .gg-side-row") ?? root.current)?.focus();
  const focusDetails = () => {
    if (compareFile) return;
    if (target?.kind === "commit") { if (inspected !== target.id) choose(target.id); focusSoon(".git-commit-details"); }
    else if (target?.kind === "working" && !root.current?.querySelector<HTMLButtonElement>(".gg-working")?.disabled) { if (!showChanges) working(); focusSoon(".git-changes"); }
  };
  const switchSideTab = (step: number) => {
    const at = sideTabs.indexOf(sidebar), next = sideTabs[(Math.max(0, at) + step + sideTabs.length) % sideTabs.length];
    setSidebar(next); setContext(next); later(focusSidebar, 40);
  };
  const working = () => { setTarget({ kind: "working" }); setInspected(null); setShowAdvanced(false); setShowRebase(false); setShowChanges((value) => !value); setContext("files"); };
  const canRun = (action: string, selection = target) => gitActionEnabled(action, {
    commit: !!targetCommit(selection), head: !!repository?.head,
    referenceKind: selection?.kind === "reference" ? selection.ref.kind : undefined,
    stash: selection?.kind === "stash", remote: selection?.kind === "remote", conflict: selection?.kind === "conflict" && !!repository?.conflicts.includes(selection.file),
    branch: !!repository?.branch, remotes: repository?.remotes.length ?? 0, operation: repository?.operation ?? null, conflicts: repository?.conflicts.length ?? 0,
  });
  /** Carries out a command of a panel that handles its keys itself, with the key bound to it now, where the focus was. */
  const pressPanel = (defaultKey: string) => {
    const command = gitCommands(context, shortcuts).find((item) => item.defaults.includes(defaultKey) && !item.shared);
    pressOnPanel(command?.key || defaultKey);
  };
  const pressOnPanel = (key: string) => {
    const remembered = lastFocus.current, element = remembered?.isConnected && root.current?.contains(remembered) && !remembered.closest("input,textarea,select") ? remembered : root.current;
    if (!element) return;
    element.focus(); pressKey(key, element);
  };
  /** The element that had the focus before the palette opened. */
  const beforePalette = useRef<HTMLElement | null>(null);
  const mainRef = useRef<HTMLElement | null>(null);
  const palettePanels = useRef<Map<string, HTMLElement | null> | null>(null);
  const palette = useRef<PaletteHandle | null>(null);
  const openPalette = (action?: string) => {
    if (busy || loading || compareFile || !repository || palette.current?.isOpen()) return;
    const active = document.activeElement as HTMLElement | null;
    beforePalette.current = active && root.current?.contains(active) ? active : root.current;
    palettePanels.current = new Map();
    setHelp(false); palette.current?.open(action);
  };
  const closePalette = () => {
    palettePanels.current = null;
    palette.current?.close();
    const element = beforePalette.current?.isConnected ? beforePalette.current : root.current;
    element?.focus();
  };
  /** The panel that carries out a command with keys of its own; null while it is not open. */
  const panelOf = (item: PaletteCommand): HTMLElement | null => {
    const cache = palettePanels.current;
    if (cache?.has(item.scope)) return cache.get(item.scope) ?? null;
    let resolved: HTMLElement | null = null;
    if (item.scope === "worktrees") {
      resolved = context === "worktrees" ? beforePalette.current?.closest<HTMLElement>(".gg-worktree-main") ?? root.current?.querySelector<HTMLElement>(".gg-sidebar .gg-worktree-main") ?? null : null;
    } else {
      const open =
        (item.scope === "files" || item.scope === "diff") ? showChanges && !compareFile
        : item.scope === "details" ? !!inspected && !compareFile
        : item.scope === "tools" ? showAdvanced && !compareFile
        : item.scope === "rebase" ? showRebase && !compareFile
        : item.scope === "merge" ? target?.kind === "conflict" && !compareFile
        : false;
      const selector: Partial<Record<PaletteCommand["scope"], string>> = { files: ".git-changes", diff: ".gc-patch", details: ".git-commit-details", tools: ".git-advanced", rebase: ".git-rebase", merge: ".gm" };
      const found = open ? selector[item.scope] : undefined;
      if (found && mainRef.current) {
        for (const child of mainRef.current.children) {
          if (child.classList.contains("git-graph-lines")) continue;
          const hit = child.matches(found) ? (child as HTMLElement) : child.querySelector<HTMLElement>(found);
          if (hit) { resolved = hit; break; }
        }
      }
    }
    cache?.set(item.scope, resolved);
    return resolved;
  };
  const paletteCanRun = (item: PaletteCommand) => {
    if (/^(file|detail|diff|panel):/.test(item.command.action)) return !!panelOf(item) && !!item.command.key;
    if (stashEntryActions.includes(item.command.action) && stashRow(target)) return true;
    return canRun(item.command.action) && !(rewritesHistory(item.command.action) && stashRow(target));
  };
  const runFromPalette = (item: PaletteCommand, named?: Candidate) => {
    closePalette();
    const action = item.command.action;
    if (/^(file|detail|diff|panel):/.test(action)) { const panel = panelOf(item); if (panel) { panel.focus(); pressKey(item.command.key, panel); } return; }
    if (named && keepsSelection(action)) { if (named.ref) void cherryPick(actionCommits(target), named.ref.name, true); return; }
    const selection: GitTarget | null = !named ? target : named.ref ? { kind: "reference", ref: named.ref } : named.stash ? { kind: "stash", stash: named.stash } : named.remote ? { kind: "remote", remote: named.remote } : named.file ? { kind: "conflict", file: named.file } : { kind: "commit", id: named.id! };
    // What was named is selected too, so panels that show the selection (rebase, details) work on it.
    if (named) { setTarget(selection); const at = items.findIndex((row) => row?.id === targetCommit(selection)); if (at >= 0) setSelected(at); }
    run(action, selection, !!named);
  };
  const paletteSource = useMemo(() => ({ refs: repository?.refs ?? [], stashes: repository?.stashes ?? [], remotes: repository?.remotes ?? [], conflicts: repository?.conflicts ?? [], commits: allRows }), [repository, allRows]);
  /** The commits an action works on, oldest first: the chosen ones when the selection is one of them, else the selected commit. */
  const actionCommits = (selection: GitTarget | null): string[] => {
    const id = targetCommit(selection);
    if (selection?.kind === "reference" || !picked.size) return id ? [id] : [];
    const listed = [...picked].every((one) => rows.some((row) => row.id === one)) ? rows : allRows;
    return !id || picked.has(id) ? oldestFirst(listed, picked) : [id];
  };
  const clearPicked = () => { setPicked(new Set()); setRangeFrom(null); clickAnchor.current = null; };
  /** Makes `branch` the current branch (refused with local changes); returns the fresh repository token, or null when it did not switch. */
  const moveTo = async (branch: string): Promise<string | null> => {
    if (!repository) return null;
    if (branch === repository.branch) return repository.token;
    if (graph?.changes) { actions.setError(tt("Commit or stash local changes before switching to {branch}.", { branch })); return null; }
    await invoke("git_switch", { path, target: branch, branch: true, action: "clean", message: "", expected: repository.token });
    return (await invoke<Repository>("git_repository", { path })).token;
  };
  /** The registry command (and its key) that does the same as a drop, named in the drop's confirmation. */
  const keyboardFor = (action: string) => {
    const command = [...gitCommands("history", shortcuts), ...gitCommands("branches", shortcuts)].find((item) => item.action === action);
    return tt("Keyboard equivalent: {command} ({key})", { command: command ? tt(command.label) : action, key: (command && capOf(command)) ?? tt("Find a Git command…") });
  };
  const draggedFrom = useRef<{ commits?: string[]; ref?: string } | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const endDrag = () => { draggedFrom.current = null; setDragOver(null); };
  /** Commits dropped on a local branch: the same cherry-pick as the keyboard, after naming source and target. */
  const dropCommits = async (ids: string[], onto: GitReference) => {
    if (!repository || busy || loading || compareFile) return;
    const what = ids.length === 1 ? ids[0].slice(0, 12) : tt("{count} commits", { count: ids.length });
    const same = onto.name === repository.branch;
    const body = tt("Apply {what} on top of {branch}. Gall switches to {branch} first when it is not checked out.\n\nCommit or stash local changes first. A conflict stops with the commits before it applied; continue goes on and abort takes everything back. Commits are unsigned and hooks are disabled.\n\n{keyboard}", { what, branch: onto.name, keyboard: keyboardFor(same ? "cherryPick" : "cherryPickTo") });
    if (await A.confirm(tt("Cherry-pick {what} onto {branch}?", { what, branch: onto.name }), body, "Cherry-pick")) await cherryPick(ids, same ? null : onto.name, true);
  };
  /** Merge or rebase chosen after dropping one branch on another: merge goes into the target, rebase moves the dropped branch. */
  const integrateBranches = async (kind: "merge" | "rebase", source: GitReference, onto: GitReference) => {
    if (!repository || busy || loading || compareFile) return;
    const merge = kind === "merge", stay = merge ? onto : source;
    const lead = merge ? tt("Merge {source} into {target}.", { source: source.name, target: onto.name }) : tt("Rebase {source} onto {target}.", { source: source.name, target: onto.name });
    const body = tt("{lead} Gall switches to {branch} first when it is not checked out.\n\nCommit or stash local changes first. A conflict stops the operation; it can be continued or aborted. Commits are unsigned and hooks are disabled.\n\n{keyboard}", { lead, branch: stay.name, keyboard: keyboardFor(kind) });
    const title = merge ? tt("Merge {source} into {target}?", { source: source.name, target: onto.name }) : tt("Rebase {source} onto {target}?", { source: source.name, target: onto.name });
    const forecast = await forecastText(path, stay.id, merge ? source.id : onto.id);
    if (!await A.confirm(title, forecast ? `${forecast}\n\n${body}` : body, merge ? "Merge" : "Rebase", !merge)) return;
    setSwitchBusy(true); setError(""); actions.setError("");
    try {
      const token = await moveTo(stay.name); if (!token) return;
      await invoke<string>("git_repository_action", { path, request: { action: kind, target: merge ? source.id : onto.id }, expected: token });
      clearPicked();
      await actions.refreshHistory();
      A.notify(merge ? tt("Merged {source} into {target}", { source: source.name, target: onto.name }) : tt("Rebased {source} onto {target}", { source: source.name, target: onto.name }), "success");
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchBusy(false); }
  };
  const localByName = useMemo(() => new Map((repository?.refs ?? []).filter((r) => r.kind === "branch").map((r) => [r.name, r])), [repository]);
  const remoteTagsByCommit = useMemo(() => {
    const byCommit = new Map<string, string[]>();
    for (const tag of remoteTags ?? []) {
      const id = tag.peeled ?? tag.id;
      byCommit.set(id, [...(byCommit.get(id) ?? []), tag.name]);
    }
    return byCommit;
  }, [remoteTags]);
  const localTagsByCommit = useMemo(() => {
    const byCommit = new Map<string, Set<string>>();
    for (const ref of repository?.refs ?? []) {
      if (ref.kind !== "tag") continue;
      const names = byCommit.get(ref.id) ?? new Set<string>();
      names.add(ref.name);
      byCommit.set(ref.id, names);
    }
    return byCommit;
  }, [repository]);
  const graphTagNames = (id: string, refs: string[]) => [...new Set([
    ...refs.filter((ref) => ref.startsWith("tag: ")).map((ref) => ref.slice(5)),
    ...badgeRefs(refs, repository?.refs ?? []).badges.filter((badge) => badge.kind === "remote" && /^[^/]+\/tags\//.test(badge.label)).map((badge) => badge.label.replace(/^[^/]+\/tags\//, "")),
    ...(remoteTagsByCommit.get(id) ?? []),
  ])];
  const tagPresence = (id: string, names: string[]) => {
    if (!remoteTags) return "unknown";
    const states = names.map((name) => {
      const local = localTagsByCommit.get(id)?.has(name) ?? false;
      const remote = remoteTagsByCommit.get(id)?.includes(name) ?? false;
      return local && remote ? "both" : local ? "local" : remote ? "remote" : "unknown";
    });
    return states.every((state) => state === states[0]) ? states[0] ?? "unknown" : "unknown";
  };
  const tagDescription = (name: string, id: string) => {
    const presence = tagPresence(id, [name]);
    const locations = [
      ...(presence === "local" || presence === "both" ? [tt("Local")] : []),
      ...(presence === "remote" || presence === "both" ? [`${tt("Remote")} (origin)`] : []),
    ];
    return locations.length ? `${name} · ${locations.join(" / ")}` : `${name} · ${tt("Status unavailable")}`;
  };

  // One set of delegated handlers on the workspace covers the sidebar rows, the graph rows and their branch labels.
  const dropTargetOf = (e: React.DragEvent): GitReference | null => {
    const from = draggedFrom.current, name = (e.target as HTMLElement).closest?.<HTMLElement>("[data-drop-ref]")?.dataset.dropRef;
    if (!from || !name || from.ref === name || busy || loading || compareFile || e.dataTransfer.types.includes("Files")) return null;
    if (from.ref && !repository?.refs.some((r) => r.kind === "branch" && r.fullName === from.ref)) return null;
    return repository?.refs.find((r) => r.kind === "branch" && r.fullName === name) ?? null;
  };
  /** The commit row that a dragged staged file or hunk would go into. */
  const fixupTarget = (e: React.DragEvent): string | null => {
    const change = draggedChange();
    if (!change?.staged || busy || loading || compareFile) return null;
    return (e.target as HTMLElement).closest?.<HTMLElement>(".gg-row[data-commit]")?.dataset.commit ?? null;
  };
  const dragHandlers = {
    onDragStart: (e: React.DragEvent) => {
      const el = e.target as HTMLElement, ref = el.closest?.<HTMLElement>("[data-drag-ref]")?.dataset.dragRef, commit = el.closest?.<HTMLElement>("[data-commit]")?.dataset.commit;
      draggedFrom.current = null;
      if (ref) { e.dataTransfer.setData(REF_DRAG, ref); e.dataTransfer.setData("text/plain", ref); e.dataTransfer.effectAllowed = "copyMove"; draggedFrom.current = { ref }; }
      else if (commit && !busy && !loading && !compareFile) { const ids = actionCommits({ kind: "commit", id: commit }); e.dataTransfer.setData(COMMIT_DRAG, ids.join(",")); e.dataTransfer.setData("text/plain", ids.join("\n")); e.dataTransfer.effectAllowed = "copyMove"; draggedFrom.current = { commits: ids }; }
    },
    onDragOver: (e: React.DragEvent) => {
      const commit = fixupTarget(e);
      if (commit) { e.preventDefault(); e.dataTransfer.dropEffect = "move"; setDragOver(commit); return; }
      const found = dropTargetOf(e);
      if (found) { e.preventDefault(); e.dataTransfer.dropEffect = draggedFrom.current?.commits ? "copy" : "move"; setDragOver(found.fullName); } else setDragOver(null);
    },
    onDragLeave: (e: React.DragEvent) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragOver(null); },
    onDrop: (e: React.DragEvent) => {
      const commit = fixupTarget(e), change = draggedChange();
      if (commit && change) {
        e.preventDefault();
        endChangeDrag(); endDrag();
        window.dispatchEvent(new CustomEvent("gall-git-fixup", { detail: { file: change.file, range: change.range ?? null, target: commit } }));
        return;
      }
      const found = dropTargetOf(e), from = draggedFrom.current;
      endDrag();
      if (!found || !from) return;
      e.preventDefault();
      if (from.commits) { void dropCommits(from.commits, found); return; }
      const source = repository?.refs.find((r) => r.kind === "branch" && r.fullName === from.ref);
      if (!source) return;
      A.showMenuAt(eventAnchor(e), [
        { label: tt("Merge {source} into {target}…", { source: source.name, target: found.name }), run: () => void integrateBranches("merge", source, found) },
        { label: tt("Rebase {source} onto {target}…", { source: source.name, target: found.name }), run: () => void integrateBranches("rebase", source, found) },
      ]);
    },
    onDragEnd: endDrag,
  };
  /**
   * Squashes the chosen commits into one: `v`, `j`, `j`, `s`, the message, Ctrl+Enter. It is the interactive rebase (plan, validation,
   * recovery reference and journal entry included) with the oldest chosen commit as a pick and the others as squashes.
   */
  const squashPicked = async (ids: string[]) => {
    if (!repository || busy || loading || compareFile || ids.length < 2) return;
    setSwitchBusy(true); setError(""); actions.setError("");
    try {
      const details = await invoke<{ parents: string[] }>("git_commit_details", { path, commit: ids[0], base: null });
      const plan = await invoke<{ base: string; token: string; commits: PlanCommit[] }>("git_rebase_plan", { path, base: details.parents[0] ?? ROOT_BASE });
      const chosen = new Set(ids);
      if (!squashRange(plan.commits, chosen, "")) { actions.setError(tt("Choose commits that follow one another (Shift+click or v and j) to squash them.")); return; }
      const messages = await commitMessages(path, ids);
      const answer = await A.dialog({ title: tt("Squash {count} commits", { count: ids.length }), message: tt("The message of the one commit that replaces them. {branch}: descendant commit IDs change, and the old HEAD is saved for recovery.", { branch: repository.branch ?? "HEAD" }), input: { value: combinedMessage(plan.commits.map((commit) => ({ ...commit, message: messages.get(commit.id) ?? "" })), chosen), multiline: true }, buttons: [{ id: "cancel", label: tt("Cancel") }, { id: "squash", label: tt("Squash"), primary: true }] });
      const message = answer?.button === "squash" ? answer.value.trim() : "";
      if (!message) return;
      const built = squashRange(plan.commits, chosen, message)!;
      const progressId = createProgressId("squash");
      setSwitchProgressTask({ id: progressId, seed: { operation: "rebase", phase: "Rebasing" } });
      const output = await invoke<string>("git_rebase_execute", { path, base: plan.base, expected: plan.token, entries: built.entries, progressId });
      clearPicked();
      await actions.refreshHistory();
      A.notify(tt("Squashed {count} commits", { count: ids.length }), "success");
      if (output) actions.setOutput(output);
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  // A command of the user running now (its id, for Stop) and whether the output log is open (it opens when a run fails).
  // Hint mode: labels on every control that is on screen, typed to click (`;`, or Ctrl+; from anywhere, a dialog included).
  const [hints, setHints] = useState(false);
  const stopHints = useCallback(() => setHints(false), []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === ";" && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && !e.defaultPrevented) { e.preventDefault(); setHints(true); } };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const [customRun, setCustomRun] = useState<string | null>(null), [outputOpen, setOutputOpen] = useState(false);
  /**
   * Runs a command the user defined: the variables of the selection go into single arguments (never a shell), the log pane gets
   * the output, and a failure, a stop or a timeout says so. `file` is the file of Working Changes for a command of that context.
   */
  const runCustom = async (id: string, selection: GitTarget | null, file?: string) => {
    const def = (getState().settings?.gitCommands ?? []).find((item: GitCommandDef) => item.id === id);
    if (!def || !repository || busy || loading || compareFile) return;
    const found = customTarget(selection, repository, file);
    if (!found || found.context !== def.context) { A.notify(tt("“{label}” works on a {context}. Select one first.", { label: def.label, context: tt(def.context) })); return; }
    const expanded = expandCommand(def.command, found.variables);
    if ("error" in expanded) { actions.setError(`${def.label}: ${expanded.error}`); return; }
    const line = joinCommandLine(expanded.argv);
    if (def.confirm && !await A.confirm(tt("Run {label}?", { label: def.label }), tt("This runs in the repository folder, without a shell:\n\n{command}", { command: line }), tt("Run"))) return;
    const run = `custom-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    setSwitchBusy(true); setError(""); actions.setError(""); setCustomRun(run);
    setSwitchProgressTask({ id: run, seed: { operation: "custom", phase: "Running command", cancellable: true, detail: def.label } });
    try {
      const outcome = await invoke<{ code: number | null; output: string; timedOut: boolean; cancelled: boolean }>("git_custom_run", { path, id: run, argv: expanded.argv });
      actions.setOutput(`$ ${line}\n${outcome.output}`);
      if (outcome.cancelled) A.notify(tt("Stopped {label}", { label: def.label }), "warn");
      else if (outcome.timedOut) { setOutputOpen(true); actions.setError(tt("{label} took too long and was stopped.", { label: def.label })); }
      else if (outcome.code !== 0) { setOutputOpen(true); actions.setError(tt("{label} failed (exit code {code}). Its output is below.", { label: def.label, code: outcome.code ?? "?" })); }
      else A.notify(tt("{label} finished", { label: def.label }), "success");
      await actions.refreshHistory();
    } catch (e) { actions.setError(String(e)); }
    finally { setCustomRun(null); setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  /** Forgets the changes a branch remembers, or turns them into a stash. */
  const wipAction = async (action: "discardWip" | "wipStash", branch: string) => {
    if (busy) return;
    if (action === "discardWip" && !await A.confirm(tt("Discard the changes {branch} remembers?", { branch }), tt("They are no longer kept for the branch. A hidden recovery copy (a reference under refs/gall/recovery, not a stash) remains."), tt("Discard"), true)) return;
    setSwitchBusy(true); actions.setError("");
    try {
      await invoke(action === "discardWip" ? "git_wip_discard" : "git_wip_stash", { path, branch });
      await loadWip();
      A.notify(action === "discardWip" ? tt("Forgot the changes {branch} remembered", { branch }) : tt("Saved the changes {branch} remembered as a stash", { branch }), "success");
      await reload();
    } catch (e) { actions.setError(String(e)); }
    finally { setSwitchBusy(false); }
  };
  /** Cherry-picks as one operation, onto the current branch or, after switching to it, another one. Choosing a target in the palette is the confirmation. */
  const cherryPick = async (ids: string[], branch: string | null, confirmed: boolean) => {
    if (!repository || busy || loading || compareFile || !ids.length) return;
    const what = ids.length === 1 ? ids[0].slice(0, 12) : tt("{count} commits", { count: ids.length });
    if (!confirmed && !await A.confirm(tt("Cherry-pick onto {branch}?", { branch: repository.branch ?? "detached HEAD" }), tt("Apply {what} on top of {branch}.\n\nCommit or stash local changes first. A conflict stops with the commits before it applied; continue goes on and abort takes everything back. Commits are unsigned and hooks are disabled.", { what, branch: repository.branch ?? "detached HEAD" }), "Cherry-pick")) return;
    setSwitchBusy(true); setError(""); actions.setError("");
    try {
      const mainline = ids.length === 1 ? await chooseMainline(path, ids[0]) : 0;
      if (mainline == null) return;
      let token = repository.token;
      if (branch) { const next = await moveTo(branch); if (!next) return; token = next; }
      const request = mainline ? { action: "integrateMerge", target: ids[0], mainline, revert: false }
        : ids.length === 1 ? { action: "cherryPick", target: ids[0] } : { action: "cherryPickMany", targets: ids };
      const progressId = createProgressId("pick");
      setSwitchProgressTask({ id: progressId, seed: { operation: "cherryPick", phase: "Cherry-picking" } });
      await invoke<string>("git_repository_action", { path, request, expected: token, progressId });
      clearPicked();
      await actions.refreshHistory();
      A.notify(branch ? tt("Cherry-picked {what} to {branch}", { what, branch }) : tt("Cherry-picked {what}", { what }), "success");
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  /** Goes to a state of the operation journal after showing what moves. */
  const jumpTo = async (journal: JournalView, applied: number) => {
    if (busy || loading || compareFile) return;
    const { undo, redo, moves } = jumpPreview(journal, applied), count = undo + redo;
    if (!count) return;
    const entry = journal.entries[undo ? journal.applied - 1 : journal.applied], name = `${tt(journalKinds[entry.kind])} · ${entry.label}`;
    const title = undo ? (count === 1 ? tt("Undo {name}?", { name }) : tt("Undo {count} operations?", { count })) : count === 1 ? tt("Redo {name}?", { name }) : tt("Redo {count} operations?", { count });
    const blocked = undo ? journal.undoBlocked : journal.redoBlocked;
    if (blocked) { actions.setError(blocked); return; }
    if (!await A.confirm(title, tt("{moves}\n\nLocal changes are kept: where they would be overwritten, nothing happens. Undone commits stay available for a redo.", { moves: moves.join("\n") }), undo ? "Undo" : "Redo")) return;
    setSwitchBusy(true); setError(""); actions.setError("");
    try {
      const progressId = createProgressId("journal");
      setSwitchProgressTask({ id: progressId, seed: { operation: "journal", phase: "Restoring Git state" } });
      const note = await invoke<string>("git_journal_move", { path, applied, expected: journal.applied, progressId });
      clearPicked();
      await actions.refreshHistory();
      A.notify(undo ? (count === 1 ? tt("Undone: {name}", { name }) : tt("Undone: {count} operations", { count })) : count === 1 ? tt("Redone: {name}", { name }) : tt("Redone: {count} operations", { count }), "success");
      if (note) A.notify(note, "warn");
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  const undoRedo = async (action: "undo" | "redo") => {
    if (busy || loading || compareFile) return;
    try {
      const journal = await invoke<JournalView>("git_journal", { path });
      const applied = journal.applied + (action === "undo" ? -1 : 1);
      if (applied < 0) A.notify(tt("No operation to undo. Only operations done in Gall are recorded."));
      else if (applied > journal.entries.length) A.notify(tt("No operation to redo."));
      else await jumpTo(journal, applied);
    } catch (e) { actions.setError(String(e)); }
  };
  const closeJournal = () => { setShowJournal(false); setShowReflog(false); later(() => { if (!root.current?.contains(document.activeElement) || document.activeElement === document.body) (root.current?.querySelector<HTMLElement>(".git-graph-lines .gg-row.selected") ?? root.current)?.focus(); }, 30); };
  const savePatches = async (ids: string[]) => {
    if (busy || loading || compareFile || !ids.length) return;
    setSwitchBusy(true); setError("");
    try {
      const destination = await save({ defaultPath: ids.length === 1 ? `${ids[0].slice(0, 8)}.patch` : `${ids.length}-commits.patch`, filters: [{ name: "Git patch", extensions: ["patch"] }] });
      if (!destination) return;
      const { token } = await invoke<{ token: string }>("git_advanced", { path, commit: null });
      const patchId = `patch-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      setSwitchProgressTask({ id: patchId, seed: { operation: "patch", phase: "Exporting patches", cancellable: true } });
      await invoke<string>("git_advanced_action", { path, request: { action: "exportPatches", commits: ids, destination }, expected: token, id: patchId });
      A.notify(tt("Saved {count} commits as {path}", { count: ids.length, path: destination }), "success");
    } catch (e) { actions.setError(String(e)); }
    finally { setSwitchProgressTask(null); setSwitchBusy(false); }
  };
  const copyText = async (text: () => Promise<string> | string, done: string) => {
    try { await invoke("clip_set_text", { text: await text() }); A.notify(done); } catch (e) { setError(String(e)); }
  };
  const copyPatches = (ids: string[]) => copyText(async () => { let all = ""; for (const commit of ids) all += await invoke<string>("git_commit_patch_text", { path, commit }); return all; }, tt("Copied the patches of {count} commits.", { count: ids.length }));
  /** The stash that a selected graph row stands for (its commit is a stash entry), if any. */
  const stashRow = (selection: GitTarget | null) => selection?.kind === "commit" ? stashAtCommit(repository?.stashes ?? [], selection.id) : undefined;
  /** `chosen`: the user named the selection for this very action (in the palette), which needs no second confirmation to cherry-pick. */
  const run = (action: string, selection = target, chosen = false) => {
    if (stashEntryActions.includes(action)) { const stash = stashRow(selection); if (stash) selection = { kind: "stash", stash }; }
    // A cherry-pick without a commit asks for one.
    if (busy || loading || (action !== "cherryPick" && !canRun(action, selection))) return;
    // A stash row is not a normal commit: the menu hides these actions, so the keys must not run them either.
    if (rewritesHistory(action) && stashRow(selection)) return;
    setHelp(false);
    if (action.startsWith(CUSTOM_PREFIX)) {
      const id = action.slice(CUSTOM_PREFIX.length), def = (getState().settings?.gitCommands ?? []).find((item: GitCommandDef) => item.id === id);
      // A command of Working Changes needs its file: that panel knows it and runs it.
      if (def?.context === "file") { const changes = root.current?.querySelector(".git-changes"); if (changes) changes.dispatchEvent(new CustomEvent("gall-git-command", { detail: action })); else A.notify(tt("Open Working Changes and select a file first.")); return; }
      void runCustom(id, selection);
      return;
    }
    if (action.startsWith("file:") || action.startsWith("detail:")) { root.current?.querySelector(action.startsWith("file:") ? ".git-changes" : ".git-commit-details")?.dispatchEvent(new CustomEvent("gall-git-command", { detail: action.slice(action.indexOf(":") + 1) })); return; }
    if (action.startsWith("diff:")) {
      const diff = root.current?.querySelector<HTMLElement>(".gc-patch"), command = gitCommands("diff", shortcuts).find((item) => item.action === action);
      if (diff && command?.key) { diff.focus(); pressKey(command.key, diff); }
      return;
    }
    if (action.startsWith("panel:")) { pressPanel(action.slice(6)); return; }
    if (action === "palette") { openPalette(); return; }
    if (action === "discardWip" || action === "wipStash") {
      const name = selection?.kind === "reference" && selection.ref.kind === "branch" ? selection.ref.name : null;
      if (!name || !wipBranches.has(name)) A.notify(tt("This branch remembers no changes. Turn on Settings › Git › Remember changes per branch."));
      else void wipAction(action, name);
      return;
    }
    if (action === "hints") { setHelp(false); setHints(true); return; }
    if (action === "showStashes" || action === "stashSearch") {
      if (compareFile) return;
      setSidebar("stashes"); setContext("stashes");
      later(() => { if (action === "stashSearch") root.current?.querySelector<HTMLInputElement>(".gs-search")?.focus(); else focusFirstStash(); }, 30);
      return;
    }
    if (action === "undo" || action === "redo") { void undoRedo(action); return; }
    if (action === "journal") { if (!compareFile) { setHelp(false); setShowReflog(false); setShowJournal((value) => !value); } return; }
    if (action === "reflog") { if (!compareFile) { setHelp(false); setShowJournal(false); setShowReflog((value) => !value); } return; }
    if (action === "pick") {
      const id = selection?.kind === "commit" ? selection.id : null;
      if (id && !compareFile) { setRangeFrom(null); clickAnchor.current = id; setPicked((current) => toggleId(current, id)); }
      return;
    }
    if (action === "pickRange") {
      const id = selection?.kind === "commit" ? selection.id : null;
      if (rangeFrom) setRangeFrom(null);
      else if (id && !compareFile) { setRangeFrom(id); clickAnchor.current = id; setPicked(new Set([id])); }
      return;
    }
    if (action === "cherryPick" && !compareFile) {
      const ids = actionCommits(selection);
      if (ids.length) void cherryPick(ids, null, chosen); else openPalette("cherryPick");
      return;
    }
    if (action === "cherryPickTo" && !compareFile) { if (actionCommits(selection).length) openPalette("cherryPickTo"); return; }
    if (action === "quick:squash" && !compareFile && picked.size > 1) { void squashPicked(actionCommits(selection)); return; }
    if (action === "savePatch") { void savePatches(actionCommits(selection)); return; }
    if (action === "showHistory") {
      if (selection?.kind === "reference") { setReference(selection.ref.fullName); setSearch(""); beforeSearch.current = null; setSelected(0); A.notify(tt("Showing history of {name}. Pick “All references” to show everything.", { name: selection.ref.name })); }
      return;
    }
    if (action === "search") { searchRef.current?.focus(); searchRef.current?.select(); return; }
    if (action === "search:next" || action === "search:previous") {
      const at = nextHit(rows.findIndex((row) => row.id === targetCommit(target)), rows.length, action === "search:next" ? 1 : -1);
      if (at >= 0) { root.current?.querySelector<HTMLElement>(".git-graph-lines")?.focus(); choose(rows[at].id); later(focusHistory, 20); }
      return;
    }
    if (action === "copyMenu") { void copyMenu(selection); return; }
    if (action === "tools" || action.startsWith("tools:")) { openTools(action.slice(6) || undefined); return; }
    if (action === "compareBase") {
      const id = targetCommit(selection); if (!id) return;
      if (compareBase && compareBase !== id) { setRangeBase(compareBase); inspect(id); setCompareBase(null); }
      else { setCompareBase(id); A.notify(tt("Comparison base: {id}. Select another commit and press Ctrl+E.", { id: id.slice(0, 8) })); }
      return;
    }
    if (action === "interactiveRebase" || action === "interactiveRebaseOnto") { setRebaseOnto(action === "interactiveRebaseOnto" || selection?.kind === "reference"); setShowRebase(true); setShowAdvanced(false); setShowChanges(false); setInspected(null); return; }
    if (action === "operation") { openMenu("conflicts", selection, undefined, true); return; }
    if (compareFile && action !== "copyHash" && action !== "copyRef" && action !== "refresh") return;
    if (action === "applyPatch") { void applyClipboardPatch(); return; }
    if (action === "worktreeTry") { void scratchWorktree(selection); return; }
    if (action === "worktreeHere" || action === "worktreeNewBranch") { void quickWorktree(selection, action === "worktreeNewBranch"); return; }
    void actions.perform(action, selection).then(() => {
      if (action === "refresh" || action === "fetch") setRemoteTagRefresh((value) => value + 1);
    });
  };
  const operationMenu = (selection = target): MenuItem[] => repository?.operation ? ["continue", "skip", "abort"].map((action) => ({ label: `${action[0].toUpperCase() + action.slice(1)} ${repository.operation}…`, danger: action !== "continue", disabled: busy || !canRun(action, selection), run: () => run(action, selection) })) : [];
  /** Repository-wide actions (the toolbar Actions button); the object menus never repeat them. */
  const repositoryActions = ["showStashes", "reflog", "palette", "undo", "redo", "journal", "fetch", "pull", "push", "forcePush", "refresh", "search", "operation", "tools"];
  const openRepositoryMenu = () => {
    const entry = (action: string, label: string, danger?: boolean): MenuItem => ({ label, keyCap: capOf([...gitSharedCommands(shortcuts), ...gitCommands("history", shortcuts), ...gitCommands("files", shortcuts)].find((command) => command.action === action)), disabled: busy || loading || !canRun(action) || (!!compareFile && !["refresh", "search"].includes(action)), danger, run: () => run(action) });
    const operation = operationMenu();
    const menu: MenuItem[] = [
      ...(operation.length ? [...operation, { separator: true } as MenuItem] : []),
      entry("fetch", tt("Fetch remote")), entry("pull", tt("Pull (fast-forward only)")), entry("push", tt("Push branch")), entry("forcePush", tt("Force push with explicit lease…"), true),
      { separator: true },
      entry("stashSave", tt("Stash working changes…")), entry("refresh", tt("Refresh workspace")), entry("search", tt("Search commits")), entry("tools", tt("Repository tools…")), entry("palette", tt("Find a Git command…")),
      { separator: true },
      entry("undo", tt("Undo the last operation…")), entry("redo", tt("Redo the undone operation…")), entry("journal", tt("Operation history (undo timeline)")), entry("reflog", tt("Reflog: find lost commits")),
      ...(!compareFile ? [{ label: "Terminal here", run: () => run("terminal") } as MenuItem] : []),
    ];
    A.showMenuAt(belowAnchor(root.current?.querySelector(".gg-actions")), menu);
  };
  const openMenu = (scope = context, selection = target, anchor?: Anchor, operationOnly = false) => {
    const readOnly = (action: string) => !!compareFile && !["copyHash", "copyRef", "copyMenu", "refresh", "search", "compareBase"].includes(action);
    const item = (action: string, label: string, keyCap?: string, danger?: boolean, extra: Partial<MenuItem> = {}): MenuItem => ({ label, keyCap, disabled: busy || loading || !canRun(action, selection) || readOnly(action), danger, run: () => run(action, selection), ...extra });
    const top: MenuItem[] = [], groups = new Map<string, MenuItem[]>();
    const place = (group: string | null, entry: MenuItem) => { if (!group) top.push(entry); else groups.set(group, [...(groups.get(group) ?? []), entry]); };
    let menu: MenuItem[] = [];
    const operationItems = operationMenu(selection);
    const onStash = !operationOnly && !compareFile ? stashRow(selection) : undefined;
    if (operationOnly) menu = operationItems.length ? operationItems : [{ label: tt("No operation in progress"), disabled: true }];
    else if (scope === "history" && !compareFile && selection?.kind === "commit" && picked.size > 1 && picked.has(selection.id)) {
      const ids = actionCommits(selection), count = ids.length, cap = (action: string) => capOf(gitCommands("history", shortcuts).find((command) => command.action === action));
      const entry = (label: string, run: () => void, keyCap?: string, disabled = false): MenuItem => ({ label, keyCap, disabled: disabled || busy || loading, run });
      menu = [
        entry(tt("Cherry-pick {count} commits onto the current branch…", { count }), () => void cherryPick(ids, null, false), cap("cherryPick")),
        entry(tt("Cherry-pick {count} commits to another branch…", { count }), () => openPalette("cherryPickTo"), cap("cherryPickTo")),
        entry(tt("Squash {count} commits into one…", { count }), () => void squashPicked(ids), cap("quick:squash")),
        { separator: true },
        entry(tt("Copy {count} hashes", { count }), () => void copyText(() => ids.join("\n"), tt("Copied {count} hashes.", { count }))),
        entry(tt("Copy {count} commits as a patch", { count }), () => void copyPatches(ids)),
        entry(tt("Save {count} commits as a patch file…", { count }), () => void savePatches(ids), cap("savePatch")),
        entry(tt("Compare the two commits"), () => { setRangeBase(ids[0]); inspect(ids[1]); }, undefined, count !== 2),
        { separator: true },
        entry(tt("Leave out this commit"), () => run("pick", selection), cap("pick")),
        entry(tt("Clear the choice"), clearPicked),
      ];
      if (operationItems.length) menu.unshift(...operationItems, { separator: true });
    }
    else if (onStash) {
      // A stash row is not an ordinary commit: history rewriting does not apply, so offer the stash actions instead.
      const entry: GitTarget = { kind: "stash", stash: onStash };
      menu = [
        ...gitCommands("stashes", shortcuts).filter((command) => stashEntryActions.includes(command.action)).map((command) => item(command.action, command.label, capOf(command), command.danger, { disabled: busy || loading, run: () => run(command.action, entry) })),
        { separator: true },
        item("copyMenu", tt("Copy commit attribute…"), capOf(gitCommands("history", shortcuts).find((command) => command.action === "copyMenu"))),
      ];
      if (operationItems.length) menu.unshift(...operationItems, { separator: true });
    } else {
      const commitId = targetCommit(selection);
      if (!compareFile && selection?.kind === "commit" && commitId) for (const ref of repository?.refs ?? []) if (ref.kind === "branch" && ref.id === commitId && ref.name !== repository?.branch) top.push({ label: tt("Checkout branch {name}…", { name: ref.name }), disabled: busy || loading, run: () => run("checkout", { kind: "reference" as const, ref }) });
      if (!compareFile && selection?.kind === "commit" && commitId) for (const ref of repository?.refs ?? []) if (ref.kind === "remote" && ref.id === commitId) top.push({ label: tt("Checkout remote branch {name}…", { name: ref.name }), disabled: busy || loading, run: () => run("checkoutRemote", { kind: "reference" as const, ref }) });
      if (selection?.kind === "reference" && selection.ref.kind === "tag") top.push({ label: tt("Delete local tag…"), danger: true, disabled: busy, run: () => run("deleteTag", selection) });
      for (const command of gitCommands(scope, shortcuts)) if (!repositoryActions.includes(command.action) && command.action !== "pickRange" && (command.action !== "setUpstream" || canRun(command.action, selection))) place(gitMenuGroup(command.action), item(command.action, command.label, capOf(command), command.danger));
      if (!compareFile) {
        // Lazygit has no commit-panel keys for these (rebase is `r` and merge `M` on a branch;
        // `b` there opens bisect options), so they are menu-only and never shadow its keys.
        place(tt("Rewrite history"), item("interactiveRebaseOnto", tt("Interactive rebase current branch onto this…")));
        if (scope === "history") {
          place(tt("Rewrite history"), item("rebase", tt("Rebase current branch onto this commit…")));
          place("Integrate", item("merge", tt("Merge this commit into current branch…")));
        }
      }
      menu = [...top];
      const sorted = [...groups].sort((x, y) => gitMenuGroups.indexOf(x[0]) - gitMenuGroups.indexOf(y[0]));
      if (top.length && groups.size) menu.push({ separator: true });
      for (const [label, submenu] of sorted) menu.push({ label, submenu });
      if (operationItems.length) menu.unshift(...operationItems, { separator: true });
    }
    A.showMenuAt(anchor ?? keyMenuAnchor(selection), menu);
  };
  const move = (direction: number) => {
    if (busy || loading) return;
    const current = target?.kind === "working" ? items.findIndex((item) => item === null) : items.findIndex((item) => item?.id === targetCommit(target));
    const index = Math.max(0, Math.min((current >= 0 ? current : selected) + direction, items.length - 1)); setSelected(index); root.current?.querySelector<HTMLElement>(".git-graph-lines")?.focus();
    const item = items[index];
    if (item && rangeFrom) setPicked(new Set(rangeIds(rows, rangeFrom, item.id)));
    if (item) { if (compareFile) setTarget({ kind: "commit", id: item.id }); else inspect(item.id); }
    else { setTarget({ kind: "working" }); setInspected(null); setShowChanges(false); }
  };
  /** Leaves a lower panel (details, changes, tools, rebase, conflict) for the commit list; false when focus is not in one. */
  const leavePanel = (element: HTMLElement) => {
    const inPanel = element.closest(".git-rebase") ? "rebase" : element.closest(".git-advanced") ? "tools" : element.closest(".git-changes,.gg-working-details") ? "changes" : element.closest(".git-commit-details") ? "details" : element.closest(".gg-conflict") ? "conflict" : "";
    if (!inPanel) return false;
    if (inPanel === "rebase") setShowRebase(false); else if (inPanel === "tools") setShowAdvanced(false); else if (inPanel === "changes") setShowChanges(false); else if (inPanel === "details") { setInspected(null); setRangeBase(null); }
    setContext("history"); if (inPanel === "rebase") root.current?.focus(); else focusHistory();
    return true;
  };
  /** Esc without the focus inside a panel: closes the help or the lowest open panel; false when none is open. */
  const stepOut = () => {
    if (help) { setHelp(false); return true; }
    if (showJournal || showReflog) { closeJournal(); return true; }
    if (picked.size) { clearPicked(); return true; }
    if (showRebase) setShowRebase(false);
    else if (showAdvanced) setShowAdvanced(false);
    else if (showChanges) setShowChanges(false);
    else if (inspected) { setInspected(null); setRangeBase(null); }
    else return false;
    setContext("history");
    if (showRebase) root.current?.focus(); else focusHistory();
    return true;
  };
  stepOutRef.current = stepOut;
  const captureKey = (e: React.KeyboardEvent) => {
    const element = e.target as HTMLElement, pressed = gitKeyOf(e), chord = isGitChord(pressed);
    if (e.key === "Escape" && help) { e.preventDefault(); e.stopPropagation(); setHelp(false); return; }
    // `q` (Vim keys) works like Escape: out of a lower panel to the commit list first, then out of the window.
    if (vim && pressed === "q" && !busy && !element.closest("input,textarea,select,.gm")) {
      e.preventDefault(); e.stopPropagation();
      if (help) setHelp(false); else if (!leavePanel(element)) void askClose();
      return;
    }
    if (element.closest(".gp,.gj")) return;
    // The palette opens from every panel, with its chord from a field too.
    const shared = !busy ? gitSharedCommands(shortcuts).find((item) => item.action === "palette" && item.keys.includes(pressed)) : undefined;
    if (shared && (chord || (vim && !element.closest("input,textarea,select")))) { e.preventDefault(); e.stopPropagation(); openPalette(); return; }
    if (element.closest("input,textarea,select,.gm,.git-advanced,.git-rebase") || (!vim && !chord)) return;
    if (pressed === "?") { e.preventDefault(); e.stopPropagation(); setHelp((value) => !value); return; }
    if (busy) return;
    const local = element.closest(".git-changes,.git-commit-details");
    if (!local && !compareFile && !chord) {
      const inSidebar = !!element.closest(".gg-sidebar"), inHistory = element === e.currentTarget || !!element.closest(".git-graph-lines");
      const go = (action: () => void) => { e.preventDefault(); e.stopPropagation(); action(); };
      if (inSidebar && pressed === "l") return go(focusHistory);
      if (inSidebar && (pressed === "[" || pressed === "]")) return go(() => switchSideTab(pressed === "]" ? 1 : -1));
      if (inHistory && pressed === "h") return go(focusSidebar);
      if (inHistory && pressed === "l") return go(focusDetails);
    }
    if (element.closest("button:not(.gg-row):not(.gg-side-row)") && [" ", "Enter"].includes(pressed)) return;
    const scope = element.closest(".git-changes") ? "files" : element.closest(".git-commit-details") ? "details" : context;
    // On a stash row of the graph, Space, g and d act on the stash like in the Stashes tab.
    const stashCommand = scope === "history" && !compareFile && stashRow(target) ? gitCommandForKey("stashes", pressed, shortcuts) : undefined;
    // The diff uses some keys of the shared commands itself (x chooses a line there).
    if (element.closest(".gc-patch") && gitCommandForKey("diff", pressed, shortcuts)) return;
    // n and N go through the matches while a search is shown.
    const match = searching && scope === "history" && !local ? gitCommandForKey("search", pressed, shortcuts) : undefined;
    if (match && (vim || match.custom)) { e.preventDefault(); e.stopPropagation(); run(match.action); return; }
    const command = stashCommand && stashEntryActions.includes(stashCommand.action) ? stashCommand : gitCommandForKey(scope, pressed, shortcuts);
    // Without Vim keys only a chord the user chose runs a command.
    if (!command || (!vim && !command.custom && !command.always)) return;
    if (command.action === "pick" && target?.kind === "working" && !compareFile) { e.preventDefault(); e.stopPropagation(); working(); return; }
    // The panel carries out its own commands; only the shared ones and saving a stash are done here.
    if (/^(file|detail|panel|diff):/.test(command.action) || (local && !command.shared && command.action !== "stashSave")) return;
    e.preventDefault(); e.stopPropagation(); run(command.action);
  };
  const sideSelect = (selection: GitTarget, details = true) => {
    // Choosing a local or remote branch focuses it visually (not while a reference or file scope, or a comparison, is shown).
    if (selection.kind === "reference" && (selection.ref.kind === "tag" || isRemoteTag(selection.ref, repository?.refs ?? []))) setFocusRef(null);
    if (selection.kind === "reference" && (selection.ref.kind === "branch" || (selection.ref.kind === "remote" && !isRemoteTag(selection.ref, repository?.refs ?? []))) && !reference && !scoped && !compareFile) setFocusRef(selection.ref.fullName);
    setTarget(selection); setContext(sidebar); setShowChanges(false); setShowAdvanced(false); setShowRebase(false); setRangeBase(null); if (details) setInspected(targetCommit(selection));
  };
  const isCurrent = (ref: { kind: string; name: string }) => ref.kind === "branch" && repository?.branch === ref.name;
  const currentRef = repository?.refs.find((ref) => ref.kind === "branch" && ref.name === repository.branch);
  const forecasts = useForecasts(path, repository?.refs, repository?.head ?? "", showSidebar && sidebar === "branches" && !compareFile && !repository?.operation && !loading, getState().settings?.gitForecastBranches ?? 0);
  // The forecast of another branch: a control beside its row (never inside the row's button), which opens
  // the list of files that would conflict. It only shows; nothing is merged and no resolver opens.
  const forecastControl = (ref: GitReference) => {
    const value = ref.kind === "branch" && !isCurrent(ref) ? forecasts.get(ref.id) : undefined;
    if (!value?.files.length) return null;
    const files = value.files.join("\n");
    return <button type="button" className="gg-warn gg-forecast" key="forecast" disabled={busy} title={tt("Show which files would conflict (nothing is merged)")}
      aria-label={`${tt("Forecast for {branch}", { branch: ref.name })}: ${tt(value.files.length === 1 ? "{count} file in conflict" : "{count} files in conflict", { count: value.files.length })}`}
      onClick={() => void A.dialog({ title: tt("Forecast for {branch}", { branch: ref.name }), message: `${tt("Merging {branch} into the current branch would conflict in:\n{files}", { branch: ref.name, files })}`, buttons: [{ id: "ok", label: tt("Close"), primary: true }] })}>⚠ {value.files.length}</button>;
  };
  /** The merge of the selected branch as a row above the history, as it would be. */
  const ghost = target?.kind === "reference" && target.ref.kind === "branch" && repository?.branch && target.ref.name !== repository.branch ? forecasts.get(target.ref.id) : undefined;
  const sideButton = (key: string, selection: GitTarget, label: string, subtitle?: string, current = false, ahead = 0, behind = 0, gone = false, group = "", remoteState = "") => <button key={key} aria-current={selection.kind === "reference" && focusedRef?.fullName === selection.ref.fullName ? "true" : undefined} className={`gg-side-row ${group ? `gg-row-${group}` : ""} ${current ? "current" : ""} ${selection.kind === "reference" && focusedRef?.fullName === selection.ref.fullName ? "gg-focused" : ""} ${selection.kind === "reference" && dragOver === selection.ref.fullName ? "gg-drop-target" : ""}${JSON.stringify(target) === JSON.stringify(selection) ? "selected" : ""}`} disabled={busy} draggable={selection.kind === "reference"} data-drag-ref={selection.kind === "reference" ? selection.ref.fullName : undefined} data-drop-ref={selection.kind === "reference" && selection.ref.kind === "branch" ? selection.ref.fullName : undefined} onFocus={() => sideSelect(selection)} onClick={() => sideSelect(selection)} onContextMenu={(e) => { e.preventDefault(); sideSelect(selection, false); openMenu(sidebar, selection, eventAnchor(e)); }}><span>
    {selection.kind === "reference" && focusedRef?.fullName === selection.ref.fullName && (
      <span className="gg-ref-status gg-ref-highlighted" role="img" aria-label={tt("Highlighted")} title={tt("The graph highlights what this branch contains. Choose All references to stop.")}>
        <Icon name="eye" size={14} />
      </span>
    )}
    {current && <em className="gg-current-pill">{tt("Current")}</em>}
    <span className="gg-name" title={label}>{label}</span>
    {selection.kind === "reference" && selection.ref.kind === "branch" && wipBranches.has(selection.ref.name) && <em className="gg-current-pill gg-wip" title={tt("This branch remembers unfinished changes. They come back when you switch to it.")}>{tt("WIP")}</em>}
    {(ahead || behind) ? <span className="gg-track" title={tt("{ahead} outgoing (to push), {behind} incoming (to pull) relative to the upstream", { ahead, behind })}>{ahead ? `↑${ahead}` : ""}
    {behind ? ` ↓${behind}` : ""}</span> : null}
    {gone && <span className="gg-gone" title={tt("The upstream branch no longer exists on the remote")}>{tt("upstream gone")}</span>}
    {remoteState === "synced" && <span className="gg-synced" title={tt("On the remote and up to date")}>✓</span>}
    {remoteState === "untracked" && <span className="gg-local-only" title={tt("A branch with this name exists on a remote, but this one does not track it")}>{tt("not tracking")}</span>}
    {remoteState === "differs" && <span className="gg-gone" title={tt("A tag with this name exists on the remote but points to a different commit")}>{tt("differs from remote")}</span>}
    {remoteState === "local" && <span className="gg-ref-status" role="img" aria-label={tt("Local only")} title={tt("Local only")}><Icon name="wifiOff" size={14} /></span>}
  </span>{subtitle && <small>{subtitle}</small>}<span aria-hidden="true" title={tt("Actions (right-click or Shift+F10)")} className="gg-row-more" onClick={(e) => { e.stopPropagation(); sideSelect(selection, false); openMenu(sidebar, selection, eventAnchor(e)); }}>⋯</span></button>;
  // The unresolved files of a stopped operation: where they are shown, and what the control opens.
  const active = activeConflicts(repository);
  const openActiveConflicts = async () => {
    if (!active) return;
    // Unsaved choices or manual edits of the file in the editor are not dropped without asking, as for a click on another row.
    const leaving = !(target?.kind === "conflict" && target.file === active.first);
    if (leaving && dirty.current.has("merge") && !await A.confirm(tt("Switch conflict file?"), tt("Unsaved plans and conflict choices in this window are discarded."), "Switch")) return;
    // The first unresolved file in the conflict editor, through the same selection as a click on its row.
    setSidebar("conflicts"); setShowSidebar(true);
    sideSelect({ kind: "conflict", file: active.first });
    setContext("conflicts");
  };
  const activeControl = (label?: string) => active && <button type="button" className="gg-warn gg-active-conflicts" key="conflicts" disabled={busy} title={tt("Open the first unresolved file in the conflict editor")}
    aria-label={`${label ? `${label}: ` : ""}${tt(active.count === 1 ? "{count} file in conflict" : "{count} files in conflict", { count: active.count })}`} onClick={() => void openActiveConflicts()}>⚠ {label ? `${label} · ` : ""}{active.count}</button>;
  /** A row of the sidebar; the ⚠ controls are buttons beside its button, not nested in it. */
  const sideRow = (...args: Parameters<typeof sideButton>) => {
    const [key, selection, , , current] = args;
    const ref = selection.kind === "reference" ? selection.ref : null;
    const conflicts = current && ref?.kind === "branch" && active?.onCurrentRow ? activeControl() : null;
    const forecast = ref ? forecastControl(ref) : null;
    const button = sideButton(...args);
    return conflicts || forecast ? <div key={key} className="gg-side-item">{button}{conflicts}{forecast}</div> : button;
  };
  // The panel that has keyboard focus: outlined, named in the footer, and the other panels show their selection muted.
  const panel = gitPanelOf(context), panelLabel = panel === "sidebar" ? tt(sideLabels[sidebar]) : panel === "history" ? tt("Commits") : panel === "changes" ? tt("Working Changes") : panel === "tools" ? tt("Repository tools") : tt("Commit details");
  // A stash row of the history: the live stash it stands for (by its commit, which a renumbering does not change).
  const stashOfRow = (row: { id: string; stash?: string }) => row.stash ? repository?.stashes.find((stash) => stash.id === row.id) : undefined;
  const rowTarget = (row: { id: string; stash?: string }): GitTarget => { const stash = stashOfRow(row); return stash ? { kind: "stash", stash } : { kind: "commit", id: row.id }; };
  const rowSelected = (row: { id: string; stash?: string }) => row.stash ? target?.kind === "stash" && target.stash.id === row.id : targetCommit(target) === row.id && target?.kind !== "stash";
  const activeCommit = targetCommit(target), shownStash = !compareFile && target?.kind === "stash" ? repository?.stashes.find((stash) => stash.id === target.stash.id) : undefined, hasDetails = showChanges || !!inspected || !!shownStash || target?.kind === "conflict" || showAdvanced || showRebase;
  return <div className="backdrop glass center" onMouseDown={() => { if (!busy) void askClose(); }}>
    <div ref={root} className={`git-graph gg-workspace gg-focus-${panel} ${showRebase ? "gg-rebasing" : ""} ${target?.kind === "conflict" && !compareFile ? "gg-conflicting" : ""} ${hasDetails ? "gg-with-details" : ""} ${showChanges ? "gg-with-changes" : ""}`} role="dialog" aria-label={tt("Git graph")} tabIndex={-1} onMouseDown={(e) => { e.stopPropagation(); if (getState().menu) setState({ menu: null }); }} onKeyDownCapture={captureKey} onFocusCapture={(e) => {
      const el = e.target as HTMLElement; if (el.closest(".gp,.gj")) return; lastFocus.current = el; lastSideRow.current = el.classList.contains("gg-side-row") && el.closest(".gg-sidebar") ? [...(root.current?.querySelectorAll(".gg-sidebar .gg-side-row") ?? [])].indexOf(el) : -1; if (el === e.currentTarget) setContext("history"); else if (el.closest(".git-changes")) setContext("files"); else if (el.closest(".git-commit-details")) setContext("details"); else if (el.closest(".git-advanced")) setContext("tools"); else if (el.closest(".gg-sidebar")) setContext(sidebar); else if (el.closest(".git-graph-lines")) setContext("history");
    }} onKeyDown={(e) => {
      const el = e.target as HTMLElement;
      // A panel that handled Escape itself (and went back to the commit list) has already prevented it.
      if (e.key === "Escape" && !busy) {
        if (e.defaultPrevented) return;
        e.preventDefault();
        // The search field: first the text, then back to the commits, and only then the window.
        if (el === searchRef.current) { if (search) leaveSearch(); else if (items.length) focusHistory(); else root.current?.focus(); return; }
        // Out of the matches: the search ends and what was shown before it comes back.
        if (searching && !picked.size && (el === e.currentTarget || el.closest(".git-graph-lines"))) { leaveSearch(); later(() => { if (!document.activeElement || document.activeElement === document.body) root.current?.focus(); }, 80); return; }
        if (showJournal || showReflog) { closeJournal(); return; }
        // The choice of commits goes before the window (a running range is part of it).
        if (picked.size && !el.closest(".git-changes,.git-commit-details,.git-advanced,.git-rebase,.gg-conflict")) { clearPicked(); return; }
        if (!leavePanel(el)) void askClose();
        return;
      }
      if (busy || loading) return;
      if ((e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) && !e.ctrlKey && !e.metaKey && !e.altKey && !el.closest("input,textarea,select,.git-advanced,.git-rebase,.git-changes,.git-commit-details")) {
        // Same menu as a right-click on the selected row.
        e.preventDefault(); e.stopPropagation();
        const row = selectionRow(target);
        openMenu((row ?? el).closest(".gg-sidebar") ? sidebar : "history", target, row ? undefined : elementAnchor(el));
        return;
      }
      if (el.closest("input,select,textarea,button:not(.gg-row),.gg-sidebar,.git-advanced,.git-rebase") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "ArrowDown" || (vim && e.key === "j")) { e.preventDefault(); move(1); }
      if (e.key === "ArrowUp" || (vim && e.key === "k")) { e.preventDefault(); move(-1); }
      if (e.key === "Enter" && !busy && !loading) { e.preventDefault(); if (target?.kind === "working" && !compareFile) working(); else { const item = items.find((row) => row?.id === targetCommit(target)) ?? items[selected]; if (item) choose(item.id); } }
    }}>
      <header><div><h2>{compareFile ? tt("Compare with Git commit") : tt("Git workspace")}<span className="gg-repo" title={repository?.root ?? path}>{basename(repository?.root ?? path)}</span></h2><span className="gg-subtitle">{repository?.branch ?? (graph?.head ? tt("Detached HEAD") : tt("Repository history"))}{currentRef && (currentRef.ahead || currentRef.behind) ? <b className="gg-track" title={tt("{ahead} ahead, {behind} behind {upstream}", { ahead: currentRef.ahead, behind: currentRef.behind, upstream: currentRef.upstream ?? "" })}>{currentRef.ahead ? ` ↑${currentRef.ahead}` : ""}{currentRef.behind ? ` ↓${currentRef.behind}` : ""}</b> : null} · {wanted ? <span className="gg-found" role="status">{found.running && <span className="spinner" />}{found.truncated ? tt("first {count} matches in the whole history", { count: rows.length }) : tt(rows.length === 1 ? "{count} match in the whole history" : "{count} matches in the whole history", { count: rows.length })}</span> : <>{rows.length}{searching ? ` of ${allRows.length}` : ""} {tt("commits")}</>}{compareBase && tt(" · Comparison base {id}", { id: compareBase.slice(0, 8) })}{picked.size > 0 && <span className="gg-picked" role="status">{rangeFrom ? tt("{count} chosen · range", { count: picked.size }) : tt("{count} chosen", { count: picked.size })}<button title={tt("Clear the choice (Esc)")} aria-label={tt("Clear the choice")} onClick={clearPicked}>✕</button></span>}</span></div><div className="gg-header-actions"><button onClick={() => setHelp((value) => !value)} aria-label={tt("Git keyboard shortcuts")} title={tt("Git keyboard shortcuts (?)")}>?</button><button disabled={busy} onClick={() => void askClose()} aria-label={tt("Close Git graph")} title={tt("Close Git workspace (Esc)")}>✕</button></div></header>
      <div className="gg-toolbar">
        {!compareFile && <button aria-label={tt("Toggle repository panels")} title={tt("Show or hide the repository panels")} aria-expanded={showSidebar} onClick={() => setShowSidebar((value) => !value)}>☰</button>}
        {focusFile && path === initialPath && <label className="gg-reflog" title={focusFile}><input type="checkbox" aria-label={tt("Only this file")} checked={scoped} disabled={busy} onChange={(e) => toggleFileScope(e.target.checked)} />{tt("Only this file")}</label>}
        <input ref={searchRef} className="gg-search" disabled={busy} aria-label={tt("Search Git history")} title={scoped ? tt("Filter loaded file history") : tt("Text, or a: author, f: file, s: content, g: message, or a hash. The whole history is searched for a prefix and for text that no loaded commit has.")} placeholder={scoped ? tt("Filter loaded file history") : tt("Search commits…  a: author  f: file  s: content  g: message")} spellCheck={false} value={search} onChange={(e) => changeSearch(e.target.value)} onKeyDown={(e) => {
          if ((e.key !== "Enter" && e.key !== "ArrowDown") || e.nativeEvent.isComposing || busy || loading || !rows.length) return;
          e.preventDefault();
          e.stopPropagation();
          if (compareFile && e.key === "ArrowDown") {
            root.current?.querySelector<HTMLElement>(".git-graph-lines .gg-row[data-commit]")?.focus();
            return;
          }
          choose(rows[0].id);
          if (!compareFile) later(focusHistory, 20);
        }} />
        {!compareFile && submodules.length > 0 && <RepoPicker value={path} mainPath={initialPath} mainName={basename(mainRoot)} entries={submodules} disabled={busy} vim={vim} onSelect={(next) => void switchRepository(next)} />}
        {focusedRef && <button type="button" className="btn gg-unfocus" title={tt("Stop highlighting {name}", { name: focusedRef.name })} onClick={() => setFocusRef(null)}>{tt("All references")}</button>}<ThemedSelect aria-label={tt("Filter Git reference")} title={tt("Show only commits of one reference")} value={reference} disabled={busy || scoped} onChange={(e) => setReference(e.target.value)}><option value="">{tt("All references")}</option>{reference && !repository?.refs.some((ref) => ref.fullName === reference && refGroups.branches.some(([, , filter]) => filter(ref, repository.refs))) && <option value={reference}>{repository?.refs.find((ref) => ref.fullName === reference)?.name ?? reference}</option>}{refGroups.branches.map(([kind, title, filter]) => <optgroup key={kind} label={tt(title)}>{repository?.refs.filter((ref) => filter(ref, repository.refs)).map((ref) => <option key={ref.fullName} value={ref.fullName}>{ref.name}</option>)}</optgroup>)}</ThemedSelect>
        <label className="gg-reflog" title={tt("Include commits reachable only through reflog entries")}><input type="checkbox" checked={reflog} disabled={busy || scoped || !!reference} onChange={(e) => { setError(""); setReflog(e.target.checked); }} />{tt("Reflog")}</label>
        <button disabled={busy || loading} onClick={() => { setError(""); actions.setError(""); void reload(); }} aria-label={tt("Refresh Git workspace")} title={tt("Refresh the Git workspace")}>↻</button>
        {!compareFile && <button className="gg-checkout" title={tt("Check out the selected commit")} disabled={busy || loading || !activeCommit} onClick={() => run("checkout")}>{tt("Checkout")}<GitKeyCap scope="history" k="c" /></button>}
        <button className="gg-actions" disabled={busy || loading || !graph} aria-haspopup="menu" title={tt("Repository actions: fetch, pull, push, stash, refresh")} onClick={() => openRepositoryMenu()}>{tt("Actions")}</button>
        {!compareFile && <button className="gg-advanced-button" title={tt("Worktrees, bisect, patches, notes and more")} disabled={busy} onClick={() => { setAdvancedTab(undefined); setShowAdvanced((value) => !value); setShowRebase(false); setInspected(null); setShowChanges(false); }}>{tt("Tools")}<GitKeyCap scope="history" k="x" /></button>}
      </div>
      {(error || actions.error) && <div className={`gg-error ${openError === (error || actions.error) ? "open" : ""}`}><p role="alert">{error || actions.error}</p><button aria-expanded={openError === (error || actions.error)} title={tt("Show the whole message")} aria-label={tt("Show the whole message")} onClick={() => setOpenError((shown) => shown === (error || actions.error) ? "" : error || actions.error)}>{openError === (error || actions.error) ? "▴" : "▾"}</button></div>}
      {repository?.operation && !compareFile && <div className="gg-operation"><strong>{repository.operation} {tt("in progress")}</strong>{repository.operationCommit && <span className="gg-applying" title={repository.operationCommit.id}>{repository.operation === "merge" ? "merging" : "applying"} <code>{repository.operationCommit.id.slice(0, 8)}</code> {repository.operationCommit.subject}{repository.operationCommit.step ? ` (${repository.operationCommit.step[0]}/${repository.operationCommit.step[1]})` : ""}</span>}<span>{repository.conflicts.length} {tt("conflicted files")}</span><button disabled={busy || !!repository.conflicts.length} onClick={() => run("continue")}>{tt("Continue")}</button><button disabled={busy || !canRun("skip")} onClick={() => run("skip")}>{tt("Skip…")}</button><button disabled={busy} onClick={() => run("abort")}>{tt("Abort…")}</button></div>}
      <div className="gg-workspace-body" {...dragHandlers}>
        {showSidebar && !compareFile && <aside className="gg-sidebar" aria-label={tt("Repository panels")} onKeyDown={(e) => {
          if (["ArrowDown", "ArrowUp"].includes(e.key) || (vim && ["j", "k"].includes(e.key))) { e.preventDefault(); e.stopPropagation(); const list = [...e.currentTarget.querySelectorAll<HTMLButtonElement>(".gg-side-row")]; const at = list.indexOf(document.activeElement as HTMLButtonElement); const next = Math.max(0, Math.min(list.length - 1, at + (["ArrowDown", "j"].includes(e.key) ? 1 : -1))); list[next]?.focus(); list[next]?.click(); }
        }}>
          <nav className="gg-side-tabs">{sideTabs.map((name) => <button key={name} className={name === "conflicts" ? "gg-conflicts-tab" : name === "worktrees" && dropTab ? "gg-drop" : undefined} onDragOver={(e) => { if (name === "worktrees" && draggedFrom.current?.ref && !e.dataTransfer.types.includes("Files")) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; setDropTab(true); } }} onDragLeave={() => setDropTab(false)} onDrop={(e) => {
            setDropTab(false);
            const ref = name === "worktrees" ? repository?.refs.find((r) => r.fullName === draggedFrom.current?.ref) : undefined;
            if (ref) { e.preventDefault(); void quickWorktree({ kind: "reference", ref }); }
          }} aria-pressed={sidebar === name} title={tt(sideLabels[name])} onClick={() => { setSidebar(name); setContext(name); }}>{name === "branches" ? tt("Refs") : name === "conflicts" ? `⚠ ${tt(conflictCount === 1 ? "{count} conflicted file" : "{count} conflicted files", { count: conflictCount })}` : tt(sideLabels[name])}</button>)}</nav>
          <div className="gg-side-tools"><strong>{sidebar === "branches" ? tt("References") : tt(sideLabels[sidebar])}</strong>{sidebar !== "conflicts" && <button disabled={busy || !repository} aria-label={tt("Add {value}", { value: tt(sideNoun[sidebar]) })} title={tt("Add {value}", { value: tt(sideNoun[sidebar]) })} onClick={() => sidebar === "worktrees" ? openTools("worktrees") : run(sidebar === "branches" ? "createBranch" : sidebar === "tags" ? "createTag" : sidebar === "remotes" ? "addRemote" : "stashSave", sidebar === "tags" && repository?.head ? { kind: "commit", id: repository.head } : target)}>+</button>}</div>
          <div className="gg-side-list">
            {sidebar === "branches" && active && !active.onCurrentRow && <div className="gg-side-item gg-side-active">{activeControl(tt("Conflicts"))}</div>}
            {(sidebar === "branches" || sidebar === "tags") && refGroups[sidebar].map(([key, title, filter]) => { const everything = repository?.refs.filter((ref) => filter(ref, repository.refs)).sort((a, b) => Number(isCurrent(b)) - Number(isCurrent(a))) ?? []; const expanded = showAllGroups.has(key); const list = expanded ? everything : everything.filter((ref) => !hasLocalCounterpart(ref, repository?.refs ?? [])); const hidden = everything.length - list.length, duplicates = everything.filter((ref) => hasLocalCounterpart(ref, repository?.refs ?? [])).length; const toggleGroup = () => setShowAllGroups((current) => { const next = new Set(current); if (!next.delete(key)) next.add(key); return next; }); return <section key={key} className={`gg-ref-group gg-group-${key}`}><h3>{tt(title)}<span>{list.length}</span></h3>{list.map((ref) => sideRow(ref.fullName, { kind: "reference", ref }, ref.name, ref.upstream ?? ref.id.slice(0, 8), isCurrent(ref), ref.ahead, ref.behind, ref.upstreamGone, key, ref.kind === "tag" ? tagRemoteState(ref, remoteTags) : ref.kind !== "branch" || ref.upstreamGone ? "" : ref.upstream ? (ref.ahead || ref.behind ? "" : "synced") : repository?.refs.some((r) => r.kind === "remote" && r.name.endsWith(`/${ref.name}`)) ? "untracked" : "local"))}{!list.length && !duplicates && <p className="gg-empty">{tt("None")}</p>}{duplicates > 0 && <p className="gg-empty gg-hidden-note">{expanded ? tt("Including {duplicates} already local", { duplicates }) : list.length ? tt("{hidden} more already local", { hidden }) : tt("Nothing remote-only ({hidden} already local)", { hidden })} · <button className="gg-link" title={tt("Entries that already have a local copy are hidden here; the local entry shows whether it is in sync.")} onClick={toggleGroup}>{expanded ? tt("hide them") : tt("show anyway")}</button></p>}</section>; })}
            {sidebar === "stashes" && <GitStashList path={path} stashes={repository?.stashes ?? []} row={(stash, label, subtitle) => sideRow(stash.id + stash.selector, { kind: "stash", stash }, label, subtitle)} onFirst={focusFirstStash} onLeave={focusFirstStash} />}
            {sidebar === "remotes" && (repository?.remotes.length ? repository.remotes.map((remote) => sideRow(remote.name, { kind: "remote", remote }, remote.name, remote.fetchUrl)) : <p className="gg-empty">{tt("No remotes. Use + to add one.")}</p>)}
            {sidebar === "worktrees" && <GitWorktreeList path={path} reloadKey={repository} busy={busy} vim={vim} onBusy={setChildBusy} onChanged={() => void reload()} onCreate={() => openTools("worktrees")} />}
            {sidebar === "conflicts" && (repository?.conflicts.length ? repository.conflicts.map((file) => sideRow(file, { kind: "conflict", file }, file)) : <p className="gg-empty">{tt("No conflicts")}</p>)}
          </div>
        </aside>}
        <main ref={mainRef} className="gg-main">
          {!graph ? <div className="gg-empty" role="status">{loadProgress ? <GitProgressBar progress={loadProgress} /> : <><p>{tt("History could not be loaded.")}</p><button onClick={() => { setShowAdvanced(true); setError(""); }}>{tt("Repository setup…")}</button></>}</div> : <div className="git-graph-lines" tabIndex={-1} onFocus={() => setContext("history")}>
            {searchProgress && <GitProgressBar progress={searchProgress} compact onCancel={found.cancel} />}
            {ghost && target?.kind === "reference" && <div className={`gg-ghost ${ghost.files.length ? "conflict" : ""}`} aria-hidden="true"><svg width="36" height="24"><path d="M 10 24 V 12 M 26 24 Q 26 12 10 12" /><circle cx="10" cy="12" r="5" /></svg><span>{tt("Merge {source} into {target}", { source: target.ref.name, target: repository?.branch ?? "" })}</span><small>{ghost.files.length ? tt(ghost.files.length === 1 ? "{count} file in conflict" : "{count} files in conflict", { count: ghost.files.length }) : tt("no conflicts")}</small></div>}
            <div className="gg-columns" style={{ gridTemplateColumns: grid }}><span>{tt("Graph")}</span><span>{tt("Commit")}</span><span>{tt("Author")}</span><span>{tt("Date")}</span><span>{tt("Hash")}</span><span /></div>
            {items.map((row, index) => row === null ? <button key="working" disabled={busy || loading || !!graph.readOnlyReason || (!!repository?.operation && !repository.editStop) || !!repository?.bisectActive} className={`gg-row gg-working ${target?.kind === "working" ? "selected" : ""} ${showChanges ? "expanded" : ""}`} style={{ gridTemplateColumns: grid }} onFocus={() => { setSelected(index); setContext("history"); setTarget({ kind: "working" }); }} onClick={working} aria-expanded={showChanges}>
              <svg width={graphWidth} height={ROW_HEIGHT} aria-hidden="true" className="gg-lanes">{[...new Set([...(headRow?.edges.map((edge) => edge.from) ?? []), headRow?.lane ?? 0])].filter((lane) => lane !== headRow?.lane || !headRow.tip).map((lane) => <path key={lane} className={`gg-color-${(lane === headRow?.lane ? headRow.color : headRow?.edges.find((e) => e.from === lane)?.color ?? 0) % 6}`} d={`M ${18 + lane * 18} 0 V ${ROW_HEIGHT}`} />)}<path className={`gg-color-${(headRow?.color ?? 0) % 6}`} d={`M ${18 + workingLane * 18} ${ROW_MID} V ${ROW_HEIGHT}`} /><circle className={`gg-color-${(headRow?.color ?? 0) % 6} gg-working-dot`} cx={18 + workingLane * 18} cy={ROW_MID} r="6" /></svg>
              <span className="gg-message"><b>{tt("Working Changes")}</b><span className="gg-badge">{graph.changes ? tt("{count} changed", { count: graph.changes.trimEnd().split("\n").length }) : tt("Clean")}</span></span><span className="gg-muted">{tt("Local")}</span><span /><span className="gg-muted">{showChanges ? tt("Collapse") : tt("Review")}</span><span />
            </button> : <button key={row.key} data-commit={row.id} data-stash={row.stash} disabled={busy || loading} draggable={!compareFile && !row.stash} className={`gg-row ${row.stash ? "gg-stash-row" : ""} ${reached && !inFocus(reached, row.key) ? "gg-dim" : ""} ${dragOver === row.id ? "gg-drop-target" : ""} ${rowSelected(row) ? "selected" : ""} ${picked.has(row.id) ? "picked" : ""}`} aria-selected={picked.has(row.id) || undefined} style={{ gridTemplateColumns: grid }} onFocus={() => { setSelected(index); setContext("history"); setTarget(rowTarget(row)); }} onClick={(e) => {
              if (compareFile) { choose(row.id); return; }
              // A stash row shows the stash preview, not commit details.
              if (row.stash && !e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey && stashOfRow(row)) { clearPicked(); clickAnchor.current = row.id; setContext("history"); setRangeBase(null); setTarget(rowTarget(row)); return; }
              // Compare: Alt+click (the base is the commit that was shown). Ctrl+click and Shift+click choose several commits.
              if (e.altKey && (compareBase || inspected)) { setRangeBase(compareBase ?? inspected); inspect(row.id); setCompareBase(null); return; }
              if (e.ctrlKey || e.metaKey) { setRangeFrom(null); clickAnchor.current = row.id; setPicked((current) => toggleId(current.size || !activeCommit || activeCommit === row.id || target?.kind !== "commit" ? current : new Set([activeCommit]), row.id)); setTarget({ kind: "commit", id: row.id }); setSelected(index); return; }
              if (e.shiftKey) { setRangeFrom(null); const from = clickAnchor.current ?? (target?.kind === "commit" ? target.id : row.id); clickAnchor.current = from; setPicked(new Set(rangeIds(rows, from, row.id))); setTarget({ kind: "commit", id: row.id }); setSelected(index); return; }
              clearPicked(); clickAnchor.current = row.id; choose(row.id);
            }} onDoubleClick={(e) => { if (compareFile || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return; if (doubleClick === "checkout") run("checkout", { kind: "commit", id: row.id }); else focusDetails(); }} onContextMenu={(e) => { e.preventDefault(); setTarget({ kind: "commit", id: row.id }); setSelected(index); openMenu("history", { kind: "commit", id: row.id }, eventAnchor(e)); }} title={`${row.subject}\n${row.id}\n${row.author} · ${row.date}${row.refs.length ? `\n${row.refs.join(", ")}` : ""}`}>
              <svg width={graphWidth} height={ROW_HEIGHT} aria-hidden="true" className="gg-lanes">{!compareFile && row.id === graph.head && <path className={`gg-working-link gg-color-${row.color % 6}`} d={`M ${18 + workingLane * 18} 0 Q ${18 + workingLane * 18} ${ROW_MID} ${18 + row.lane * 18} ${ROW_MID}`} />}{row.edges.map((edge, j) => <path key={j} className={`gg-color-${edge.color % 6} ${reached && !inFocus(reached, edge.owner) ? "gg-dim-edge" : ""}`} d={edgePath(edge, row.lane)} />)}{!row.tip && <path className={`gg-color-${row.color % 6} ${reached && row.incoming && !inFocus(reached, row.incoming) ? "gg-dim-edge" : ""}`} d={`M ${18 + row.lane * 18} 0 V ${ROW_MID}`} />}{row.stash ? <rect className={`gg-color-${row.color % 6} gg-stash-node ${reached && !inFocus(reached, row.key) ? "gg-dim-edge" : ""}`} x={18 + row.lane * 18 - 6} y={ROW_MID - 6} width={12} height={12} rx={2} /> : graphTagNames(row.id, row.refs).length > 0 ? <g className={`gg-tag-node gg-tag-${tagPresence(row.id, graphTagNames(row.id, row.refs))} ${reached && !inFocus(reached, row.key) ? "gg-dim-edge" : ""}`}>
                <title>{graphTagNames(row.id, row.refs).map((name) => tagDescription(name, row.id)).join("\n")}</title>
                <rect x={18 + row.lane * 18 - 7} y={ROW_MID - 7} width={14} height={14} rx={2} />
                {tagPresence(row.id, graphTagNames(row.id, row.refs)) === "both" && <path d={`M ${18 + row.lane * 18 - 6} ${ROW_MID - 6} h 6 v 12 h -6 z`} />}
                <text x={18 + row.lane * 18} y={ROW_MID} textAnchor="middle" dominantBaseline="central">T</text>
              </g> : <circle className={`gg-color-${row.color % 6} ${reached && !inFocus(reached, row.key) ? "gg-dim-edge" : ""}`} cx={18 + row.lane * 18} cy={ROW_MID} r={row.id === graph.head ? 6 : 4} />}</svg>
              <span className="gg-message">{row.stash && <span className="gg-badge stash" title={tt("A stash: its changes are saved aside and applied from the Stashes tab or the menu")}>{row.stash}</span>}{!row.stash && row.id === graph.head && <span className="gg-badge head">{tt("HEAD")}</span>}{(() => {
                const badges = badgeRefs(row.refs, repository?.refs ?? []).badges.filter((badge) =>
                  badge.kind !== "tag" && !(badge.kind === "remote" && /^[^/]+\/tags\//.test(badge.label))
                );
                const branches = badges.filter((badge) => badge.kind === "branch").sort((a, b) => Number(b.current) - Number(a.current));
                const attached = new Map<string, string[]>();
                const hiddenRemotes = new Set<string>();
                for (const branch of branches) {
                  const local = localByName.get(branch.label);
                  const upstream = badges.find((badge) => badge.kind === "remote" && badge.label === local?.upstream);
                  if (!local || !upstream) continue;
                  const remoteHead = badges.find((badge) => badge.kind === "remote" && badge.label === `${upstream.label.split("/")[0]}/HEAD`);
                  const names = [upstream.label, ...(remoteHead ? [remoteHead.label] : [])];
                  attached.set(branch.label, names);
                  names.forEach((name) => hiddenRemotes.add(name));
                }
                const visibleBranches = new Set(branches.slice(0, 2).map((badge) => badge.label));
                const overflow = branches.slice(2);
                return <>
                  {badges.filter((badge) => (badge.kind !== "remote" || !hiddenRemotes.has(badge.label)) && (badge.kind !== "branch" || visibleBranches.has(badge.label))).sort((a, b) => Number(b.current) - Number(a.current)).map((badge) => {
                    const local = badge.kind === "branch" ? localByName.get(badge.label) : undefined;
                    const remotes = attached.get(badge.label);
                    const detail = remotes && local ? [remotes.join(", "), tt("{ahead} outgoing (to push), {behind} incoming (to pull) relative to the upstream", { ahead: local.ahead, behind: local.behind })].join(" · ") : "";
                    return <span draggable={!!local && !compareFile} data-drag-ref={compareFile ? undefined : local?.fullName} data-drop-ref={compareFile ? undefined : local?.fullName} className={`gg-badge ${local && dragOver === local.fullName ? "gg-drop-target" : ""} ${badge.kind === "saved" ? "saved" : badge.kind === "remote" ? "remote" : badge.current ? "current" : ""}`} key={badge.fullName ?? `${badge.kind}:${badge.label}`} onDoubleClick={(e) => { if (local && !compareFile) { e.stopPropagation(); run("checkout", { kind: "reference", ref: local }); } }}>
                      {badge.kind === "saved" ? tt("Saved commit") : badge.label}{remotes && <span className="gg-ref-cloud" title={detail}><span aria-hidden="true">☁</span><span className="gg-ref-details">{detail}</span></span>}
                    </span>;
                  })}
                  {overflow.length > 0 && <span className="gg-ref-overflow">
                    <span className="gg-badge" title={[...overflow.map((badge) => [badge.label, ...(attached.get(badge.label) ?? [])].join(" · ")), tt("Commit actions (right-click or Shift+F10)")].join("\n")} onClick={(e) => { e.stopPropagation(); setTarget({ kind: "commit", id: row.id }); setSelected(index); openMenu("history", { kind: "commit", id: row.id }, eventAnchor(e)); }}>+{overflow.length}<span className="gg-ref-details">{tt("Commit actions (right-click or Shift+F10)")}</span></span>
                    <span className="gg-ref-overflow-list">{overflow.map((badge) => {
                      const local = localByName.get(badge.label);
                      return <span key={badge.label} className="gg-badge" draggable={!!local && !compareFile} data-drag-ref={compareFile ? undefined : local?.fullName} data-drop-ref={compareFile ? undefined : local?.fullName} title={(attached.get(badge.label) ?? []).join(", ")} onDoubleClick={(e) => { if (local && !compareFile) { e.stopPropagation(); run("checkout", { kind: "reference", ref: local }); } }}>{badge.label}</span>;
                    })}</span>
                  </span>}
                </>;
              })()}{graphTagNames(row.id, row.refs).map((name) => <span className="gg-tag-name" key={name}>{tagDescription(name, row.id)}</span>)}<span>{searching ? marked(row.subject, marks.subject) : row.subject}</span></span><span className="gg-muted">{searching ? marked(row.author, marks.author) : row.author}</span><time className="gg-muted" title={row.date}>{row.date?.slice(0, 10)}</time><code className="gg-hash">{row.id.slice(0, 8)}</code><span aria-hidden="true" title={tt("Commit actions (right-click or Shift+F10)")} className="gg-row-more" onClick={(e) => { e.stopPropagation(); setTarget({ kind: "commit", id: row.id }); setSelected(index); openMenu("history", { kind: "commit", id: row.id }, eventAnchor(e)); }}>⋯</span>
            </button>)}
            {!items.length && <p className="gg-empty" role={wanted ? "status" : undefined}>{wanted ? found.error ? tt("The search failed: {error}", { error: found.error }) : found.running ? tt("Searching the whole history…") : tt("No commit in the whole history matches.") : searching ? tt("No matching commit. Type at least two letters.") : tt("No commits on this reference.")}</p>}
            {scoped && fileHistory && (fileHistory.incomplete || (fileHistory.truncated && fileLimit >= 1000)) && <p className="gg-empty" role="status">{tt("Only part of this file’s history is shown.")}</p>}
            {scoped && fileHistory?.truncated && fileLimit < 1000 && <button className="gg-load-more" disabled={busy || loading} onClick={() => { rememberScope(); setFileLimit(Math.min(1000, fileLimit + 200)); }}>{tt("Load 200 more commits")}</button>}
            {!scoped && !wanted && allRows.length >= limit && limit < 20000 && <button className="gg-load-more" disabled={busy || loading} onClick={() => { if (focusFile) rememberScope(); setLimit((value) => Math.min(20000, value + 200)); }}>{tt("Load 200 more commits")}</button>}
          </div>}
          {graph?.readOnlyReason && <p className="gg-read-only">{graph.readOnlyReason}</p>}
          {showChanges && !compareFile && <section className="gg-working-details" aria-label={tt("Working Changes")}><div className="gg-working-title"><b>{tt("Working Changes")}</b><button disabled={busy} onClick={() => { setShowChanges(false); setContext("history"); focusHistory(); }}>{tt("Collapse")}</button></div><GitChanges path={path} onCustom={(id, file) => void runCustom(id, null, file)} onBusy={setChildBusy} onChanged={() => void reload()} onBack={focusHistory} onClose={() => { setShowChanges(false); setContext("history"); focusHistory(); }} /></section>}
          {shownStash && <GitStashPreview path={path} stash={shownStash} onClose={() => { setTarget(null); setContext("stashes"); focusFirstStash(); }} onBack={focusFirstStash} actions={<>{gitCommands("stashes", shortcuts).filter((command) => stashEntryActions.includes(command.action)).map((command) => <button key={command.action} className={command.danger ? "danger" : undefined} disabled={busy || loading} onClick={() => run(command.action, { kind: "stash", stash: shownStash })}>{tt(command.label)}</button>)}</>} />}
          {inspected && !shownStash && !compareFile && <GitCommitDetails path={path} commit={inspected} base={rangeBase ?? undefined} onClose={() => { setInspected(null); setRangeBase(null); setContext("history"); focusHistory(); }} onBack={focusHistory} onCheckout={reload} onNavigate={(id) => { setRangeBase(null); inspect(id); }} onAction={(action) => action === "menu" ? openMenu("history", { kind: "commit", id: inspected }, belowAnchor(root.current?.querySelector(".gcd-actions"))) : run(action)} />}
          {target?.kind === "conflict" && !compareFile && <section className="gg-conflict" tabIndex={-1} aria-label={tt("Conflict resolution")}>{repository?.operationCommit && <div className="gg-conflict-commit" role="note"><span>{repository.operation === "merge" ? tt("Merging") : repository.operation === "rebase" ? tt("Replaying") : repository.operation === "revert" ? tt("Reverting") : tt("Applying")}{repository.operationCommit.step ? ` ${repository.operationCommit.step[0]}/${repository.operationCommit.step[1]}` : ""}</span><code>{repository.operationCommit.id.slice(0, 8)}</code><strong title={repository.operationCommit.subject}>{repository.operationCommit.subject}</strong></div>}<div className="gg-working-title">
            <strong>{target.file}</strong>
            <div>
              {(conflictError || conflict?.binary) && <>
                <button disabled={busy} onClick={() => run("resolveOurs")}>{tt("Use current Git side (ours)…")}<GitKeyCap scope="conflicts" k="o" /></button>
                <button disabled={busy} onClick={() => run("resolveTheirs")}>{tt("Use incoming Git side (theirs)…")}<GitKeyCap scope="conflicts" k="t" /></button>
              </>}
              <button disabled={busy} onClick={() => run("editConflict")}>{tt("Edit file")}<GitKeyCap scope="conflicts" k="e" /></button>
              <button disabled={busy} onClick={() => run("resolveMark")} title={tt("Stage the working file exactly as it is on disk")}>{tt("Mark resolved…")}<GitKeyCap scope="conflicts" k=" " /></button>
            </div>
          </div>{conflictError ? <p role="alert">{conflictError}</p> : !conflict ? <p>{tt("Loading conflict…")}</p> : conflict.binary ? <p>{tt("Binary conflict")}</p> : <GitMergeEditor gitOperation={repository?.operation ?? null} conflict={conflict} vim={vim} busy={busy} onDirty={markMerge} onOpenExternal={() => run("editConflict")} onConfirm={resolveWith} />}</section>}
          {showAdvanced && !compareFile && <GitAdvanced key={advancedTab ?? "tools"} path={path} commit={activeCommit ?? undefined} reference={target?.kind === "reference" ? target.ref : undefined} repository={repository ?? undefined} initialTab={advancedTab} onBusy={setChildBusy} onChanged={() => void reload()} onBack={focusHistory} onClose={() => { setShowAdvanced(false); setContext("history"); focusHistory(); }} />}
          {showRebase && !compareFile && <GitRebase path={path} commit={activeCommit ?? undefined} onto={rebaseOnto} onDirty={markRebase} onBusy={setChildBusy} onChanged={() => void reload()} onClose={() => { setShowRebase(false); root.current?.focus(); }} />}
        </main>
      </div>
      {actions.output && <details className="gg-output" open={outputOpen} onToggle={(e) => setOutputOpen((e.currentTarget as HTMLDetailsElement).open)}><summary>{tt("Last operation output")}<button onClick={(e) => { e.preventDefault(); actions.setOutput(""); }} aria-label={tt("Clear Git output")} title={tt("Clear output")}>✕</button></summary><pre>{actions.output}</pre></details>}
      <footer>{(() => {
        const activeProgress = actions.progress ?? switchProgress;
        if (busy && activeProgress) {
          const onCancel = customRun
            ? () => void invoke("git_custom_cancel", { id: customRun })
            : actions.job
              ? actions.cancel
              : switchProgressTask?.seed.cancellable
                ? () => void invoke("git_advanced_cancel", { id: switchProgressTask.id })
                : undefined;
          const cancelLabel = customRun ? tt("Stop command") : actions.job ? tt("Cancel remote operation") : tt("Cancel");
          return <GitProgressBar progress={activeProgress} inline onCancel={onCancel} cancelLabel={cancelLabel} />;
        }
        return <><span>{busy ? tt("Running Git operation…") : compareFile ? tt("Read-only revision selection") : graph?.changes ? `● ${tt("Local changes")}` : `✓ ${tt("Working tree clean")}`}</span>{customRun ? <button onClick={() => void invoke("git_custom_cancel", { id: customRun })}>{tt("Stop command")}</button> : actions.job ? <button onClick={actions.cancel}>{tt("Cancel remote operation")}</button> : <span><b className="gg-panel-name" title={tt("Panel with keyboard focus")}>{panelLabel}</b>{vim ? <>{gitKeys.hints(context).map((hint) => <button key={hint.key} className="gg-hint" tabIndex={-1} disabled={busy} title={tt("Run this command")} onMouseDown={(e) => e.preventDefault()} onClick={() => pressOnPanel(hint.key)}><b>{hint.name === "Space" ? tt("Space") : hint.name}</b> {tt(hint.label)}</button>)}{tt(gitNavigationHint(context))} · <button className="gg-hint" tabIndex={-1} onMouseDown={(e) => e.preventDefault()} onClick={() => setHelp((value) => !value)}>{tt("? keys")}</button></> : tt("Select to inspect · right-click or Shift+F10 for actions · Tab moves between panels")}</span>}</>;
      })()}</footer>
      {hints && <HintLabels root={root.current} onDone={stopHints} />}
      <GitPaletteHost handle={palette} scope={context} source={paletteSource} current={{ branch: repository?.branch ?? null, head: repository?.head ?? "" }} canRun={paletteCanRun} onRun={runFromPalette} onClose={closePalette} />
      {showJournal && !compareFile && <GitJournal path={path} revision={graph} busy={busy || loading} onJump={(journal, applied) => void jumpTo(journal, applied)} onClose={closeJournal} />}
      {showReflog && !compareFile && <GitReflog path={path} revision={graph} busy={busy || loading} onShow={(id) => { closeJournal(); choose(id); }} onRestore={(action, id) => { run(action, { kind: "commit", id }); }} onTimeline={() => { setShowReflog(false); setShowJournal(true); }} onClose={closeJournal} />}
      {help && <section className="gg-key-help" aria-label={tt("Git keyboard shortcuts")}><div><strong>{tt(GIT_SCOPE_LABELS[context])} · {tt("actions")}</strong><button aria-label={tt("Close Git shortcuts")} title={tt("Close shortcuts")} onClick={() => setHelp(false)}>✕</button></div><p>{vim ? tt("Lazygit keys apply to the focused panel. No prefix needed. Move between panels with h and l (sidebar, commit list, details; Working Changes is the first commit row), switch sidebar tabs with [ and ], and press Esc to close details and return to the commit list.") : tt("Vim shortcuts are disabled. Use the buttons or Tab and Enter.")} {tt("Settings › Shortcuts changes the keys.")}</p>{[...gitCommands(context, shortcuts), ...(context === "files" ? gitCommands("diff", shortcuts) : []), ...(context === "history" && searching ? gitCommands("search", shortcuts) : [])].map((command) => <button key={command.id} disabled={busy || loading || !canRun(command.action)} onClick={() => run(command.action)}>{capOf(command) && <kbd>{capOf(command) === "Space" ? tt("Space") : capOf(command)}</kbd>}{tt(command.label)}</button>)}</section>}
    </div>
  </div>;
}
