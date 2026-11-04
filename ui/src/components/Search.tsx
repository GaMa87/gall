// Search window (Q-01, Q-02): recursive wildcard search streamed from the core,
// plus a Tags tab listing items by colour.
import { useEffect, useRef, useState } from "react";
import { api, events } from "../api/ipc";
import type { FileEntry, Tagged } from "../api/types";
import { formatSize } from "../core/format";
import { ancestors, basename, parent } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { FileIcon, Icon, TAG_COLORS } from "./Icon";

let searchSeq = 1;

export function SearchWindow({ root }: { root: string }) {
  const [tab, setTab] = useState<"files" | "tags">("files");
  const [pattern, setPattern] = useState("");
  const [scope, setScope] = useState(root);
  const [hidden, setHidden] = useState(false);
  const [results, setResults] = useState<FileEntry[]>([]);
  const [running, setRunning] = useState(false);
  const [done, setDone] = useState<string>("");
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
    return () => {
      api.searchCancel();
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (tab === "tags") api.tagged(tagColor).then(setTagged).catch(A.fail);
  }, [tab, tagColor]);

  const start = () => {
    if (!pattern.trim()) return;
    id.current = searchSeq++;
    setResults([]);
    setSel(new Set());
    setRunning(true);
    setDone("");
    api.searchStart(id.current, scope, pattern, hidden).catch(A.fail);
  };

  const locate = (p: string) => {
    const b = A.activeBrowser();
    const dir = parent(p);
    A.openOverlay(null);
    if (dir) A.navigate(b, dir);
    setTimeout(() => controller(b)?.select([p], p), 200);
  };

  const openSelected = () => {
    const list = sel.size ? [...sel].map((i) => results[i]) : results.slice(0, 1);
    A.openEntries(A.activeBrowser(), list.filter(Boolean));
  };

  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal search-win" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <div className="shead">
          <button className={`btn ${tab === "files" ? "primary" : ""}`} onClick={() => setTab("files")}>Files</button>
          <button className={`btn ${tab === "tags" ? "primary" : ""}`} onClick={() => setTab("tags")}>Tags</button>
          {tab === "files" ? (
            <>
              <input ref={inputRef} type="text" placeholder="Name or wildcard: *.pdf, report?.txt" value={pattern}
                onChange={(e) => setPattern(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") { if (results.length && !running && e.ctrlKey) openSelected(); else start(); } }} />
              <select value={scope} onChange={(e) => setScope(e.target.value)} title="Search in">
                <option value={root}>This folder: {basename(root)}</option>
                {top !== root && <option value={top}>Whole drive: {top}</option>}
              </select>
              <label style={{ display: "flex", gap: 4, alignItems: "center" }}><input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> hidden</label>
              {running ? <button className="btn" onClick={() => api.searchCancel()}>Stop</button> : <button className="btn primary" onClick={start}>Search</button>}
            </>
          ) : (
            <div className="tags" style={{ gap: 4 }}>
              <button style={{ background: "var(--bg-3)", color: "var(--fg)", width: 40, height: 26, fontSize: 11 }} onClick={() => setTagColor(null)}>All</button>
              {TAG_COLORS.slice(1).map((c, i) => (
                <button key={i} style={{ background: c, width: 30, height: 26, borderColor: tagColor === i + 1 ? "var(--fg)" : "transparent" }} onClick={() => setTagColor(i + 1)}>{i + 1}</button>
              ))}
            </div>
          )}
        </div>
        <div className="results">
          {tab === "files"
            ? results.map((r, i) => (
                <div key={r.path} className={`r ${sel.has(i) ? "on" : ""}`}
                  onClick={(e) => setSel((s) => { const n = new Set(e.ctrlKey ? s : []); n.has(i) && e.ctrlKey ? n.delete(i) : n.add(i); return n; })}
                  onDoubleClick={() => A.openEntries(A.activeBrowser(), [r])}>
                  <button className="icon-btn" title="Locate in browser" onClick={(e) => { e.stopPropagation(); locate(r.path); }}><Icon name="locate" size={14} /></button>
                  <FileIcon entry={r} />
                  <span className="n">{r.name}</span>
                  <span className="p" title={r.path}><bdi>{parent(r.path)}</bdi></span>
                  <span style={{ textAlign: "right", color: "var(--fg-dim)" }}>{r.isDir ? "" : formatSize(r.size)}</span>
                </div>
              ))
            : tagged.map((t) => (
                <div key={t.path} className="r" onDoubleClick={() => locate(t.path)}>
                  <button className="icon-btn" title="Locate" onClick={() => locate(t.path)}><Icon name="locate" size={14} /></button>
                  <span className="tagdot" style={{ width: 10, height: 10, borderRadius: 5, background: TAG_COLORS[t.color] }} />
                  <span className="n">{basename(t.path)}</span>
                  <span className="p"><bdi>{parent(t.path)}</bdi></span>
                  <span />
                </div>
              ))}
        </div>
        <div className="foot">
          <span>{tab === "files" ? (running ? `Searching… ${results.length}` : done || "Wildcards: * any text, ? one character. Plain text matches anywhere in the name.") : `${tagged.length} tagged item${tagged.length === 1 ? "" : "s"}`}</span>
          {tab === "files" && results.length > 0 && <span>Enter: search again · Ctrl+Enter / double-click: open · ⌖ locate</span>}
        </div>
      </div>
    </div>
  );
}
