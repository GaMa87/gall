import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { journalKinds, journalNodes, jumpPreview, refMove, type JournalView } from "../core/gitJournal";
import { useGitKeys } from "../state/gitKeys";
import { formatLocale, tt } from "../i18n";
import "./GitJournal.css";

type Props = {
  path: string;
  /** The journal is read again when this changes. */
  revision: unknown;
  busy: boolean;
  onJump: (view: JournalView, applied: number) => void;
  onClose: () => void;
};

/** The operation journal as a timeline (`U`): every node is a state to go back or forward to. */
export function GitJournal({ path, revision, busy, onJump, onClose }: Props) {
  const { vim } = useGitKeys();
  const [view, setView] = useState<JournalView | null>(null), [error, setError] = useState(""), [selected, setSelected] = useState<number | null>(null);
  const root = useRef<HTMLElement>(null), request = useRef(0);
  const load = useCallback(() => {
    const ticket = ++request.current;
    invoke<JournalView>("git_journal", { path }).then((value) => { if (ticket === request.current) { setView(value); setError(""); } }).catch((e) => { if (ticket === request.current) setError(String(e)); });
  }, [path]);
  useEffect(() => { setSelected(null); load(); return () => { request.current++; }; }, [load, revision]);
  useEffect(() => { root.current?.focus(); }, []);
  useEffect(() => { root.current?.querySelector(".gj-node.selected")?.scrollIntoView({ block: "nearest" }); }, [selected, view]);
  const nodes = view ? journalNodes(view) : [], at = Math.max(0, nodes.findIndex((node) => node.applied === (selected ?? view?.applied)));
  const chosen = nodes[at], preview = view && chosen ? jumpPreview(view, chosen.applied) : null;
  const blocked = view && preview ? (preview.undo ? view.undoBlocked : preview.redo ? view.redoBlocked : null) : null;
  const jump = () => { if (view && chosen && preview && !busy && !blocked && preview.undo + preview.redo > 0) onJump(view, chosen.applied); };
  return <section className="gj" ref={root} tabIndex={-1} role="dialog" aria-label={tt("Operation history")} onKeyDown={(e) => {
    e.stopPropagation();
    const go = (step: number) => { e.preventDefault(); const next = nodes[Math.max(0, Math.min(at + step, nodes.length - 1))]; if (next) setSelected(next.applied); };
    if (e.key === "Escape" || (vim && e.key === "q")) { e.preventDefault(); onClose(); }
    else if (e.ctrlKey || e.metaKey || e.altKey) return;
    else if (e.key === "ArrowDown" || (vim && e.key === "j")) go(1);
    else if (e.key === "ArrowUp" || (vim && e.key === "k")) go(-1);
    else if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); jump(); }
  }}>
    <div className="gj-head"><strong>{tt("Operation history")}</strong><button aria-label={tt("Close operation history")} title={tt("Close (Esc)")} onClick={onClose}>✕</button></div>
    {error && <p className="gj-error" role="alert">{error}</p>}
    {view && !nodes.length && <p className="gj-empty">{tt("Commits, merges, rebases, resets, checkouts and other operations done in Gall appear here and can be undone.")}</p>}
    <div className="gj-list" role="listbox" aria-label={tt("Operations, newest first")}>{nodes.map((node, i) => <button key={node.applied} role="option" aria-selected={i === at} tabIndex={-1} disabled={busy} className={`gj-node ${node.state} ${i === at ? "selected" : ""}`} onClick={() => { setSelected(node.applied); root.current?.focus(); }} onDoubleClick={jump}>
      <i aria-hidden="true" />
      <span className="gj-text"><b>{node.entry ? tt(journalKinds[node.entry.kind]) : tt("Before these operations")}</b>{node.entry && <span title={node.entry.label}>{node.entry.label}</span>}{node.state === "current" && <em>{tt("Now")}</em>}</span>
      {node.entry && <small>{refMove(node.entry.before, node.entry.after)} · {new Date(node.entry.time * 1000).toLocaleString(formatLocale())}</small>}
    </button>)}</div>
    {preview && <div className="gj-foot">
      {blocked ? <p className="gj-error" role="alert">{blocked}</p> : preview.moves.length ? <p>{preview.undo ? tt("Undoes {count}:", { count: preview.undo }) : tt("Redoes {count}:", { count: preview.redo })}{preview.moves.map((move) => <code key={move}>{move}</code>)}</p> : <p>{tt("This is the current state.")}</p>}
      <button disabled={busy || !!blocked || preview.undo + preview.redo === 0} onClick={jump}>{preview.redo ? tt("Go forward to here…") : tt("Go back to here…")}</button>
    </div>}
  </section>;
}
