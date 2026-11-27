import { ThemedSelect } from "./ThemedSelect";
import { Fragment, memo, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { eventAnchor } from "../core/anchor";
import { gitKeyOf } from "../core/gitCommands";
import { diffLineNumbers, uncommitted, type BlameLine, type BlameMode } from "../core/gitBlame";
import { diffRange, lineActionsEnabled, parseRange, PromiseCache, scrubChange, type ScrubBase, RevisionCache, selectedLines, type HistoryEntry, type LineHistoryEntry } from "../core/gitFileHistory";
import { blameShort, blameTitle, useBlame, type BlameRequest } from "../state/gitBlame";
import { useGitKeys } from "../state/gitKeys";
import { commitSides } from "../core/gitImages";
import { GitImageDiff } from "./GitImageDiff";
import { GitKeyCap } from "./GitKeyCap";
import { isImage } from "./ImageCompare";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import type { GitCommitDetails as Details, GeneratedReason } from "../api/types";
import { GENERATED_REASON, collapsedRows, generatedSummary, stepVisible } from "../core/outline";
import { join, joinRel } from "../core/path";
import { useStore } from "../state/store";
import { formatLocale, tt } from "../i18n";
import { createProgressId } from "../core/gitProgress";
import { GitProgressBar, useGitProgress, useLocalProgress } from "./GitProgressBar";
import { PatchText, formattingOnly, usePatchMarks } from "./PatchMarks";
import "./GitCommitDetails.css";

type View = "diff" | "file" | "history" | "blame" | "lines";
/** Lines of a file in a commit whose history is shown. */
type Lines = { commit: string; file: string; start: number; end: number };
const BLAME_KEY = "git:blame";
const MAX_BLAME = 10000;

/** A line of the Blame view. Memoised: moving the cursor draws two lines, not ten thousand. */
const BlameRow = memo(function BlameRow({ line, text, index, current, now, onOpen, onPick }: { line: BlameLine | undefined; text: string; index: number; current: boolean; now: number; onOpen: (index: number) => void; onPick: (index: number) => void }) {
  return <div className={`gcd-blame-line ${current ? "current" : ""}`} data-line={index} onMouseDown={() => onPick(index)}>
    <button tabIndex={current ? 0 : -1} disabled={!line || uncommitted(line.commit)} title={line ? blameTitle(line, now) : undefined} onClick={() => onOpen(index)}><code>{line ? line.commit.id.slice(0, 8) : "…"}</code><span>{line?.commit.author}</span></button>
    <span className="gcd-line-number">{index + 1}</span><code>{text || " "}</code>
  </div>;
});

/** Tickets of line histories: only the last one of the window goes on. */
let lineTicket = Date.now();
type Props = { path: string; commit: string; base?: string; onClose: () => void; onBack?: () => void; onNavigate: (id: string) => void; onCheckout: () => Promise<void>; onAction?: (action: string) => void };

export function GitCommitDetails({ path, commit, base, onClose, onBack, onNavigate, onCheckout, onAction }: Props) {
  const [details, setDetails] = useState<Details | null>(null);
  const [parent, setParent] = useState<{ commit: string; id: string } | null>(null);
  const [selected, setSelected] = useState(0);
  const [view, setView] = useState<View>("diff");
  const [patch, setPatch] = useState("");
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [blameText, setBlameText] = useState<string[] | null>(null), [blameAt, setBlameAt] = useState(0);
  // "Peeled" revisions: blame as of before the commit of a line, again and again.
  const [peeled, setPeeled] = useState<{ commit: string; file: string }[]>([]);
  // Beside every line, or only for the line that was clicked.
  const [blameMode, setBlameMode] = useState<BlameMode>(() => localStorage.getItem(BLAME_KEY) === "all" ? "all" : "line"), [caret, setCaret] = useState<number | null>(null);
  const opened = useRef(Date.now()), now = opened.current;
  // An SVG is text as well; the diff of its text comes first.
  const [asText, setAsText] = useState(true);
  const [lines, setLines] = useState<Lines | null>(null), [lineHistory, setLineHistory] = useState<{ entries: LineHistoryEntry[]; truncated: boolean } | null>(null);
  // The revisions of the file for the File view; `at` is the one shown, 0 the one of this commit.
  const [revisionList, setRevisionList] = useState<{ details: Details; path: string; entries: HistoryEntry[] } | null>(null);
  const [at, setAt] = useState(0);
  // The list of revisions failed, or the scrubber is moving to another revision (the lines shown are then not its own).
  const [historyError, setHistoryError] = useState(""), [transition, setTransition] = useState(false);
  // The change of each revision (against its first parent, or the parent chosen for a merge) instead of the file as it was.
  const [scrubDiff, setScrubDiff] = useState(false), [parentChoice, setParentChoice] = useState(0);
  const [scrub, setScrub] = useState<{ key: string; result?: { text: string; base: ScrubBase; none: boolean }; error?: string } | null>(null);
  const [parentCount, setParentCount] = useState(1);
  const parentsCache = useMemo(() => new PromiseCache<string[]>(32), [path]);
  const patchCache = useMemo(() => new PromiseCache<{ text: string; none: boolean }>(32), [path]);
  const loadScrub = (entry: HistoryEntry, choice: number) => scrubChange(entry, choice, {
    parentsOf: (id) => parentsCache.get(id, () => api.gitCommitDetails(path, id, null).then((d) => d.parents)),
    patch: (key, load) => patchCache.get(key, load),
    load: (e, base) => invoke<string>("git_commit_patch", { path, commit: e.id, base, file: e.file }),
  });
  const cache = useMemo(() => new RevisionCache((entry) => invoke<string>("git_commit_content", { path, commit: entry.id, file: entry.file })), [path]);
  const [error, setError] = useState("");
  const [previewError, setPreviewError] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [previewProgressId, setPreviewProgressId] = useState<string | null>(null);
  const [lineHistoryTicket, setLineHistoryTicket] = useState<number>(0);
  const [copied, setCopied] = useState(false);
  const root = useRef<HTMLElement>(null);
  const fileList = useRef<HTMLDivElement>(null);
  const preview = useRef<HTMLDivElement>(null);
  const gitKeys = useGitKeys(), { vim } = gitKeys;
  const effectiveBase = base ?? (parent?.commit === commit ? parent.id : undefined);
  const current = details?.files[selected];
  // Generated files (lockfiles, build output) are drawn collapsed by default; they stay in the list and act as any other.
  const [generatedFiles, setGeneratedFiles] = useState<ReadonlyMap<string, GeneratedReason>>(new Map());
  const [revealedGenerated, setRevealedGenerated] = useState(false);
  const listed = (details?.files ?? []).map((file) => ({ path: file.path, staged: true }));
  const collapsed = collapsedRows(listed, generatedFiles, { staged: revealedGenerated, unstaged: revealedGenerated });
  const generatedSummaryOfList = generatedSummary(listed, generatedFiles, true);
  const generatedCount = generatedSummaryOfList.count, generatedReasons = generatedSummaryOfList.reasons;
  const listKey = listed.map((f) => f.path).join("\n");
  const generatedPatternsKey = (useStore((s) => s.settings?.generatedPatterns) ?? []).join("\n");
  useEffect(() => {
    let live = true;
    setRevealedGenerated(false);
    Promise.all([...new Set(listed.map((f) => f.path))].map((file) => api.generatedClassify(join(path, file), "").then((g) => [file, g] as const).catch(() => [file, null] as const)))
      .then((found) => { if (live) setGeneratedFiles(new Map(found.flatMap(([file, g]) => (g ? [[file, g.reason] as const] : [])))); });
    return () => { live = false; };
  }, [path, listKey, generatedPatternsKey]);
  const deleted = current?.status.startsWith("D") ?? false;
  const inspectedCommit = deleted ? details?.base : details?.id;
  const top = peeled[peeled.length - 1], targetCommit = top?.commit ?? inspectedCommit, targetFile = top?.file ?? current?.path;
  const blameTarget = useMemo(() => targetCommit && targetFile ? { commit: targetCommit, file: targetFile } : null, [targetCommit, targetFile]);

  useEffect(() => {
    let active = true;
    setDetails(null); setLoading(true); setError(""); setSelected(0); setView("diff"); setCopied(false); setLines(null); setPeeled([]);
    api.gitCommitDetails(path, commit, effectiveBase).then((value) => {
      if (active) setDetails(value);
    }).catch((e) => { if (active) setError(String(e)); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [path, commit, effectiveBase]);

  useEffect(() => {
    let active = true;
    setPatch(""); setHistory([]); setBlameText(null); setLineHistory(null); setPreviewError(""); setAt(0); setCaret(null);
    let ticket = 0;
    if (!current || !details) { setLoadingPreview(false); return; }
    setLoadingPreview(true);
    setPreviewProgressId(null);
    setLineHistoryTicket(0);
    const load = async () => {
      if (view === "history") {
        const progressId = createProgressId("fhist");
        setPreviewProgressId(progressId);
        const value = await invoke<HistoryEntry[]>("git_file_history", { path, file: current.path, limit: 200, commit: details.id, progressId });
        if (active) setHistory(value);
      } else if (view === "lines") {
        if (!lines) return;
        ticket = ++lineTicket;
        const progressId = createProgressId("lhist");
        setLineHistoryTicket(ticket);
        setPreviewProgressId(progressId);
        const value = await invoke<{ entries: LineHistoryEntry[]; truncated: boolean }>("git_line_history", { path, ...lines, ticket, progressId });
        if (active) setLineHistory(value);
      } else if (view === "blame") {
        if (!blameTarget) throw new Error(tt("This file has no revision to annotate."));
        const value = await cache.get({ id: blameTarget.commit, file: blameTarget.file, subject: "", author: "", date: "" });
        // The line break at the end starts no line.
        if (active) setBlameText(value === "" ? [] : value.replace(/\r?\n$/, "").split(/\r?\n/));
      } else if (view === "file") {
        if (!inspectedCommit) throw new Error(tt("This file has no revision to preview."));
        const value = await invoke<string>("git_commit_content", { path, commit: inspectedCommit, file: current.path });
        if (active) setPatch(value);
      } else {
        const value = await invoke<string>("git_commit_patch", { path, commit: details.id, base: effectiveBase ?? null, file: current.path });
        if (active) setPatch(value);
      }
    };
    void load().catch((e) => { if (active) setPreviewError(String(e)); }).finally(() => { if (active) { setLoadingPreview(false); setPreviewProgressId(null); setLineHistoryTicket(0); } });
    return () => { active = false; if (ticket) invoke("git_line_history_cancel", { ticket }).catch(() => {}); };
  }, [path, current?.path, details, effectiveBase, inspectedCommit, view, lines, blameTarget, cache]);
  useEffect(() => { setPeeled([]); setBlameAt(0); }, [current?.path]);

  // The revisions are read in the background once the file is on screen.
  useEffect(() => {
    setRevisionList(null);
    setHistoryError("");
    if (view !== "file" || loadingPreview || !current || !details || !inspectedCommit) return;
    let active = true;
    invoke<HistoryEntry[]>("git_file_history", { path, file: current.path, limit: 200, commit: inspectedCommit }).then((list) => {
      if (!active) return;
      setRevisionList({ details, path: current.path, entries: list });
      if (list[1]) cache.get(list[1]).catch(() => {});
    }).catch((e) => { if (active) setHistoryError(String(e)); });
    return () => { active = false; };
  }, [view, loadingPreview, path, current?.path, details, inspectedCommit, cache]); // eslint-disable-line react-hooks/exhaustive-deps
  const revisions = revisionList && revisionList.details === details && revisionList.path === current?.path ? revisionList.entries : [];
  const wanted = useRef(0);
  const goTo = (index: number) => {
    const next = Math.max(0, Math.min(index, revisions.length - 1)), entry = revisions[next];
    if (!entry || next === at) return;
    setAt(next);
    const ticket = ++wanted.current;
    // The change of the revision is loaded by its own effect: the text of the revision is not needed (and a binary one
    // would fail and hide the change).
    if (scrubDiff) return;
    setTransition(true);
    cache.get(entry).then((text) => { if (ticket === wanted.current) { setPatch(text); setPreviewError(""); setTransition(false); } }).catch((e) => { if (ticket === wanted.current) { setPreviewError(String(e)); setTransition(false); } });
    // One ahead in the direction of the step.
    const ahead = revisions[next + (next > at ? 1 : -1)];
    if (ahead) cache.get(ahead).catch(() => {});
  };
  useEffect(() => { wanted.current++; setTransition(false); }, [view, current?.path, details]);
  const shownEntry = revisions[at];
  const scrubKey = shownEntry ? `${shownEntry.id}:${parentChoice}` : "";
  useEffect(() => { const i = details?.parents.indexOf(effectiveBase ?? "") ?? -1; setParentChoice(i >= 0 ? i : 0); }, [details?.id, effectiveBase]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!scrubDiff || view !== "file" || !shownEntry) { setScrub(null); return; }
    let live = true;
    const key = scrubKey;
    setScrub({ key });
    loadScrub(shownEntry, parentChoice).then((result) => {
      if (!live) return;
      setScrub({ key, result });
      setParentCount(result.base.kind === "parent" ? result.base.count : 1);
      // One revision ahead on each side, so that stepping shows the next change at once.
      for (const n of [revisions[at + 1], revisions[at - 1]]) if (n) void loadScrub(n, parentChoice).catch(() => {});
    }).catch((e) => { if (live) setScrub({ key, error: String(e) }); });
    return () => { live = false; };
  }, [scrubDiff, view, shownEntry?.id, shownEntry?.file, parentChoice, path]); // eslint-disable-line react-hooks/exhaustive-deps

  // Blame is asked for once it is wanted and the text is on screen; nothing waits for it.
  const revision = revisions[at], shownFile = view === "file" ? (revision ? { commit: revision.id, file: revision.file } : inspectedCommit && current ? { commit: inspectedCommit, file: current.path } : null)
    : view === "diff" && details && current && !deleted ? { commit: details.id, file: current.path } : null;
  const beside = blameMode === "all" && (view === "file" || view === "diff") && !(view === "file" && scrubDiff);
  const blameRequest: BlameRequest | null = loadingPreview || previewError ? null : view === "blame" ? blameTarget && { path, ...blameTarget } : shownFile && (beside || caret != null) ? { path, ...shownFile } : null;
  const blame = useBlame(blameRequest);
  const toggleBlame = () => setBlameMode((mode) => { const next = mode === "all" ? "line" : "all"; localStorage.setItem(BLAME_KEY, next); return next; });
  /** The line of the text under the cursor (0-based), counted in what is shown. */
  const placeCaret = () => {
    const pre = preview.current?.querySelector("pre"), selection = window.getSelection();
    if ((view !== "diff" && view !== "file") || (view === "file" && scrubDiff) || !pre || !selection || selection.rangeCount === 0 || !pre.contains(selection.focusNode)) return;
    const before = document.createRange();
    before.selectNodeContents(pre);
    before.setEnd(selection.focusNode!, selection.focusOffset);
    const shown = before.toString().split("\n").length - 1;
    setCaret(view === "diff" ? shown : Math.max(0, shown - (deleted ? 1 : 0)));
  };
  const numbers = useMemo(() => view === "diff" ? diffLineNumbers(patch.split("\n", 20001)) : null, [view, patch]);
  const caretLine = caret == null || !blame ? undefined : blame.lines[numbers ? numbers[caret] ?? -1 : caret];
  /** The line that Enter and P mean: the cursor of the Blame view, else the clicked line. */
  const blamedLine = view === "blame" ? blame?.lines[blameAt] : caretLine;
  const openBlamed = (line = blamedLine) => { if (line && !uncommitted(line.commit)) onNavigate(line.commit.id); };
  const peel = () => {
    if (!blamedLine) return;
    const before = blamedLine.commit.previous;
    if (!before) { A.notify(uncommitted(blamedLine.commit) ? tt("This line is not committed yet.") : tt("The file begins with this commit.")); return; }
    if (view === "blame") setBlameAt(Math.max(0, blamedLine.original - 1));
    setPeeled((list) => [...list, { commit: before[0], file: before[1] }]);
    setView("blame");
  };
  const openAt = useRef((_: number) => {}); openAt.current = (index) => openBlamed(blame?.lines[index]);
  const openRow = useMemo(() => (index: number) => openAt.current(index), []);
  const moveBlame = (step: number) => setBlameAt((index) => {
    const next = Math.max(0, Math.min(index + step, Math.min(blameText?.length ?? 1, MAX_BLAME) - 1));
    requestAnimationFrame(() => preview.current?.querySelector(`[data-line="${next}"]`)?.scrollIntoView({ block: "nearest" }));
    return next;
  });
  const gutter = (index: number | null) => {
    if (!beside) return null;
    const line = index == null ? undefined : blame?.lines[index];
    return <i className="gcd-gutter" title={line ? blameTitle(line, now) : undefined} onClick={() => openBlamed(line)}>{line ? blameShort(line, now) : ""}</i>;
  };

  /** The lines selected in the preview, as lines of a file in a commit. */
  const chosenLines = (): Lines | null => {
    if (!current || !details || !lineActionsEnabled({ view, transition, scrubDiff })) return null;
    const pre = preview.current?.querySelector("pre"), selection = window.getSelection();
    if (!pre || !selection || selection.rangeCount === 0 || !pre.contains(selection.anchorNode) || !pre.contains(selection.focusNode)) return null;
    const range = selection.getRangeAt(0), before = range.cloneRange();
    before.selectNodeContents(pre);
    before.setEnd(range.startContainer, range.startOffset);
    const found = selectedLines(before.toString(), range.toString());
    if (!found) return null;
    if (view === "diff") {
      const part = diffRange(patchLines, found[0], found[1]);
      if (!part) return null;
      // Removed lines are lines of the file before the change.
      if (part.side === "new") return { commit: details.id, file: current.path, start: part.start, end: part.end };
      return details.base ? { commit: details.base, file: current.original ?? current.path, start: part.start, end: part.end } : null;
    }
    const entry = revisions[at], shown = entry ? { commit: entry.id, file: entry.file } : inspectedCommit ? { commit: inspectedCommit, file: current.path } : null;
    // A deleted file has a line of explanation above it.
    const skip = deleted ? 1 : 0;
    return shown && found[1] >= skip ? { ...shown, start: Math.max(1, found[0] + 1 - skip), end: found[1] + 1 - skip } : null;
  };
  const showLines = async () => {
    // Not while the scrubber moves: the revision on screen is about to change.
    if (transition || scrubDiff) return;
    const chosen = view === "diff" || view === "file" ? chosenLines() : null;
    if (chosen) { setLines(chosen); setView("lines"); return; }
    if (!current || !inspectedCommit) return;
    const typed = await A.promptText(tt("Lines of {file} (for example 10-20)", { file: current.path }), "", "Show history");
    if (typed === null) return;
    const range = parseRange(typed, Number.MAX_SAFE_INTEGER);
    if (!range) { setError(tt("Type lines as 10-20 or 10.")); return; }
    // Typed lines belong to the revision on screen under the name the file had there, like selected ones.
    const shownRevision = view === "file" ? (revisions[at] ? { commit: revisions[at].id, file: revisions[at].file } : undefined) : view === "blame" ? blameTarget ?? undefined : undefined;
    setLines({ commit: shownRevision?.commit ?? inspectedCommit, file: shownRevision?.file ?? current.path, ...range }); setView("lines");
  };

  useEffect(() => { fileList.current?.querySelector(".selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);

  const copy = async () => {
    try { await invoke("clip_set_text", { text: commit }); setCopied(true); }
    catch (e) { setError(String(e)); }
  };
  const copyPath = async () => {
    if (!current) return;
    try { await invoke("clip_set_text", { text: current.path }); A.notify(tt("File path copied")); }
    catch (e) { setError(String(e)); }
  };
  const edit = async () => {
    if (!details || !current) return;
    try {
      const resolved = await api.pathResolve(joinRel(details.root, current.path));
      if (!resolved.exists || resolved.isDir) throw new Error(tt("This historical file is not present in the working folder."));
      A.editPaths([resolved.path]);
    } catch (e) { setError(String(e)); }
  };
  const [checkingOut, setCheckingOut] = useState(false);
  const checkoutPending = useRef(false);
  const detailVersion = useRef(0);
  useEffect(() => () => { detailVersion.current++; }, [path, commit, effectiveBase]);
  const checkoutTarget = view === "file" && revision
    ? { commit: revision.id, file: revision.file }
    : inspectedCommit && current ? { commit: inspectedCommit, file: current.path } : null;
  const checkoutFile = async () => {
    if (!checkoutTarget || checkoutPending.current) return;
    const version = detailVersion.current;
    checkoutPending.current = true;
    setCheckingOut(true);
    setError("");
    try {
      await invoke("git_checkout_commit_file", { path, ...checkoutTarget });
      await onCheckout();
    } catch (error) {
      if (version === detailVersion.current) setError(String(error));
    } finally {
      checkoutPending.current = false;
      if (root.current) setCheckingOut(false);
    }
  };
  useEffect(() => {
    const element = root.current;
    const command = (event: Event) => {
      const key = (event as CustomEvent<string>).detail;
      if (key === "b") setView("blame");
      else if (key === "L") void showLines();
      else if (key === "B") toggleBlame();
      else if (key === "P") peel();
      else if (key === "f") setView("history");
      else if (key === "d") setView("diff");
      else if (key === "v") setView("file");
      else if (key === "Enter") preview.current?.focus();
      else if (key === "y") void copyPath();
      else if (key === "e") void edit();
      else if (key === "c") void checkoutFile();
    };
    element?.addEventListener("gall-git-command", command);
    return () => element?.removeEventListener("gall-git-command", command);
  }); // eslint-disable-line react-hooks/exhaustive-deps
  const move = (direction: number) => setSelected((value) => stepVisible(collapsed, value, direction < 0 ? -1 : 1));
  const patchLines = patch.split("\n", 20001);
  const svg = !!current && /\.svg$/i.test(current.path), showImage = view === "diff" && !!current && isImage(current.path) && (!svg || !asText);
  const totalAdded = details?.files.reduce((sum, file) => sum + (file.added ?? 0), 0) ?? 0;
  const totalDeleted = details?.files.reduce((sum, file) => sum + (file.deleted ?? 0), 0) ?? 0;
  // What really changed on each line of the diff (structural diff); it shows without it.
  const marks = usePatchMarks(current?.path, view === "diff" ? patch : null);

  const loadingViewLabel = { lines: "Loading line history", diff: "Loading diff", file: "Loading file", history: "Loading file history", blame: "Loading blame" } as const;
  const commitLoadProgress = useLocalProgress(loading, "Loading commit");
  const backendPreviewProgress = useGitProgress(
    loadingPreview ? previewProgressId : null,
    { operation: view === "lines" ? "lineHistory" : "fileHistory", phase: loadingViewLabel[view], cancellable: view === "lines" },
  );
  const localPreviewProgress = useLocalProgress(loadingPreview && !previewProgressId, loadingViewLabel[view]);
  const previewProgress = backendPreviewProgress ?? localPreviewProgress;
  const blameProgress = useGitProgress(
    view === "blame" && blame && !blame.done && !blame.error ? blame.progressId : null,
    { operation: "blame", phase: "Blaming file", cancellable: true, detail: blameTarget?.file },
  );
  /** `D` and the `±` button: show the change of each revision, or the file as of the revision on screen. */
  const toggleScrub = () => {
    const on = !scrubDiff;
          setScrubDiff(on);
    // Back to the file: its text as of the revision on screen (it was not read while the change was shown).
    if (!on && revisions[at]) { const ticket = ++wanted.current; setTransition(true); cache.get(revisions[at]).then((text) => { if (ticket === wanted.current) { setPatch(text); setPreviewError(""); setTransition(false); } }).catch((e) => { if (ticket === wanted.current) { setPreviewError(String(e)); setTransition(false); } }); }
  };
  return <section className="git-commit-details" ref={root} aria-label={base ? tt("Commit comparison") : tt("Commit details")} tabIndex={-1} onKeyDown={(e) => {
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    const { key, on } = gitKeys.press(["details"], e);
    // A chord that is no command here belongs to the system (copy, zoom).
    if ((e.ctrlKey || e.metaKey || e.altKey) && !on) return;
    if ((e.ctrlKey || e.metaKey || e.altKey) && key === gitKeyOf(e)) return;
    e.stopPropagation();
    const inPreview = preview.current?.contains(e.target as Node);
    if (vim && key === "h") { e.preventDefault(); if (inPreview) fileList.current?.focus(); else onBack?.(); }
    else if (vim && key === "l") { e.preventDefault(); preview.current?.focus(); }
    else if (on && key === "b") { e.preventDefault(); setView("blame"); }
    else if (on && key === "L") { e.preventDefault(); void showLines(); }
    else if (on && key === "B") { e.preventDefault(); toggleBlame(); }
    else if (on && key === "P") { e.preventDefault(); peel(); }
    else if (key === "Backspace" && view === "blame" && peeled.length) { e.preventDefault(); setPeeled((list) => list.slice(0, -1)); }
    else if (key === "Enter" && inPreview && blamedLine && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); openBlamed(); }
    else if (view === "file" && (key === "[" || key === "]")) { e.preventDefault(); goTo(at + (key === "[" ? 1 : -1)); }
    else if (on && key === "D" && view === "file" && revisions.length) { e.preventDefault(); toggleScrub(); }
    else if (on && key === "f") { e.preventDefault(); setView("history"); }
    else if (on && key === "d") { e.preventDefault(); setView("diff"); }
    else if (on && key === "v") { e.preventDefault(); setView("file"); }
    else if (on && key === "y") { e.preventDefault(); void copyPath(); }
    else if (on && key === "e") { e.preventDefault(); void edit(); }
    else if (on && key === "c") { e.preventDefault(); void checkoutFile(); }
    else if (key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); preview.current?.focus(); }
    else if (e.key === "ArrowDown" || (vim && key === "j")) {
      if (!inPreview) { e.preventDefault(); move(1); }
      else if (view === "blame") { e.preventDefault(); moveBlame(1); }
      else if (key === "j") { e.preventDefault(); preview.current?.scrollBy(0, 24); }
    } else if (e.key === "ArrowUp" || (vim && key === "k")) {
      if (!inPreview) { e.preventDefault(); move(-1); }
      else if (view === "blame") { e.preventDefault(); moveBlame(-1); }
      else if (key === "k") { e.preventDefault(); preview.current?.scrollBy(0, -24); }
    }
  }}>
    <div className="gcd-heading"><div><strong>{base ? tt("Commit comparison") : tt("Commit details")}</strong><button className="gcd-hash" title={tt("Copy full commit ID")} onClick={() => void copy()}>{commit}{copied && ` · ${tt("Copied")}`}</button></div><button onClick={onClose} aria-label={tt("Close commit details")} title={tt("Close commit details")}>✕</button></div>
    {error && <p className="gcd-error" role="alert">{error}</p>}
    {loading ? (commitLoadProgress ? <GitProgressBar progress={commitLoadProgress} compact /> : <p className="gcd-notice" role="status">{tt("Loading commit…")}</p>) : !details ? <p className="gcd-notice">{tt("Commit could not be loaded.")}</p> : <>
      <div className="gcd-metadata"><div className="gcd-identity"><span><b>{details.author}</b> <span className="gcd-dim">&lt;{details.email}&gt;</span></span><time dateTime={details.date}>{new Date(details.date).toLocaleString(formatLocale())}</time><span className="gcd-dim" title={tt("Committed {date}", { date: details.commitDate })}>{tt("Committer:")} {details.committer}</span></div><pre className="gcd-message">{details.message}</pre><div className="gcd-parents"><span className="gcd-dim">{details.parents.length ? tt("Parents") : tt("Root commit")}</span>{details.parents.map((id) => <button key={id} title={id} onClick={() => onNavigate(id)}>{id.slice(0, 8)}</button>)}{onAction && <button className="gcd-actions" onClick={() => onAction("menu")}>{tt("Commit actions…")}</button>}</div></div>
      <div className="gcd-toolbar"><span className="gcd-count">{tt(details.files.length === 1 ? "{count} file" : "{count} files", { count: details.files.length })} <span className="gcd-added">+{totalAdded}</span> <span className="gcd-removed">−{totalDeleted}</span></span>{base ? <span className="gcd-base" title={`${base} → ${commit}`}>{base.slice(0, 8)} → {commit.slice(0, 8)}</span> : details.parents.length > 1 ? <label>{tt("Against parent")} <ThemedSelect aria-label={tt("Comparison parent")} title={tt("Parent commit to compare against")} value={effectiveBase ?? details.parents[0]} onChange={(e) => setParent({ commit, id: e.target.value })}>{details.parents.map((id, index) => <option key={id} value={id}>{tt("Parent {index} · {hash}", { index: index + 1, hash: id.slice(0, 8) })}</option>)}</ThemedSelect></label> : <span className="gcd-base">{details.base ? tt("Against {hash}", { hash: details.base.slice(0, 8) }) : tt("Against empty tree")}</span>}<div className="gcd-tabs" role="group" aria-label={tt("File views")}>{([ ["diff", "Diff", "d"], ["file", "File", "v"], ["history", "History", "f"], ["blame", "Blame", "b"], ...(lines ? [["lines", "Lines", "L"]] as const : []) ] as const).map(([id, label, key]) => <button key={id} disabled={!current} aria-pressed={view === id} className={view === id ? "on" : ""} onClick={() => setView(id)}>{tt(label)}<GitKeyCap scope="details" k={key} /></button>)}</div><button disabled={!checkoutTarget || checkingOut} onClick={() => void checkoutFile()}>{tt("Check out file")}<GitKeyCap scope="details" k="c" /></button></div>
      {details.files.length === 0 ? <p className="gcd-notice">{tt("No file changes between these revisions.")}</p> : <div className="gcd-body"><div className="gcd-files" ref={fileList} tabIndex={0} aria-label={tt("Commit files")}>{details.files.map((file, index) => <Fragment key={file.path}>{!collapsed[index] && <button className={`gcd-file ${index === selected ? "selected" : ""}`} title={file.original ? `${file.original} → ${file.path}` : file.path} onFocus={() => setSelected(index)} onClick={() => { setSelected(index); fileList.current?.focus(); }}><span className={`gcd-status gcd-status-${file.status[0]}`}>{file.status[0]}</span><span className="gcd-file-path">{file.original && <small>{file.original} → </small>}{file.path}</span><span className="gcd-file-count">{file.added === null ? tt("Binary") : <><span className="gcd-added">+{file.added}</span> <span className="gcd-removed">−{file.deleted}</span></>}</span></button>}{index === details.files.length - 1 && generatedCount > 0 && <div className="gc-generated" role="status"><span>{revealedGenerated ? tt("{count} generated files shown", { count: generatedCount }) : tt("{count} generated files collapsed ({reasons})", { count: generatedCount, reasons: generatedReasons.map((r) => tt(GENERATED_REASON[r])).join(", ") })}</span> <button className="gg-link" onClick={() => setRevealedGenerated((v) => !v)}>{revealedGenerated ? tt("hide them") : tt("show them")}</button></div>}</Fragment>)}</div><div className="gcd-preview" ref={preview} tabIndex={0} aria-label={view === "diff" ? tt("Commit file diff") : view === "file" ? tt("Historical file") : view === "history" ? tt("File history") : view === "lines" ? tt("Line history") : tt("File blame")} onClick={placeCaret} onKeyUp={(e) => { if (e.key.startsWith("Arrow")) placeCaret(); }} onContextMenu={(e) => { if ((view !== "diff" && view !== "file") || !current) return; e.preventDefault(); e.stopPropagation(); A.showMenuAt(eventAnchor(e), [{ label: tt("History of the selected lines…"), disabled: transition || scrubDiff, run: () => void showLines() }, { label: tt("Blame beside every line"), checked: blameMode === "all", run: toggleBlame }, { label: tt("Copy"), disabled: !window.getSelection()?.toString(), run: () => { void invoke("clip_set_text", { text: window.getSelection()?.toString() ?? "" }).catch((err) => setError(String(err))); } }]); }}>{view === "file" && historyError && <div className="gcd-scrub"><span className="gcd-error" role="alert">{historyError}</span></div>}{view === "file" && revisions.length > 1 && <div className="gcd-scrub" aria-busy={transition}><button title={tt("Older revision ([)")} aria-label={tt("Older revision")} disabled={at >= revisions.length - 1} onClick={() => goTo(at + 1)}>‹</button><input type="range" min={0} max={revisions.length - 1} value={revisions.length - 1 - at} aria-label={tt("Revision of the file")} aria-valuetext={revisions[at]?.subject} onChange={(e) => goTo(revisions.length - 1 - Number(e.target.value))} /><button title={tt("Newer revision (])")} aria-label={tt("Newer revision")} disabled={at === 0} onClick={() => goTo(at - 1)}>›</button><button className="gcd-scrub-at" title={`${revisions[at].id}\n${revisions[at].author} · ${new Date(revisions[at].date).toLocaleString(formatLocale())}\n${tt("Show this commit")}`} onClick={() => onNavigate(revisions[at].id)}><code>{revisions[at].id.slice(0, 8)}</code> {revisions[at].subject}</button><small className="gcd-scrub-meta">{revisions[at].author} · {new Date(revisions[at].date).toLocaleDateString(formatLocale())}</small>{scrubDiff && (revisions[at].parents?.length ?? parentCount) > 1 && <select aria-label={tt("Compare with parent")} value={Math.min(parentChoice, (revisions[at].parents?.length ?? parentCount) - 1)} onChange={(e) => setParentChoice(Number(e.target.value))}>{Array.from({ length: revisions[at].parents?.length ?? parentCount }, (_, i) => <option key={i} value={i}>{tt("Parent {n}", { n: i + 1 })}</option>)}</select>}<button aria-pressed={scrubDiff} aria-label={tt("Show the change of each revision against its parent")} title={tt("Show the change of each revision against its parent")} onClick={toggleScrub}>±</button><small>{at + 1} / {revisions.length}</small></div>}
        {loadingPreview ? (previewProgress ? <GitProgressBar progress={previewProgress} compact onCancel={view === "lines" && lineHistoryTicket ? () => { void invoke("git_line_history_cancel", { ticket: lineHistoryTicket }).catch(() => {}); setLoadingPreview(false); setPreviewProgressId(null); } : undefined} /> : <p className="gcd-notice" role="status">{tt(loadingViewLabel[view])}</p>) : previewError && !(view === "file" && scrubDiff) ? <p className="gcd-error" role="alert">{previewError}</p> : showImage ? <>{svg && <button className="gid-toggle" onClick={() => setAsText(true)}>{tt("Show as text")}</button>}<GitImageDiff path={path} sides={commitSides(current!, details.id, effectiveBase ?? details.base)} /></> : view === "lines" ? <div className="gcd-lines"><p className="gcd-notice">{lines && tt("Lines {start}–{end} of {file}", { start: lines.start, end: lines.end, file: lines.file })}</p>{lineHistory?.entries.map((entry) => <section key={entry.id}><button className="gcd-history" title={entry.id} onClick={() => onNavigate(entry.id)}><code>{entry.id.slice(0, 8)}</code><span>{entry.subject}</span><small>{entry.author} · {new Date(entry.date).toLocaleDateString(formatLocale())}</small></button><pre className="gcd-patch">{entry.patch.slice(Math.max(0, entry.patch.search(/^@@ /m))).split("\n").map((line, i) => <span key={i} className={line.startsWith("+") ? "gcd-added" : line.startsWith("-") ? "gcd-removed" : line.startsWith("@@") ? "gcd-hunk" : ""}>{line}{"\n"}</span>)}</pre></section>)}{lineHistory && !lineHistory.entries.length && <p className="gcd-notice">{tt("No commit changed these lines.")}</p>}{lineHistory?.truncated && <p className="gcd-notice">{tt("Only the newest {count} commits are shown.", { count: lineHistory.entries.length })}</p>}</div> : view === "history" ? history.length === 0 ? <p className="gcd-notice">{tt("No file history at this revision.")}</p> : history.map((entry) => <button className="gcd-history" key={entry.id} title={entry.id} onClick={() => onNavigate(entry.id)}><code>{entry.id.slice(0, 8)}</code><span>{entry.subject}</span><small>{entry.author} · {entry.date.slice(0, 10)}</small></button>) : view === "blame" ? <div className="gcd-blame">{blameProgress && <GitProgressBar progress={blameProgress} compact onCancel={blame?.cancel} />}{peeled.length > 0 && <p className="gcd-notice gcd-peeled"><button onClick={() => setPeeled((list) => list.slice(0, -1))}>{tt("Back")}</button> {tt("Blame as of {hash} · {file}", { hash: blameTarget?.commit.slice(0, 8) ?? "", file: blameTarget?.file ?? "" })}</p>}{deleted && !peeled.length && <p className="gcd-notice">{tt("Showing the file before deletion.")}</p>}{blame?.error ? <p className="gcd-error" role="alert">{blame.error}</p> : blameText?.length === 0 ? <p className="gcd-notice">{tt("This file is empty.")}</p> : blameText?.slice(0, MAX_BLAME).map((text, index) => <BlameRow key={index} line={blame?.lines[index]} text={text} index={index} current={index === blameAt} now={now} onOpen={openRow} onPick={setBlameAt} />)}{(blame?.limited || (blameText?.length ?? 0) > MAX_BLAME) && <p className="gcd-notice">{tt("Blame preview limited to 10,000 lines.")}</p>}{blame?.ignored && <p className="gcd-notice">{tt("Commits listed in .git-blame-ignore-revs are skipped.")}</p>}</div> : <>{svg && view === "diff" && <button className="gid-toggle" onClick={() => setAsText(false)}>{tt("Show as image")}</button>}<pre className="gcd-patch">{deleted && view === "file" && <span className="gcd-dim">{tt("[File before deletion]")}{"\n"}</span>}{view === "file" && scrubDiff ? (scrub && scrub.key === scrubKey && scrub.error ? <span className="gcd-error" role="alert">{scrub.error}{"\n"}</span> : !scrub?.result || scrub.key !== scrubKey ? <span className="gcd-dim">{tt("Loading the change…")}{"\n"}</span> : <>
          <span className="gcd-dim">{scrub.result.base.kind === "root" ? tt("First commit: compared with an empty tree") : scrub.result.base.count > 1 ? tt("Merge: compared with parent {n} of {count}", { n: scrub.result.base.index + 1, count: scrub.result.base.count }) : tt("Compared with its parent")}{"\n"}</span>
          {scrub.result.none ? <span className="gcd-dim">{tt("This revision does not change the file against this parent.")}{"\n"}</span> : scrub.result.text.replace(/\n$/, "").split("\n", 20001).slice(0, 20000).map((line, index) => <span key={index} className={line.startsWith("+") && !line.startsWith("+++") ? "gcd-added" : line.startsWith("-") && !line.startsWith("---") ? "gcd-removed" : line.startsWith("@@") ? "gcd-hunk" : ""}>{line}{"\n"}</span>)}{scrub.result.text.split("\n", 20002).length > 20001 && <span className="gcd-dim">{tt("[Diff preview limited to 20,000 lines]")}</span>}</>) : patch ? view === "diff" ? <>{patchLines.slice(0, 20000).map((line, index) => <span key={index} title={formattingOnly(marks, index) ? tt("Only the formatting of this line differs") : undefined} className={`${line.startsWith("+") && !line.startsWith("+++") ? "gcd-added" : line.startsWith("-") && !line.startsWith("---") ? "gcd-removed" : line.startsWith("@@") ? "gcd-hunk" : ""} ${formattingOnly(marks, index) ? "gcd-fmt" : ""}`}>{gutter(numbers?.[index] ?? null)}<PatchText text={line} spans={marks?.[index]} />{"\n"}</span>)}{patchLines.length > 20000 && <span className="gcd-dim">{tt("[Diff preview limited to 20,000 lines]")}</span>}</> : beside ? patch.replace(/\n$/, "").split("\n", MAX_BLAME).map((line, index) => <span key={index}>{gutter(index)}{line}{"\n"}</span>) : patch : view === "file" ? tt("Empty file") : tt("No textual difference")}</pre></>}
      </div></div>}
      {(view === "diff" || view === "file") && caretLine && <div className="gcd-caret"><button title={blameTitle(caretLine, now)} disabled={uncommitted(caretLine.commit)} onClick={() => openBlamed(caretLine)}><code>{caretLine.commit.id.slice(0, 8)}</code> {blameShort(caretLine, now)} · {caretLine.commit.subject}</button><button onClick={peel} disabled={!caretLine.commit.previous}>{tt("Before this commit")}</button></div>}
      <div className="gcd-foot">{vim ? `${gitKeys.hints("details").map((hint) => `${hint.name} ${tt(hint.label)}`).join(" · ")} · ${tt("h/l focus · Esc close details")}` : tt("Arrow keys select files · Tab selects controls · Esc closes details")}</div>
    </>}
  </section>;
}
