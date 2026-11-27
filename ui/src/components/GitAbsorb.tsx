import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { rankCommits, touchedHunks, type AbsorbProposal } from "../core/gitAbsorb";
import type { LineRange } from "../core/gitFileHistory";
import * as A from "../state/actions";
import { tt } from "../i18n";
import { createProgressId } from "../core/gitProgress";
import { GitProgressBar, useGitProgress } from "./GitProgressBar";
import "./GitJournal.css";
import "./GitAbsorb.css";

/** `file` (and its lines) goes into a chosen commit; without it every staged hunk is proposed its commit. */
export type AbsorbRequest = { file?: string; range?: LineRange | null; target?: string };
type Props = { path: string; request: AbsorbRequest; onBusy: (busy: boolean) => void; onDone: () => void; onClose: () => void };
let nextTicket = 0;

/** Staged changes into the commits they belong to (`b`), or into one that is chosen (`F`). */
export function GitAbsorb({ path, request, onBusy, onDone, onClose }: Props) {
  const [proposal, setProposal] = useState<AbsorbProposal | null>(null), [error, setError] = useState(""), [busy, setBusy] = useState(false);
  const [proposeProgressId, setProposeProgressId] = useState<string | null>(null);
  const [applyProgressId, setApplyProgressId] = useState<string | null>(null);
  const [off, setOff] = useState<Set<number>>(new Set()), [query, setQuery] = useState(""), [at, setAt] = useState(0);
  const root = useRef<HTMLElement>(null), input = useRef<HTMLInputElement>(null);
  const picking = !!request.file;
  const mounted = useRef(false);
  const proposeProgress = useGitProgress(proposeProgressId, { operation: "absorb", phase: "Analyzing staged changes", cancellable: true });
  const applyProgress = useGitProgress(applyProgressId, { operation: "absorb", phase: "Absorbing changes", cancellable: false });
  useEffect(() => {
    const ticket = ++nextTicket;
    const progressId = createProgressId("absorb-propose");
    setProposeProgressId(progressId);
    let alive = true, finished = false;
    mounted.current = true;
    invoke<AbsorbProposal>("git_absorb_propose", { path, ticket, progressId })
      .then((value) => { if (alive) setProposal(value); })
      .catch((e) => { if (alive) setError(String(e)); })
      .finally(() => { finished = true; if (alive) setProposeProgressId(null); });
    return () => {
      alive = false;
      mounted.current = false;
      if (!finished) void invoke("git_absorb_cancel", { ticket }).catch(() => {});
    };
  }, [path]);
  useEffect(() => { ((picking && input.current) || root.current)?.focus(); }, [picking, proposal]);
  const subjects = useMemo(() => new Map((proposal?.stack ?? []).map((commit) => [commit.id, commit.subject])), [proposal]);
  const chosen = useMemo(() => proposal && request.file ? touchedHunks(proposal.hunks, request.file, request.range ?? null) : [], [proposal, request]);
  const commits = useMemo(() => rankCommits(proposal?.stack ?? [], query), [proposal, query]);
  const index = Math.min(at, Math.max(0, commits.length - 1));
  useEffect(() => { root.current?.querySelector(".ab-commit.on")?.scrollIntoView({ block: "nearest" }); }, [index]);
  const absorbed = (proposal?.hunks ?? []).filter((hunk) => hunk.target && !off.has(hunk.index));
  const run = async (assignments: { hunk: number; target: string }[]) => {
    if (!proposal || busy || !assignments.length) return;
    const targets = [...new Set(assignments.map((item) => item.target))];
    setBusy(true); onBusy(true); setError("");
    try {
      const list = targets.map((id) => `${id.slice(0, 8)} ${subjects.get(id) ?? ""}`).join("\n");
      if (!await A.confirm(tt("Put {count} staged changes into their commits?", { count: assignments.length }), tt("{list}\n\nThese commits and those after them get new IDs. Working files stay as they are, and the old HEAD is kept for an undo. When anything would not fit, nothing happens.", { list }), tt("Rewrite commits"), true)) return;
      if (!mounted.current) return;
      const progressId = createProgressId("absorb-apply");
      setApplyProgressId(progressId);
      const count = await invoke<number>("git_absorb_apply", { path, assignments, expected: proposal.token, progressId });
      A.notify(tt("{changes} changes absorbed, {count} commits rewritten", { changes: assignments.length, count }), "success");
      if (mounted.current) onDone();
    } catch (e) { if (mounted.current) setError(String(e)); }
    finally { setApplyProgressId(null); setBusy(false); onBusy(false); }
  };
  const into = (target: string | undefined) => { if (target) void run(chosen.map((hunk) => ({ hunk: hunk.index, target }))); };
  // Dropped on a commit: only the confirmation is left.
  const dropped = useRef(false);
  useEffect(() => {
    if (!proposal || !request.target || dropped.current) return;
    dropped.current = true;
    if (!chosen.length) setError(tt("Nothing here can be put into a commit."));
    else if (!subjects.has(request.target)) setError(tt("This commit cannot be rewritten here: it is pushed, behind a merge or too far back."));
    else into(request.target);
  }, [proposal]); // eslint-disable-line react-hooks/exhaustive-deps
  return <section className="gj ab" ref={root} tabIndex={-1} role="dialog" aria-label={tt("Absorb staged changes")} onKeyDown={(e) => {
    e.stopPropagation();
    if (e.key === "Escape") { e.preventDefault(); if (!busy) onClose(); }
    else if (picking && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setAt(Math.max(0, Math.min(index + (e.key === "ArrowDown" ? 1 : -1), commits.length - 1))); }
    else if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); if (picking) into(commits[index]?.id); else void run(absorbed.map((hunk) => ({ hunk: hunk.index, target: hunk.target! }))); }
    else if (!picking && ["j", "k", "ArrowDown", "ArrowUp"].includes(e.key) && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      const boxes = [...root.current!.querySelectorAll<HTMLElement>(".ab-hunk input:not(:disabled)")], now = boxes.indexOf(document.activeElement as HTMLElement);
      boxes[Math.max(0, Math.min(boxes.length - 1, now + (e.key === "j" || e.key === "ArrowDown" ? 1 : -1)))]?.focus();
    }
  }}>
    <div className="gj-head"><strong>{picking ? tt("Put changes into a commit") : tt("Absorb staged changes")}</strong><button aria-label={tt("Close")} title={tt("Close (Esc)")} disabled={busy} onClick={onClose}>✕</button></div>
    {error && <p className="gj-error ab-note" role="alert">{error}</p>}
    {applyProgress && <GitProgressBar progress={applyProgress} compact />}
    {!proposal && !error && (proposeProgress ? <GitProgressBar progress={proposeProgress} onCancel={onClose} /> : <p className="gj-empty" role="status">{tt("Finding the commits of the staged changes…")}</p>)}
    {proposal && picking && <>
      <p className="ab-note">{proposal.reason ? tt(proposal.reason) : chosen.length ? tt("{count} staged changes of {file}", { count: chosen.length, file: request.file ?? "" }) : tt("Nothing here can be put into a commit.")}</p>
      <div className="gr-search"><input ref={input} type="search" spellCheck={false} aria-label={tt("Choose a commit")} placeholder={tt("Commit…")} value={query} onChange={(e) => { setQuery(e.target.value); setAt(0); }} /></div>
      <div className="gj-list" role="listbox" aria-label={tt("Commits, newest first")}>{commits.map((commit, i) => <div key={commit.id} role="option" aria-selected={i === index} className={`ab-commit ${i === index ? "on" : ""}`} onClick={() => { setAt(i); into(commit.id); }}><code>{commit.id.slice(0, 8)}</code> {commit.subject}</div>)}{!commits.length && <p className="gj-empty">{tt("No commit matches.")}</p>}</div>
    </>}
    {proposal && !picking && <>
      <div className="gj-list">{proposal.hunks.map((hunk) => <label key={hunk.index} className={`ab-hunk ${hunk.target ? "" : "left"}`}>
        <input type="checkbox" disabled={busy || !hunk.target} checked={!!hunk.target && !off.has(hunk.index)} onChange={() => setOff((old) => { const next = new Set(old); if (!next.delete(hunk.index)) next.add(hunk.index); return next; })} />
        <div>
          <b title={hunk.file}>{hunk.file}{hunk.newStart > 0 && `:${hunk.newStart}`}</b>
          <small>{hunk.target ? <>→ <code>{hunk.target.slice(0, 8)}</code> {subjects.get(hunk.target)} {!hunk.certain && <em>{tt("inserted between its lines")}</em>}</> : tt("stays staged: {reason}", { reason: tt(hunk.reason ?? "") })}</small>
          {hunk.lines.length > 0 && <pre>{hunk.lines.slice(0, 8).map((line, i) => <span key={i} className={line.startsWith("+") ? "gcd-added" : line.startsWith("-") ? "gcd-removed" : ""}>{line}{"\n"}</span>)}{hunk.lines.length > 8 && "…"}</pre>}
        </div>
      </label>)}{!proposal.hunks.length && <p className="gj-empty">{tt("Nothing is staged.")}</p>}</div>
      <div className="gj-foot"><p>{proposal.reason ? tt(proposal.reason) : tt("{count} of {total} staged changes are absorbed; the others stay staged.", { count: absorbed.length, total: proposal.hunks.length })}</p><button disabled={busy || !absorbed.length} onClick={() => void run(absorbed.map((hunk) => ({ hunk: hunk.index, target: hunk.target! })))}>{busy ? tt("Rewriting…") : tt("Absorb {count}…", { count: absorbed.length })}</button></div>
    </>}
  </section>;
}
