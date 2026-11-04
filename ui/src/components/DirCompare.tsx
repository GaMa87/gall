// Folder comparison: one "zipper" list with the left entry, a status column and
// the right entry on each row. Filter by status, open file diffs, copy single
// rows, or preview and run a sync (update / mirror, either way) that is one
// undoable operation.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { DirCmpOptions, DirCmpResult, DirItem, DirMeta } from "../api/types";
import { formatDate, formatSize } from "../core/format";
import { joinRel } from "../core/path";
import { describePlan, planSync, visibleRows, type Direction, type SyncMode, type SyncPlan } from "../core/syncPlan";
import * as A from "../state/actions";
import { refreshAll } from "../state/controllers";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";

type Filter = "diff" | "all" | "different" | "leftOnly" | "rightOnly" | "same";
const OPTS_KEY = "dircmp:opts";

// Last result per pair, so coming back from a file diff is instant.
const cache = new Map<string, { res: DirCmpResult; collapsed: Set<string>; filter: Filter }>();

function loadOpts(): DirCmpOptions {
  try {
    return { content: false, hidden: false, respectGitignore: false, ...JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}") };
  } catch {
    return { content: false, hidden: false, respectGitignore: false };
  }
}

const STATUS_GLYPH: Record<DirItem["status"], string> = { same: "=", different: "≠", leftOnly: "◧", rightOnly: "◨" };
const STATUS_TEXT: Record<DirItem["status"], string> = { same: "identical", different: "differs", leftOnly: "only on the left", rightOnly: "only on the right" };
const REASON: Record<string, string> = { content: "content differs", size: "size differs", date: "modified at a different time", type: "file on one side, folder on the other" };

export function DirCompare({ left: l0, right: r0 }: { left: string; right: string }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const [sides, setSides] = useState<[string, string]>([l0, r0]);
  const key = `${sides[0]}\u0000${sides[1]}`;
  const cached = cache.get(key);
  const [opts, setOpts] = useState<DirCmpOptions>(loadOpts);
  const [res, setRes] = useState<DirCmpResult | null>(cached?.res ?? null);
  const [loading, setLoading] = useState(!cached);
  const [err, setErr] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>(cached?.filter ?? "diff");
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(cached?.collapsed ?? new Set());
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [cursor, setCursor] = useState(0);
  const [plan, setPlan] = useState<SyncPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const listRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const rowH = Math.round(fontSize * 1.9);

  const run = useCallback((keepView = false) => {
    setLoading(true);
    setErr(null);
    api.dircmpCompare(sides[0], sides[1], opts)
      .then((r) => {
        setRes(r);
        setPlan(null);
        setSel(new Set());
        if (!keepView) {
          // Identical folders start collapsed; folders with differences open.
          setCollapsed(new Set(r.items.filter((i) => i.isDir && i.status === "same").map((i) => i.rel)));
          setCursor(0);
        }
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, [sides, opts]);
  // First open: reuse the cached result if any. Later: swapped sides = fresh view,
  // changed options = compare again and keep the view.
  const mounted = useRef(false);
  const prevSides = useRef(sides);
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      if (!cached) run();
      return;
    }
    const swapped = prevSides.current !== sides;
    prevSides.current = sides;
    run(!swapped);
  }, [run]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { localStorage.setItem(OPTS_KEY, JSON.stringify(opts)); }, [opts]);
  useEffect(() => { if (res) cache.set(key, { res, collapsed, filter }); }, [res, collapsed, filter, key]);
  useEffect(() => { rootRef.current?.focus(); }, []);
  useEffect(() => () => { api.dircmpCancel().catch(() => {}); }, []);

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
    if (it.isDir) return toggleDir(it.rel);
    if (!it.left || !it.right) return A.notify(`${it.name} exists only on the ${it.left ? "left" : "right"}`, "info");
    A.openOverlay({ type: "diff", a: { kind: "file", path: joinRel(sides[0], it.rel) }, b: { kind: "file", path: joinRel(sides[1], it.rel) }, back: { type: "dircmp", left: sides[0], right: sides[1] } });
  };

  const execute = async (p: SyncPlan) => {
    if (!p.ops.length) return A.notify("Nothing to sync", "info");
    setBusy(true);
    try {
      const r = await api.syncApply(p.ops);
      A.notify(`Synced: ${r.copied} copied${r.replaced ? ` (${r.replaced} replaced, old versions in trash)` : ""}${r.trashed ? `, ${r.trashed} moved to trash` : ""}`, "success", { label: "Undo", run: () => A.undo() });
      A.refreshUndo();
      refreshAll();
      run(true);
    } catch (e) {
      A.fail(e);
      run(true);
    } finally {
      setBusy(false);
    }
  };
  const preview = (dir: Direction, mode: SyncMode) => {
    const p = planSync(items, sides[0], sides[1], dir, mode, mode === "selected" ? (sel.size ? sel : new Set([rows[cursor]?.rel].filter(Boolean) as string[])) : undefined);
    if (!p.ops.length) return A.notify(mode === "update" ? "Nothing newer or missing on that side" : "Nothing to copy", "info");
    // A single row copied by hand runs straight away (it is undoable); bigger syncs preview first.
    if (mode === "selected" && p.ops.length <= 1 && p.trashes === 0) return execute(p);
    setPlan(p);
  };
  const syncMenu = (e: React.MouseEvent) => A.showMenu(e.clientX, e.clientY, [
    { label: "Update right  (copy missing + newer →)", run: () => preview("toRight", "update") },
    { label: "Update left  (← copy missing + newer)", run: () => preview("toLeft", "update") },
    { separator: true },
    { label: "Mirror to right  (right = left, extras to trash)", run: () => preview("toRight", "mirror") },
    { label: "Mirror to left  (left = right, extras to trash)", run: () => preview("toLeft", "mirror") },
  ]);

  const close = () => (plan ? setPlan(null) : A.openOverlay(null));
  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if ((e.target as HTMLElement).tagName === "INPUT") {
      if (e.key === "Escape" || e.key === "Enter" || e.key === "ArrowDown") { e.preventDefault(); rootRef.current?.focus(); }
      return;
    }
    const it = rows[cursor];
    const k = e.key;
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
    else if (k === ".") setOpts((o) => ({ ...o, hidden: !o.hidden }));
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
          <button className="dc-twisty" onClick={(e) => { e.stopPropagation(); toggleDir(it.rel); }}>
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
    <div className="backdrop glass center" onMouseDown={() => A.openOverlay(null)}>
      <div className="dircmp" ref={rootRef} tabIndex={-1} onKeyDown={onKey} onMouseDown={(e) => e.stopPropagation()}>
        <header className="dc-head">
          <div className="dc-paths">
            <div className="dc-path l" title={sides[0]}><Icon name="folder" size={15} /><span>{"‎" + sides[0] + "‎"}</span></div>
            <button className="icon-btn" title="Swap sides (s)" onClick={() => setSides(([a, b]) => [b, a])}><Icon name="compare" size={15} /></button>
            <div className="dc-path r" title={sides[1]}><Icon name="folder" size={15} /><span>{"‎" + sides[1] + "‎"}</span></div>
            <button className="icon-btn" title="Close (Esc)" onClick={() => A.openOverlay(null)}><Icon name="close" size={15} /></button>
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
            <input className="dc-search" value={query} placeholder="Filter paths… (/)" spellCheck={false} onChange={(e) => setQuery(e.target.value)} />
            <div className="dv-toggles">
              <button className={`tgl ${opts.content ? "on" : ""}`} title="Compare file contents, not just size and date (c)" onClick={() => setOpts((o) => ({ ...o, content: !o.content }))}>{"{ }"}</button>
              <button className={`tgl ${opts.hidden ? "on" : ""}`} title="Include hidden files (.)" onClick={() => setOpts((o) => ({ ...o, hidden: !o.hidden }))}>.*</button>
              <button className={`tgl ${opts.respectGitignore ? "on" : ""}`} title="Skip files ignored by .gitignore" onClick={() => setOpts((o) => ({ ...o, respectGitignore: !o.respectGitignore }))}>git</button>
            </div>
            <span className="spacer" />
            <button className="dc-btn" disabled={!res || busy} title="Copy the selected rows (or the row under the cursor) to the right (>)" onClick={() => preview("toRight", "selected")}>Copy →</button>
            <button className="dc-btn" disabled={!res || busy} title="Copy the selected rows to the left (<)" onClick={() => preview("toLeft", "selected")}>← Copy</button>
            <button className="dc-btn primary" disabled={!res || busy} onClick={syncMenu}>Sync <Icon name="chevronDown" size={12} /></button>
            <button className="icon-btn" title="Compare again (r)" onClick={() => run(true)}><Icon name="refresh" size={15} /></button>
          </div>
        </header>

        {plan && (
          <div className="dc-plan">
            <Icon name="compare" size={16} />
            <span><b>{plan.mode === "mirror" ? "Mirror" : plan.mode === "update" ? "Update" : "Copy"}</b> {describePlan(plan)} · {formatSize(plan.bytes)}</span>
            <small>Replaced files and removed items go to the trash; the whole sync is one Undo.</small>
            <span className="spacer" />
            <button className="dc-btn" onClick={() => setPlan(null)}>Cancel</button>
            <button className="dc-btn primary" disabled={busy} onClick={() => execute(plan).then(() => setPlan(null))}>{busy ? "Syncing…" : "Run (Enter)"}</button>
          </div>
        )}
        {err && <div className="dv-msg err"><Icon name="close" size={16} /> {err}</div>}
        {loading && <div className="dc-loading">Comparing folders…</div>}
        {res?.truncated && <div className="dv-msg">Stopped after {(300000).toLocaleString()} entries per side; narrow the comparison to a subfolder.</div>}
        {res && !loading && rows.length === 0 && (
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
                        <button className="dc-copy l" title="Copy to the left" onClick={(e) => { e.stopPropagation(); setCursor(i); setSel(new Set([it.rel])); execute(planSync(items, sides[0], sides[1], "toLeft", "selected", new Set([it.rel]))); }} disabled={it.status === "same" || it.status === "leftOnly"}>‹</button>
                        <span className="dc-glyph">{it.newer === "left" && it.status === "different" ? "◂" : ""}{STATUS_GLYPH[it.status]}{it.newer === "right" && it.status === "different" ? "▸" : ""}</span>
                        <button className="dc-copy r" title="Copy to the right" onClick={(e) => { e.stopPropagation(); setCursor(i); setSel(new Set([it.rel])); execute(planSync(items, sides[0], sides[1], "toRight", "selected", new Set([it.rel]))); }} disabled={it.status === "same" || it.status === "rightOnly"}>›</button>
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
          <span><kbd>Enter</kbd> open diff / folder</span>
          <span><kbd>n</kbd>/<kbd>N</kbd> next/prev difference</span>
          <span><kbd>Space</kbd> select</span>
          <span><kbd>&gt;</kbd>/<kbd>&lt;</kbd> copy right/left</span>
          <span><kbd>c</kbd> contents</span>
          <span><kbd>a</kbd> all/diff</span>
          <span><kbd>Esc</kbd> close</span>
          {res && <span className="spacer" />}
          {res && <span>{res.items.length.toLocaleString()} entries · {res.elapsedMs} ms</span>}
        </footer>
      </div>
    </div>
  );
}
