import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { age, ageWords } from "../core/age";
import { eventAnchor, textAnchor } from "../core/anchor";
import type { JournalView } from "../core/gitJournal";
import { reflogRows, searchReflog, type ReflogEntry, type ReflogRow } from "../core/gitReflog";
import { useGitKeys } from "../state/gitKeys";
import * as A from "../state/actions";
import { tt } from "../i18n";
import "./GitJournal.css";
import "./GitReflog.css";

type Props = {
  path: string;
  /** Read again when this changes. */
  revision: unknown;
  busy: boolean;
  onShow: (id: string) => void;
  onRestore: (action: "reset" | "createBranch", id: string) => void;
  onTimeline: () => void;
  onClose: () => void;
};
const PAGE = 300;

/** Where HEAD and the branches have been (`H`), with the operations of Gall: the place to look for lost work. */
export function GitReflog({ path, revision, busy, onShow, onRestore, onTimeline, onClose }: Props) {
  const { vim } = useGitKeys();
  const [entries, setEntries] = useState<ReflogEntry[] | null>(null), [journal, setJournal] = useState<JournalView | null>(null), [truncated, setTruncated] = useState(false), [error, setError] = useState("");
  const [query, setQuery] = useState(""), [at, setAt] = useState(0), [count, setCount] = useState(PAGE);
  const root = useRef<HTMLElement>(null), input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    let alive = true;
    invoke<{ entries: ReflogEntry[]; truncated: boolean }>("git_reflog", { path }).then((value) => { if (alive) { setEntries(value.entries); setTruncated(value.truncated); setError(""); } }).catch((e) => { if (alive) setError(String(e)); });
    invoke<JournalView>("git_journal", { path }).then((value) => { if (alive) setJournal(value); }).catch(() => {});
    return () => { alive = false; };
  }, [path, revision]);
  useEffect(() => { root.current?.focus(); }, []);
  const rows = useMemo(() => reflogRows(entries ?? [], journal), [entries, journal]);
  const found = useMemo(() => searchReflog(rows, query), [rows, query]);
  const index = Math.min(at, Math.max(0, found.length - 1)), row = found[index];
  useEffect(() => { root.current?.querySelector(".gr-row.on")?.scrollIntoView({ block: "nearest" }); }, [index, count]);
  const go = (step: number) => {
    const next = Math.max(0, Math.min(index + step, found.length - 1));
    setAt(next);
    if (next >= count - 5) setCount((n) => n + PAGE);
  };
  const restore = (target: ReflogRow | undefined, event?: React.MouseEvent) => {
    if (!target || busy) return;
    const element = root.current?.querySelector(".gr-row.on") ?? null;
    A.showMenuAt(event ? eventAnchor(event) : textAnchor(element, element), [
      { label: tt("Show this commit"), run: () => onShow(target.id) },
      { separator: true },
      { label: tt("New branch here…"), accel: "n", run: () => onRestore("createBranch", target.id) },
      { label: tt("Reset the current branch to here…"), accel: "r", danger: true, run: () => onRestore("reset", target.id) },
      { separator: true },
      { label: tt("Operation history (undo timeline)"), run: onTimeline },
      { label: tt("Copy full commit ID"), run: () => { invoke("clip_set_text", { text: target.id }).catch(A.fail); } },
    ]);
  };
  const now = Date.now();
  return <section className="gj gr" ref={root} tabIndex={-1} role="dialog" aria-label={tt("Reflog")} onKeyDown={(e) => {
    e.stopPropagation();
    const typing = e.target === input.current;
    if (e.key === "Escape" || (!typing && vim && e.key === "q")) { e.preventDefault(); if (typing && query) setQuery(""); else if (typing) root.current?.focus(); else onClose(); }
    else if (e.ctrlKey || e.metaKey || e.altKey) return;
    else if (e.key === "ArrowDown" || (!typing && vim && e.key === "j")) { e.preventDefault(); go(1); }
    else if (e.key === "ArrowUp" || (!typing && vim && e.key === "k")) { e.preventDefault(); go(-1); }
    else if (e.key === "PageDown" || e.key === "PageUp") { e.preventDefault(); go(e.key === "PageDown" ? 12 : -12); }
    else if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); if (row) onShow(row.id); }
    else if (!typing && e.key === "/") { e.preventDefault(); input.current?.focus(); }
    else if (!typing && e.key === "R") { e.preventDefault(); restore(row); }
  }}>
    <div className="gj-head"><strong>{tt("Reflog")}</strong><button aria-label={tt("Close reflog")} title={tt("Close (Esc)")} onClick={onClose}>✕</button></div>
    <div className="gr-search"><input ref={input} type="search" spellCheck={false} aria-label={tt("Search the reflog")} placeholder={tt("Search: action, branch, message, hash…")} value={query} onChange={(e) => { setQuery(e.target.value); setAt(0); setCount(PAGE); }} /></div>
    {error && <p className="gj-error gr-note" role="alert">{error}</p>}
    <div className="gj-list gr-list" role="listbox" aria-label={tt("Reflog entries, newest first")} onScroll={(e) => { const el = e.currentTarget; if (el.scrollTop + el.clientHeight > el.scrollHeight - 200 && count < found.length) setCount((n) => n + PAGE); }}>
      {found.slice(0, count).map((item, i) => <div key={item.key} role="option" aria-selected={i === index} className={`gr-row ${i === index ? "on" : ""} ${item.gall ? "gall" : ""}`} onClick={() => { setAt(i); root.current?.focus(); }} onDoubleClick={() => onShow(item.id)} onContextMenu={(e) => { e.preventDefault(); setAt(i); restore(item, e); }} title={`${item.gall ? tt("State before this operation of Gall") : item.key}\n${item.id}\n${item.message}`}>
        <div><code>{item.id.slice(0, 8)}</code><b>{item.gall ? tt(item.action) : item.action}</b><span>{item.message}</span></div>
        <small>{item.gall ? tt("Gall") : item.key} · <time title={new Date(item.time * 1000).toLocaleString()}>{age(item.time * 1000, now).label}</time></small>
      </div>)}
      {entries && !found.length && <p className="gj-empty">{query ? tt("No entry matches.") : tt("The reflog is empty.")}</p>}
      {!entries && !error && <p className="gj-empty" role="status">{tt("Loading reflog…")}</p>}
      {truncated && count >= found.length && <p className="gj-empty">{tt("Older entries are not shown.")}</p>}
    </div>
    {row && <div className="gj-foot"><p>{row.gall ? tt("State before: {what}", { what: `${tt(row.action)} ${row.message}` }) : `${row.action ? `${row.action}: ` : ""}${row.message}`} · {tt("{value} ago", { value: ageWords(row.time * 1000, now) })}</p><div className="gr-actions"><button disabled={busy} onClick={() => onShow(row.id)}>{tt("Show this commit")}</button><button disabled={busy} onClick={(e) => restore(row, e)}>{tt("Restore…")}{vim && <kbd>R</kbd>}</button></div></div>}
  </section>;
}
