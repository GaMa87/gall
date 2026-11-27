// Compare view: side-by-side or unified line diff with word-level highlights,
// folded unchanged regions, an overview ruler and keyboard navigation between
// changes. Binary files get a byte-level verdict.
import { nextRequestId } from "../core/requestId";
import { maxOf } from "../core/maxOf";
import { tt } from "../i18n";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { AiExplainScope, CompareSummary, DiffOp, DiffOptions, DiffResult, DiffSource, Granularity, PatchFormat, SemanticVerdict, SyntaxHighlight, WhitespaceMode, Generated, OutlineResult, AiOutcome } from "../api/types";
import { CompareExplanation } from "./CompareExplanation";
import { isSnapshotCurrent, type ExplainSnapshot } from "../core/explain";
import { CompareOutline } from "./CompareOutline";
import { SemanticDetails } from "./SemanticDetails";
import { CompareTabsBar } from "./CompareTabsBar";
import { findTab, getCompareTabsState, neighbourTab, updateTabState, useCompareTabs } from "../state/compareTabs";
import { rowOfAnchor, scrollAnchor, type AnchorRow, type ScrollAnchor } from "../core/compareSession";
import { tabKey } from "../core/compareTabs";
import { finishCompareWait, useCompareWaitClose } from "../state/launch";
import { collapsedKeyBehavior, GENERATED_REASON, outlineErrorMessage, suppressed } from "../core/outline";
import { activeFilters, defaultOpts, migrateOpts, parseRules } from "../core/diffOptions";
import { applyBlock, buildSplit, buildUnified, indexDiffRows, joinLines, moveTarget, offsets, nextMoveEnd, rowAt, rulerBuckets, segments, wrappedLines, type Cell, type RulerMark, type Spans, type SplitRow, type SyntaxRuns, type UnifiedRow } from "../core/diffRows";
import { blameShort, blameTitle, useBlame, type BlameRequest } from "../state/gitBlame";
import { uncommitted } from "../core/gitBlame";
import { refreshAll } from "../state/controllers";
import { primaryMod } from "../core/keymap";
import { formatSize } from "../core/format";
import { basename, join, parent, tildify } from "../core/path";
import * as A from "../state/actions";
import { useStore, type Overlay } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { DocumentCompare } from "./DocumentCompare";
import { HexDiff } from "./HexDiff";
import { ImageCompare, isImage } from "./ImageCompare";
import { isTable, TableDiff } from "./TableDiff";
import { isSameDocumentKind } from "../core/documentBlocks";

const CONTEXT = 3;
const LINE_PAGE = 64;
let compareRequestId = Date.now() * 1024;
const OPTS_KEY = "diff:opts";
const PATCH_LABEL: Record<PatchFormat, string> = { unified: "Unified patch", context: "Context patch", normal: "Normal patch" };
const MODE_KEY = "diff:mode";
const WRAP_KEY = "diff:wrap";
const BLAME_COLS = 24;
function loadOpts(): DiffOptions {
  try {
    return migrateOpts(JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}") as Record<string, unknown>);
  } catch {
    return defaultOpts;
  }
}

const srcName = (s: DiffSource) => (s.kind === "text" ? s.label : s.kind === "gitBlob" ? basename(s.file) : basename(s.path));
const srcPath = (s: DiffSource) => (s.kind === "text" || s.kind === "gitBlob" ? "" : s.path);

function Text({ text, spans, sx, kind }: { text: string; spans?: Spans; sx?: SyntaxRuns; kind: "del" | "ins" | "chg" | "eq" }) {
  return (
    <>
      {segments(text, spans, sx).map(([t, hi, cat], i) => {
        const inner = cat ? <span className={`sx${cat}`}>{t}</span> : t;
        return hi ? <mark key={i} className={`dv-word ${kind}`}>{inner}</mark> : <span key={i}>{inner}</span>;
      })}
      {text === "" && "​"}
    </>
  );
}

export function DiffView({ a: a0, b: b0, back, wait }: { a: DiffSource; b: DiffSource; back?: Overlay; wait?: boolean }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const home = useStore((s) => s.info?.home);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const generatedPatternsKey = (useStore((s) => s.settings?.generatedPatterns) ?? []).join("\n");
  const { tabs: openCompareTabs } = useCompareTabs();
  const initialTab = useMemo(() => (wait ? undefined : findTab(a0, b0)), [wait, a0, b0]);
  const [src, setSrc] = useState<[DiffSource, DiffSource]>([a0, b0]);
  const [opts, setOpts] = useState<DiffOptions>(() => (
    initialTab?.options && Object.keys(initialTab.options).length > 0
      ? migrateOpts({ ...initialTab.options })
      : loadOpts()
  ));
  // The More options panel; its rule boxes are applied when left, not on every key.
  const [more, setMore] = useState(false);
  const [lineRules, setLineRules] = useState(() => (opts.ignoreLines ?? []).join("\n"));
  const [textRules, setTextRules] = useState(() => (opts.ignoreText ?? []).join("\n"));
  const [mode, setMode] = useState<"split" | "unified">(() => initialTab?.mode ?? (localStorage.getItem(MODE_KEY) === "unified" ? "unified" : "split"));
  const pendingRestore = useRef<{ scroll?: ScrollAnchor | null; activeHunk?: number | null } | null>(
    initialTab && (initialTab.scroll != null || initialTab.activeHunk != null)
      ? { scroll: initialTab.scroll, activeHunk: initialTab.activeHunk }
      : null,
  );
  const [exporting, setExporting] = useState<"html" | "pdf" | null>(null);
  const gPending = useRef(false);
  const gTimer = useRef<number | null>(null);
  useEffect(() => () => { if (gTimer.current != null) window.clearTimeout(gTimer.current); }, []);
  const [fold, setFold] = useState(true);
  // Wrap long lines (default): no sideways scrolling when one line is very long.
  const [wrap, setWrap] = useState(() => localStorage.getItem(WRAP_KEY) !== "0");
  const [bodyW, setBodyW] = useState(1000);
  const [charW, setCharW] = useState(8);
  const probeRef = useRef<HTMLSpanElement>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [res, setRes] = useState<CompareSummary | null>(null);
  const [tx, setText] = useState<DiffResult["text"]>(null);
  const [paged, setPaged] = useState<{ id: number; ops: DiffOp[] } | null>(null);
  const [visibleLines, setVisibleLines] = useState<{ id: number; left: Map<number, string>; right: Map<number, string> } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [cur, setCur] = useState(-1);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const bodyRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const rowH = Math.round(fontSize * 1.55);
  // Blame of the right side, off until asked for (B, or the pointer on a line number).
  const [blameAll, setBlameAll] = useState(false), [blameWanted, setBlameWanted] = useState(false);
  const opened = useRef(Date.now());
  // Merge edits held in memory until saved (null = side unchanged from its source).
  const [allowChanges, setAllowChanges] = useState(false);
  const allowChangesRef = useRef(false);
  const permissionVersion = useRef(0);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [edits, setEdits] = useState<[string[] | null, string[] | null]>([null, null]);
  const [history, setHistory] = useState<[string[] | null, string[] | null][]>([]);
  const meta = useRef<[{ crlf: boolean; finalNewline: boolean; revision?: string }, { crlf: boolean; finalNewline: boolean; revision?: string }]>([
    { crlf: false, finalNewline: true }, { crlf: false, finalNewline: true }]);
  const keepView = useRef(false);
  // CSV / spreadsheets get the table view (CSV can switch to plain text).
  const [tableView, setTableView] = useState(true);
  const bothTables = src[0].kind === "file" && src[1].kind === "file" && isTable(src[0].path) && isTable(src[1].path);
  const tableMode = bothTables && tableView;
  // Two SVG files are text, but their picture is what is compared first: the image modes by default, the text diff one click away.
  // The format of Copy patch and Save patch, kept between windows.
  const [patchFormat, setPatchFormat] = useState<PatchFormat>(() => { const saved = localStorage.getItem("diff:patchFormat"); return saved === "context" || saved === "normal" ? saved : "unified"; });
  useEffect(() => { localStorage.setItem("diff:patchFormat", patchFormat); }, [patchFormat]);
  // Two Word documents (.docx) or two PDFs open in the positioned block comparison first,
  // with one-click fallback to the line-by-line text comparison or byte comparison on error.
  const [docView, setDocView] = useState<"document" | "binary">("document");
  const bothDocs = src[0].kind === "file" && src[1].kind === "file" && isSameDocumentKind(src[0].path, src[1].path);
  const docMode = bothDocs && docView === "document";
  const [svgView, setSvgView] = useState<"image" | "text">("image");
  const bothSvg = src[0].kind === "file" && src[1].kind === "file" && /\.svg$/i.test(src[0].path) && /\.svg$/i.test(src[1].path);
  const svgImage = bothSvg && svgView === "image";
  /** A view of its own (a table, a document, a picture) instead of the line diff: the controls of the line diff are hidden. */
  const richMode = tableMode || svgImage || docMode;
  const binaryFilePair = !richMode && !!(res?.binary || (res?.limit && res.limit !== "minimalWorkLimit")) && src[0].kind === "file" && src[1].kind === "file";
  const binaryImageMode = binaryFilePair && src[0].kind === "file" && src[1].kind === "file" && isImage(src[0].path) && isImage(src[1].path);
  const hexMode = binaryFilePair && !binaryImageMode;
  const imageMode = svgImage || binaryImageMode;
  const csvOnly = bothTables && ![src[0], src[1]].some((x) => x.kind === "file" && /\.(xlsx|xlsm|xls|xlsb|ods)$/i.test(x.path));
  const dirty = !!(edits[0] || edits[1]);
  const editable = (i: 0 | 1) => src[i].kind === "file";
  const eff = useCallback((i: 0 | 1): DiffSource => {
    const e = edits[i];
    return e ? { kind: "text", label: srcName(src[i]), text: joinLines(e, meta.current[i].crlf, meta.current[i].finalNewline) } : src[i];
  }, [edits, src]);

  // Only the latest comparison's reply counts: options toggled quickly (w, i, e…) or merges
  // in a row start several, and an older reply arriving last showed the wrong diff.
  const runSeq = useRef(0);
  const run = useCallback(() => {
    setLoading(true);
    setErr(null);
    const keep = keepView.current;
    keepView.current = false;
    const my = ++compareRequestId;
    runSeq.current = my;
    api.diffPageOpen(my, eff(0), eff(1), opts)
      .then(async (r) => {
        if (my !== runSeq.current) return;
        const text = r.text && await api.diffPageSmall(my);
        if (my !== runSeq.current) return;
        const ops: DiffOp[] = [];
        if (r.text && !text) {
          let start: number | null = 0;
          while (start !== null) {
            const page = await api.diffPageHunks(my, start, 256);
            if (my !== runSeq.current) return;
            ops.push(...page.items);
            start = page.next;
          }
        }
        if (!edits[0]) meta.current[0] = { crlf: r.left.crlf, finalNewline: r.left.finalNewline, revision: r.left.revision };
        if (!edits[1]) meta.current[1] = { crlf: r.right.crlf, finalNewline: r.right.finalNewline, revision: r.right.revision };
        setRes(r);
        setText(text);
        setPaged(r.text && !text ? { id: my, ops } : null);
        if (!keep) { setCur(-1); setExpanded(new Set()); }
      })
      .catch((e) => { if (my === runSeq.current) setErr(String(e)); })
      .finally(() => { if (my === runSeq.current) setLoading(false); });
  }, [eff, edits, opts]);
  useLayoutEffect(run, [run]);
  useEffect(() => () => {
    const through = runSeq.current;
    runSeq.current = 0;
    void api.diffPageClose(through).catch(() => {});
  }, []);
  useEffect(() => { localStorage.setItem(OPTS_KEY, JSON.stringify(opts)); }, [opts]);
  useEffect(() => { localStorage.setItem(MODE_KEY, mode); }, [mode]);
  useEffect(() => { localStorage.setItem(WRAP_KEY, wrap ? "1" : "0"); }, [wrap]);
  useEffect(() => { rootRef.current?.focus(); }, []);

  const textSummary = res?.text;

  // ── syntax colours: fetched in the background after the diff is shown ──
  const [hl, setHl] = useState<[SyntaxHighlight | null, SyntaxHighlight | null]>([null, null]);
  const hlFor = useRef<[string, string[]][]>([["", []], ["", []]]);
  const hlReq = useRef([0, 0]);
  useEffect(() => {
    if (!tx) return;
    // Clipboard text has no name: colour it as the file on the other side.
    const nameOf = (i: 0 | 1) => (src[i].kind === "text" && src[1 - i].kind !== "text" ? srcName(src[1 - i]) : srcName(src[i]));
    const t = setTimeout(() => {
      ([0, 1] as const).forEach((i) => {
        const lines = i === 0 ? tx.leftLines : tx.rightLines;
        const name = nameOf(i);
        const [pn, pl] = hlFor.current[i];
        if (pn === name && pl.length === lines.length && pl.every((l, k) => l === lines[k])) return;
        hlFor.current[i] = [name, lines];
        const id = ++hlReq.current[i];
        if (pn !== name) setHl((h) => (i === 0 ? [null, h[1]] : [h[0], null]));
        api.diffHighlight(name, lines)
          .then((r) => { if (hlReq.current[i] === id) setHl((h) => (i === 0 ? [r, h[1]] : [h[0], r])); })
          .catch(() => {});
      });
    }, 100);
    return () => clearTimeout(t);
  }, [tx, src]);
  const built = useMemo(() => {
    if (!tx) return null;
    const o = { context: fold ? CONTEXT : Infinity, expanded, moves: tx.moves, marks: { a: tx.leftMarks, b: tx.rightMarks } };
    return mode === "split" ? buildSplit(tx.ops, tx.pairs, o) : buildUnified(tx.ops, tx.pairs, o);
  }, [tx, mode, fold, expanded]);
  const compact = useMemo(() => paged && indexDiffRows(paged.ops, mode, {
    context: fold ? CONTEXT : Infinity, expanded,
  }), [paged, mode, fold, expanded]);
  const rulerBlocks = useMemo(() => {
    if (!compact) return [];
    const step = Math.max(1, Math.ceil(compact.blocks.length / 100));
    return compact.blocks.filter((_, index) => index % step === 0);
  }, [compact]);
  const rows = built?.rows ?? [];
  const blocks = useMemo(() => compact?.blocks.map((block) => block.start) ?? built?.blocks ?? [], [compact, built]);
  const blockStart = useMemo(() => new Map(blocks.map((r, k) => [r, k])), [blocks]);
  const blockOfRow = useMemo(() => {
    const m = new Map<number, number>();
    rows.forEach((r, i) => { if (r.kind === "line" && r.block != null) m.set(i, r.block); });
    return m;
  }, [rows]);

  const savedScrollTop = useRef(0);
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const measure = () => {
      // A hidden (collapsed) body measures 0×0: keep the previous dimensions so wrapping never collapses to 10 cols.
      if (el.clientWidth > 0 && el.clientHeight > 0) {
        setViewH(el.clientHeight);
        setBodyW(el.clientWidth);
      }
      const p = probeRef.current;
      if (p && p.offsetWidth) setCharW(p.getBoundingClientRect().width / 100);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [res, fontSize]);

  const lastMoveEnd = useRef<{ row: number; top: number } | null>(null);
  const goBlock = (i: number) => {
    if (!blocks.length) return;
    const k = (i + blocks.length) % blocks.length;
    setCur(k);
    const el = bodyRef.current;
    if (el) el.scrollTop = Math.max(0, topAt(blocks[k]) - el.clientHeight / 3);
  };
  const next = () => goBlock(cur < 0 ? blocks.findIndex((r) => topAt(r) > scrollTop + 4) : cur + 1);
  const prev = () => goBlock(cur < 0 ? blocks.length - 1 : cur - 1);

  // ── merge ──
  const blockCopy = useRef(false);
  const applyBlock_ = async (dir: "toRight" | "toLeft", k = cur) => {
    if (!allowChangesRef.current || savingRef.current || loading || err || blockCopy.current) return;
    const permission = permissionVersion.current;
    const target = dir === "toRight" ? 1 : 0;
    if (!editable(target)) return A.notify(`The ${target ? "right" : "left"} side is read-only`, "warn");
    const block = Math.max(0, k);
    const op = compact?.blocks[block]?.op ?? tx?.ops.filter((o) => o.tag !== "equal" && !o.cosmetic)[block];
    if (!op) return;
    let changed: string[];
    if (tx) {
      const result = applyBlock(tx.leftLines, tx.rightLines, op, dir);
      changed = target === 0 ? result.left : result.right;
    } else if (paged && textSummary) {
      const id = paged.id;
      const read = async (side: "left" | "right", begin: number, end: number) => {
        const lines: string[] = [];
        let start: number | null = begin;
        while (start != null && start < end && id === runSeq.current) {
          const page = await api.diffPageLines(id, side, start, Math.min(256, end - start));
          for (const line of page.items) lines.push(line);
          start = page.next;
        }
        return lines;
      };
      blockCopy.current = true;
      try {
        const dst = target === 0 ? "left" : "right";
        const src = target === 0 ? "right" : "left";
        const [at, len, from, count] = target === 0
          ? [op.a, op.aLen, op.b, op.bLen]
          : [op.b, op.bLen, op.a, op.aLen];
        const [before, replacement, after] = await Promise.all([
          read(dst, 0, at),
          read(src, from, from + count),
          read(dst, at + len, textSummary[dst].lines),
        ]);
        if (id !== runSeq.current || !allowChangesRef.current || permission !== permissionVersion.current) return;
        changed = before.concat(replacement, after);
      } catch (error) {
        if (id === runSeq.current) A.fail(error);
        return;
      } finally {
        blockCopy.current = false;
      }
    } else {
      return;
    }
    setHistory((h) => [...h, edits]);
    keepView.current = true;
    setEdits(target === 0 ? [changed, edits[1]] : [edits[0], changed]);
  };
  const undoEdit = () => {
    if (blockCopy.current || !history.length) return;
    keepView.current = true;
    setEdits(history[history.length - 1]);
    setHistory((h) => h.slice(0, -1));
  };
  const leavePrompt = useRef(false);
  const confirmLeave = async () => {
    if (blockCopy.current || savingRef.current) {
      A.notify(tt("Wait for the comparison to finish before switching tabs or closing it."), "info");
      return false;
    }
    if (!dirty) return true;
    if (leavePrompt.current) return false;
    leavePrompt.current = true;
    try {
      return await A.confirm("Discard unsaved changes?", "Blocks you copied between the files have not been saved.", "Discard");
    } finally {
      leavePrompt.current = false;
    }
  };
  const saveEdits = async () => {
    if (!allowChangesRef.current || savingRef.current || blockCopy.current || !dirty) return;
    savingRef.current = true;
    setSaving(true);
    try {
      let undoable = true;
      const undoIds: number[] = [];
      for (const i of [0, 1] as const) {
        const e = edits[i], s = src[i];
        if (e && s.kind === "file") {
          const saved = await api.saveTextWithUndo(s.path, joinLines(e, meta.current[i].crlf, meta.current[i].finalNewline), meta.current[i].revision);
          undoable = saved.undoable && undoable;
          if (saved.undoId != null) undoIds.push(saved.undoId);
          // Saved: this side is the file again, and a retry after a failure of the other side must not
          // be checked against the revision from before this save.
          setEdits((prev) => (i === 0 ? [null, prev[1]] : [prev[0], null]));
        }
      }
      if (undoable) A.notify("Saved. The previous version is in the trash; Undo restores it.", "success", undoIds.length ? { label: "Undo", run: () => void A.undoRecords(undoIds) } : undefined);
      else A.notify("Saved. The trash isn't available here, so the previous version can't be restored.", "warn");
      setHistory([]);
      keepView.current = true;
      setEdits([null, null]);
      A.refreshUndo();
      refreshAll();
    } catch (e) {
      A.fail(e);
      // One side may be saved, or the file changed on disk: show what is there now, keep unsaved edits.
      keepView.current = true;
      run();
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
    A.invalidateAllFolderSizes(); // also after a failure: one side may be saved
  };

  const copyPatch = async () => {
    try {
      await api.clipSetText(await api.diffPatch(eff(0), eff(1), patchFormat));
      A.notify(tt("Patch copied ({format})", { format: tt(PATCH_LABEL[patchFormat]) }), "success");
    } catch (e) { A.fail(e); }
  };
  // The patch of the text on screen, in the apply dialog: below the folder of the left file by default (where it turns the
  // left side into the right), any other folder chosen there. Closing the dialog comes back to this comparison.
  const applyPatch = async () => {
    if (wait) return;
    try {
      const text = await api.diffPatch(eff(0), eff(1), patchFormat);
      if (!text.trim()) { A.notify(tt("The two sides are the same: there is no patch to apply"), "info"); return; }
      if (!(await confirmLeave())) return;
      const from = srcPath(src[0]) || srcPath(src[1]);
      A.openOverlay({ type: "applyPatch", root: (from && parent(from)) || undefined, text, back: { type: "diff", a: src[0], b: src[1], back } });
    } catch (e) { A.fail(e); }
  };
  const savePatch = async () => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const dir = srcPath(src[1]) ? parent(srcPath(src[1])) : null;
    const file = `${srcName(src[0])}.patch`;
    const dest = await save({ defaultPath: dir ? join(dir, file) : file, filters: [{ name: "Patch", extensions: ["patch", "diff"] }] });
    if (!dest) return;
    // The same sources as Copy patch: the current text of each side, unsaved edits included, not the files on disk.
    api.diffSavePatch(eff(0), eff(1), dest, patchFormat).then(() => A.notify(`Saved ${basename(dest)}`, "success")).catch(A.fail);
  };
  const exportReport = useCallback(async (format: "html" | "pdf", destOverride?: string) => {
    if (exporting) return;
    const activeKey = tabKey(a0, b0);
    const currentTabs = wait ? [] : getCompareTabsState().tabs;
    const pairs: [DiffSource, DiffSource][] = currentTabs.length > 0
      ? currentTabs.map((t) => (tabKey(t.a, t.b) === activeKey ? [eff(0), eff(1)] : [t.a, t.b]))
      : [[eff(0), eff(1)]];
    let dest = destOverride;
    if (!dest) {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const dir = srcPath(src[1]) ? parent(srcPath(src[1])) : srcPath(src[0]) ? parent(srcPath(src[0])) : null;
      const ext = format === "html" ? "html" : "pdf";
      const file = pairs.length === 1 ? `${srcName(src[0])}-compare.${ext}` : `comparison-report.${ext}`;
      const picked = await save({
        defaultPath: dir ? join(dir, file) : file,
        filters: [{ name: format.toUpperCase(), extensions: format === "html" ? ["html", "htm"] : ["pdf"] }],
      });
      if (!picked) return;
      dest = picked;
    }
    setExporting(format);
    let cancelled = false;
    const toastId = A.notify(
      tt(format === "html" ? "Exporting HTML report ({count})…" : "Exporting PDF report ({count})…", { count: pairs.length }),
      "info",
      { label: tt("Cancel"), run: () => { cancelled = true; void api.compareReportCancel().catch(() => {}); } },
      undefined,
      undefined,
      true,
    );
    const title = pairs.length === 1
      ? `${srcName(src[0])} ↔ ${srcName(src[1])}`
      : tt("Comparison report ({count})", { count: pairs.length });
    try {
      if (format === "html") await api.compareReportHtml(dest, title, pairs, opts);
      else await api.compareReportPdf(dest, title, pairs, opts);
      A.dismiss(toastId);
      A.notify(tt("Exported {name}", { name: basename(dest) }), "success");
    } catch (e) {
      A.dismiss(toastId);
      if (cancelled || /cancel/i.test(String(e))) A.notify(tt("Report export cancelled"), "info");
      else A.fail(e);
    } finally {
      setExporting(null);
    }
  }, [exporting, a0, b0, wait, eff, src, opts]);
  const exportReportRef = useRef(exportReport);
  exportReportRef.current = exportReport;
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const onExport = (e: Event) => {
      const detail = (e as CustomEvent<{ format: "html" | "pdf"; dest?: string }>).detail;
      if (detail?.format === "html" || detail?.format === "pdf") void exportReportRef.current(detail.format, detail.dest);
    };
    el.addEventListener("gall-compare-export", onExport);
    return () => el.removeEventListener("gall-compare-export", onExport);
  }, []);
  const toggle = (k: keyof DiffOptions) => setOpts((o) => ({ ...o, [k]: !o[k] }));
  const swap = () => {
    if (blockCopy.current) return;
    setSrc(([x, y]) => [y, x]);
    setEdits(([x, y]) => [y, x]);
    setHistory((h) => h.map(([x, y]) => [y, x]));
    meta.current = [meta.current[1], meta.current[0]];
  };
  // Opened from the folder comparison: Esc goes back there.
  const closing = useRef(false);
  const close = async (native = false) => {
    if (closing.current) return;
    closing.current = true;
    try {
      if (!(await confirmLeave())) return;
      if (wait && (native || !back)) {
        const outcome = back ? "accepted" : err ? "error" : res ? "accepted" : "cancelled";
        await finishCompareWait(outcome, undefined, !back ? err ?? undefined : undefined);
      } else A.openOverlay(back ?? null);
    } finally { closing.current = false; }
  };
  useCompareWaitClose(wait, () => void close(true));
  useEffect(() => {
    if (wait && !back && err && !dirty) void finishCompareWait("error", undefined, err);
  }, [wait, back, err, dirty]);

  const right = src[1];
  // Lines of an edited side are not those of the file any more; HEAD of the work tree is asked for as the file on disk.
  const blameRequest: BlameRequest | null = !(blameAll || blameWanted) || loading || edits[1] || !textSummary ? null
    : right.kind === "file" ? { path: parent(right.path) ?? right.path, commit: null, file: right.path, absolute: true }
    : right.kind === "gitCommit" ? { path: parent(right.path) ?? right.path, commit: right.commit, file: right.path, absolute: true } : null;
  const blame = useBlame(blameRequest);
  const canBlame = right.kind === "file" || right.kind === "gitCommit";
  const blameOf = (n: number) => blame?.lines[n];
  const toggleBlame = () => {
    if (!canBlame) return A.notify(tt("Blame is available when the right side is a file of a Git repository"), "info");
    setBlameAll((on) => !on);
  };
  const openBlamed = async (n: number) => {
    const line = blameOf(n);
    if (wait || !line || uncommitted(line.commit) || (right.kind !== "file" && right.kind !== "gitCommit")) return;
    if (!(await confirmLeave())) return;
    A.openOverlay({ type: "gitGraph", path: parent(right.path) ?? right.path, show: line.commit.id });
  };
  useEffect(() => { if (blame?.error && blameAll) { A.notify(blame.error, "warn"); setBlameAll(false); } }, [blame?.error, blameAll]);
  const blameCell = (n: number | undefined) => {
    if (!blameAll) return null;
    const line = n == null ? undefined : blameOf(n);
    return <span className="dv-blame" style={{ width: `${BLAME_COLS}ch` }} title={line ? blameTitle(line, opened.current) : undefined} onClick={() => n != null && void openBlamed(n)}>{line ? blameShort(line, opened.current) : ""}</span>;
  };
  const blameTip = (n: number) => { const line = blameOf(n); return line ? blameTitle(line, opened.current) : undefined; };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    // Ordinary typing in a field (also the rule boxes of the options) is not a command.
    if (["INPUT", "TEXTAREA", "SELECT"].includes((e.target as HTMLElement).tagName)) return;
    const el = bodyRef.current;
    const k = e.key;
    const by = (n: number) => { if (el) el.scrollTop += n; };
    // Ctrl+Tab and Ctrl+Shift+Tab, Ctrl+PageDown and Ctrl+PageUp: the next and the previous comparison tab.
    if (e.ctrlKey && !e.altKey && !e.metaKey && (k === "Tab" || k === "PageDown" || k === "PageUp")) {
      const tab = wait ? null : neighbourTab(k === "PageUp" || (k === "Tab" && e.shiftKey) ? -1 : 1);
      if (tab) {
        e.preventDefault();
        void confirmLeave().then((allowed) => {
          if (allowed) A.openOverlay({ type: "diff", a: tab.a, b: tab.b, back });
        });
      }
      return;
    }
    // Vim profile: gt / gT switch comparison tabs without Ctrl, gg jumps to top.
    if (vim && !wait && gPending.current && !e.ctrlKey && !e.altKey && !e.metaKey) {
      gPending.current = false;
      if (gTimer.current != null) { window.clearTimeout(gTimer.current); gTimer.current = null; }
      if (k === "t" || k === "T") {
        const tab = neighbourTab(k === "t" ? 1 : -1);
        if (tab) {
          void confirmLeave().then((allowed) => {
            if (allowed) A.openOverlay({ type: "diff", a: tab.a, b: tab.b, back });
          });
        }
        e.preventDefault();
        return;
      }
      if (k === "g" && !(imageMode || tableMode || docMode)) {
        if (!collapsed && el) el.scrollTop = 0;
        e.preventDefault();
        return;
      }
    }
    if (vim && !wait && !e.ctrlKey && !e.altKey && !e.metaKey && k === "g") {
      gPending.current = true;
      if (gTimer.current != null) window.clearTimeout(gTimer.current);
      gTimer.current = window.setTimeout(() => { gPending.current = false; gTimer.current = null; }, 900);
      e.preventDefault();
      return;
    }
    if (imageMode || tableMode || docMode) {
      if (e.metaKey) return;
      if (k === "Escape" || k === "q") close();
      else if (k === "s") swap();
      else if (k === "r") run();
      else return;
      e.preventDefault();
      return;
    }
    if (primaryMod(e) && k === "s") saveEdits();
    else if (primaryMod(e) && k === "z") undoEdit();
    // Other Cmd chords are the system's (Cmd+Q, Cmd+C…): not the letter commands below
    // (Cmd+S swapped the sides, Cmd+Q closed the view instead of quitting).
    else if (e.metaKey) return;
    // A collapsed generated body is not looked at: never apply, jump or scroll unseen (reveal it with H first).
    else if (e.altKey && k === "ArrowRight") { if (!collapsed) applyBlock_("toRight"); }
    else if (e.altKey && k === "ArrowLeft") { if (!collapsed) applyBlock_("toLeft"); }
    else if (k === "Escape" || k === "q") close();
    else if (collapsed && collapsedKeyBehavior(k, e.ctrlKey, e.altKey) === "notify") {
      A.notify(tt("Generated file is collapsed — press H to show it."), "info");
    } else if (collapsed && collapsedKeyBehavior(k, e.ctrlKey, e.altKey) === "ignore") {
      // Do not scroll a hidden body.
    } else if (k === "n" || k === "F8" || (k === "]" && !e.ctrlKey) || (e.altKey && k === "ArrowDown")) next();
    else if (k === "N" || (k === "F8" && e.shiftKey) || k === "[" || (e.altKey && k === "ArrowUp")) prev();
    else if (k === "j" || k === "ArrowDown") by(rowH);
    else if (k === "k" || k === "ArrowUp") by(-rowH);
    else if (k === "PageDown" || k === " " || (e.ctrlKey && k === "d")) by((el?.clientHeight ?? 400) * (k === "d" ? 0.5 : 0.9));
    else if (k === "PageUp" || (e.ctrlKey && k === "u")) by(-(el?.clientHeight ?? 400) * (k === "u" ? 0.5 : 0.9));
    else if (k === "g" || k === "Home") { if (el) el.scrollTop = 0; }
    else if (k === "G" || k === "End") { if (el) el.scrollTop = el.scrollHeight; }
    else if (k === "u") setMode((m) => (m === "split" ? "unified" : "split"));
    else if (k === "w") setOpts((o) => ({ ...o, whitespace: o.whitespace === "none" || o.whitespace === undefined ? "all" : "none" }));
    else if (k === "i") toggle("ignoreCase");
    else if (k === "e") toggle("ignoreEol");
    else if (k === "m") toggle("minimal");
    else if (k === "t" && !compact) toggle("structural");
    else if (k === "v" && !compact) toggle("movesEdited");
    else if (k === "o" && !compact) setOutlineOpen((v) => !v);
    else if (k === "O" && !compact && outlineOpen) setOutlineSide((v) => (v === "a" ? "b" : "a"));
    else if (k === "S" && !compact && semantic?.verdict === "different") setSemanticOpen((v) => !v);
    else if (k === "E" && !compact) toggleAi();
    else if (k === "H" && generated) setRevealed((v) => !v);
    else if (k === "x") jumpMove(false);
    else if (k === "X") jumpMove(true);
    else if (k === "z") setFold((f) => !f);
    else if (k === "l") setWrap((v) => !v);
    else if (k === "s") swap();
    else if (k === "r") run();
    else if (k === "y") copyPatch();
    else if (k === "p" && !wait) void applyPatch();
    else if (k === "B") toggleBlame();
    else return;
    e.preventDefault();
  };

  // ── row rendering ──
  const leftLines = tx?.leftLines ?? [];
  const rightLines = tx?.rightLines ?? [];
  const maxLen = useMemo(() => compact
    ? Math.max(textSummary?.left.maxLineColumns ?? 1, textSummary?.right.maxLineColumns ?? 1)
    : Math.max(maxOf(leftLines, (l) => l.length, 1), maxOf(rightLines, (l) => l.length, 1)),
  [compact, textSummary, leftLines, rightLines]);
  const digits = String(Math.max(textSummary?.left.lines ?? 0, textSummary?.right.lines ?? 0)).length + 1;
  const numW = `${digits}ch`;
  // Columns available for code when wrapping: the cell minus the line number
  // (0.9em font), the sign (1.6ch), padding and the divider.
  const cols = useMemo(() => {
    const numPx = digits * 0.9 * charW;
    const blamePx = blameAll ? BLAME_COLS * charW * 0.9 + 8 : 0;
    const avail = mode === "split" ? bodyW / 2 - numPx - 1.6 * charW - 13 - blamePx : bodyW - 2 * numPx - 1.6 * charW - 12 - blamePx;
    return Math.max(10, Math.floor(avail / charW));
  }, [digits, charW, bodyW, mode, blameAll]);
  const off = useMemo(() => {
    if (!wrap) return offsets(rows.map(() => rowH));
    return offsets(rows.map((r) => {
      if (r.kind === "fold") return rowH;
      if (mode === "split") {
        const s = r as Extract<SplitRow, { kind: "line" }>;
        const n = Math.max(s.l ? wrappedLines(leftLines[s.l.n] ?? "", cols) : 1, s.r ? wrappedLines(rightLines[s.r.n] ?? "", cols) : 1);
        return n * rowH;
      }
      const u = r as Extract<UnifiedRow, { kind: "line" }>;
      const line = u.side === "a" ? leftLines[u.an!] : rightLines[u.bn!];
      return wrappedLines(line ?? "", cols) * rowH;
    }));
  }, [rows, wrap, rowH, cols, mode, leftLines, rightLines]);
  // The changes of the ruler: where each lies in the document and its kind. Drawn in buckets.
  const rulerMarks = useMemo<RulerMark[]>(() => blocks.map((ri, k) => {
    const r = rows[ri];
    let n = 1;
    while (ri + n < rows.length && blockOfRow.get(ri + n) === k) n++;
    const tag = r && r.kind === "line" ? ("t" in r ? (r.t === "eq" ? "chg" : r.t) : r.l && r.r ? "chg" : r.l ? "del" : "ins") : "chg";
    return { top: off[ri] ?? 0, height: (off[ri + n] ?? off[ri] ?? 0) - (off[ri] ?? 0), tag };
  }), [blocks, rows, blockOfRow, off]);
  const rowCount = compact?.length ?? rows.length;
  const topAt = (i: number) => compact ? i * rowH : off[i];
  const rowToAnchor = useCallback((r: SplitRow | UnifiedRow | undefined): AnchorRow => {
    if (!r || r.kind !== "line") return { left: null, right: null };
    if ("side" in r) return { left: r.an != null ? r.an + 1 : null, right: r.bn != null ? r.bn + 1 : null };
    return { left: r.l != null ? r.l.n + 1 : null, right: r.r != null ? r.r.n + 1 : null };
  }, []);
  const anchorForRowIndex = useCallback((start: number): ScrollAnchor | null => {
    if (rowCount === 0) return null;
    const clamped = Math.max(0, Math.min(start, rowCount - 1));
    const read = (i: number) => (compact ? compact.rowAt(i) : rows[i]);
    for (let i = clamped; i < Math.min(rowCount, clamped + 8); i++) {
      const a = scrollAnchor(rowToAnchor(read(i)));
      if (a) return a;
    }
    for (let i = clamped - 1; i >= Math.max(0, clamped - 8); i--) {
      const a = scrollAnchor(rowToAnchor(read(i)));
      if (a) return a;
    }
    return null;
  }, [rowCount, compact, rows, rowToAnchor]);

  // Restore saved scroll anchor and active hunk once rows are ready after switching tabs or restarting.
  const justRestored = useRef(false);
  useLayoutEffect(() => {
    if (loading || err || rowCount === 0 || !pendingRestore.current) return;
    const restore = pendingRestore.current;
    pendingRestore.current = null;
    justRestored.current = true;
    if (restore.activeHunk != null && restore.activeHunk >= 0 && restore.activeHunk < blocks.length) {
      setCur(restore.activeHunk);
    }
    const el = bodyRef.current;
    if (!el) return;
    if (restore.scroll) {
      const anchorRows: AnchorRow[] = compact
        ? Array.from({ length: compact.length }, (_, i) => rowToAnchor(compact.rowAt(i)))
        : rows.map(rowToAnchor);
      const idx = rowOfAnchor(anchorRows, restore.scroll);
      const y = (compact ? idx * rowH : off[idx]) ?? 0;
      el.scrollTop = y;
      setScrollTop(y);
    } else if (restore.activeHunk != null && restore.activeHunk >= 0 && restore.activeHunk < blocks.length) {
      const rowIdx = blocks[restore.activeHunk];
      const y = Math.max(0, ((compact ? rowIdx * rowH : off[rowIdx]) ?? 0) - el.clientHeight / 3);
      el.scrollTop = y;
      setScrollTop(y);
    }
  }, [loading, err, rowCount, compact, rows, blocks, rowH, off, rowToAnchor]);

  // Keep this tab's saved view state (mode, options, scroll anchor, active hunk) up to date.
  const latestTabState = useRef<{ mode: "split" | "unified"; options: DiffOptions; scroll: ScrollAnchor | null; activeHunk: number | null }>({
    mode,
    options: opts,
    scroll: initialTab?.scroll ?? null,
    activeHunk: initialTab?.activeHunk ?? null,
  });
  useEffect(() => {
    if (wait || pendingRestore.current) return;
    if (justRestored.current) { justRestored.current = false; return; }
    const topPx = bodyRef.current?.scrollTop ?? scrollTop;
    const topIdx = rowCount > 0 ? (compact ? Math.floor(topPx / rowH) : rowAt(off, topPx)) : 0;
    const anchor = rowCount > 0 ? anchorForRowIndex(topIdx) : latestTabState.current.scroll;
    const snapshot = { mode, options: opts, scroll: anchor, activeHunk: cur >= 0 ? cur : null };
    latestTabState.current = snapshot;
    updateTabState(a0, b0, snapshot);
  }, [wait, a0, b0, mode, opts, cur, scrollTop, rowCount, compact, rowH, off, anchorForRowIndex]);
  useEffect(() => () => {
    if (!wait && !pendingRestore.current) updateTabState(a0, b0, latestTabState.current);
  }, [wait, a0, b0]);
  const first = Math.max(0, (compact ? Math.floor(scrollTop / rowH) : rowAt(off, scrollTop)) - 20);
  const last = Math.min(rowCount, first + 200, (compact ? Math.floor((scrollTop + viewH) / rowH) : rowAt(off, scrollTop + viewH)) + 21);
  const visibleRows = useMemo(() => compact
    ? Array.from({ length: Math.max(0, last - first) }, (_, i) => compact.rowAt(first + i)!)
    : rows.slice(first, last), [compact, rows, first, last]);
  const rowStyle = (i: number) => ({ top: topAt(i), height: topAt(i + 1) - topAt(i) });

  const wantedPages = (side: "left" | "right") => {
    const pages = new Set<number>();
    for (const row of visibleRows) {
      if (row.kind !== "line") continue;
      const n = "side" in row
        ? (row.side === "a" && side === "left" ? row.an : row.side === "b" && side === "right" ? row.bn : undefined)
        : (side === "left" ? row.l?.n : row.r?.n);
      if (n != null) pages.add(Math.floor(n / LINE_PAGE) * LINE_PAGE);
    }
    return [...pages].sort((a, b) => a - b).join(",");
  };
  const leftPages = compact ? wantedPages("left") : "";
  const rightPages = compact ? wantedPages("right") : "";
  const pageId = paged?.id;
  useEffect(() => {
    if (pageId == null || loading) return;
    let live = true;
    const load = async (side: "left" | "right", key: string) => {
      const lines = new Map<number, string>();
      for (const chunk of key ? key.split(",") : []) {
        let start: number | null = Number(chunk);
        const end = start + LINE_PAGE;
        while (start != null && start < end && live && pageId === runSeq.current) {
          const page = await api.diffPageLines(pageId, side, start, end - start);
          page.items.forEach((line, i) => lines.set(page.start + i, line));
          start = page.next;
        }
      }
      return lines;
    };
    void Promise.all([load("left", leftPages), load("right", rightPages)]).then(([left, right]) => {
      if (live && pageId === runSeq.current) setVisibleLines({ id: pageId, left, right });
    }).catch((error) => {
      if (live && pageId === runSeq.current) setErr(String(error));
    });
    return () => { live = false; };
  }, [pageId, leftPages, rightPages, loading]);
  const lineAt = (side: "l" | "r", n: number) => compact
    ? (visibleLines?.id === pageId ? (side === "l" ? visibleLines?.left : visibleLines?.right)?.get(n) : undefined) ?? ""
    : (side === "l" ? leftLines : rightLines)[n] ?? "";

  // Scroll to a line of one side (the other end of a move).
  // Moved lines lie in changed blocks, which never fold, so their rows always exist (-1: not in this view).
  const rowOfLine = (side: "l" | "r", n: number) => rows.findIndex((r) => r.kind === "line" && (mode === "split"
    ? (side === "l" ? (r as Extract<SplitRow, { kind: "line" }>).l?.n === n : (r as Extract<SplitRow, { kind: "line" }>).r?.n === n)
    : (side === "l" ? (r as Extract<UnifiedRow, { kind: "line" }>).side === "a" && (r as Extract<UnifiedRow, { kind: "line" }>).an === n
      : (r as Extract<UnifiedRow, { kind: "line" }>).side === "b" && (r as Extract<UnifiedRow, { kind: "line" }>).bn === n)));
  const jumpTo = (side: "l" | "r", n: number) => {
    const i = rowOfLine(side, n);
    const el = bodyRef.current;
    if (i >= 0 && el) el.scrollTop = Math.max(0, off[i] - el.clientHeight / 3);
  };
  // x / Shift+X: the next / previous end of a moved block (the source and the destination of each), wrapping around.
  const jumpMove = (back: boolean) => {
    const moves = tx?.moves ?? [];
    if (!moves.length) return A.notify("No moved blocks in this comparison", "info");
    const el = bodyRef.current;
    // The jump puts its target a third down the window, so the row at the top is above it: continue from the
    // end last jumped to while the view has not been scrolled since, else from the top row.
    const last = lastMoveEnd.current;
    const from = last && el && Math.abs(el.scrollTop - last.top) < 2 ? last.row : rowAt(off, scrollTop + 4);
    const end = nextMoveEnd(moves, (e) => rowOfLine(e.side, e.n), from, back);
    if (!end) return;
    jumpTo(end.side, end.n);
    if (el) lastMoveEnd.current = { row: rowOfLine(end.side, end.n), top: el.scrollTop };
  };
  /** The sign of a moved line: ⇅, click goes to where it moved to / came from. */
  const moveSign = (mv: number, side: "l" | "r", n: number) => {
    const to = moveTarget(tx!.moves, mv, side, n);
    const m = tx!.moves[mv];
    const how = m.edited ? "Moved with edits" : "Moved";
    const tip = side === "l" ? `${how} to line ${to + 1} (click to go there) · block ${m.id}` : `${how} from line ${to + 1} (click to go there) · block ${m.id}`;
    return <span className={`dv-sign mv ${m.edited ? "edited" : ""}`} title={tip} onClick={(e) => { e.stopPropagation(); jumpTo(side === "l" ? "r" : "l", to); }}>⇅</span>;
  };

  useEffect(() => { lastMoveEnd.current = null; }, [tx]);
  const cell = (c: Cell | undefined, side: "l" | "r") => {
    const sx = c ? hl[side === "l" ? 0 : 1]?.lines[c.n] : undefined;
    if (!c) return <div className={`dv-cell empty ${side}`}>{side === "r" && blameCell(undefined)}<span className="dv-num" style={{ width: numW }} /><span className="dv-code" /></div>;
    return (
      <div className={`dv-cell ${c.t} ${side} ${c.mv != null ? "mv" : ""} ${c.fmt ? "fmt" : ""}`} data-side={side} data-line={c.n + 1}>
        {side === "r" && blameCell(c.n)}
        <span className="dv-num" style={{ width: numW }} title={side === "r" ? blameTip(c.n) : undefined} onMouseEnter={side === "r" && canBlame && !blameWanted ? () => setBlameWanted(true) : undefined}>{c.n + 1}</span>
        {c.mv != null ? moveSign(c.mv, side, c.n) : <span className="dv-sign" title={c.fmt ? "Only the formatting of this line differs" : undefined}>{c.fmt ? "≈" : c.t === "del" || (c.t === "chg" && side === "l") ? "−" : c.t === "ins" || (c.t === "chg" && side === "r") ? "+" : ""}</span>}
        <span className="dv-code"><Text text={lineAt(side, c.n)} spans={c.spans} sx={sx} kind={c.t} /></span>
      </div>
    );
  };
  const foldRow = (r: Extract<SplitRow | UnifiedRow, { kind: "fold" }>, i: number) => (
    <button key={i} className="dv-fold" style={rowStyle(i)} onClick={() => setExpanded((s) => new Set(s).add(r.id))}>
      <Icon name="more" size={14} /> {r.count} unchanged line{r.count === 1 ? "" : "s"} · click to show
    </button>
  );

  const renderRow = (r: SplitRow | UnifiedRow, i: number) => {
    if (r.kind === "fold") return foldRow(r, i);
    const isCur = cur >= 0 && (r.block ?? blockOfRow.get(i)) === cur;
    const style = { ...rowStyle(i), lineHeight: `${rowH}px` };
    if (mode === "split") {
      const s = r as Extract<SplitRow, { kind: "line" }>;
      const k = blockStart.get(i);
      return (
        <div key={i} className={`dv-row split ${isCur ? "cur" : ""}`} style={style}>
          {cell(s.l, "l")}{cell(s.r, "r")}
          {k != null && ((allowChanges && (editable(0) || editable(1))) || !compact) && (
            <span className="dv-merge">
              {allowChanges && editable(0) && <button title={tt("Copy this change to the left (Alt+←)")} onClick={() => { setCur(k); applyBlock_("toLeft", k); }}>‹</button>}
              {allowChanges && editable(1) && <button title={tt("Copy this change to the right (Alt+→)")} onClick={() => { setCur(k); applyBlock_("toRight", k); }}>›</button>}
              {!compact && <button title={tt("Explain this change")} aria-label={`${tt("Explain this change")} (${k + 1})`} onClick={() => { setCur(k); askAi({ kind: "hunk", index: k }); }}>✦</button>}
            </span>
          )}
        </div>
      );
    }
    const u = r as Extract<UnifiedRow, { kind: "line" }>;
    const n = u.side === "a" ? u.an! : u.bn!;
    const k = blockStart.get(i);
    return (
      <div key={i} className={`dv-row unified ${u.t} ${u.mv != null ? "mv" : ""} ${u.fmt ? "fmt" : ""} ${isCur ? "cur" : ""}`} style={style} data-side={u.side === "a" ? "l" : "r"} data-line={n + 1}>
        {blameCell(u.bn)}
        <span className="dv-num" style={{ width: numW }}>{u.an != null ? u.an + 1 : ""}</span>
        <span className="dv-num" style={{ width: numW }}>{u.bn != null ? u.bn + 1 : ""}</span>
        {u.mv != null ? moveSign(u.mv, u.side === "a" ? "l" : "r", n) : <span className="dv-sign" title={u.fmt ? "Only the formatting of this line differs" : undefined}>{u.fmt ? "≈" : u.t === "del" ? "−" : u.t === "ins" ? "+" : ""}</span>}
        <span className="dv-code"><Text text={lineAt(u.side === "a" ? "l" : "r", n)} spans={u.spans} sx={hl[u.side === "a" ? 0 : 1]?.lines[n]} kind={u.t} /></span>
        {k != null && !compact && (
          <span className="dv-merge">
            <button title={tt("Explain this change")} aria-label={`${tt("Explain this change")} (${k + 1})`} onClick={() => { setCur(k); askAi({ kind: "hunk", index: k }); }}>✦</button>
          </span>
        )}
      </div>
    );
  };

  const changes = blocks.length;
  const identical = !!textSummary && textSummary.added === 0 && textSummary.removed === 0;
  const onlyCrlfDiffers = textSummary?.onlyCrlfDiffers;
  const pathChanged = src[0].kind === "gitBlob" && src[1].kind === "gitBlob" && src[0].file !== src[1].file;
  const anyIgnore = activeFilters(opts) > 0 || opts.ignoreCase;
  // Blocks whose content is the same and only the formatting differs (structural diff).
  // The syntax-tree verdict for Rust, JavaScript and TypeScript files: conservative, so "unknown" shows nothing.
  const [semantic, setSemantic] = useState<SemanticVerdict | null>(null);
  const [semanticOpen, setSemanticOpen] = useState(false);
  const semanticName = srcName(src[1]);
  const semanticLeftPath = src[0].kind === "file" && !edits[0] ? src[0].path : null;
  const semanticRightPath = src[1].kind === "file" && !edits[1] ? src[1].path : null;
  useEffect(() => {
    setSemantic(null);
    if (!tx || compact || !/\.(rs|[mc]?js|jsx|[mc]?ts|tsx)$/i.test(semanticName)) {
      setSemanticOpen(false);
      return;
    }
    let live = true;
    api.semanticCompare(
      semanticName,
      semanticLeftPath ? null : tx.leftLines.join("\n"),
      semanticRightPath ? null : tx.rightLines.join("\n"),
      semanticLeftPath,
      semanticRightPath,
    ).then((v) => {
      if (!live) return;
      setSemantic(v);
      if (v.verdict !== "different") setSemanticOpen(false);
    }).catch(() => {});
    return () => { live = false; };
  }, [tx, compact, semanticName, semanticLeftPath, semanticRightPath]);
  // The outline of the right side (opened with `o`) and the generated-file collapse (presentation only: the counts,
  // the patch, staging, merge and sync do not depend on it; `H` or the button reveals the body for this view).
  const [outlineOpen, setOutlineOpen] = useState(false);
  const [outline, setOutline] = useState<OutlineResult | null>(null);
  // Which side the outline is made from: the right (newer) one by default, the left one after `O` or its button.
  const [outlineSide, setOutlineSide] = useState<"a" | "b">("b");
  const [generated, setGenerated] = useState<Generated | null>(null);
  const [revealed, setRevealed] = useState(false);
  const generatedPath = src[1].kind === "gitBlob"
    ? join(src[1].root, src[1].file)
    : (srcPath(src[1]) || srcName(src[1]));
  // Another file: classify afresh and hide the body again. A change of options or edits of the same file keeps
  // the answer (and the reveal) until the new one arrives, so the body does not flash or collapse again.
  useEffect(() => { setGenerated(null); setRevealed(false); }, [generatedPath]);
  useEffect(() => {
    if (!tx || compact) return;
    let live = true;
    api.generatedClassify(generatedPath, tx.rightLines.join("\n")).then((g) => { if (live) setGenerated(g); }).catch(() => {});
    return () => { live = false; };
  }, [tx, compact, generatedPath, generatedPatternsKey]);
  useEffect(() => {
    if (!outlineOpen) { setOutline(null); return; }
    if (!tx || compact) return;
    let live = true;
    setOutline(null);
    const left = outlineSide === "a";
    api.semanticOutline(left ? srcName(src[0]) : semanticName, (left ? tx.leftLines : tx.rightLines).join("\n"), !left).then((o) => { if (live) setOutline(o); }).catch(() => {});
    return () => { live = false; };
  }, [outlineOpen, outlineSide, tx, compact, semanticName]);
  // The explanation by the local model: opt-in, asked only when the user presses `E` or the button (never at first
  // paint), never joining the full texts unless asked, and shown as out of date when the texts or options change.
  const optsKey = useMemo(() => JSON.stringify(opts), [opts]);
  const explainSnapshot: ExplainSnapshot | null = tx && !compact
    ? { leftLines: tx.leftLines, rightLines: tx.rightLines, optsKey }
    : null;
  const [ai, setAi] = useState<{ asked: ExplainSnapshot; scope: AiExplainScope; outcome: AiOutcome | null } | null>(null);
  const aiSeq = useRef(0), aiId = useRef(0), aiPending = useRef(false);
  const readSelectionScope = (): Extract<AiExplainScope, { kind: "selection" }> | null => {
    const body = bodyRef.current;
    const sel = typeof window !== "undefined" ? window.getSelection() : null;
    if (!body || !sel || sel.isCollapsed || sel.rangeCount === 0) return null;
    const lineOf = (node: Node | null): { side: "left" | "right"; line: number } | null => {
      const el = node instanceof Element ? node : node?.parentElement;
      const cellEl = el?.closest("[data-side][data-line]");
      if (!cellEl || !body.contains(cellEl)) return null;
      const side = cellEl.getAttribute("data-side") === "l" ? "left" : "right";
      const line = Number(cellEl.getAttribute("data-line"));
      return Number.isInteger(line) && line > 0 ? { side, line } : null;
    };
    const a = lineOf(sel.anchorNode);
    const b = lineOf(sel.focusNode);
    if (!a || !b || a.side !== b.side) return null;
    return { kind: "selection", side: a.side, startLine: Math.min(a.line, b.line), endLine: Math.max(a.line, b.line) };
  };
  const askAi = (scope: AiExplainScope = { kind: "all" }) => {
    if (!explainSnapshot || !tx || !res) return;
    if (aiPending.current) { aiPending.current = false; void api.compareAiCancel(aiId.current).catch(() => {}); }
    const asked = explainSnapshot, seq = ++aiSeq.current;
    // Join the lines lazily here (never on ordinary comparison updates when E is not pressed).
    const left = tx.leftLines.join("\n");
    const right = tx.rightLines.join("\n");
    // An id of its own (unique across windows), so that a cancel always reaches this request and no other.
    const id = nextRequestId();
    aiId.current = id;
    aiPending.current = true;
    setAi({ asked, scope, outcome: null });
    api.compareAiExplain(id, res.left.label, res.right.label, left, right, opts, scope)
      .then((outcome) => { if (seq === aiSeq.current) { aiPending.current = false; setAi({ asked, scope, outcome }); } })
      .catch((e) => { if (seq === aiSeq.current) { aiPending.current = false; setAi({ asked, scope, outcome: { outcome: "failed", message: String(e) } }); } });
  };
  const stopAi = () => { if (aiPending.current) { aiPending.current = false; void api.compareAiCancel(aiId.current).catch(() => {}); } aiSeq.current++; };
  const closeAi = () => { stopAi(); setAi(null); rootRef.current?.focus(); };
  // `E` and the button: explain the current selection when one exists, else ask / close / refresh stale answer.
  const toggleAi = () => {
    const sel = readSelectionScope();
    if (sel) return askAi(sel);
    if (!ai) return askAi({ kind: "all" });
    if (ai.outcome && !isSnapshotCurrent(ai.asked, explainSnapshot)) { stopAi(); return askAi(ai.scope); }
    closeAi();
  };
  const cancelAi = () => { stopAi(); if (ai) setAi({ asked: ai.asked, scope: ai.scope, outcome: { outcome: "cancelled" } }); };
  // Closing the window ends any in-flight local AI or syntax-tree comparison so nothing stays running in the background.
  useEffect(() => () => {
    if (aiPending.current) void api.compareAiCancel(aiId.current).catch(() => {});
    void api.semanticCompareCancel().catch(() => {});
  }, []);
  const collapsed = suppressed(generated?.reason, revealed);
  const wasCollapsed = useRef(collapsed);
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (wasCollapsed.current && !collapsed && el) {
      if (el.clientWidth > 0 && el.clientHeight > 0) {
        setViewH(el.clientHeight);
        setBodyW(el.clientWidth);
      }
      el.scrollTop = savedScrollTop.current;
      setScrollTop(savedScrollTop.current);
    }
    wasCollapsed.current = collapsed;
  }, [collapsed]);
  const cosmetic = useMemo(() => (tx?.ops ?? paged?.ops)?.filter((o) => o.cosmetic).length ?? 0, [tx, paged]);
  const total = topAt(rowCount) ?? 0;

  const fileHead = (s: DiffSource, side: "l" | "r") => (
    <div className={`dv-file ${side}`} title={srcPath(s) || srcName(s)}>
      {s.kind === "text" ? <Icon name="paste" size={15} /> : <FileIcon entry={{ isDir: false, ext: srcName(s).split(".").pop() ?? "", name: srcName(s) }} size={15} />}
      <b>{srcName(s)}</b>
      {edits[side === "l" ? 0 : 1] && <span className="dv-dirty" title={tt("Changed here, not saved yet")}>● modified</span>}
      {s.kind === "gitHead" && <span className="dv-tag">{tt("HEAD")}</span>}
      {s.kind === "gitCommit" && <span className="dv-tag" title={s.commit}>{s.commit.slice(0, 12)}</span>}
      {s.kind === "gitBlob" && <span className="dv-tag" title={s.commit}>{s.isHead ? tt("HEAD") : s.commit.slice(0, 12)}</span>}
      {s.kind === "gitBlob" && s.lastTouch && <span className="dv-path" title={`${s.lastTouch.id} · ${s.lastTouch.author} · ${s.lastTouch.date}`}>{s.lastTouch.subject}</span>}
      {/* LRM marks keep the leading "/" in place: the path is right-to-left only so it ellipsizes from the left. */}
      <span className="dv-path">{srcPath(s) && "\u200e" + tildify(parent(srcPath(s)) ?? "", home) + "\u200e"}</span>
    </div>
  );

  return (
    <div className="backdrop glass center" onMouseDown={() => void close()}>
      <div className="diffv" ref={rootRef} tabIndex={-1} onKeyDown={onKey} onMouseDown={(e) => e.stopPropagation()}>
        {!wait && <CompareTabsBar a={a0} b={b0} back={back} beforeNavigate={confirmLeave} />}
        <header className="dv-head">
          <div className="dv-files">
            {fileHead(src[0], "l")}
            <button className="icon-btn dv-swap" title={tt("Swap sides (s)")} onClick={swap}><Icon name="compare" size={15} /></button>
            {fileHead(src[1], "r")}
            <button className="icon-btn" title={tt("Close (Esc)")} onClick={() => void close()}><Icon name="close" size={15} /></button>
          </div>
          <div className="dv-bar">
            {textSummary && !richMode && (
              <span className="dv-stat" title={`${(textSummary.ratio * 100).toFixed(1)} % similar`}>
                <b className="add">+{textSummary.added}</b> <b className="del">−{textSummary.removed}</b>
                <span className="dv-ratio"><i style={{ width: `${textSummary.ratio * 100}%` }} /></span>
                <span>{tt("{percent}% similar", { percent: Math.round(textSummary.ratio * 100) })}</span>
                {!!tx?.moves.length && (
                  <span className="dv-moved" title={tt("Lines removed in one place and added unchanged in another (indentation aside)")}>
                    ⇅ {tt(tx.moves.some((m) => m.edited) ? "{count} moved (some edited)" : "{count} moved", { count: tx.moves.reduce((n, m) => n + m.len, 0) })}
                  </span>
                )}
                {semantic?.verdict === "equivalent" && (
                  <span className="dv-fmt" title={tt("The syntax trees are the same: only comments, layout, safe trailing delimiters or the order of Rust use lines differ")}>
                    ≡ {tt("same syntax tree")}
                  </span>
                )}
                {semantic?.verdict === "different" && (
                  <button
                    type="button"
                    className={`dv-fmt dv-sem-btn ${semanticOpen ? "on" : ""}`}
                    aria-pressed={semanticOpen}
                    aria-expanded={semanticOpen}
                    title={`${tt("Show syntax changes (S)")}\n${semantic.changes.map((c) => `${c.kind}${c.name ? ` ${c.name}` : ""}${c.right ? ` (${tt("right line {line}", { line: c.right.line })})` : c.left ? ` (${tt("left line {line}", { line: c.left.line })})` : ""}`).join("\n")}`}
                    onClick={() => setSemanticOpen((v) => !v)}
                  >
                    ƒ {tt(semantic.changes.length === 1 ? "{count} syntax change" : "{count} syntax changes", { count: semantic.changes.length })}
                  </button>
                )}
                {cosmetic > 0 && (
                  <span className="dv-fmt" title={tt("Places where only the formatting differs (line breaks, indentation, spacing): shown dimmed, not counted as changes")}>
                    ≈ {tt("{count} formatting only", { count: cosmetic })}
                  </span>
                )}
              </span>
            )}
            {/* Text-only controls: hidden for binary files, images and tables. */}
            {(textSummary || (res?.limit && !hexMode && !binaryImageMode) || !res) && !richMode && (
              <>
            <div className="seg" role="tablist">
              <button className={mode === "split" ? "on" : ""} title={tt("Side by side (u)")} onClick={() => setMode("split")}>{tt("Side by side")}</button>
              <button className={mode === "unified" ? "on" : ""} title={tt("Unified, like git diff (u)")} onClick={() => setMode("unified")}>{tt("Unified")}</button>
            </div>
            <div className="dv-toggles">
              <button className={`tgl ${opts.whitespace && opts.whitespace !== "none" ? "on" : ""}`} title={tt("Ignore whitespace (w)")} onClick={() => setOpts((o) => ({ ...o, whitespace: o.whitespace === "none" || o.whitespace === undefined ? "all" : "none" }))}>␣</button>
              <button className={`tgl ${opts.ignoreCase ? "on" : ""}`} title={tt("Ignore case (i)")} onClick={() => toggle("ignoreCase")}>Aa</button>
              <button className={`tgl ${opts.ignoreEol ? "on" : ""}`} title={tt("Ignore line endings CRLF/LF (e)")} onClick={() => toggle("ignoreEol")}>⏎</button>
              <button className={`tgl ${opts.minimal ? "on" : ""}`} title={tt("Minimal diff: fewest changed lines, often harder to read (m)")} onClick={() => toggle("minimal")}>M</button>
              <button className={`tgl ${opts.structural ? "on" : ""}`} title={tt("Structural diff: compare code by its tokens, so reformatting is not a change and only what really changed is marked (t)")} disabled={!!compact} onClick={() => toggle("structural")}>{"{}"}</button>
              <button className={`tgl ${opts.movesEdited ? "on" : ""}`} title="Also find moved blocks that were edited on the way: anchored by lines that are unique in both files (v). Jump between moved blocks with x / Shift+X" disabled={!!compact} onClick={() => toggle("movesEdited")}>⇅</button>
              <button className={`tgl ${ai ? "on" : ""}`} title={tt("Explain the changes with a local model you have set up, only when you ask (E)")} aria-pressed={!!ai} disabled={!!compact} onClick={toggleAi}>✦</button>
              <button className={`tgl ${outlineOpen ? "on" : ""}`} title={tt("Outline of modules, classes and functions with their changes (o)")} aria-pressed={outlineOpen} disabled={!!compact} onClick={() => setOutlineOpen((v) => !v)}>☰</button>
              <button className={`tgl ${fold ? "on" : ""}`} title={tt("Fold unchanged lines (z)")} onClick={() => setFold((f) => !f)}>⋯</button>
              <button className={`tgl ${wrap ? "on" : ""}`} title={tt("Wrap long lines (l)")} disabled={!!compact} onClick={() => setWrap((v) => !v)}>↩</button>
              <button className={`tgl ${more || activeFilters(opts) > 0 ? "on" : ""}`} title={tt("More options")} aria-expanded={more} aria-haspopup="dialog" onClick={() => setMore((v) => !v)}>
                ⚙{activeFilters(opts) > 0 && <sup className="dv-badge" title={tt("Filters active: {count}", { count: activeFilters(opts) })}>{activeFilters(opts)}</sup>}
              </button>
            </div>
            {more && (
              <div className="dv-more" role="dialog" aria-label={tt("More options")} onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setMore(false); } }}>
                <div className="dv-more-row">
                  <span>{tt("Whitespace")}</span>
                  <div className="seg" role="group" aria-label={tt("Whitespace")}>
                    {(["none", "leading", "trailing", "all"] as WhitespaceMode[]).map((m) => (
                      <button key={m} className={(opts.whitespace ?? "none") === m ? "on" : ""} aria-pressed={(opts.whitespace ?? "none") === m} onClick={() => setOpts((o) => ({ ...o, whitespace: m }))}>
                        {m === "none" ? tt("Count all") : m === "leading" ? tt("Ignore indentation") : m === "trailing" ? tt("Ignore trailing space") : tt("Ignore all whitespace")}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="dv-more-row">
                  <span>{tt("Mark changes by")}</span>
                  <div className="seg" role="group" aria-label={tt("Mark changes by")}>
                    {(["line", "word", "char"] as Granularity[]).map((g) => (
                      <button key={g} className={(opts.granularity ?? "word") === g ? "on" : ""} aria-pressed={(opts.granularity ?? "word") === g} onClick={() => setOpts((o) => ({ ...o, granularity: g }))}>
                        {g === "line" ? tt("Line") : g === "word" ? tt("Word") : tt("Character")}
                      </button>
                    ))}
                  </div>
                </div>
                <label className="dv-more-row">
                  <input type="checkbox" checked={!!opts.ignoreComments} onChange={() => toggle("ignoreComments")} />
                  <span>{tt("Ignore comments")}</span>
                </label>
                <label className="dv-more-col">
                  <span>{tt("Ignore lines matching (regular expressions, one per line)")}</span>
                  <textarea rows={3} spellCheck={false} value={lineRules} onChange={(e) => setLineRules(e.target.value)} onBlur={() => setOpts((o) => ({ ...o, ignoreLines: parseRules(lineRules) }))} />
                </label>
                <label className="dv-more-col">
                  <span>{tt("Ignore text matching (regular expressions, one per line)")}</span>
                  <textarea rows={3} spellCheck={false} value={textRules} onChange={(e) => setTextRules(e.target.value)} onBlur={() => setOpts((o) => ({ ...o, ignoreText: parseRules(textRules) }))} />
                </label>
              </div>
            )}
              </>
            )}
            <span className="spacer" />
            {textSummary && changes > 0 && !richMode && (
              <div className="dv-nav">
                <button className="icon-btn" title={tt("Previous change (N, [, Shift+F8)")} onClick={prev}><Icon name="up" size={15} /></button>
                <span className="dv-count">{cur >= 0 ? cur + 1 : "–"} / {changes}</span>
                <button className="icon-btn" title={tt("Next change (n, ], F8)")} onClick={next}><Icon name="down" size={15} /></button>
              </div>
            )}
            {bothTables && !tableView && <button className="dc-btn" title={tt("Compare as a table (columns and rows matched)")} onClick={() => setTableView(true)}>{tt("Table view")}</button>}
            {bothSvg && svgView === "text" && <button className="dc-btn" title={tt("Compare the pictures")} onClick={() => setSvgView("image")}>{tt("Image view")}</button>}
            {textSummary && !richMode && (editable(0) || editable(1)) && (
              <label className="dv-allow-changes">
                <input type="checkbox" checked={allowChanges} disabled={saving} onChange={(e) => {
                  allowChangesRef.current = e.target.checked;
                  permissionVersion.current++;
                  setAllowChanges(e.target.checked);
                }} />
                {tt("Allow changes")}
              </label>
            )}
            {dirty && <button className="dc-btn" title={tt("Undo the last copied block (Ctrl+Z)")} onClick={undoEdit} disabled={!history.length}>{tt("Undo")}</button>}
            {dirty && <button className="dc-btn primary" title={tt("Save the changed file(s) (Ctrl+S)")} onClick={saveEdits} disabled={!allowChanges || saving}>{tt("Save")}</button>}
            {textSummary && !richMode && (
              <select className="dc-select" aria-label={tt("Patch format")} title={tt("Format of Copy patch and Save patch")} value={patchFormat} onChange={(e) => setPatchFormat(e.target.value as PatchFormat)}>
                {(Object.keys(PATCH_LABEL) as PatchFormat[]).map((f) => <option key={f} value={f}>{tt(PATCH_LABEL[f])}</option>)}
              </select>
            )}
            {textSummary && !richMode && <button className="icon-btn" title={tt("Copy patch (y)")} onClick={copyPatch}><Icon name="copy" size={15} /></button>}
            {textSummary && !richMode && !wait && <button className="icon-btn" title={tt("Apply this patch to a folder… (p)")} aria-label={tt("Apply this patch to a folder… (p)")} onClick={() => void applyPatch()}><Icon name="paste" size={15} /></button>}
            {textSummary && !richMode && <button className="icon-btn" title={tt("Save patch…")} onClick={savePatch}><Icon name="download" size={15} /></button>}
            {textSummary && !richMode && <button className="dc-btn dv-export-html" disabled={!!exporting} title={tt("Export HTML report of open comparisons…")} onClick={() => void exportReport("html")}>{tt("Export HTML")}</button>}
            {textSummary && !richMode && <button className="dc-btn dv-export-pdf" disabled={!!exporting} title={tt("Export PDF report of open comparisons…")} onClick={() => void exportReport("pdf")}>{tt("Export PDF")}</button>}
            <button className="icon-btn" title={tt("Reload (r)")} onClick={run}><Icon name="refresh" size={15} /></button>
          </div>
        </header>

        {err && <div className="dv-msg err"><Icon name="close" size={16} /> {err}</div>}
        {loading && !res && <div className="dv-msg">{tt("Comparing…")}</div>}

        {!richMode && !binaryImageMode && res?.limit && <div className="dv-msg">{res.limit === "minimalWorkLimit"
          ? tt("This input exceeds the minimal diff limit. Turn off minimal diff to compare it.")
          : tt("The input exceeds the text comparison limit.")}</div>}
        {!richMode && compact && <div className="dv-msg">{tt("Large comparison: showing line changes without wrapping or inline highlights.")}</div>}

        {svgImage && src[0].kind === "file" && src[1].kind === "file" && (
          <>
            <div className="dv-msg">
              <Icon name="compare" size={16} />
              {tt("SVG pictures. The files are text: ")}
              <button className="link-btn" onClick={() => setSvgView("text")} title={tt("Compare the SVG source as text")}>{tt("Show as text")}</button>
            </div>
            <ImageCompare left={src[0].path} right={src[1].path} leftSize={res?.left.size} rightSize={res?.right.size} />
          </>
        )}

        {docMode && src[0].kind === "file" && src[1].kind === "file" && (
          <DocumentCompare
            left={src[0].path}
            right={src[1].path}
            onText={(leftDoc, rightDoc) => {
              if (src[0].kind === "file" && src[1].kind === "file") {
                setSrc([
                  { kind: "text", label: srcName(src[0]), text: leftDoc.text },
                  { kind: "text", label: srcName(src[1]), text: rightDoc.text },
                ]);
              }
            }}
            onBinaryFallback={() => setDocView("binary")}
            onCancelClose={() => void close()}
          />
        )}

        {tableMode && src[0].kind === "file" && src[1].kind === "file" && (
          <TableDiff left={src[0].path} right={src[1].path} onText={csvOnly ? () => setTableView(false) : undefined} />
        )}

        {binaryImageMode && src[0].kind === "file" && src[1].kind === "file" && res && (
          <>
            <div className={`dv-msg ${res.binary?.identical ? "same" : ""}`}>
              <Icon name={res.binary?.identical ? "check" : "compare"} size={16} />
              {res.binary ? (res.binary.identical ? "Files are identical, byte for byte" : `Files differ (${formatSize(res.left.size)} vs ${formatSize(res.right.size)})`) : `${formatSize(res.left.size)} · ${formatSize(res.right.size)}`}
            </div>
            <ImageCompare left={src[0].path} right={src[1].path} leftSize={res.left.size} rightSize={res.right.size} />
          </>
        )}

        {/* Two binary files: the bytes side by side, differences marked (D-04). */}
        {hexMode && src[0].kind === "file" && src[1].kind === "file" && res && (
          <HexDiff left={src[0].path} right={src[1].path} leftLabel={res.left.label} rightLabel={res.right.label} />
        )}

        {!richMode && res?.binary && !(src[0].kind === "file" && src[1].kind === "file") && (
          <div className="dv-binary">
            <div className={`dv-verdict ${res.binary.identical ? "same" : "diff"}`}>
              <Icon name={res.binary.identical ? "check" : "compare"} size={28} />
              <div>
                <b>{res.binary.identical ? "Files are identical" : "Files differ"}</b>
                <span>{res.binary.identical ? `Byte for byte (${res.binary.reason})` : res.binary.firstDifference != null ? `First difference at byte ${res.binary.firstDifference.toLocaleString()} (0x${res.binary.firstDifference.toString(16).toUpperCase()}) · ${res.binary.reason}` : res.binary.reason}</span>
              </div>
            </div>
            <div className="dv-sizes">
              <div><span>{res.left.label}</span><b>{formatSize(res.left.size)}</b></div>
              <div><span>{res.right.label}</span><b>{formatSize(res.right.size)}</b></div>
            </div>
          </div>
        )}

        {!richMode && onlyCrlfDiffers && <div className="dv-msg" role="status">{tt("Only line endings differ (CRLF vs LF).")}</div>}
        {pathChanged && <div className="dv-msg" role="status">{tt("The file path changed between revisions.")}</div>}
        {!richMode && textSummary && identical && !onlyCrlfDiffers && (
          <div className="dv-msg same"><Icon name="check" size={16} /> {cosmetic > 0 ? `Same content: only the formatting differs in ${cosmetic} place${cosmetic === 1 ? "" : "s"}` : pathChanged ? tt("No textual difference") : "No differences"}{anyIgnore ? " (ignoring " + [opts.whitespace && opts.whitespace !== "none" && (opts.whitespace === "all" ? "whitespace" : opts.whitespace === "leading" ? "indentation" : "trailing whitespace"), opts.ignoreComments && "comments", ((opts.ignoreLines?.length ?? 0) + (opts.ignoreText?.length ?? 0) > 0) && "matching text", opts.ignoreCase && "case"].filter(Boolean).join(" and ") + ")" : ""}
            {res && res.left.crlf !== res.right.crlf && " · line endings differ (CRLF vs LF)"}
            {res && res.left.finalNewline !== res.right.finalNewline && " · final newline differs"}
          </div>
        )}

        {!richMode && textSummary && (
          <div className="dv-main">
            {mode === "split" && (
              <div className="dv-colheads">
                <span>{res!.left.label}{res!.left.crlf ? " · CRLF" : ""}{hl[0] ? ` · ${hl[0].language}` : ""}</span>
                <span>{res!.right.label}{res!.right.crlf ? " · CRLF" : ""}{hl[1] ? ` · ${hl[1].language}` : ""}</span>
              </div>
            )}
            {collapsed && generated && (
              <div className="dv-generated" role="status">
                <span>{tt("Generated file ({reason}): {added} added, {removed} removed, not shown.", { reason: tt(GENERATED_REASON[generated.reason]), added: textSummary.added, removed: textSummary.removed })}</span>
                <button className="tgl" onClick={() => setRevealed(true)}>Show (H)</button>
              </div>
            )}
            <div className="dv-scroll">
              <div className="dv-body" hidden={collapsed} ref={bodyRef} onScroll={(e) => { const top = e.currentTarget.scrollTop; savedScrollTop.current = top; setScrollTop(top); }}
                style={{ ["--dv-chars" as string]: `${maxLen + 2}ch` }}>
                <span className="dv-probe" aria-hidden><span ref={probeRef}>{"0".repeat(100)}</span></span>
                <div className={`dv-canvas ${mode} ${wrap && !compact ? "wrap" : ""}`} style={{ height: total }}>
                  {visibleRows.map((r, k) => renderRow(r, first + k))}
                </div>
              </div>
              <div className="dv-ruler" hidden={collapsed} title={tt("Changes overview: click to jump")}
                tabIndex={0} role="slider" aria-orientation="vertical" aria-label={tt("Changes overview: click to jump")}
                aria-valuemin={changes > 0 ? 1 : 0} aria-valuemax={changes} aria-valuenow={cur >= 0 ? cur + 1 : 0}
                onKeyDown={(e) => {
                  // Up and down move between changes, Home and End go to the first and last.
                  if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Home" || e.key === "End") {
                    e.preventDefault();
                    e.stopPropagation();
                    if (e.key === "ArrowDown") next();
                    else if (e.key === "ArrowUp") prev();
                    else goBlock(e.key === "Home" ? 0 : blocks.length - 1);
                  }
                }}
                onMouseDown={(e) => {
                  const rr = e.currentTarget.getBoundingClientRect();
                  const el = bodyRef.current;
                  if (el) el.scrollTop = ((e.clientY - rr.top) / rr.height) * total - el.clientHeight / 2;
                }}>
                {compact && rulerBlocks.map(({ start, length, op }) => (
                  <i key={start} className={`m ${op.tag === "insert" ? "ins" : op.tag === "delete" ? "del" : "chg"}`} style={{ top: `${topAt(start) / Math.max(total, 1) * 100}%`, height: `max(3px, ${length * rowH / Math.max(total, 1) * 100}%)` }} />
                ))}
                {!compact && rows.length > 0 && rulerBuckets(rulerMarks, total, Math.min(rows.length, Math.max(1, Math.floor(viewH / 3)))).map((b) => (
                  <i key={b.first} className={`m ${b.tag} ${cur >= b.first && cur <= b.last ? "cur" : ""}`} data-count={b.count} style={{ top: `${(b.top / Math.max(total, 1)) * 100}%`, height: `max(3px, ${(b.height / Math.max(total, 1)) * 100}%)` }} />
                ))}
                <b className="view" style={{ top: `${(scrollTop / Math.max(total, 1)) * 100}%`, height: `${Math.min(100, (viewH / Math.max(total, 1)) * 100)}%` }} />
              </div>
              {ai && (
                <CompareExplanation
                  outcome={ai.outcome}
                  scope={ai.scope}
                  hunkCount={changes}
                  activeHunk={cur}
                  selectionScope={readSelectionScope()}
                  stale={!isSnapshotCurrent(ai.asked, explainSnapshot)}
                  onScope={(nextScope) => askAi(nextScope)}
                  onHunk={(hunkIndex) => goBlock(hunkIndex)}
                  onEvidence={(side, line) => jumpTo(side === "left" ? "l" : "r", line - 1)}
                  onCancel={cancelAi}
                  onClose={closeAi}
                />
              )}
              {semanticOpen && !compact && semantic?.verdict === "different" && (
                <SemanticDetails
                  changes={semantic.changes}
                  vim={vim}
                  onJump={(side, line) => jumpTo(side, line - 1)}
                  onClose={() => { setSemanticOpen(false); rootRef.current?.focus(); }}
                />
              )}
              {outlineOpen && !compact && (
                outline && "Ok" in outline
                  ? <CompareOutline nodes={outline.Ok.nodes} truncated={outline.Ok.truncated} ops={tx!.ops} side={outlineSide} onSwitchSide={() => setOutlineSide((v) => (v === "a" ? "b" : "a"))} onJump={(line) => jumpTo(outlineSide === "a" ? "l" : "r", line - 1)} onClose={() => { setOutlineOpen(false); rootRef.current?.focus(); }} />
                  : (
                    <div className="dv-outline" tabIndex={-1} onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setOutlineOpen(false); rootRef.current?.focus(); } }}>
                      <div className="dv-outline-head">
                        <b>{tt("Outline")} · {outlineSide === "a" ? tt("left") : tt("right")}</b>
                        <button className="icon-btn" aria-label={tt("Outline of the other side (O)")} title={tt("Outline of the other side (O)")} onClick={() => setOutlineSide((v) => (v === "a" ? "b" : "a"))}>⇄</button>
                        <button className="icon-btn" aria-label={tt("Close outline (Esc)")} title={tt("Close outline (Esc)")} onClick={() => { setOutlineOpen(false); rootRef.current?.focus(); }}>×</button>
                      </div>
                      <p className="dv-outline-empty" role="status">{!tx ? tt("The outline is not available for large files.") : outline ? outlineErrorMessage(outline.Err) : tt("Reading the outline…")}</p>
                    </div>
                  )
              )}
            </div>
          </div>
        )}
        {/* Mode-specific shortcut footers: image, table, hex, or text diff. */}
        {imageMode ? (
          <footer className="dv-foot">
            <span><kbd>1</kbd>–<kbd>4</kbd> {tt("mode")}</span>
            <span><kbd>[</kbd>/<kbd>]</kbd> {tt("divider / opacity")}</span>
            <span><kbd>s</kbd> swap</span>
            <span><kbd>Esc</kbd> close</span>
          </footer>
        ) : tableMode || docMode ? (
          <footer className="dv-foot">
            <span><kbd>n</kbd>/<kbd>N</kbd> {tt("next/prev change")}</span>
            <span><kbd>c</kbd> {tt("changes only")}</span>
            <span><kbd>s</kbd> swap</span>
            <span><kbd>Esc</kbd> close</span>
          </footer>
        ) : hexMode ? (
          <footer className="dv-foot">
            <span><kbd>n</kbd>/<kbd>N</kbd> or <kbd>F8</kbd> {tt("next/prev difference")}</span>
            <span><kbd>j</kbd>/<kbd>k</kbd> scroll</span>
            <span><kbd>g</kbd>/<kbd>G</kbd> start/end</span>
            <span><kbd>Esc</kbd> close</span>
          </footer>
        ) : (
        <footer className="dv-foot">
          <span><kbd>n</kbd>/<kbd>N</kbd> {tt("next/prev change")}</span>
          <span><kbd>u</kbd> view</span>
          <span><kbd>w</kbd> whitespace</span>
          <span><kbd>t</kbd> structural</span>
          <span><kbd>x</kbd>/<kbd>X</kbd> moved block</span>
          <span><kbd>o</kbd> {tt("outline")}</span>
          {semantic?.verdict === "different" && <span><kbd>S</kbd> {tt("syntax changes")}</span>}
          <span><kbd>E</kbd> {tt("explain")}</span>
          {generated && <span><kbd>H</kbd> {tt("generated")}</span>}
          <span><kbd>z</kbd> fold</span>
          <span><kbd>l</kbd> wrap</span>
          <span><kbd>s</kbd> swap</span>
          {allowChanges && (editable(0) || editable(1)) && <span><kbd>{tt("Alt")}</kbd>+<kbd>←</kbd>/<kbd>→</kbd> copy change · <kbd>{tt("Ctrl")}</kbd>+<kbd>S</kbd> save</span>}
          <span><kbd>y</kbd> {tt("copy patch")}</span>
          {!wait && <span><kbd>p</kbd> {tt("apply patch")}</span>}
          {canBlame && <span><kbd>B</kbd> {tt("blame")}</span>}
          {!wait && openCompareTabs.length > 1 && <span><kbd>{tt("Ctrl")}</kbd>+<kbd>Tab</kbd>{vim && <> / <kbd>gt</kbd>·<kbd>gT</kbd></>} {tt("tabs")}</span>}
          <span><kbd>Esc</kbd> close</span>
        </footer>
        )}
      </div>
    </div>
  );
}
