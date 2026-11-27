import { ThemedSelect } from "./ThemedSelect";
import { tt } from "../i18n";
// Search window (Q-01, Q-02): recursive wildcard search streamed from the core,
// plus a Tags tab listing items by colour.
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { api, events } from "../api/ipc";
import type { HiddenCount, SearchHit, Tagged } from "../api/types";
import { formatSize } from "../core/format";
import { emptyHiddenText, footerHiddenText } from "../core/hiddenHint";
import { ancestors, basename, parent } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { FileIcon, Icon, TAG_COLORS } from "./Icon";
import { primaryMod } from "../core/keymap";
import { platformLabel } from "../core/keymap";

let searchSeq = 1;

/** Shows the folder of `p` in the active browser, with `p` selected. */
function locate(p: string) {
  const b = A.activeBrowser();
  const dir = parent(p);
  A.openOverlay(null);
  if (dir) A.navigate(b, dir);
  setTimeout(() => controller(b)?.select([p], p), 200);
}

/** One found file. Memoized: results arrive in batches of a few hundred, and without it
 *  every batch rendered all rows so far again (thousands, each with its icon). */
const ResultRow = memo(function ResultRow({ entry: r, index, on, pick }: { entry: SearchHit; index: number; on: boolean; pick: (index: number, add: boolean) => void }) {
  return (
    <div className={`r ${on ? "on" : ""} ${r.hidden ? "is-hidden" : ""}`}
      onClick={(e) => pick(index, primaryMod(e))}
      onDoubleClick={() => A.openEntries(A.activeBrowser(), [r])}>
      <button className="icon-btn" title={tt("Locate in browser")} onClick={(e) => { e.stopPropagation(); locate(r.path); }}><Icon name="locate" size={14} /></button>
      <FileIcon entry={r} />
      <span className="n"><span className="nt">{r.name}</span>{r.hidden && <span className="hidden-badge">hidden</span>}</span>
      <span className="p" title={r.path}><bdi>{parent(r.path)}</bdi></span>
      <span style={{ textAlign: "right", color: "var(--fg-dim)" }}>{r.isDir ? "" : formatSize(r.size)}</span>
    </div>
  );
});

export function SearchWindow({ root, initialPattern, initialEverything }: { root: string; initialPattern?: string; initialEverything?: boolean }) {
  const [tab, setTab] = useState<"files" | "tags">("files");
  const [pattern, setPattern] = useState(initialPattern ?? "");
  const [scope, setScope] = useState(root);
  const [hidden, setHidden] = useState(!!initialEverything);
  // Also what Settings › Search excludes, .gitignore, .gallignore and cache folders.
  const [everything, setEverything] = useState(!!initialEverything);
  const [results, setResults] = useState<SearchHit[]>([]);
  const [running, setRunning] = useState(false);
  const [done, setDone] = useState<string>("");
  const [hiddenCount, setHiddenCount] = useState<HiddenCount | null>(null);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [tagColor, setTagColor] = useState<number | null>(null);
  const [tagged, setTagged] = useState<Tagged[]>([]);
  const id = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const top = ancestors(root)[0];

  useEffect(() => {
    inputRef.current?.focus();
    const un1 = events.searchBatch((b) => b.id === id.current && setResults((r) => [...r, ...b.entries]));
    const un2 = events.searchDone((d) => {
      if (d.id !== id.current) return;
      setRunning(false);
      setDone(`${d.hits} found${d.cancelled ? " (stopped)" : ""}`);
    });
    const un3 = events.searchHidden((h) => {
      if (h.id !== id.current) return;
      setHiddenCount(h.count > 0 ? { count: h.count, capped: h.capped } : null);
    });
    return () => {
      api.searchCancel();
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (tab === "tags") api.tagged(tagColor).then(setTagged).catch(A.fail);
  }, [tab, tagColor]);

  const start = (nextEverything = everything, nextHidden = hidden) => {
    if (!pattern.trim()) return;
    id.current = searchSeq++;
    setResults([]);
    setSel(new Set());
    setHiddenCount(null);
    setRunning(true);
    setDone("");
    api.searchStart(id.current, scope, pattern, nextHidden, nextEverything).catch(A.fail);
  };
  const toggleEverything = () => {
    const next = !everything;
    setEverything(next);
    if (pattern.trim()) start(next, hidden);
  };
  // Opened from Quick Open with a query: search right away.
  const autoStarted = useRef(false);
  useEffect(() => {
    if (autoStarted.current || !initialPattern?.trim()) return;
    autoStarted.current = true;
    start();
  });

  const pick = useCallback((i: number, add: boolean) => {
    setSel((s) => {
      const n = new Set(add ? s : []);
      if (add && n.has(i)) n.delete(i);
      else n.add(i);
      return n;
    });
  }, []);

  const openSelected = () => {
    const list = sel.size ? [...sel].map((i) => results[i]) : results.slice(0, 1);
    A.openEntries(A.activeBrowser(), list.filter(Boolean));
  };

  const showHiddenHint = tab === "files" && !everything && !!hiddenCount && hiddenCount.count > 0;

  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal search-win" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") A.openOverlay(null);
          if (tab === "files" && e.altKey && !e.ctrlKey && !e.metaKey && e.key.toLowerCase() === "i") {
            e.preventDefault();
            toggleEverything();
          }
        }}>
        <div className="shead">
          <button className={`btn ${tab === "files" ? "primary" : ""}`} onClick={() => setTab("files")}>{tt("Files")}</button>
          <button className={`btn ${tab === "tags" ? "primary" : ""}`} onClick={() => setTab("tags")}>{tt("Tags")}</button>
          {tab === "files" ? (
            <>
              <input ref={inputRef} type="text" placeholder={tt("Name or wildcard: *.pdf, report?.txt")} value={pattern}
                onChange={(e) => setPattern(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") { if (results.length && !running && primaryMod(e)) openSelected(); else start(); } }} />
              <ThemedSelect value={scope} onChange={(e) => setScope(e.target.value)} title={tt("Search in")}>
                <option value={root}>This folder: {basename(root)}</option>
                {top !== root && <option value={top}>Whole drive: {top}</option>}
              </ThemedSelect>
              <label style={{ display: "flex", gap: 4, alignItems: "center" }}><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> hidden</label>
              <label style={{ display: "flex", gap: 4, alignItems: "center" }} title={tt("Also search what is normally skipped: Settings › Search › Never index or search, .gitignore, .gallignore and cache folders (Alt+I)")}><input type="checkbox" checked={everything} onChange={(e) => { const next = e.target.checked; setEverything(next); if (pattern.trim() && (done || running)) start(next, hidden); }} /> ignored</label>
              {running ? <button className="btn" onClick={() => api.searchCancel()}>{tt("Stop")}</button> : <button className="btn primary" onClick={() => start()}>{tt("Search")}</button>}
            </>
          ) : (
            <div className="tags" style={{ gap: 4 }}>
              <button style={{ background: "var(--bg-3)", color: "var(--fg)", width: 40, height: 26, fontSize: 11 }} title={tt("Any colour tag")} onClick={() => setTagColor(null)}>{tt("All")}</button>
              {TAG_COLORS.slice(1).map((c, i) => (
                <button key={i} style={{ background: c, width: 30, height: 26, borderColor: tagColor === i + 1 ? "var(--fg)" : "transparent" }} title={`Colour tag ${i + 1}`} onClick={() => setTagColor(i + 1)}>{i + 1}</button>
              ))}
            </div>
          )}
        </div>
        <div className="results">
          {tab === "files"
            ? results.length > 0
              ? results.map((r, i) => <ResultRow key={r.path} entry={r} index={i} on={sel.has(i)} pick={pick} />)
              : !running && done ? (
                <div className="search-empty">
                  {showHiddenHint ? (
                    <>
                      <div>{tt("No results.")} <b>{emptyHiddenText(hiddenCount!)}</b>.</div>
                      <button className="hidden-action" onClick={() => { toggleEverything(); inputRef.current?.focus(); }}>
                        {tt("Press")} <kbd>{platformLabel("Alt+I")}</kbd> {tt("to include hidden files")}
                      </button>
                    </>
                  ) : "No results"}
                </div>
              ) : null
            : tagged.map((t) => (
                <div key={t.path} className="r" onDoubleClick={() => locate(t.path)}>
                  <button className="icon-btn" title={tt("Locate")} onClick={() => locate(t.path)}><Icon name="locate" size={14} /></button>
                  <span className="tagdot" style={{ width: 10, height: 10, borderRadius: 5, background: TAG_COLORS[t.color] }} />
                  <span className="n">{basename(t.path)}</span>
                  <span className="p"><bdi>{parent(t.path)}</bdi></span>
                  <span />
                </div>
              ))}
        </div>
        {tab === "files" && results.length > 0 && showHiddenHint && (
          <button className="hidden-bar" onClick={() => { toggleEverything(); inputRef.current?.focus(); }}>
            <Icon name="eye" size={13} />
            <span>{footerHiddenText(hiddenCount!)}, <kbd>{platformLabel("Alt+I")}</kbd> {tt("to include")}</span>
          </button>
        )}
        <div className="foot">
          <span>{tab === "files" ? (running ? `Searching… ${results.length}` : done || platformLabel("Wildcards: * any text, ? one character. Plain text matches anywhere in the name. Skips node_modules, .gitignore and .gallignore; Alt+I searches them too.")) : `${tagged.length} tagged item${tagged.length === 1 ? "" : "s"}`}</span>
          {tab === "files" && results.length > 0 && <span>{platformLabel("Enter: search again · Ctrl+Enter / double-click: open · ⌖ locate")}</span>}
        </div>
      </div>
    </div>
  );
}
