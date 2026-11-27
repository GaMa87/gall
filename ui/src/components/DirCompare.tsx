// Folder comparison: one "zipper" list with the left entry, a status column and
// the right entry on each row. Filter by status, open file diffs, copy single
// rows, or preview and run a sync (update / mirror, either way) that is one
// undoable operation.
import { activeFilterCount, formOf, parseFilters, type FilterForm } from "../core/dirFilters";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { tt } from "../i18n";
import { api } from "../api/ipc";
import type { DirCmpOptions, DirCmpResult, DirItem, DirMeta } from "../api/types";
import { formatDate, formatSize } from "../core/format";
import { describePlan, planSync, sidePath, syncBlocked, visibleRows, type Direction, type SyncMode, type SyncPlan } from "../core/syncPlan";
import * as A from "../state/actions";
import { refreshAll } from "../state/controllers";
import { finishCompareWait, useCompareWaitClose } from "../state/launch";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";

type Filter = "diff" | "all" | "different" | "leftOnly" | "rightOnly" | "same";
const OPTS_KEY = "dircmp:opts";

// Last result per pair, so coming back from a file diff is instant.
const cache = new Map<string, { res: DirCmpResult; collapsed: Set<string>; filter: Filter }>();

/** Saved options, tolerating a damaged or older entry: every field is checked, nothing odd reaches the core. */
function loadOpts(): DirCmpOptions {
  const base: DirCmpOptions = { content: false, hidden: false, respectGitignore: false };
  try {
    const saved = JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}") as Record<string, unknown>;
    const flag = (v: unknown) => v === true;
    const globs = (v: unknown) => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);
    const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    return { content: flag(saved.content), hidden: flag(saved.hidden), respectGitignore: flag(saved.respectGitignore), include: globs(saved.include), exclude: globs(saved.exclude), minSize: num(saved.minSize), maxSize: num(saved.maxSize), modifiedAfter: num(saved.modifiedAfter), modifiedBefore: num(saved.modifiedBefore) };
  } catch {
    return base;
  }
}

const STATUS_GLYPH: Record<DirItem["status"], string> = { same: "=", different: "≠", leftOnly: "◧", rightOnly: "◨" };
const STATUS_TEXT: Record<DirItem["status"], string> = { same: "identical", different: "differs", leftOnly: "only on the left", rightOnly: "only on the right" };
const REASON: Record<string, string> = { content: "content differs", size: "size differs", date: "modified at a different time", type: "file on one side, folder on the other", unreadable: "could not be read, so its bytes were not compared" };

/** How long a shown sync plan stays valid (ten minutes). */
const MAX_PLAN_AGE_MS = 10 * 60 * 1000;

export function DirCompare({ left: l0, right: r0, wait }: { left: string; right: string; wait?: boolean }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const [sides, setSides] = useState<[string, string]>([l0, r0]);
  const key = `${sides[0]}\u0000${sides[1]}`;
  const cached = cache.get(key);
  const [opts, setOpts] = useState<DirCmpOptions>(loadOpts);
  const [res, setRes] = useState<DirCmpResult | null>(cached?.res ?? null);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>(cached?.filter ?? "diff");
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(cached?.collapsed ?? new Set());
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [cursor, setCursor] = useState(0);
  // The filters are typed in a form and applied together (Enter or the button): a scan per keystroke would be wasteful.
  const [showFilters, setShowFilters] = useState(false);
  const [form, setForm] = useState<FilterForm>(() => formOf(loadOpts()));
  const [formError, setFormError] = useState("");
  // Bumped by the `f` key: the form is shown and its first field takes the focus once it is in the page.
  const [focusFilters, setFocusFilters] = useState(0);
  useEffect(() => { if (focusFilters) (rootRef.current?.querySelector(".dc-filters input") as HTMLInputElement | null)?.focus(); }, [focusFilters]);
  const applyFilters = () => {
    // Not while a sync runs: a new scan would end it half way.
    if (busy) return;
    const parsed = parseFilters(form);
    if (!parsed.ok) { setFormError(tt(parsed.error)); return; }
    setFormError("");
    setOpts((o) => ({ ...o, ...parsed.fields }));
  };
  const clearFilters = () => { setForm(formOf({})); setFormError(""); setOpts((o) => ({ ...o, include: [], exclude: [], minSize: null, maxSize: null, modifiedAfter: null, modifiedBefore: null })); };
  const [plan, setPlan] = useState<SyncPlan | null>(null);
  const [busy, setBusy] = useState(false);
  // Set before the first await: a second Enter in the same tick still sees the old `busy`.
  const running = useRef(false);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const listRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const rowH = Math.round(fontSize * 1.9);

  const request = useRef(0);
  const activeScan = useRef<DirCmpResult | null>(null);
  const run = useCallback((keepView = false) => {
    const id = ++request.current;
    activeScan.current = null;
    setPlan(null);
    setLoading(true);
    setErr(null);
    api.dircmpCompare(sides[0], sides[1], opts)
      .then((r) => {
        if (request.current !== id) return;
        activeScan.current = r;
        setRes(r);
        setSel(new Set());
        if (!keepView) {
          // Identical folders start collapsed; folders with differences open.
          setCollapsed(new Set(r.items.filter((i) => i.isDir && i.status === "same").map((i) => i.rel)));
          setCursor(0);
        }
      })
      .catch((e) => { if (request.current === id) setErr(String(e)); })
      .finally(() => { if (request.current === id) setLoading(false); });
  }, [sides, opts]);
  // Cached rows are for browsing while a fresh scan runs. Keep the view on option changes.
  const mounted = useRef(false);
  const prevSides = useRef(sides);
  useLayoutEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      run(!!cached);
      return;
    }
    const swapped = prevSides.current !== sides;
    prevSides.current = sides;
    if (swapped) setRes(null);
    run(!swapped);
  }, [run]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { localStorage.setItem(OPTS_KEY, JSON.stringify(opts)); }, [opts]);
  useEffect(() => { if (res && res === activeScan.current) cache.set(key, { res, collapsed, filter }); }, [res, collapsed, filter, key]);
  useEffect(() => { rootRef.current?.focus(); }, []);
  useEffect(() => () => { ++request.current; activeScan.current = null; api.dircmpCancel().catch(() => {}); }, []);

  const items = res?.items ?? [];
  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const show = (it: DirItem) => {
      if (q && !it.rel.toLowerCase().includes(q)) return false;
      if (it.isDir && it.status !== "leftOnly" && it.status !== "rightOnly" && it.reason !== "type") return filter === "all" || (filter === "same" && it.status === "same");
      if (filter === "all") return true;
      if (filter === "diff") return it.status !== "same";
      return it.status === filter;
    };
    return visibleRows(items, show, collapsed);
  }, [items, filter, query, collapsed]);

  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    setViewH(el.clientHeight);
    return () => ro.disconnect();
  }, [res]);

  const scrollTo = (i: number) => {
    const el = listRef.current;
    if (!el) return;
    const top = i * rowH;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + rowH > el.scrollTop + el.clientHeight) el.scrollTop = top + rowH - el.clientHeight;
  };
  const moveCursor = (i: number) => {
    const n = Math.max(0, Math.min(rows.length - 1, i));
    setCursor(n);
    scrollTo(n);
  };

  const toggleDir = (rel: string, open?: boolean) =>
    setCollapsed((c) => {
      const n = new Set(c);
      const isOpen = !n.has(rel);
      if (open ?? !isOpen) n.delete(rel);
      else n.add(rel);
      return n;
    });

  const openItem = (it: DirItem) => {
    if (!res) return;
    if (it.isDir) return toggleDir(it.rel);
    if (!it.left || !it.right) return A.notify(`${it.name} exists only on the ${it.left ? "left" : "right"}`, "info");
    A.openOverlay({ type: "diff", a: { kind: "file", path: sidePath(res.left, it, "left") }, b: { kind: "file", path: sidePath(res.right, it, "right") }, wait,
      back: { type: "dircmp", left: sides[0], right: sides[1], wait } });
  };

  // A plan is a picture of the folders at one moment: one shown for a long time is dropped and the folders are read again.
  const plannedAt = useRef(0);
  const execute = async (p: SyncPlan) => {
    if (!res || res !== activeScan.current) return;
    if (Date.now() - plannedAt.current > MAX_PLAN_AGE_MS) {
      setPlan(null);
      A.notify(tt("This plan is out of date. The folders are compared again; check the plan and apply it."), "warn");
      run(true);
      return;
    }
    const blocked = syncBlocked(p.mode, res);
    if (blocked) return A.notify(tt(blocked), "warn");
    if (!p.ops.length) return A.notify("Nothing to sync", "info");
    // A second run would cancel the first one midway (one cancel flag for the dialog).
    if (running.current) return;
    const scanRequest = request.current;
    running.current = true;
    setBusy(true);
    try {
      const r = await api.syncApply(p.ops, p.checks);
      A.notify(`Synced: ${r.copied} copied${r.replaced ? ` (${r.replaced} replaced, old versions in trash)` : ""}${r.trashed ? `, ${r.trashed} moved to trash` : ""}`, "success", A.undoAction(r.undoId));
      A.refreshUndo();
      refreshAll();
      A.invalidateAllFolderSizes();
      if (request.current === scanRequest) run(true);
    } catch (e) {
      A.fail(e);
      A.invalidateAllFolderSizes(); // part of the plan may be applied
      if (request.current === scanRequest) run(true);
    } finally {
      running.current = false;
      setBusy(false);
    }
  };
  /** The folders a plan copies or trashes as a whole get the fingerprint of everything below them (hidden files
   *  included), so a file that appears there after the preview stops the write instead of being copied or trashed. */
  const withTrees = async (p: SyncPlan): Promise<SyncPlan | null> => {
    const folders = p.checks.filter((c) => c.dir);
    const files = p.checks.filter((c) => !c.dir && c.stamp);
    if (!folders.length && !files.length) return p;
    try {
      const [trees, exact] = await Promise.all([
        folders.length ? api.syncTreeFingerprints(folders.map((c) => c.path)) : Promise.resolve([] as string[]),
        files.length ? api.syncFileFingerprints(files.map((c) => c.path)) : Promise.resolve([] as (string | null)[]),
      ]);
      const treeOf = new Map(folders.map((c, i) => [c.path, trees[i]] as const));
      const exactOf = new Map(files.map((c, i) => [c.path, exact[i]] as const));
      return { ...p, checks: p.checks.map((c) => (c.dir ? { ...c, tree: treeOf.get(c.path) } : exactOf.get(c.path) ? { ...c, exact: exactOf.get(c.path) ?? undefined } : c)) };
    } catch (e) { A.fail(e); return null; }
  };
  const preview = async (dir: Direction, mode: SyncMode, only?: Set<string>) => {
    if (!res || res !== activeScan.current) return;
    const blocked = syncBlocked(mode, res);
    if (blocked) return A.notify(tt(blocked), "warn");
    const planned = planSync(res, res.left, res.right, dir, mode, mode === "selected" ? (only ?? (sel.size ? sel : new Set([rows[cursor]?.rel].filter(Boolean) as string[]))) : undefined);
    if (!planned.ops.length) return A.notify(mode === "update" ? "Nothing newer or missing on that side" : "Nothing to copy", "info");
    const scan = res;
    const p = await withTrees(planned);
    // The scan may have been replaced while the folders were looked at.
    if (!p || scan !== activeScan.current) return;
    plannedAt.current = Date.now();
    // A single row copied by hand runs straight away (it is undoable); bigger syncs preview first.
    if (mode === "selected" && p.ops.length <= 1 && p.trashes === 0) return execute(p);
    setPlan(p);
  };
  const bulkBlocked = res && syncBlocked("update", res);
  const selectedBlocked = !res || loading || !!err || busy || !!syncBlocked("selected", res);
  const syncMenu = (e: React.MouseEvent) => A.showMenuFor(e, [
    { label: "Update right  (copy missing + newer →)", run: () => preview("toRight", "update") },
    { label: "Update left  (← copy missing + newer)", run: () => preview("toLeft", "update") },
    { separator: true },
    { label: "Mirror to right  (right = left, extras to trash)", run: () => preview("toRight", "mirror") },
    { label: "Mirror to left  (left = right, extras to trash)", run: () => preview("toLeft", "mirror") },
  ]);

  const leave = () => {
    if (wait) {
      if (!running.current) void finishCompareWait(err ? "error" : res ? "accepted" : "cancelled", undefined, err ?? undefined);
    } else A.openOverlay(null);
  };
  const close = () => (plan ? setPlan(null) : leave());
  useCompareWaitClose(wait, leave);
  useEffect(() => {
    if (wait && err) void finishCompareWait("error", undefined, err);
  }, [wait, err]);
  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if ((e.target as HTMLElement).tagName === "INPUT") {
      if (e.key === "Escape" || e.key === "Enter" || e.key === "ArrowDown") { e.preventDefault(); rootRef.current?.focus(); }
      return;
    }
    const it = rows[cursor];
    const k = e.key;
    if (running.current && ["s", "r", "c", "."].includes(k)) {
      e.preventDefault();
      return;
    }
    if (k === "Escape" || k === "q") close();
    else if (k === "j" || k === "ArrowDown") moveCursor(cursor + 1);
    else if (k === "k" || k === "ArrowUp") moveCursor(cursor - 1);
    else if (k === "PageDown") moveCursor(cursor + Math.floor(viewH / rowH));
    else if (k === "PageUp") moveCursor(cursor - Math.floor(viewH / rowH));
    else if (k === "g" || k === "Home") moveCursor(0);
    else if (k === "G" || k === "End") moveCursor(rows.length - 1);
    else if (k === "Enter" && plan) execute(plan).then(() => setPlan(null));
    else if (k === "Enter" && it) openItem(it);
    else if ((k === "l" || k === "ArrowRight") && it?.isDir) toggleDir(it.rel, true);
    else if ((k === "h" || k === "ArrowLeft") && it?.isDir) toggleDir(it.rel, false);
    else if (k === " " && it) setSel((s) => { const n = new Set(s); n.has(it.rel) ? n.delete(it.rel) : n.add(it.rel); return n; });
    else if (k === "]" || k === "n") { const i = rows.findIndex((r, j) => j > cursor && r.status !== "same" && !r.isDir); if (i >= 0) moveCursor(i); }
    else if (k === "[" || k === "N") { for (let i = cursor - 1; i >= 0; i--) if (rows[i].status !== "same" && !rows[i].isDir) { moveCursor(i); break; } }
    else if (k === ">") preview("toRight", "selected");
    else if (k === "<") preview("toLeft", "selected");
    else if (k === "s") setSides(([a, b]) => [b, a]);
    else if (k === "r") run(true);
    else if (k === "c") setOpts((o) => ({ ...o, content: !o.content }));
    else if (k === "C" && opts.content) setOpts((o) => ({ ...o, fresh: !o.fresh }));
    else if (k === ".") setOpts((o) => ({ ...o, hidden: !o.hidden }));
    else if (k === "f" && !busy) { setShowFilters(true); setFocusFilters((n) => n + 1); }
    else if (k === "F" && !busy) clearFilters();
    else if (k === "/") { e.preventDefault(); (rootRef.current?.querySelector(".dc-search") as HTMLInputElement | null)?.focus(); return; }
    else if (k === "a") setFilter((f) => (f === "all" ? "diff" : "all"));
    else return;
    e.preventDefault();
  };

  const c = res?.counts;
  const chip = (f: Filter, label: string, n?: number, cls = "") => (
    <button className={`dc-chip ${cls} ${filter === f ? "on" : ""}`} onClick={() => setFilter(f)}>{label}{n != null && <b>{n}</b>}</button>
  );
  const metaCell = (m: DirMeta | null, it: DirItem, side: "l" | "r") => {
    if (!m) return <div className={`dc-side ${side} missing`} />;
    const newer = it.newer === (side === "l" ? "left" : "right");
    return (
      <div className={`dc-side ${side}`} style={{ paddingLeft: side === "l" ? 8 + it.depth * 16 : 8 }}>
        {it.isDir ? (
          <button className="dc-twisty" title={tt("Expand or collapse folder")} onClick={(e) => { e.stopPropagation(); toggleDir(it.rel); }}>
            <Icon name={collapsed.has(it.rel) ? "chevronRight" : "chevronDown"} size={12} />
          </button>
        ) : <span className="dc-twisty" />}
        <FileIcon entry={{ isDir: it.isDir, ext: it.name.split(".").pop() ?? "", name: it.name }} size={15} />
        <span className="dc-name">{it.name}</span>
        {!it.isDir && <span className="dc-size">{formatSize(m.size)}</span>}
        <span className={`dc-date ${newer ? "newer" : ""}`} title={newer ? "newer" : undefined}>{formatDate(m.mtime * 1000)}</span>
      </div>
    );
  };

  const first = Math.max(0, Math.floor(scrollTop / rowH) - 15);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewH) / rowH) + 15);
  const actionGlyph = (a?: string) => (a === "copyRight" ? "→" : a === "copyLeft" ? "←" : a === "trashLeft" || a === "trashRight" ? "🗑" : "");

  return (
    <div className="backdrop glass center" onMouseDown={leave}>
      <div className="dircmp" ref={rootRef} tabIndex={-1} onKeyDown={onKey} onMouseDown={(e) => e.stopPropagation()}>
        <header className="dc-head">
          <div className="dc-paths">
            <div className="dc-path l" title={sides[0]}><Icon name="folder" size={15} /><span>{"‎" + sides[0] + "‎"}</span></div>
            <button className="icon-btn" disabled={busy} title={tt("Swap sides (s)")} onClick={() => setSides(([a, b]) => [b, a])}><Icon name="compare" size={15} /></button>
            <div className="dc-path r" title={sides[1]}><Icon name="folder" size={15} /><span>{"‎" + sides[1] + "‎"}</span></div>
            <button className="icon-btn" title={tt("Close (Esc)")} onClick={leave}><Icon name="close" size={15} /></button>
          </div>
          <div className="dc-bar">
            <div className="dc-chips">
              {chip("diff", "Differences", c ? c.different + c.leftOnly + c.rightOnly : undefined)}
              {chip("different", "≠ Changed", c?.different, "chg")}
              {chip("leftOnly", "◧ Left only", c?.leftOnly, "lo")}
              {chip("rightOnly", "◨ Right only", c?.rightOnly, "ro")}
              {chip("same", "= Same", c?.same)}
              {chip("all", "All")}
            </div>
            <input className="dc-search" value={query} placeholder={tt("Filter paths… (/)")} spellCheck={false} onChange={(e) => setQuery(e.target.value)} />
            <div className="dv-toggles">
              <button className={`tgl ${opts.content ? "on" : ""}`} disabled={busy} title={tt("Compare file contents, not just size and date (c)")} onClick={() => setOpts((o) => ({ ...o, content: !o.content }))}>{"{ }"}</button>
              {opts.content && <button className={`tgl ${opts.fresh ? "on" : ""}`} disabled={busy} aria-pressed={!!opts.fresh} title={tt("Read every file again, ignoring remembered results (C)")} onClick={() => setOpts((o) => ({ ...o, fresh: !o.fresh }))}>↻</button>}
              <button className={`tgl ${opts.hidden ? "on" : ""}`} disabled={busy} title={tt("Include hidden files (.)")} onClick={() => setOpts((o) => ({ ...o, hidden: !o.hidden }))}>.*</button>
              <button className={`tgl ${showFilters || activeFilterCount(opts) > 0 ? "on" : ""}`} disabled={busy} aria-expanded={showFilters} title={tt("Filters: include and exclude patterns, sizes, dates (f, Shift+F clears)")} onClick={() => setShowFilters((v) => !v)}>▽{activeFilterCount(opts) > 0 ? ` ${activeFilterCount(opts)}` : ""}</button>
              <button className={`tgl ${opts.respectGitignore ? "on" : ""}`} disabled={busy} title={tt("Skip files ignored by .gitignore")} onClick={() => setOpts((o) => ({ ...o, respectGitignore: !o.respectGitignore }))}>git</button>
            </div>
            <span className="spacer" />
            <button className="dc-btn" disabled={selectedBlocked} title={tt("Copy the selected rows (or the row under the cursor) to the right (>)")} onClick={() => preview("toRight", "selected")}>Copy →</button>
            <button className="dc-btn" disabled={selectedBlocked} title={tt("Copy the selected rows to the left (<)")} onClick={() => preview("toLeft", "selected")}>← Copy</button>
            <button className="dc-btn primary" disabled={loading || !!err || busy || !res || !!bulkBlocked} title={bulkBlocked ? tt(bulkBlocked) : undefined} onClick={syncMenu}>{tt("Sync")} <Icon name="chevronDown" size={12} /></button>
            <button className="icon-btn" disabled={busy} title={tt("Compare again (r)")} onClick={() => run(true)}><Icon name="refresh" size={15} /></button>
          </div>
          {showFilters && (
            <div className="dc-filters" role="group" aria-label={tt("Filters")} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") { e.preventDefault(); applyFilters(); } if (e.key === "Escape") { e.preventDefault(); setShowFilters(false); rootRef.current?.focus(); } }}>
              <label>{tt("Only")}<input disabled={busy} value={form.include} placeholder="*.rs, src/**" spellCheck={false} onChange={(e) => setForm({ ...form, include: e.target.value })} /></label>
              <label>{tt("Leave out")}<input disabled={busy} value={form.exclude} placeholder="*.log, node_modules/" spellCheck={false} onChange={(e) => setForm({ ...form, exclude: e.target.value })} /></label>
              <label>{tt("Size from")}<input disabled={busy} value={form.minSize} placeholder="10k" spellCheck={false} onChange={(e) => setForm({ ...form, minSize: e.target.value })} /></label>
              <label>{tt("to")}<input disabled={busy} value={form.maxSize} placeholder="2 MB" spellCheck={false} onChange={(e) => setForm({ ...form, maxSize: e.target.value })} /></label>
              <label>{tt("Modified from")}<input disabled={busy} value={form.after} placeholder="2025-01-31" spellCheck={false} onChange={(e) => setForm({ ...form, after: e.target.value })} /></label>
              <label>{tt("until")}<input disabled={busy} value={form.before} placeholder="2025-12-31" spellCheck={false} onChange={(e) => setForm({ ...form, before: e.target.value })} /></label>
              <button className="dc-btn primary" disabled={busy} onClick={applyFilters}>{tt("Apply")}</button>
              <button className="dc-btn" disabled={busy} onClick={clearFilters}>{tt("Clear")}</button>
              {formError && <span className="dc-filter-error" role="alert">{formError}</span>}
              <span className="dc-filter-note">{tt("Left out on one side is left out on both. A folder that holds left-out files is never copied or trashed as a whole.")}</span>
            </div>
          )}
        </header>

        {plan && (
          <div className="dc-plan">
            <Icon name="compare" size={16} />
            <span><b>{plan.mode === "mirror" ? "Mirror" : plan.mode === "update" ? "Update" : "Copy"}</b> {describePlan(plan)} · {formatSize(plan.bytes)}</span>
            <small>{tt("Replaced files and removed items go to the trash; the whole sync is one Undo.")}</small>
            <span className="spacer" />
            <button className="dc-btn" onClick={() => setPlan(null)}>{tt("Cancel")}</button>
            <button className="dc-btn primary" disabled={busy} onClick={() => execute(plan).then(() => setPlan(null))}>{busy ? "Syncing…" : "Run (Enter)"}</button>
          </div>
        )}
        {err && <div className="dv-msg err"><Icon name="close" size={16} /> {err}</div>}
        {loading && <div className="dc-loading">{tt("Comparing folders…")}</div>}
        {res?.incompleteReasons.length ? <div className="dv-msg">{tt("The folder scan is incomplete. Sync is unavailable.")}</div> : null}
        {res?.truncated && <div className="dv-msg">Stopped after {(300000).toLocaleString()} entries per side, so the list is incomplete and Sync is off; compare a subfolder instead.</div>}
        {res && !loading && !err && !bulkBlocked && rows.length === 0 && (
          <div className="dv-msg same"><Icon name="check" size={16} /> {filter === "diff" ? `No differences (${c?.same ?? 0} identical files${opts.content ? ", contents compared" : ", by size and date"})` : "Nothing to show for this filter"}</div>
        )}

        <div className="dc-list" ref={listRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
          <div style={{ height: rows.length * rowH, position: "relative" }}>
            {rows.slice(first, last).map((it, k) => {
              const i = first + k;
              const act = plan?.actions.get(it.rel);
              return (
                <div key={it.rel} className={`dc-row ${it.status} ${i === cursor ? "cursor" : ""} ${sel.has(it.rel) ? "sel" : ""} ${plan ? (act ? "planned" : "dim") : ""}`}
                  style={{ top: i * rowH, height: rowH }} data-rel={it.rel}
                  onClick={(e) => {
                    setCursor(i);
                    if (e.ctrlKey || e.metaKey) setSel((s) => { const n = new Set(s); n.has(it.rel) ? n.delete(it.rel) : n.add(it.rel); return n; });
                    else setSel(new Set());
                  }}
                  onDoubleClick={() => openItem(it)}>
                  {metaCell(it.left, it, "l")}
                  <div className="dc-mid" title={`${STATUS_TEXT[it.status]}${it.reason ? ` · ${REASON[it.reason]}` : ""}${it.newer ? ` · ${it.newer} is newer` : ""}`}>
                    {act ? <span className={`dc-act ${act}`}>{actionGlyph(act)}</span> : (
                      <>
                        <button className="dc-copy l" title={tt("Copy to the left")} onClick={(e) => { e.stopPropagation(); setCursor(i); setSel(new Set([it.rel])); void preview("toLeft", "selected", new Set([it.rel])); }} disabled={selectedBlocked || it.status === "same" || it.status === "leftOnly"}>‹</button>
                        <span className="dc-glyph">{it.newer === "left" && it.status === "different" ? "◂" : ""}{STATUS_GLYPH[it.status]}{it.newer === "right" && it.status === "different" ? "▸" : ""}</span>
                        <button className="dc-copy r" title={tt("Copy to the right")} onClick={(e) => { e.stopPropagation(); setCursor(i); setSel(new Set([it.rel])); void preview("toRight", "selected", new Set([it.rel])); }} disabled={selectedBlocked || it.status === "same" || it.status === "rightOnly"}>›</button>
                      </>
                    )}
                  </div>
                  {metaCell(it.right, it, "r")}
                </div>
              );
            })}
          </div>
        </div>
        <footer className="dv-foot">
          <span><kbd>Enter</kbd> {tt("open diff / folder")}</span>
          <span><kbd>n</kbd>/<kbd>N</kbd> {tt("next/prev difference")}</span>
          <span><kbd>{tt("Space")}</kbd> select</span>
          <span><kbd>&gt;</kbd>/<kbd>&lt;</kbd> {tt("copy right/left")}</span>
          <span><kbd>c</kbd> contents</span>
          <span><kbd>a</kbd> all/diff</span>
          <span><kbd>f</kbd> filters</span>
          <span><kbd>Esc</kbd> close</span>
          {res && <span className="spacer" />}
          {res && <span>{res.items.length.toLocaleString()} entries · {res.elapsedMs} ms</span>}
        </footer>
      </div>
    </div>
  );
}
