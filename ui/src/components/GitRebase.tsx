import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../state/store";
import * as A from "../state/actions";
import "./GitRebase.css";

type Action = "pick" | "reword" | "squash" | "fixup" | "drop" | "edit";
type Commit = { id: string; subject: string; message: string };
type Plan = { head: string; base: string; token: string; skippedMerges: number; commits: Commit[] };
type Entry = Commit & { action: Action };
const actions: Action[] = ["pick", "reword", "squash", "fixup", "drop", "edit"];
export function GitRebase({ path, commit, onto = false, onBusy, onChanged, onClose }: { path: string; commit?: string; onto?: boolean; onBusy: (busy: boolean) => void; onChanged: () => void; onClose: () => void }) {
  const [base, setBase] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState("");
  const [result, setResult] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [help, setHelp] = useState(false);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const ticket = useRef(0);
  const gate = useRef(false);
  const dragged = useRef<number | null>(null);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const load = useCallback(async (target: string) => {
    const current = ++ticket.current;
    setLoading(true); setError(""); setResult(""); setPlan(null); setEntries([]);
    try {
      const value = await invoke<Plan>("git_rebase_plan", { path, base: target });
      if (current === ticket.current) { setPlan(value); setEntries(value.commits.map((entry) => ({ ...entry, action: "pick" }))); setSelected(0); }
    } catch (e) { if (current === ticket.current) setError(String(e)); }
    finally { if (current === ticket.current) setLoading(false); }
  }, [path]);
  useEffect(() => {
    let active = true;
    if (commit) {
      const current = ++ticket.current;
      setLoading(true); setPlan(null); setEntries([]); setError("");
      if (onto) { setBase(commit); void load(commit); return () => { active = false; ticket.current++; }; }
      invoke<{ parents: string[] }>("git_commit_details", { path, commit, base: null }).then((details) => {
        if (!active || current !== ticket.current) return;
        const parent = details.parents[0];
        if (!parent) { setError("The root commit cannot be included in this editor. Select a later commit."); setLoading(false); return; }
        setBase(parent); void load(parent);
      }).catch((error) => { if (active && current === ticket.current) { setError(String(error)); setLoading(false); } });
    }
    root.current?.focus();
    return () => { active = false; ticket.current++; };
  }, [commit, load, onto, path]);
  useEffect(() => { root.current?.querySelector(".grw-entry.selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);
  const updateAction = (index: number, action: Action) => setEntries((current) => current.map((entry, i) => i === index ? { ...entry, action, message: action === "squash" && entry.action !== "squash" ? "" : action === "reword" && !entry.message ? plan?.commits.find((original) => original.id === entry.id)?.message ?? "" : entry.message } : entry));
  const move = (index: number, direction: number) => {
    const target = index + direction;
    if (target < 0 || target >= entries.length || busy) return;
    setEntries((current) => { const next = [...current]; [next[index], next[target]] = [next[target], next[index]]; return next; });
    setSelected(target);
  };
  const drop = (target: number) => {
    const source = dragged.current; dragged.current = null;
    if (source === null || source === target || busy) return;
    setEntries((current) => { const next = [...current]; const [entry] = next.splice(source, 1); next.splice(target, 0, entry); return next; }); setSelected(target);
  };
  const run = async () => {
    if (gate.current || !plan || !entries.length) return;
    gate.current = true; setBusy(true); onBusy(true); setError(""); setResult("");
    try {
      const counts = actions.map((action) => `${entries.filter((entry) => entry.action === action).length} ${action}`).filter((value) => !value.startsWith("0 "));
      if (!await A.confirm("Rewrite this commit range?", `Replay ${entries.length} commits after ${plan.base.slice(0, 12)} through ${plan.head.slice(0, 12)}.${plan.skippedMerges ? `\n\n${plan.skippedMerges} merge ${plan.skippedMerges === 1 ? "commit is" : "commits are"} flattened: its commits are replayed linearly and conflict resolutions made in the merge are not kept.` : ""}\n\n${counts.join(" · ")}\n\nThis changes commit IDs and may conflict. Avoid rewriting shared history. Gall retains the original HEAD as a recovery reference. Working files must be clean; commits are unsigned and repository hooks are disabled.`, "Start interactive rebase", true)) return;
      const output = await invoke<string>("git_rebase_execute", { path, base: plan.base, expected: plan.token, entries: entries.map(({ id, action, message }) => ({ id, action, message })) });
      setResult(output || "Rebase completed."); onChanged();
      setPlan(null); setEntries([]);
    } catch (e) { setError(String(e)); onChanged(); }
    finally { gate.current = false; setBusy(false); onBusy(false); }
  };
  const changedCount = entries.filter((entry) => entry.action !== "pick").length;
  const movedCount = entries.filter((entry, index) => plan?.commits[index]?.id !== entry.id).length;
  const outputCount = entries.filter((entry) => !["drop", "fixup", "squash"].includes(entry.action)).length;
  const destination = (index: number) => {
    let previous = { id: "", subject: "" };
    let found = false;
    for (const entry of entries.slice(0, index)) {
      if (["pick", "reword", "edit"].includes(entry.action)) { previous = { id: entry.id, subject: entry.action === "reword" && entry.message.trim() ? entry.message.trim().split("\n")[0] : entry.subject }; found = true; }
      else if (entry.action === "squash" && found && entry.message.trim()) previous = { id: previous.id, subject: entry.message.trim().split("\n")[0] };
    }
    return found ? previous : null;
  };
  return <div className="git-rebase" ref={root} tabIndex={-1} aria-label="Interactive rebase" aria-busy={busy || loading} onKeyDown={(e) => {
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    e.stopPropagation();
    if (e.key === "Escape" && !busy) { e.preventDefault(); onClose(); return; }
    if (e.key === "?" && vim) { e.preventDefault(); setHelp((value) => !value); return; }
    if ((e.ctrlKey && ["j", "k"].includes(e.key)) || (e.altKey && ["ArrowUp", "ArrowDown"].includes(e.key))) { e.preventDefault(); move(selected, e.key === "j" || e.key === "ArrowDown" ? -1 : 1); return; }
    if (e.key === "ArrowDown" || (vim && e.key === "j")) { e.preventDefault(); setSelected((index) => Math.max(0, index - 1)); }
    else if (e.key === "ArrowUp" || (vim && e.key === "k")) { e.preventDefault(); setSelected((index) => Math.min(index + 1, Math.max(0, entries.length - 1))); }
    else if (vim && e.key === "J") { e.preventDefault(); move(selected, -1); }
    else if (vim && e.key === "K") { e.preventDefault(); move(selected, 1); }
    else if (vim && !busy) {
      const action = ({ p: "pick", r: "reword", s: "squash", f: "fixup", d: "drop", e: "edit" } as Record<string, Action>)[e.key];
      if (action) { e.preventDefault(); updateAction(selected, action); }
    }
  }}>
    <div className="grw-heading">
      <div className="grw-hero">
        <span className="grw-hero-icon" aria-hidden="true"><svg viewBox="0 0 32 32" fill="none"><path d="M9 7v18M23 7v4c0 7-14 3-14 10" /><circle cx="9" cy="7" r="3" /><circle cx="23" cy="7" r="3" /><circle cx="9" cy="25" r="3" /></svg></span>
        <div className="grw-hero-copy"><span className="grw-eyebrow">History editor</span><strong>Edit commits through HEAD</strong><small>Reorder the story. Keep the changes that matter.</small></div>
      </div>
      <div className="grw-heading-actions"><button disabled={busy} onClick={() => setHelp((value) => !value)} aria-expanded={help} title="Rebase commands">? Commands</button><button className="grw-close" disabled={busy} onClick={onClose} aria-label="Close interactive rebase">✕</button></div>
    </div>
    <div className="grw-overview">
      <div className="grw-summary" aria-label="Rebase plan summary"><span className="grw-chip"><b>{entries.length}</b> {entries.length === 1 ? "commit" : "commits"}</span>{changedCount > 0 && <span className="grw-chip changed"><b>{changedCount}</b> {changedCount === 1 ? "action" : "actions"}</span>}{movedCount > 0 && <span className="grw-chip moved"><b>{movedCount}</b> moved</span>}<span className="grw-direction"><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 2v12m-4-4 4 4 4-4" /></svg>Newest first</span></div>
      <details className="grw-range-options"><summary aria-disabled={busy} onClick={(event) => { if (busy) event.preventDefault(); }}>Change range<svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg></summary><form className="grw-base" onSubmit={(e) => { e.preventDefault(); if (!busy) void load(base); }}><label>Unchanged base (exclusive)<input aria-label="Rebase base commit" value={base} disabled={busy} onChange={(e) => { setBase(e.target.value); setPlan(null); setEntries([]); }} placeholder="Full commit ID" /></label><button disabled={loading || busy || !base.trim()} type="submit">Load commits</button></form></details>
    </div>
    {plan && <div className="grw-range" aria-label="Commit range"><span className="grw-range-point"><i aria-hidden="true" /><span>Base</span><code title={plan.base}>{plan.base.slice(0, 8)}</code><small>unchanged</small></span><span className="grw-range-line" aria-hidden="true" /><span className="grw-range-point head"><i aria-hidden="true" /><span>HEAD</span><code title={plan.head}>{plan.head.slice(0, 8)}</code></span></div>}
    {plan && plan.skippedMerges > 0 && <p className="grw-note" role="note">{plan.skippedMerges} merge {plan.skippedMerges === 1 ? "commit is" : "commits are"} not listed; its commits are replayed linearly, like <code>git rebase -i</code>.</p>}
    {help && <div className="grw-help"><span><kbd>j</kbd>/<kbd>k</kbd> select</span><span><kbd>J</kbd>/<kbd>K</kbd> or <kbd>Ctrl+j</kbd>/<kbd>Ctrl+k</kbd> reorder</span><span><kbd>p</kbd> pick</span><span><kbd>r</kbd> reword</span><span><kbd>s</kbd> squash</span><span><kbd>f</kbd> fixup</span><span><kbd>d</kbd> drop</span><span><kbd>e</kbd> edit stop</span><span>Drag a card or use its arrows to change the order.</span></div>}
    {error && <p className="grw-error" role="alert">{error}</p>}
    {result && <pre className="grw-result" role="status">{result}</pre>}
    <div className="grw-entries">{loading ? <div className="grw-state" role="status"><span className="grw-loading-dot" /><strong>Loading your commits</strong><span>Preparing the history range…</span></div> : plan && !entries.length ? <div className="grw-state"><strong>No commits in this range</strong><span>Choose an earlier base to include commits through HEAD.</span></div> : !plan && !result && !error ? <div className="grw-state"><strong>Choose a range to begin</strong><span>Open Change range and select a base commit.</span></div> : [...entries.keys()].reverse().map((i) => {
      const entry = entries[i];
      const into = destination(i);
      const originalPosition = plan?.commits.findIndex((original) => original.id === entry.id) ?? i;
      return <div key={entry.id} className={`grw-entry ${selected === i ? "selected" : ""} ${entry.action} ${dragOver === i && dragging !== i ? "drag-over" : ""} ${dragging === i ? "dragging" : ""}`} tabIndex={0} draggable={!busy} onDragStart={(event) => {
        if ((event.target as HTMLElement).closest("input,textarea,select,button")) { event.preventDefault(); return; }
        dragged.current = i; setDragging(i); event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", entry.id);
      }} onDragOver={(event) => { if (dragged.current !== null && !busy) { event.preventDefault(); event.dataTransfer.dropEffect = "move"; setDragOver(i); } }} onDrop={(event) => { event.preventDefault(); drop(i); setDragOver(null); setDragging(null); }} onDragEnd={() => { dragged.current = null; setDragOver(null); setDragging(null); }} onFocus={() => setSelected(i)} onClick={() => setSelected(i)}>
        <span className="grw-step" aria-hidden="true">{i + 1}</span>
        <div className="grw-card"><div className="grw-entry-line">
          <span className="grw-drag" aria-hidden="true"><svg viewBox="0 0 12 20" fill="currentColor"><circle cx="3" cy="5" r="1.3" /><circle cx="9" cy="5" r="1.3" /><circle cx="3" cy="10" r="1.3" /><circle cx="9" cy="10" r="1.3" /><circle cx="3" cy="15" r="1.3" /><circle cx="9" cy="15" r="1.3" /></svg></span>
          <div className="grw-commit-copy"><strong className="grw-subject" title={entry.subject}>{entry.subject}</strong><div className="grw-commit-meta"><code title={entry.id}>{entry.id.slice(0, 8)}</code>{entry.id === plan?.head && <span className="grw-head-badge">HEAD</span>}{originalPosition !== i && <span className="grw-moved-badge">from {originalPosition + 1}</span>}</div></div>
          <span className="grw-action-picker"><select aria-label={`Action for ${entry.subject}`} value={entry.action} disabled={busy} onChange={(e) => updateAction(i, e.target.value as Action)}>{actions.map((action) => <option key={action} value={action}>{action[0].toUpperCase() + action.slice(1)}</option>)}</select><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg></span>
          <div className="grw-move-controls"><button disabled={busy || i === entries.length - 1} onClick={(event) => { event.stopPropagation(); move(i, 1); }} aria-label={`Move ${entry.subject} up`} title="Move up (newer)"><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 15V5m-4 4 4-4 4 4" /></svg></button><button disabled={busy || i === 0} onClick={(event) => { event.stopPropagation(); move(i, -1); }} aria-label={`Move ${entry.subject} down`} title="Move down (older)"><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 5v10m-4-4 4 4 4-4" /></svg></button></div>
        </div>
        {(entry.action === "fixup" || entry.action === "squash") && <div className={`grw-combine-note ${!into ? "invalid" : ""}`}><svg viewBox="0 0 18 18" fill="none" aria-hidden="true"><path d="M13 14V9a4 4 0 0 0-4-4H4m3-3L4 5l3 3" /></svg><div>{into ? <><span>{entry.action === "fixup" ? "Fix up into" : "Squash into"} <strong title={into.subject}>{into.subject}</strong></span><small>{entry.action === "fixup" ? "Keep its message and combine the changes." : "Combine changes and choose the final message below."}</small></> : <span>Move a retained commit below this one to combine them.</span>}</div></div>}
        {(entry.action === "reword" || entry.action === "squash") && <label className="grw-message-editor"><span>{entry.action === "squash" ? "Combined commit message" : "New commit message"}{entry.action === "squash" && <small>Empty keeps both messages</small>}</span><textarea aria-label={`Message for ${entry.subject}`} value={entry.message} disabled={busy} onChange={(e) => setEntries((current) => current.map((value, index) => index === i ? { ...value, message: e.target.value } : value))} placeholder={entry.action === "squash" ? "Keep the existing messages, or write a new one…" : "Write the updated commit message…"} /></label>}
        {entry.action === "edit" && <div className="grw-action-note"><span className="grw-note-dot" />Pause here to amend this commit, then continue.</div>}
        {entry.action === "drop" && <div className="grw-action-note"><span className="grw-note-dot" />This commit will be removed from the rewritten history.</div>}
        </div>
      </div>;
    })}</div>
    <div className="grw-footer"><div className="grw-outcome"><span>{entries.length ? `${outputCount} ${outputCount === 1 ? "commit" : "commits"} after rebase` : "Review before applying"}</span><small>Original HEAD saved for recovery</small></div><button className="grw-start" disabled={busy || loading || !plan || !entries.length} onClick={() => void run()}><svg viewBox="0 0 18 18" fill="none" aria-hidden="true"><path d="M4 9h10m-4-4 4 4-4 4" /></svg><span>Start rebase…</span></button></div>
  </div>;
}
