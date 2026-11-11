import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GitChanges } from "./GitChanges";
import { GitCommitDetails } from "./GitCommitDetails";
import { GitAdvanced } from "./GitAdvanced";
import { GitRebase } from "./GitRebase";
import { GitMergeEditor } from "./GitMerge";
import { targetCommit, useGitRepositoryActions, type GitTarget } from "./useGitRepositoryActions";
import * as A from "../state/actions";
import { ROW_HEIGHT, ROW_MID, edgePath, layoutHistory } from "../core/gitGraph";
import { gitActionEnabled, gitCommandForKey, gitCommands, gitContextHint, gitMenuGroup, gitMenuGroups, gitNavigationHint, gitPanelOf, type GitContext, type GitReference, type GitSubmodule, type Repository } from "../core/gitRepository";
import { commitUrl, copyChoices, type CopyChoice } from "../core/gitCopy";
import { basename } from "../core/path";
import { refreshAll } from "../state/controllers";
import { getState, setState, useStore, type MenuItem } from "../state/store";
import "./GitGraph.css";

type Graph = { history: string; branches: string[]; head: string; changes: string; readOnlyReason?: string | null };
type Conflict = { file: string; base: string; ours: string; theirs: string; result: string; binary: boolean; token: string };
type Sidebar = "branches" | "tags" | "stashes" | "remotes" | "conflicts";
/** Tags fetched into refs/remotes/<remote>/tags/ are remote tags, not branches. */
const isRemoteTag = (ref: GitReference) => ref.kind === "remote" && /^[^/]+\/tags\//.test(ref.name);
const refGroups: Record<"branches" | "tags", [string, string, (ref: GitReference) => boolean][]> = {
  branches: [["branch", "Local branches", (ref) => ref.kind === "branch"], ["remote", "Remote branches", (ref) => ref.kind === "remote" && !isRemoteTag(ref)]],
  tags: [["tag", "Tags", (ref) => ref.kind === "tag"], ["remoteTag", "Remote tags", isRemoteTag]],
};
const sideLabels: Record<Sidebar, string> = { branches: "Branches", tags: "Tags", stashes: "Stashes", remotes: "Remotes", conflicts: "Conflicts" };
export function GitGraph({ path: initialPath, compareFile }: { path: string; compareFile?: string }) {
  // The workspace follows the selected repository: the opened one, or one of its submodules.
  const [path, setPath] = useState(initialPath), [mainRoot, setMainRoot] = useState(initialPath);
  const [reflog, setReflog] = useState(false), [limit, setLimit] = useState(200), [reference, setReference] = useState(""), [search, setSearch] = useState("");
  const [loading, setLoading] = useState(false), [graph, setGraph] = useState<Graph | null>(null), [repository, setRepository] = useState<Repository | null>(null), [error, setError] = useState("");
  const [childBusy, setChildBusy] = useState(false), [switchBusy, setSwitchBusy] = useState(false);
  const [showChanges, setShowChanges] = useState(false), [showAdvanced, setShowAdvanced] = useState(false), [advancedTab, setAdvancedTab] = useState<string | undefined>(undefined), [showRebase, setShowRebase] = useState(false), [rebaseOnto, setRebaseOnto] = useState(false);
  const [inspected, setInspected] = useState<string | null>(null), [compareBase, setCompareBase] = useState<string | null>(null), [rangeBase, setRangeBase] = useState<string | null>(null);
  const [selected, setSelected] = useState(0), [target, setTarget] = useState<GitTarget | null>({ kind: "working" }), [context, setContext] = useState<GitContext>("history");
  const [sidebar, setSidebar] = useState<Sidebar>("branches"), [showSidebar, setShowSidebar] = useState(() => window.innerWidth >= 1000 && !compareFile), [help, setHelp] = useState(false);
  const [submodules, setSubmodules] = useState<GitSubmodule[]>([]), [conflict, setConflict] = useState<Conflict | null>(null), [conflictError, setConflictError] = useState("");
  const request = useRef(0), root = useRef<HTMLDivElement>(null), searchRef = useRef<HTMLInputElement>(null);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const load = useCallback(async () => {
    const ticket = ++request.current; setLoading(true);
    try {
      const { graph: value, repository: repo } = await invoke<{ graph: Graph; repository: Repository }>("git_graph_view", { path, comparison: !!compareFile, reflog, limit, reference: reference || null });
      if (ticket === request.current) { setGraph(value); setRepository(repo); setError(""); setTarget((current) => current?.kind === "conflict" && !repo.conflicts.includes(current.file) ? { kind: "working" } : current); if (compareFile) { const first = layoutHistory(value.history)[0]; setTarget(first ? { kind: "commit", id: first.id } : null); } }
    } catch (e) { if (ticket === request.current) setError(String(e)); }
    finally { if (ticket === request.current) setLoading(false); }
  }, [path, compareFile, reflog, limit, reference]);
  useEffect(() => { setGraph(null); setError(""); setSelected(0); setTarget(compareFile ? null : { kind: "working" }); setInspected(null); void load(); root.current?.focus(); return () => { request.current++; }; }, [load]);
  useEffect(() => {
    const onFocus = () => { if (!document.hidden) void load(); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);
  useEffect(() => { setPath(initialPath); setMainRoot(initialPath); setSubmodules([]); }, [initialPath]);
  useEffect(() => {
    if (compareFile || path !== initialPath) return;
    let active = true;
    void invoke<{ repository: string | null; submodules: GitSubmodule[] }>("git_advanced", { path, commit: null }).then((value) => { if (active) { setSubmodules(value.submodules); if (value.repository) setMainRoot(value.repository); } }).catch(() => { if (active) setSubmodules([]); });
    return () => { active = false; };
  }, [path, initialPath, compareFile, repository]);
  const switchRepository = async (next: string) => {
    if (next === path || busy) return;
    if (!await A.confirm("Switch repository?", "Unsaved plans and conflict choices in this window are discarded.", "Switch")) return;
    setReference(""); setSearch(""); setLimit(200); setShowChanges(false); setShowAdvanced(false); setShowRebase(false); setCompareBase(null); setRangeBase(null); setPath(next);
  };  const reload = useCallback(async () => { refreshAll(); await load(); }, [load]);
  const checkout = async (destination: string, branch: boolean, fresh = false) => {
    if (loading || childBusy || compareFile) return;
    setSwitchBusy(true); setError("");
    try {
      const latest = await invoke<Graph>("git_graph", { path });
      let action = "clean", message = "";
      if (latest.changes) {
        const result = await A.dialog({ title: "Local changes", message: `Choose how to preserve or discard these changes before switching:\n\n${latest.changes}`, buttons: [{ id: "cancel", label: "Cancel" }, { id: "stash", label: "Stash" }, { id: "commit", label: "Commit…" }, { id: "discard", label: "Discard…", danger: true }] });
        if (!result || result.button === "cancel") return;
        action = result.button;
        if (action === "commit") { const value = await A.promptText("Commit all tracked and untracked changes", "", "Commit & switch"); if (!value?.trim()) return; message = value; }
        if (action === "discard" && !await A.confirm("Discard local changes?", `Tracked changes and untracked files will be removed from the working folder. Ignored files are kept. Gall retains a recovery stash named “Gall recovery before discard”.\n\n${latest.changes}`, "Discard & switch", true)) return;
      } else if (!await A.confirm("Checkout revision?", `Switch ${repository?.branch ?? "detached HEAD"} to ${branch ? destination : destination.slice(0, 12)}? This replaces working files and can update their filesystem timestamps.`, "Checkout")) return;
      await invoke("git_switch", { path, target: destination, branch, action, message, expected: fresh ? null : repository?.token ?? null });
      setShowChanges(false); setInspected(null); setTarget({ kind: "working" }); setSelected(0);
      await reload(); A.notify(branch ? `Switched to ${destination}` : `Detached HEAD at ${destination.slice(0, 12)}`);
    } catch (e) { actions.setError(String(e)); await reload(); }
    finally { setSwitchBusy(false); }
  };
  const actions = useGitRepositoryActions(path, repository, reload, checkout, (head) => { setTarget({ kind: "commit", id: head }); setInspected(head); setRangeBase(null); setShowChanges(false); });
  const busy = childBusy || switchBusy || actions.busy;
  const closing = useRef(false), busyRef = useRef(false), askCloseRef = useRef<() => Promise<void>>(async () => {});
  const askClose = async () => {
    if (closing.current) return;
    closing.current = true;
    try { if (await A.confirm("Close Git workspace?", "Unsaved conflict choices, manual edits and rebase plans in this window will be discarded.", "Close")) A.openOverlay(null); else root.current?.focus(); }
    finally { closing.current = false; }
  };
  busyRef.current = busy; askCloseRef.current = askClose;
  // Rows are disabled while an action runs, which makes the browser drop focus; give it back once the action
  // (or the confirmation it opened, even when cancelled) is over, so keys keep working without pressing Esc twice.
  const lastFocus = useRef<HTMLElement | null>(null);
  const overlayType = useStore((s) => s.overlay?.type);
  useEffect(() => {
    if (busy || loading || (overlayType !== "gitGraph" && overlayType !== "gitCommits")) return;
    const active = document.activeElement;
    if (active && active !== document.body && root.current?.contains(active)) return;
    const remembered = lastFocus.current;
    if (remembered?.isConnected && root.current?.contains(remembered) && !(remembered as HTMLButtonElement).disabled) remembered.focus();
    // The window itself: focusing a row here would run its selection side effects (e.g. closing the Tools panel).
    else root.current?.focus();
  }, [busy, loading, overlayType]);
  // When nothing inside the window has focus (after a click on the backdrop or a closed dialog), Esc/q must still reach it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || e.target !== document.body) return;
      const o = getState().overlay;
      if (!o || (o.type !== "gitGraph" && o.type !== "gitCommits") || busyRef.current || getState().menu) return;
      e.preventDefault(); void askCloseRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const resolveWith = async (content: string) => {
    if (target?.kind !== "conflict") return;
    setSwitchBusy(true);
    try {
      await invoke<string>("git_repository_action", { path, request: { action: "resolveContent", file: target.file, content }, expected: repository?.token ?? null });
      A.notify(`Resolved ${target.file}`); await reload();
    } catch (e) { actions.setError(String(e)); await reload().catch(() => {}); }
    finally { setSwitchBusy(false); }
  };
  const allRows = useMemo(() => layoutHistory(graph?.history ?? ""), [graph?.history]);
  const rows = useMemo(() => allRows.filter((row) => [row.id, row.subject, row.author, ...row.refs].join(" ").toLocaleLowerCase().includes(search.toLocaleLowerCase())), [allRows, search]);
  const conflictCount = repository?.conflicts.length ?? 0;
  const sideTabs = (Object.keys(sideLabels) as Sidebar[]).filter((name) => name !== "conflicts" || conflictCount > 0).sort((x, y) => Number(y === "conflicts") - Number(x === "conflicts"));
  // The Conflicts tab exists only while there are conflicts: show it when they appear, leave it when they are gone.
  useEffect(() => { if (conflictCount) { setSidebar("conflicts"); setShowSidebar(true); } else setSidebar((current) => current === "conflicts" ? "branches" : current); }, [conflictCount > 0]);
  const items = rows.flatMap((row) => !compareFile && row.id === graph?.head && !search ? [null, row] : [row]);
  if (!compareFile && allRows.length === 0 && graph && !search) items.push(null);
  const headRow = allRows.find((r) => r.id === graph?.head), workingLane = headRow?.width ?? 1;
  const graphWidth = Math.max(56, workingLane * 18 + 36, ...allRows.map((r) => r.width * 18 + 24));
  const grid = `${graphWidth}px minmax(180px, 1fr) 110px 92px 76px 28px`;
  useEffect(() => { root.current?.querySelector(".git-graph-lines .selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);
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
    if (compareFile) { if (!busy && !loading) A.compare({ kind: "gitCommit", path: compareFile, commit: id }, { kind: "file", path: compareFile }); }
    else { setContext("history"); setRangeBase(null); inspect(id); }
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
  /** `y` on a commit: a menu of attributes to copy (hash, subject, message, author, date, URL, diff, tags). */
  const copyMenu = async (selection: GitTarget | null) => {
    const id = targetCommit(selection); if (!id) return;
    const box = (document.activeElement as HTMLElement | null)?.closest(".gg-row, .gg-side-row")?.getBoundingClientRect();
    const [x, y] = box ? [box.left + 24, box.bottom] : [40, 80];
    try {
      const details = await invoke<{ id: string; author: string; email: string; date: string; message: string }>("git_commit_details", { path, commit: id, base: null });
      const remote = repository?.remotes.find((r) => r.name === "origin") ?? repository?.remotes[0];
      const tags = repository?.refs.filter((ref) => ref.kind === "tag" && ref.id === details.id).map((ref) => ref.name) ?? [];
      const copy = async (choice: CopyChoice) => {
        try { await invoke("clip_set_text", { text: typeof choice.text === "string" ? choice.text : await choice.text() }); A.notify(`Copied ${choice.label.toLowerCase()}.`); }
        catch (error) { setError(String(error)); }
      };
      const choices = copyChoices({ ...details, tags, url: remote ? commitUrl(remote.fetchUrl, details.id) : null }, () => invoke<string>("git_commit_diff", { path, commit: details.id }));
      A.showMenu(x, y, choices.map((choice) => ({ label: choice.label, run: () => void copy(choice) })));
    } catch (error) { setError(String(error)); }
  };
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
  const run = (action: string, selection = target) => {
    if (busy || loading || !canRun(action, selection)) return;
    setHelp(false);
    if (action.startsWith("file:") || action.startsWith("detail:")) { root.current?.querySelector(action.startsWith("file:") ? ".git-changes" : ".git-commit-details")?.dispatchEvent(new CustomEvent("gall-git-command", { detail: action.slice(action.indexOf(":") + 1) })); return; }
    if (action === "showHistory") {
      if (selection?.kind === "reference") { setReference(selection.ref.fullName); setSearch(""); setSelected(0); A.notify(`Showing history of ${selection.ref.name}. Pick “All references” to show everything.`); }
      return;
    }
    if (action === "search") { searchRef.current?.focus(); return; }
    if (action === "copyMenu") { void copyMenu(selection); return; }
    if (action === "tools" || action.startsWith("tools:")) { openTools(action.slice(6) || undefined); return; }
    if (action === "compareBase") {
      const id = targetCommit(selection); if (!id) return;
      if (compareBase && compareBase !== id) { setRangeBase(compareBase); inspect(id); setCompareBase(null); }
      else { setCompareBase(id); A.notify(`Comparison base: ${id.slice(0, 8)}. Select another commit and press W.`); }
      return;
    }
    if (action === "interactiveRebase" || action === "interactiveRebaseOnto") { setRebaseOnto(action === "interactiveRebaseOnto" || selection?.kind === "reference"); setShowRebase(true); setShowAdvanced(false); setShowChanges(false); setInspected(null); return; }
    if (action === "operation") { openMenu("conflicts", selection, undefined, true); return; }
    if (compareFile && action !== "copyHash" && action !== "copyRef" && action !== "refresh") return;
    void actions.perform(action, selection);
  };
  const operationMenu = (selection = target): MenuItem[] => repository?.operation ? ["continue", "skip", "abort"].map((action) => ({ label: `${action[0].toUpperCase() + action.slice(1)} ${repository.operation}…`, danger: action !== "continue", disabled: busy || !canRun(action, selection), run: () => run(action, selection) })) : [];
  /** Repository-wide actions (the toolbar Actions button); the object menus never repeat them. */
  const repositoryActions = ["fetch", "pull", "push", "forcePush", "refresh", "search", "operation", "tools"];
  const openRepositoryMenu = (event?: { clientX: number; clientY: number }) => {
    const box = root.current?.querySelector(".gg-actions")?.getBoundingClientRect();
    const entry = (action: string, label: string, key?: string, danger?: boolean): MenuItem => ({ label, shortcut: vim && key ? key : undefined, disabled: busy || loading || !canRun(action) || (!!compareFile && !["refresh", "search"].includes(action)), danger, run: () => run(action) });
    const operation = operationMenu();
    const menu: MenuItem[] = [
      ...(operation.length ? [...operation, { separator: true } as MenuItem] : []),
      entry("fetch", "Fetch remote", "f"), entry("pull", "Pull (fast-forward only)", "p"), entry("push", "Push branch", "P"), entry("forcePush", "Force push with explicit lease…", undefined, true),
      { separator: true },
      entry("stashSave", "Stash working changes…", "s"), entry("refresh", "Refresh workspace", "R"), entry("search", "Search loaded history", "/"), entry("tools", "Repository tools…", "x"),
      ...(!compareFile ? [{ label: "Terminal here", run: () => run("terminal") } as MenuItem] : []),
    ];
    A.showMenu(event?.clientX ?? box?.left ?? 40, event?.clientY ?? box?.bottom ?? 80, menu);
  };
  const openMenu = (scope = context, selection = target, event?: { clientX: number; clientY: number }, operationOnly = false) => {
    const box = root.current?.querySelector(".gg-actions")?.getBoundingClientRect();
    const readOnly = (action: string) => !!compareFile && !["copyHash", "copyRef", "copyMenu", "refresh", "search", "compareBase"].includes(action);
    const item = (action: string, label: string, key?: string, danger?: boolean, extra: Partial<MenuItem> = {}): MenuItem => ({ label, shortcut: vim && key ? key === " " ? "Space" : key : undefined, disabled: busy || loading || !canRun(action, selection) || readOnly(action), danger, run: () => run(action, selection), ...extra });
    const top: MenuItem[] = [], groups = new Map<string, MenuItem[]>();
    const place = (group: string | null, entry: MenuItem) => { if (!group) top.push(entry); else groups.set(group, [...(groups.get(group) ?? []), entry]); };
    let menu: MenuItem[] = [];
    const operationItems = operationMenu(selection);
    if (operationOnly) menu = operationItems.length ? operationItems : [{ label: "No operation in progress", disabled: true }];
    else {
      const commitId = targetCommit(selection);
      if (!compareFile && selection?.kind === "commit" && commitId) for (const ref of repository?.refs ?? []) if (ref.kind === "branch" && ref.id === commitId && ref.name !== repository?.branch) top.push({ label: `Checkout branch ${ref.name}…`, disabled: busy || loading, run: () => run("checkout", { kind: "reference" as const, ref }) });
      if (!compareFile && selection?.kind === "commit" && commitId) for (const ref of repository?.refs ?? []) if (ref.kind === "remote" && ref.id === commitId) top.push({ label: `Checkout remote branch ${ref.name}…`, disabled: busy || loading, run: () => run("checkoutRemote", { kind: "reference" as const, ref }) });
      if (selection?.kind === "reference" && selection.ref.kind === "tag") top.push({ label: "Delete local tag…", danger: true, disabled: busy, run: () => run("deleteTag", selection) });
      for (const command of gitCommands(scope)) if (!repositoryActions.includes(command.action) && (command.action !== "setUpstream" || canRun(command.action, selection))) place(gitMenuGroup(command.action), item(command.action, command.label, command.key, command.danger));
      if (!compareFile) {
        // Lazygit has no commit-panel keys for these (rebase is `r` and merge `M` on a branch;
        // `b` there opens bisect options), so they are menu-only and never shadow its keys.
        place("Rewrite history", item("interactiveRebaseOnto", "Interactive rebase current branch onto this…"));
        if (scope === "history") {
          place("Rewrite history", item("rebase", "Rebase current branch onto this commit…"));
          place("Integrate", item("merge", "Merge this commit into current branch…"));
        }
      }
      menu = [...top];
      const sorted = [...groups].sort((x, y) => gitMenuGroups.indexOf(x[0]) - gitMenuGroups.indexOf(y[0]));
      if (top.length && groups.size) menu.push({ separator: true });
      for (const [label, submenu] of sorted) menu.push({ label, submenu });
      if (operationItems.length) menu.unshift(...operationItems, { separator: true });
    }
    A.showMenu(event?.clientX ?? box?.left ?? 40, event?.clientY ?? box?.bottom ?? 80, menu);
  };
  const move = (direction: number) => {
    if (busy || loading) return;
    const current = target?.kind === "working" ? items.findIndex((item) => item === null) : items.findIndex((item) => item?.id === targetCommit(target));
    const index = Math.max(0, Math.min((current >= 0 ? current : selected) + direction, items.length - 1)); setSelected(index); root.current?.querySelector<HTMLElement>(".git-graph-lines")?.focus();
    const item = items[index]; if (item) { if (compareFile) setTarget({ kind: "commit", id: item.id }); else inspect(item.id); }
    else { setTarget({ kind: "working" }); setInspected(null); setShowChanges(false); }
  };
  const captureKey = (e: React.KeyboardEvent) => {
    const element = e.target as HTMLElement;
    if (vim && !busy && !element.closest("input,textarea,select,.git-changes,.git-commit-details,.git-rebase,.git-advanced") && ((e.ctrlKey && ["j", "k"].includes(e.key)) || (e.altKey && ["ArrowDown", "ArrowUp"].includes(e.key)))) { e.preventDefault(); e.stopPropagation(); run(e.key === "j" || e.key === "ArrowDown" ? "quick:moveEarlier" : "quick:moveLater"); return; }
    if (e.key === "Escape" && help) { e.preventDefault(); e.stopPropagation(); setHelp(false); return; }
    if (element.closest("input,textarea,select,.gm") || e.ctrlKey || e.metaKey || e.altKey || !vim) return;
    if (element.closest(".git-advanced,.git-rebase")) return;
    if (e.key === "?") { e.preventDefault(); e.stopPropagation(); setHelp((value) => !value); return; }
    if (busy) return;
    const local = element.closest(".git-changes,.git-commit-details");
    if (!local && !compareFile) {
      const inSidebar = !!element.closest(".gg-sidebar"), inHistory = element === e.currentTarget || !!element.closest(".git-graph-lines");
      const go = (action: () => void) => { e.preventDefault(); e.stopPropagation(); action(); };
      if (inSidebar && e.key === "l") return go(focusHistory);
      if (inSidebar && (e.key === "[" || e.key === "]")) return go(() => switchSideTab(e.key === "]" ? 1 : -1));
      if (inHistory && e.key === "h") return go(focusSidebar);
      if (inHistory && e.key === "l") return go(focusDetails);
    }
    if (local && !["f", "p", "P", "R", "m", "s", "x"].includes(e.key)) return;
    if (element.closest("button:not(.gg-row):not(.gg-side-row)") && [" ", "Enter"].includes(e.key)) return;
    const scope = element.closest(".git-changes") ? "files" : element.closest(".git-commit-details") ? "details" : context;
    if (scope === "history" && e.key === " " && target?.kind === "working" && !compareFile) { e.preventDefault(); e.stopPropagation(); working(); return; }
    const command = gitCommandForKey(scope, e.key);
    if (command && !command.action.startsWith("file:") && !command.action.startsWith("detail:")) { e.preventDefault(); e.stopPropagation(); run(command.action); }
    else if (!local && e.key === "i" && scope === "history") { e.preventDefault(); e.stopPropagation(); run("interactiveRebase"); }
  };
  const sideSelect = (selection: GitTarget, details = true) => {
    setTarget(selection); setContext(sidebar); setShowChanges(false); setShowAdvanced(false); setShowRebase(false); setRangeBase(null); if (details) setInspected(targetCommit(selection));
  };
  const isCurrent = (ref: { kind: string; name: string }) => ref.kind === "branch" && repository?.branch === ref.name;
  const currentRef = repository?.refs.find((ref) => ref.kind === "branch" && ref.name === repository.branch);
  const sideRow = (key: string, selection: GitTarget, label: string, subtitle?: string, current = false, ahead = 0, behind = 0) => <button key={key} className={`gg-side-row ${current ? "current" : ""} ${JSON.stringify(target) === JSON.stringify(selection) ? "selected" : ""}`} disabled={busy} onFocus={() => sideSelect(selection)} onClick={() => sideSelect(selection)} onContextMenu={(e) => { e.preventDefault(); sideSelect(selection, false); openMenu(sidebar, selection, e); }}><span>{label}{current && <em className="gg-current-pill">Current</em>}{(ahead || behind) ? <span className="gg-track" title={`${ahead} ahead, ${behind} behind upstream`}>{ahead ? `↑${ahead}` : ""}{behind ? ` ↓${behind}` : ""}</span> : null}</span>{subtitle && <small>{subtitle}</small>}<span aria-hidden="true" title="Actions (right-click or Shift+F10)" className="gg-row-more" onClick={(e) => { e.stopPropagation(); sideSelect(selection, false); openMenu(sidebar, selection, e); }}>⋯</span></button>;
  // The panel that has keyboard focus: outlined, named in the footer, and the other panels show their selection muted.
  const panel = gitPanelOf(context), panelLabel = panel === "sidebar" ? sideLabels[sidebar] : panel === "history" ? "Commits" : panel === "changes" ? "Working Changes" : panel === "tools" ? "Repository tools" : "Commit details";
  const activeCommit = targetCommit(target), hasDetails = showChanges || !!inspected || target?.kind === "conflict" || showAdvanced || showRebase;
  return <div className="backdrop glass center" onMouseDown={() => { if (!busy) void askClose(); }}>
    <div ref={root} className={`git-graph gg-workspace gg-focus-${panel} ${showRebase ? "gg-rebasing" : ""} ${hasDetails ? "gg-with-details" : ""} ${showChanges ? "gg-with-changes" : ""}`} role="dialog" aria-label="Git graph" tabIndex={-1} onMouseDown={(e) => { e.stopPropagation(); if (getState().menu) setState({ menu: null }); }} onKeyDownCapture={captureKey} onFocusCapture={(e) => {
      const el = e.target as HTMLElement; lastFocus.current = el; if (el === e.currentTarget) setContext("history"); else if (el.closest(".git-changes")) setContext("files"); else if (el.closest(".git-commit-details")) setContext("details"); else if (el.closest(".git-advanced")) setContext("tools"); else if (el.closest(".gg-sidebar")) setContext(sidebar); else if (el.closest(".git-graph-lines")) setContext("history");
    }} onKeyDown={(e) => {
      const el = e.target as HTMLElement;
      if (e.key === "Escape" && !busy) { e.preventDefault(); void askClose(); return; }
      if (busy || loading) return;
      if ((e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) && !e.ctrlKey && !e.metaKey && !e.altKey && !el.closest("input,textarea,select,.git-advanced,.git-rebase,.git-changes,.git-commit-details")) {
        // Same menu as a right-click on the focused row.
        e.preventDefault(); e.stopPropagation();
        const box = el.getBoundingClientRect(), at = { clientX: box.left + 24, clientY: box.bottom };
        openMenu(el.closest(".gg-sidebar") ? sidebar : "history", target, at);
        return;
      }
      if (el.closest("input,select,textarea,button:not(.gg-row),.gg-sidebar,.git-advanced,.git-rebase") || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "ArrowDown" || (vim && e.key === "j")) { e.preventDefault(); move(1); }
      if (e.key === "ArrowUp" || (vim && e.key === "k")) { e.preventDefault(); move(-1); }
      if (e.key === "Enter" && !busy && !loading) { e.preventDefault(); if (target?.kind === "working" && !compareFile) working(); else { const item = items.find((row) => row?.id === targetCommit(target)) ?? items[selected]; if (item) choose(item.id); } }
    }}>
      <header><div><h2>{compareFile ? "Compare with Git commit" : "Git workspace"}<span className="gg-repo" title={repository?.root ?? path}>{basename(repository?.root ?? path)}</span></h2><span className="gg-subtitle">{repository?.branch ?? (graph?.head ? "Detached HEAD" : "Repository history")}{currentRef && (currentRef.ahead || currentRef.behind) ? <b className="gg-track" title={`${currentRef.ahead} ahead, ${currentRef.behind} behind ${currentRef.upstream}`}>{currentRef.ahead ? ` ↑${currentRef.ahead}` : ""}{currentRef.behind ? ` ↓${currentRef.behind}` : ""}</b> : null} · {rows.length}{search ? ` of ${allRows.length}` : ""} commits{compareBase && ` · Comparison base ${compareBase.slice(0, 8)}`}</span></div><div className="gg-header-actions"><button onClick={() => setHelp((value) => !value)} aria-label="Git keyboard shortcuts">?</button><button disabled={busy} onClick={() => void askClose()} aria-label="Close Git graph">✕</button></div></header>
      <div className="gg-toolbar">
        {!compareFile && <button aria-label="Toggle repository panels" aria-expanded={showSidebar} onClick={() => setShowSidebar((value) => !value)}>☰</button>}
        <input ref={searchRef} className="gg-search" disabled={busy} aria-label="Search Git history" placeholder="Search loaded history…" value={search} onChange={(e) => { setSearch(e.target.value); setSelected(0); setTarget(null); setInspected(null); setShowChanges(false); }} />
        {!compareFile && submodules.length > 0 && <select aria-label="Repository" title="Main repository or submodule" value={path} disabled={busy} onChange={(e) => void switchRepository(e.target.value)}><option value={initialPath}>{basename(mainRoot)} (main)</option>{submodules.map((module) => <option key={module.path} value={`${mainRoot.replace(/[\\/]+$/, "")}/${module.path}`} disabled={module.state === "notInitialized" || module.state === "unsupportedPath"}>{module.path}{module.state === "notInitialized" ? " (not initialized)" : ""}</option>)}</select>}
        <select aria-label="Filter Git reference" value={reference} disabled={busy} onChange={(e) => setReference(e.target.value)}><option value="">All references</option>{reference && !repository?.refs.some((ref) => ref.fullName === reference && refGroups.branches.some(([, , filter]) => filter(ref))) && <option value={reference}>{repository?.refs.find((ref) => ref.fullName === reference)?.name ?? reference}</option>}{refGroups.branches.map(([kind, title, filter]) => <optgroup key={kind} label={title}>{repository?.refs.filter(filter).map((ref) => <option key={ref.fullName} value={ref.fullName}>{ref.name}</option>)}</optgroup>)}</select>
        <label className="gg-reflog" title="Include commits reachable only through reflog entries"><input type="checkbox" checked={reflog} disabled={busy || !!reference} onChange={(e) => { setError(""); setReflog(e.target.checked); }} />Reflog</label>
        <button disabled={busy || loading} onClick={() => { setError(""); actions.setError(""); void reload(); }} aria-label="Refresh Git workspace">↻</button>
        {!compareFile && <button className="gg-checkout" disabled={busy || loading || !activeCommit} onClick={() => run("checkout")}>Checkout</button>}
        <button className="gg-actions" disabled={busy || loading || !graph} aria-haspopup="menu" title="Repository actions: fetch, pull, push, stash, refresh" onClick={() => openRepositoryMenu()}>Actions</button>
        {!compareFile && <button className="gg-advanced-button" disabled={busy} onClick={() => { setAdvancedTab(undefined); setShowAdvanced((value) => !value); setShowRebase(false); setInspected(null); setShowChanges(false); }}>Tools</button>}
      </div>
      {(error || actions.error) && <p className="gg-error" role="alert">{error || actions.error}</p>}
      {repository?.operation && !compareFile && <div className="gg-operation"><strong>{repository.operation} in progress</strong>{repository.operationCommit && <span className="gg-applying" title={repository.operationCommit.id}>{repository.operation === "merge" ? "merging" : "applying"} <code>{repository.operationCommit.id.slice(0, 8)}</code> {repository.operationCommit.subject}{repository.operationCommit.step ? ` (${repository.operationCommit.step[0]}/${repository.operationCommit.step[1]})` : ""}</span>}<span>{repository.conflicts.length} conflicted files</span><button disabled={busy || !!repository.conflicts.length} onClick={() => run("continue")}>Continue</button><button disabled={busy || !canRun("skip")} onClick={() => run("skip")}>Skip…</button><button disabled={busy} onClick={() => run("abort")}>Abort…</button></div>}
      <div className="gg-workspace-body">
        {showSidebar && !compareFile && <aside className="gg-sidebar" aria-label="Repository panels" onKeyDown={(e) => {
          if (["ArrowDown", "ArrowUp"].includes(e.key) || (vim && ["j", "k"].includes(e.key))) { e.preventDefault(); e.stopPropagation(); const list = [...e.currentTarget.querySelectorAll<HTMLButtonElement>(".gg-side-row")]; const at = list.indexOf(document.activeElement as HTMLButtonElement); const next = Math.max(0, Math.min(list.length - 1, at + (["ArrowDown", "j"].includes(e.key) ? 1 : -1))); list[next]?.focus(); list[next]?.click(); }
        }}>
          <nav className="gg-side-tabs">{sideTabs.map((name) => <button key={name} className={name === "conflicts" ? "gg-conflicts-tab" : undefined} aria-pressed={sidebar === name} title={sideLabels[name]} onClick={() => { setSidebar(name); setContext(name); }}>{name === "branches" ? "Refs" : name === "conflicts" ? `⚠ ${conflictCount} conflicted ${conflictCount === 1 ? "file" : "files"}` : sideLabels[name]}</button>)}</nav>
          <div className="gg-side-tools"><strong>{sidebar === "branches" ? "References" : sideLabels[sidebar]}</strong>{sidebar !== "conflicts" && <button disabled={busy || !repository} aria-label={`Add ${sidebar === "branches" ? "branch" : sidebar === "tags" ? "tag" : sidebar === "remotes" ? "remote" : "stash"}`} onClick={() => run(sidebar === "branches" ? "createBranch" : sidebar === "tags" ? "createTag" : sidebar === "remotes" ? "addRemote" : "stashSave", sidebar === "tags" && repository?.head ? { kind: "commit", id: repository.head } : target)}>+</button>}</div>
          <div className="gg-side-list">
            {(sidebar === "branches" || sidebar === "tags") && refGroups[sidebar].map(([key, title, filter]) => { const list = repository?.refs.filter(filter).sort((a, b) => Number(isCurrent(b)) - Number(isCurrent(a))) ?? []; return <section key={key}><h3>{title}</h3>{list.map((ref) => sideRow(ref.fullName, { kind: "reference", ref }, ref.name, ref.upstream ?? ref.id.slice(0, 8), isCurrent(ref), ref.ahead, ref.behind))}{!list.length && <p className="gg-empty">None</p>}</section>; })}
            {sidebar === "stashes" && (repository?.stashes.length ? repository.stashes.map((stash) => sideRow(stash.id + stash.selector, { kind: "stash", stash }, stash.selector, stash.message)) : <p className="gg-empty">No saved stashes</p>)}
            {sidebar === "remotes" && (repository?.remotes.length ? repository.remotes.map((remote) => sideRow(remote.name, { kind: "remote", remote }, remote.name, remote.fetchUrl)) : <p className="gg-empty">No remotes. Use + to add one.</p>)}
            {sidebar === "conflicts" && (repository?.conflicts.length ? repository.conflicts.map((file) => sideRow(file, { kind: "conflict", file }, file)) : <p className="gg-empty">No conflicts</p>)}
          </div>
        </aside>}
        <main className="gg-main">
          {!graph ? <div className="gg-empty" role="status">{loading ? "Loading history…" : <><p>History could not be loaded.</p><button onClick={() => { setShowAdvanced(true); setError(""); }}>Repository setup…</button></>}</div> : <div className="git-graph-lines" tabIndex={-1} onFocus={() => setContext("history")}>
            <div className="gg-columns" style={{ gridTemplateColumns: grid }}><span>Graph</span><span>Commit</span><span>Author</span><span>Date</span><span>Hash</span><span /></div>
            {items.map((row, index) => row === null ? <button key="working" disabled={busy || loading || !!graph.readOnlyReason || (!!repository?.operation && !repository.editStop) || !!repository?.bisectActive} className={`gg-row gg-working ${target?.kind === "working" ? "selected" : ""} ${showChanges ? "expanded" : ""}`} style={{ gridTemplateColumns: grid }} onFocus={() => { setSelected(index); setContext("history"); setTarget({ kind: "working" }); }} onClick={working} aria-expanded={showChanges}>
              <svg width={graphWidth} height={ROW_HEIGHT} aria-hidden="true" className="gg-lanes">{[...new Set([...(headRow?.edges.map((edge) => edge.from) ?? []), headRow?.lane ?? 0])].filter((lane) => lane !== headRow?.lane || !headRow.tip).map((lane) => <path key={lane} className={`gg-color-${(lane === headRow?.lane ? headRow.color : headRow?.edges.find((e) => e.from === lane)?.color ?? 0) % 6}`} d={`M ${18 + lane * 18} 0 V ${ROW_HEIGHT}`} />)}<path className={`gg-color-${(headRow?.color ?? 0) % 6}`} d={`M ${18 + workingLane * 18} ${ROW_MID} V ${ROW_HEIGHT}`} /><circle className={`gg-color-${(headRow?.color ?? 0) % 6} gg-working-dot`} cx={18 + workingLane * 18} cy={ROW_MID} r="6" /></svg>
              <span className="gg-message"><b>Working Changes</b><span className="gg-badge">{graph.changes ? `${graph.changes.trimEnd().split("\n").length} changed` : "Clean"}</span></span><span className="gg-muted">Local</span><span /><span className="gg-muted">{showChanges ? "Collapse" : "Review"}</span><span />
            </button> : <button key={row.id} data-commit={row.id} disabled={busy || loading} className={`gg-row ${targetCommit(target) === row.id ? "selected" : ""}`} style={{ gridTemplateColumns: grid }} onFocus={() => { setSelected(index); setContext("history"); setTarget({ kind: "commit", id: row.id }); }} onClick={(e) => { if (e.shiftKey && !compareFile && (compareBase || inspected)) { setRangeBase(compareBase ?? inspected); inspect(row.id); setCompareBase(null); } else choose(row.id); }} onContextMenu={(e) => { e.preventDefault(); setTarget({ kind: "commit", id: row.id }); setSelected(index); openMenu("history", { kind: "commit", id: row.id }, e); }} title={`${row.subject}\n${row.id}`}>
              <svg width={graphWidth} height={ROW_HEIGHT} aria-hidden="true" className="gg-lanes">{!compareFile && row.id === graph.head && <path className={`gg-working-link gg-color-${row.color % 6}`} d={`M ${18 + workingLane * 18} 0 Q ${18 + workingLane * 18} ${ROW_MID} ${18 + row.lane * 18} ${ROW_MID}`} />}{row.edges.map((edge, j) => <path key={j} className={`gg-color-${edge.color % 6}`} d={edgePath(edge, row.lane)} />)}{!row.tip && <path className={`gg-color-${row.color % 6}`} d={`M ${18 + row.lane * 18} 0 V ${ROW_MID}`} />}<circle className={`gg-color-${row.color % 6}`} cx={18 + row.lane * 18} cy={ROW_MID} r={row.id === graph.head ? 6 : 4} /></svg>
              <span className="gg-message">{row.id === graph.head && <span className="gg-badge head">HEAD</span>}{row.refs.filter((r) => r !== "HEAD").map((ref) => <span className={`gg-badge ${ref.startsWith("tag: ") ? "tag" : ref.startsWith("refs/gall/recovery/") ? "saved" : repository?.refs.some((r) => r.kind === "remote" && r.name === ref) ? "remote" : ref.startsWith("HEAD -> ") ? "current" : ""}`} key={ref} onDoubleClick={(e) => { const name = ref.replace("HEAD -> ", ""); const found = repository?.refs.find((r) => r.kind === "branch" && r.name === name); if (found && !compareFile) { e.stopPropagation(); run("checkout", { kind: "reference", ref: found }); } }}>{ref.startsWith("refs/gall/recovery/") ? "Saved commit" : ref.replace("HEAD -> ", "")}</span>)}<span>{row.subject}</span></span><span className="gg-muted">{row.author}</span><time className="gg-muted" title={row.date}>{row.date?.slice(0, 10)}</time><code className="gg-hash">{row.id.slice(0, 8)}</code><span aria-hidden="true" title="Commit actions (right-click or Shift+F10)" className="gg-row-more" onClick={(e) => { e.stopPropagation(); setTarget({ kind: "commit", id: row.id }); setSelected(index); openMenu("history", { kind: "commit", id: row.id }, e); }}>⋯</span>
            </button>)}
            {!items.length && <p className="gg-empty">{search ? "No matching loaded commits. Clear the search or load more history." : "No commits on this reference."}</p>}
            {allRows.length >= limit && limit < 20000 && <button className="gg-load-more" disabled={busy || loading} onClick={() => setLimit((value) => Math.min(20000, value + 200))}>Load 200 more commits</button>}
          </div>}
          {graph?.readOnlyReason && <p className="gg-read-only">{graph.readOnlyReason}</p>}
          {showChanges && !compareFile && <section className="gg-working-details" aria-label="Working Changes"><div className="gg-working-title"><b>Working Changes</b><button disabled={busy} onClick={() => { setShowChanges(false); setContext("history"); focusHistory(); }}>Collapse</button></div><GitChanges path={path} onBusy={setChildBusy} onChanged={() => void reload()} onBack={focusHistory} onClose={() => { setShowChanges(false); setContext("history"); focusHistory(); }} /></section>}
          {inspected && !compareFile && <GitCommitDetails path={path} commit={inspected} base={rangeBase ?? undefined} onClose={() => { setInspected(null); setRangeBase(null); setContext("history"); focusHistory(); }} onBack={focusHistory} onNavigate={(id) => { setRangeBase(null); inspect(id); }} onAction={(action) => action === "menu" ? openMenu("history", { kind: "commit", id: inspected }) : run(action)} />}
          {target?.kind === "conflict" && !compareFile && <section className="gg-conflict" tabIndex={-1} aria-label="Conflict resolution">{repository?.operationCommit && <div className="gg-conflict-commit" role="note"><span>{repository.operation === "merge" ? "Merging" : repository.operation === "rebase" ? "Replaying" : repository.operation === "revert" ? "Reverting" : "Applying"}{repository.operationCommit.step ? ` ${repository.operationCommit.step[0]}/${repository.operationCommit.step[1]}` : ""}</span><code>{repository.operationCommit.id.slice(0, 8)}</code><strong title={repository.operationCommit.subject}>{repository.operationCommit.subject}</strong>{repository.operation === "rebase" && <small>Ours = the branch you rebase onto · Theirs = this commit</small>}</div>}<div className="gg-working-title"><strong>{target.file}</strong><div><button disabled={busy} onClick={() => run("editConflict")}>Edit file</button><button disabled={busy || !conflict} onClick={() => run("resolveMark")} title="Stage the working file exactly as it is on disk">Mark resolved…</button></div></div>{conflictError ? <p role="alert">{conflictError}</p> : !conflict ? <p>Loading conflict…</p> : conflict.binary ? <p>Binary conflict. Use the Panel actions menu to choose a Git side, or edit the file externally.</p> : <GitMergeEditor conflict={conflict} vim={vim} busy={busy} onOpenExternal={() => run("editConflict")} onConfirm={resolveWith} />}</section>}
          {showAdvanced && !compareFile && <GitAdvanced key={advancedTab ?? "tools"} path={path} commit={activeCommit ?? undefined} initialTab={advancedTab} onBusy={setChildBusy} onChanged={() => void reload()} onBack={focusHistory} onClose={() => { setShowAdvanced(false); setContext("history"); focusHistory(); }} />}
          {showRebase && !compareFile && <GitRebase path={path} commit={activeCommit ?? undefined} onto={rebaseOnto} onBusy={setChildBusy} onChanged={() => void reload()} onClose={() => { setShowRebase(false); root.current?.focus(); }} />}
        </main>
      </div>
      {actions.output && <details className="gg-output"><summary>Last operation output<button onClick={(e) => { e.preventDefault(); actions.setOutput(""); }} aria-label="Clear Git output">✕</button></summary><pre>{actions.output}</pre></details>}
      <footer><span>{busy ? "Running Git operation…" : compareFile ? "Read-only revision selection" : graph?.changes ? "● Local changes" : "✓ Working tree clean"}</span>{actions.job ? <button onClick={actions.cancel}>Cancel remote operation</button> : <span><b className="gg-panel-name" title="Panel with keyboard focus">{panelLabel}</b>{vim ? `${gitContextHint(context)} · ${gitNavigationHint(context)} · ? keys` : "Select to inspect · right-click or Shift+F10 for actions · Tab moves between panels"}</span>}</footer>
      {help && <section className="gg-key-help" aria-label="Git keyboard shortcuts"><div><strong>{context[0].toUpperCase() + context.slice(1)} actions</strong><button aria-label="Close Git shortcuts" onClick={() => setHelp(false)}>✕</button></div><p>{vim ? "Lazygit keys apply to the focused panel. No prefix needed. Move between panels with h and l (sidebar, commit list, details; Working Changes is the first commit row), switch sidebar tabs with [ and ], and press Esc to close details and return to the commit list." : "Vim shortcuts are disabled. Use the buttons or Tab and Enter."}</p>{gitCommands(context).map((command) => <button key={command.key} disabled={busy || loading || !canRun(command.action)} onClick={() => run(command.action)}><kbd>{command.key === " " ? "Space" : command.key}</kbd>{command.label}</button>)}</section>}
    </div>
  </div>;
}
